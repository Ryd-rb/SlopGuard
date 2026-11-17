//! Explicit executable-peer policy. This module prepares *only* a validated
//! request; it deliberately fails closed before repository-controlled code if
//! the OS's authenticated kill/drain authority has not been proven. A process
//! group, PID file, environment variable or repository JSON is not authority.
use crate::{JobIdentity, JOB_PROTOCOL};
use crate::sha256::{Sha256, to_hex};
use crate::store::{invalid, require_plain_dir};
use serde::Deserialize;
use std::fs::{self, File};
use std::io::{self, ErrorKind, Read};
use std::path::{Path, PathBuf};

const MAX_PROFILE: u64 = 32 * 1024;
const MAX_PEERS: usize = 16;

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PeerGrant {
    pub operation: String,
    pub executable: PathBuf,
    pub sha256: String,
    pub tool_version: String,
    pub context: String,
    pub policy_digest: String,
    pub budget_ref: String,
    pub grant_ref: String,
}
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ExecutionProfile {
    pub version: u32,
    pub root_identity: String,
    pub peers: Vec<PeerGrant>,
}
fn private_profile_file(path: &Path) -> io::Result<Vec<u8>> {
    let meta = fs::symlink_metadata(path)?;
    if !meta.file_type().is_file() || meta.len() > MAX_PROFILE {
        return Err(invalid("execution grant is not a bounded ordinary file"));
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        if meta.file_attributes() & 0x400 != 0 {
            return Err(invalid("execution grant cannot be a reparse point"));
        }
    }
    #[cfg(unix)] {
        use std::os::unix::fs::{MetadataExt, PermissionsExt};
        if meta.permissions().mode() & 0o077 != 0 ||
            meta.uid() != unsafe { geteuid() } {
            return Err(io::Error::new(ErrorKind::PermissionDenied,
                "execution grant must be owned by current user with no group/other access"));
        }
        unsafe extern "C" { fn geteuid() -> u32; }
    }
    let mut bytes = Vec::new();
    File::open(path)?.take(MAX_PROFILE + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > MAX_PROFILE { return Err(invalid("execution grant too long")); }
    Ok(bytes)
}
pub fn default_profile_path() -> io::Result<PathBuf> {
    let home = std::env::var_os("HOME").ok_or_else(|| invalid("HOME missing"))?;
    let home = PathBuf::from(home);
    require_plain_dir(&home)?;
    Ok(home.join(".config").join("slophawk-native-coordinator").join("execution-v1.json"))
}
pub fn read_profile(path: &Path, root_identity: &str) -> io::Result<ExecutionProfile> {
    let bytes = private_profile_file(path)?;
    let profile: ExecutionProfile = serde_json::from_slice(&bytes)
        .map_err(|_| invalid("malformed explicit user execution grant"))?;
    if profile.version != 1 || profile.root_identity != root_identity ||
        profile.peers.len() > MAX_PEERS {
        return Err(invalid("execution profile version/root/size mismatch"));
    }
    Ok(profile)
}
pub(crate) fn verified_peer(path: &Path, workspace: &Path) -> io::Result<(PathBuf, String)> {
    if !path.is_absolute() { return Err(invalid("peer executable path is not absolute")); }
    let canonical = fs::canonicalize(path)?;
    if canonical.starts_with(workspace) { return Err(invalid("peer executable is inside workspace")); }
    let meta = fs::symlink_metadata(&canonical)?;
    if !meta.file_type().is_file() || meta.len() > 128 * 1024 * 1024 { return Err(invalid("executable is not a bounded regular file")); }
    let file = File::open(&canonical)?;
    let mut bounded = (&file).take(meta.len() + 1);
    let mut hasher = Sha256::new();
    let mut buf = [0u8; 64 * 1024];
    loop {
        let size = bounded.read(&mut buf)?;
        if size == 0 { break; }
        hasher.update(&buf[..size]);
    }
    let after = file.metadata()?;
    if after.len() != meta.len() || after.modified()? != meta.modified()? {
        return Err(invalid("executable changed during fingerprint"));
    }
    Ok((canonical, to_hex(&hasher.finish())))
}
pub fn prepared_job(profile: &ExecutionProfile, workspace: &Path,
                    root_identity: &str, operation: &str, context: &str,
                    epoch: &str, revision: u64, input_digest: &str,
                    job_id: &str) -> io::Result<(PathBuf, JobIdentity)> {
    if profile.root_identity != root_identity { return Err(invalid("root grant was superseded")); }
    let matches = profile.peers.iter().filter(|p|
        p.operation == operation && p.context == context).collect::<Vec<_>>();
    if matches.len() != 1 { return Err(invalid("no unique admitted explicit executable peer")); }
    let peer = matches[0];
    if peer.sha256.len() != 64 || !peer.sha256.bytes().all(|b| b.is_ascii_hexdigit()) ||
       peer.tool_version.is_empty() || peer.tool_version.len() > 128 {
        return Err(invalid("invalid pinned executable digest/version"));
    }
    let (binary, actual_digest) = verified_peer(&peer.executable, workspace)?;
    if !actual_digest.eq_ignore_ascii_case(&peer.sha256) {
        return Err(invalid("configured executable bytes no longer match grant"));
    }
    let identity = JobIdentity {
        protocol: JOB_PROTOCOL.into(), operation: operation.into(), job_id: job_id.into(),
        root_identity: root_identity.into(), context: context.into(), epoch: epoch.into(),
        committed_revision: revision, input_digest: input_digest.into(),
        tool_digest: actual_digest, policy_digest: peer.policy_digest.clone(),
        budget_ref: peer.budget_ref.clone(), grant_ref: peer.grant_ref.clone(),
    };
    identity.validate()?;
    Ok((binary, identity))
}
pub(crate) fn external_profile(path: &Path, root: &Path) -> io::Result<PathBuf> {
    if !path.is_absolute() || fs::symlink_metadata(path)?.file_type().is_symlink() {
        return Err(invalid("an explicit absolute physical execution profile is required"));
    }
    let canonical = fs::canonicalize(path)?;
    if canonical.starts_with(root) {
        return Err(invalid("workspace files cannot grant worker execution"));
    }
    require_plain_dir(canonical.parent().ok_or_else(|| invalid("missing profile directory"))?)?;
    Ok(canonical)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn repository_paths_do_not_grant_executable_peers() {
        let profile = ExecutionProfile {
            version: 1, root_identity: "a".repeat(64), peers: vec![],
        };
        assert!(prepared_job(&profile, Path::new("/workspace"), &"a".repeat(64),
            "facts", "rust:unresolved:.", "epoch", 1, &"b".repeat(64), "j1").is_err());
    }
}
