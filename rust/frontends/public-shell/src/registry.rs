//! Owner-pinned process registry. Repository-controlled settings never grant
//! command execution authority. Only an executable (and, for `dotnet`, the
//! managed DLL argument) with an exact SHA-256 may be launched.
use crate::identity::{require_physical, uppercase_hex};
use crate::{Result, failure};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::fs::{self, File};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

const MAX_REGISTRY_BYTES: u64 = 64 * 1024;
const MAX_EXECUTABLE_BYTES: u64 = 512 * 1024 * 1024;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Registry {
    schema: String,
    daemon: DaemonSelection,
    legacy: Option<Launcher>,
    trusted_peer: Option<Launcher>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
enum DaemonSelection { Legacy, TrustedPeer }

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Launcher {
    executable: CheckedFile,
    #[serde(default)]
    argv_prefix: Vec<String>,
    #[serde(default)]
    attested_files: Vec<CheckedFile>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct CheckedFile {
    path: PathBuf,
    sha256: String,
}

impl Registry {
    pub fn load(workspace_root: Option<&Path>) -> Result<Self> {
        let path = registry_path()?;
        require_physical(&path, false)?;
        if let Some(root) = workspace_root {
            if within(&path, root) {
                return Err(failure("untrusted_registry", "The launcher registry must be outside the checkout."));
            }
        }
        let file = File::open(&path).map_err(|e| failure("missing_registry", e))?;
        let mut bytes = Vec::new();
        file.take(MAX_REGISTRY_BYTES + 1).read_to_end(&mut bytes)
            .map_err(|e| failure("invalid_registry", e))?;
        if bytes.len() as u64 > MAX_REGISTRY_BYTES {
            return Err(failure("invalid_registry", "Launcher registry exceeds 64 KiB."));
        }
        let registry: Self = serde_json::from_slice(&bytes)
            .map_err(|e| failure("invalid_registry", e))?;
        if registry.schema != "slophawk.launcher-registry/1" {
            return Err(failure("invalid_registry", "Unsupported launcher registry schema."));
        }
        // Fail early if any configured entry is invalid; selection never
        // silently drops back to an unchecked executable.
        if let Some(launcher) = &registry.legacy { launcher.verify(workspace_root)?; }
        if let Some(launcher) = &registry.trusted_peer { launcher.verify(workspace_root)?; }
        Ok(registry)
    }

    pub fn legacy(&self) -> Result<&Launcher> {
        self.legacy.as_ref().ok_or_else(|| failure("missing_legacy_fallback",
            "No digest-checked C# CLI fallback was configured."))
    }

    pub fn daemon_launcher(&self) -> Result<&Launcher> {
        match self.daemon {
            DaemonSelection::Legacy => self.legacy(),
            DaemonSelection::TrustedPeer => self.trusted_peer.as_ref().ok_or_else(|| {
                failure("missing_trusted_peer", "The explicitly selected native peer is not registered.")
            }),
        }
    }
}

impl Launcher {
    fn verify(&self, workspace_root: Option<&Path>) -> Result<()> {
        self.executable.verify(workspace_root)?;
        // Only the published apphost (no prefix) or `dotnet <pinned.dll>` are
        // legal launch shapes. In particular, neither shell `-c` nor dynamically
        // interpreted arguments can be registered as an arbitrary command.
        match self.argv_prefix.as_slice() {
            [] if self.attested_files.is_empty() => (),
            [managed] if self.attested_files.len() == 1 &&
                managed.to_ascii_lowercase().ends_with(".dll") &&
                self.executable.path.file_name().is_some_and(|x|
                    x.to_string_lossy().eq_ignore_ascii_case("dotnet") ||
                    x.to_string_lossy().eq_ignore_ascii_case("dotnet.exe")) &&
                Path::new(managed) == self.attested_files[0].path.as_path() => {
                self.attested_files[0].verify(workspace_root)?;
            }
            _ => return Err(failure("invalid_registry",
                "Allowed argv: apphost with no prefix, or dotnet with one attested absolute DLL.")),
        }
        Ok(())
    }

    pub fn command(&self, args: &[String], root: Option<&Path>) -> Result<Command> {
        self.verify(root)?;
        let mut command = Command::new(&self.executable.path);
        command.args(&self.argv_prefix).args(args);
        if let Some(root) = root { command.current_dir(root); }
        Ok(command)
    }

    pub fn forward(&self, args: &[String], root: Option<&Path>) -> Result<i32> {
        let mut command = self.command(args, root)?;
        command.stdin(Stdio::inherit()).stdout(Stdio::inherit()).stderr(Stdio::inherit());
        let status = command.status().map_err(|e| failure("failed_launch", e))?;
        // Preserve the C# foreground/MCP exit codes instead of treating a
        // missing-native implementation as an empty successful capability.
        Ok(status.code().unwrap_or(1))
    }
}

impl CheckedFile {
    fn verify(&self, root: Option<&Path>) -> Result<()> {
        require_physical(&self.path, false)?;
        if let Some(root) = root {
            if within(&self.path, root) {
                return Err(failure("untrusted_launcher", "A launcher inside the checkout is not trusted."));
            }
        }
        if self.sha256.len() != 64 || !self.sha256.bytes().all(|c| c.is_ascii_hexdigit()) {
            return Err(failure("invalid_registry", "Expected a 64-character SHA-256 digest."));
        }
        let metadata = fs::metadata(&self.path).map_err(|e| failure("invalid_launcher", e))?;
        if !metadata.is_file() || metadata.len() > MAX_EXECUTABLE_BYTES {
            return Err(failure("invalid_launcher", "Launcher is not an ordinary bounded file."));
        }
        let mut file = File::open(&self.path).map_err(|e| failure("invalid_launcher", e))?;
        let mut digest = Sha256::new();
        let mut buffer = [0u8; 64 * 1024];
        let mut total = 0u64;
        loop {
            let read = file.read(&mut buffer).map_err(|e| failure("invalid_launcher", e))?;
            if read == 0 { break; }
            total += read as u64;
            if total > MAX_EXECUTABLE_BYTES {
                return Err(failure("invalid_launcher", "Launcher changed or exceeds digest byte budget."));
            }
            digest.update(&buffer[..read]);
        }
        if total != metadata.len() ||
            !uppercase_hex(&digest.finalize()).eq_ignore_ascii_case(&self.sha256) {
            return Err(failure("digest_mismatch", format!(
                "Launcher bytes differ from owner pin: {}", self.path.display())));
        }
        Ok(())
    }
}

fn within(path: &Path, root: &Path) -> bool {
    let Ok(root) = std::path::absolute(root) else { return true; };
    #[cfg(windows)]
    { path.to_string_lossy().to_uppercase().starts_with(
        &format!("{}\\", root.to_string_lossy().to_uppercase())) ||
      path.to_string_lossy().eq_ignore_ascii_case(&root.to_string_lossy()) }
    #[cfg(not(windows))]
    { path.starts_with(root) }
}

fn registry_path() -> Result<PathBuf> {
    if let Some(explicit) = std::env::var_os("SLOPHAWK_LAUNCHER_REGISTRY") {
        let path = PathBuf::from(explicit);
        if !path.is_absolute() { return Err(failure("invalid_registry", "Registry override must be absolute.")); }
        return Ok(path);
    }
    #[cfg(windows)]
    {
        let base = std::env::var_os("LOCALAPPDATA").ok_or_else(||
            failure("missing_registry", "LOCALAPPDATA is not set."))?;
        Ok(PathBuf::from(base).join("SlopHawk").join("launchers.json"))
    }
    #[cfg(not(windows))]
    {
        let base = std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
            .ok_or_else(|| failure("missing_registry", "HOME and XDG_CONFIG_HOME are unset."))?;
        Ok(base.join("slophawk").join("launchers.json"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rejects_shell_launchers_and_unpinned_managed_arguments() {
        let launcher: Launcher = serde_json::from_str(r#"{
          "executable":{"path":"/bin/sh","sha256":"00"},
          "argvPrefix":["-c","echo unsafe"]
        }"#).unwrap();
        assert!(launcher.verify(None).is_err());
    }
    #[test]
    fn rejects_unknown_registry_schema_or_fields() {
        assert!(serde_json::from_str::<Registry>(r#"{
          "schema":"slophawk.launcher-registry/2","daemon":"legacy", "extra":true
        }"#).is_err());
    }
}

#[cfg(test)]
mod launch_failure_tests {
    use super::*;
    use std::io::Write;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn verified_digest_does_not_turn_a_failed_launch_into_success() {
        let suffix = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();
        let path = std::env::temp_dir().join(format!("slophawk-not-executable-{}-{suffix}.exe",
            std::process::id()));
        {
            let mut file = File::create(&path).unwrap();
            file.write_all(b"this is not a native executable").unwrap();
        }
        let digest = uppercase_hex(&Sha256::digest(b"this is not a native executable"));
        let launcher = Launcher {
            executable: CheckedFile { path: path.clone(), sha256: digest },
            argv_prefix: Vec::new(), attested_files: Vec::new(),
        };
        let result = launcher.forward(&["status".to_string()], None);
        assert_eq!(result.unwrap_err().code, "failed_launch");
        fs::remove_file(path).unwrap();
    }
}
