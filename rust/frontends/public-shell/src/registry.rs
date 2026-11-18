//! Verify owner-pinned tools before running foreground operations.
use crate::identity::{require_physical, uppercase_hex};
use crate::{Result, failure};
use sha2::{Digest, Sha256};
use std::fs::{self, File};
use std::io::Read;
use std::path::{Path, PathBuf};

const MAX_EXECUTABLE_BYTES: u64 = 512 * 1024 * 1024;

struct CheckedFile {
    path: PathBuf,
    sha256: String,
}

impl CheckedFile {
    fn verify(&self, root: &Path) -> Result<()> {
        require_physical(&self.path, false)?;
        let candidate = self.path.canonicalize().map_err(|e| failure("invalid_launcher", e))?;
        let root = root.canonicalize().map_err(|e| failure("invalid_root", e))?;
        if candidate.starts_with(root) {
            return Err(failure("untrusted_launcher", "A launcher inside the checkout is not trusted."));
        }
        if self.sha256.len() != 64 || !self.sha256.bytes().all(|c| c.is_ascii_hexdigit()) {
            return Err(failure("invalid_tool_pin", "Expected a 64-character SHA-256 digest."));
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

/// Validate an owner-pinned executable or worker assembly.
pub fn configured_tool(name: &str, root: &Path) -> Result<PathBuf> {
    let path = std::env::var_os(name).map(PathBuf::from)
        .ok_or_else(|| failure("missing_tool", format!("Set {name} to the installed external tool.")))?;
    let sha256 = std::env::var(format!("{name}_SHA256"))
        .map_err(|_| failure("missing_tool", format!("Set {name}_SHA256 to the exact installed tool digest.")))?;
    let pin = CheckedFile { path, sha256 };
    pin.verify(root)?;
    Ok(pin.path)
}
