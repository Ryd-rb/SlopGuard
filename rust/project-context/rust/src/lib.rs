//! Read-only interpretation of bounded Cargo metadata and admitted test captures.
//!
//! This crate never starts Cargo, rustc, test executables, build scripts or proc macros.
//! The caller must admit and supervise those processes, and bind captures to a
//! checked workspace fence before calling these functions. Nothing here claims
//! complete Rust semantics, module membership, generated code or trait binding.

mod metadata;
mod receipts;
mod selection;

pub use metadata::{
    CargoEdge, CargoGraph, CargoPackage, CargoTarget, Coverage, DeclaredDependency, DepKind,
    ManifestDigest, MetadataInput, SnapshotFile, interpret_metadata,
};
pub use receipts::{CargoTestJob, Counters, Disposition, TestReceipt, classify_test_capture};
pub use selection::{Change, Impact, ImpactReason, ImpactReasonKind, ImpactTarget, select_tests};

use sha2::{Digest, Sha256};
use std::fmt;

pub const SCHEMA_VERSION: u32 = 1;
pub const MAX_METADATA_BYTES: usize = 4 * 1024 * 1024;
pub const MAX_LOG_BYTES: usize = 256 * 1024;
pub const MAX_MANIFEST_BYTES: usize = 1024 * 1024;
pub const MAX_SOURCE_TOTAL_BYTES: usize = 4 * 1024 * 1024;
pub const MAX_PACKAGES: usize = 512;
pub const MAX_EDGES: usize = 8192;
pub const MAX_TARGETS: usize = 2048;
pub const MAX_TESTS: u64 = 100_000;
pub const MAX_CHANGES: usize = 4096;

/// Failed evidence is *not* a successful empty graph or passing test run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Error {
    Invalid(&'static str),
    Unsupported(&'static str),
    Blocked(&'static str),
    Stale(&'static str),
    Malformed(&'static str),
    Limit(&'static str),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Invalid(s) => write!(f, "invalid request: {s}"),
            Self::Unsupported(s) => write!(f, "unsupported Cargo route: {s}"),
            Self::Blocked(s) => write!(f, "blocked Cargo evidence: {s}"),
            Self::Stale(s) => write!(f, "stale Cargo evidence: {s}"),
            Self::Malformed(s) => write!(f, "malformed Cargo output: {s}"),
            Self::Limit(s) => write!(f, "Cargo evidence limit exceeded: {s}"),
        }
    }
}

impl std::error::Error for Error {}

/// An immutable, explicit Cargo variant. No environment or config is inferred.
/// The worker is responsible for computing the toolchain/environment digests,
/// canonicalizing the checkout root and isolating its source snapshot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Binding {
    pub workspace_root: String,
    pub workspace_revision: u64,
    pub epoch: String,
    pub git_commit: String,
    pub target_triple: String,
    /// Human-readable rustc version, retained as a label, not proof of identity.
    pub rustc_version: String,
    /// SHA-256 of the exact `rustc -Vv` output.
    pub rustc_vv_sha256: String,
    /// SHA-256 of the selected Cargo executable, as attested by the worker.
    pub cargo_executable_sha256: String,
    /// SHA-256 of the controlled Cargo config, environment and registry identity.
    pub environment_sha256: String,
    pub features: Features,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Features {
    pub all: bool,
    pub no_default: bool,
    /// Explicitly ordered and normalized: no duplicate feature tokens.
    pub selected: Vec<String>,
}

impl Binding {
    pub fn validate(&self) -> Result<(), Error> {
        if !valid_absolute_path(&self.workspace_root) {
            return Err(Error::Unsupported(
                "only canonical Linux absolute roots are qualified",
            ));
        }
        if !safe_code(&self.epoch, 128)
            || !is_digest(&self.git_commit, 40)
            || !safe_code(&self.target_triple, 128)
            || self.rustc_version.is_empty()
            || self.rustc_version.len() > 128
            || self.rustc_version.chars().any(char::is_control)
            || !is_digest(&self.rustc_vv_sha256, 64)
            || !is_digest(&self.cargo_executable_sha256, 64)
            || !is_digest(&self.environment_sha256, 64)
        {
            return Err(Error::Invalid(
                "unbound revision, target, epoch or toolchain",
            ));
        }
        if self.features.all && (self.features.no_default || !self.features.selected.is_empty()) {
            return Err(Error::Unsupported(
                "all-features conflicts with explicit feature selection",
            ));
        }
        if self.features.selected.len() > 64 {
            return Err(Error::Limit("more than 64 selected features"));
        }
        let mut sorted = self.features.selected.clone();
        sorted.sort();
        sorted.dedup();
        if sorted != self.features.selected
            || self
                .features
                .selected
                .iter()
                .any(|name| !valid_feature(name))
        {
            return Err(Error::Unsupported(
                "feature flags are not normalized or supported",
            ));
        }
        Ok(())
    }

    /// Deterministic domain-separated cache/fence digest, independent of Debug formatting.
    pub fn digest(&self) -> Result<String, Error> {
        self.validate()?;
        let mut h = Sha256::new();
        for part in [
            "slophawk.cargo.binding.v1",
            &self.workspace_root,
            &self.epoch,
            &self.git_commit,
            &self.target_triple,
            &self.rustc_version,
            &self.rustc_vv_sha256,
            &self.cargo_executable_sha256,
            &self.environment_sha256,
        ] {
            hash_field(&mut h, part.as_bytes());
        }
        hash_field(&mut h, &self.workspace_revision.to_be_bytes());
        hash_field(
            &mut h,
            &[
                u8::from(self.features.all),
                u8::from(self.features.no_default),
            ],
        );
        for feature in &self.features.selected {
            hash_field(&mut h, feature.as_bytes());
        }
        Ok(format!("{:x}", h.finalize()))
    }
}

/// Exit is supplied by the controlled worker, never inferred from output text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkerExit {
    Exited(i32),
    TimedOut,
    Cancelled,
    Terminated,
}

/// The caller must attest admission *after* policy approval. This assertion is
/// not authentication: the coordinator must trust only its own supervised worker.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkerCapture {
    pub binding: Binding,
    /// Exact argv (not shell-interpreted), excluding the worker-pinned Cargo executable.
    pub argv: Vec<String>,
    pub admitted: bool,
    /// Set for tests, absent for metadata. Prevents replay across attempts.
    pub attempt_id: Option<u64>,
    pub fence_sequence: Option<u64>,
    pub graph_sha256: Option<String>,
    pub exit: WorkerExit,
    /// If either stream was truncated, no passing/failing test claims are accepted.
    pub truncated: bool,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
}

impl WorkerCapture {
    pub(crate) fn check(&self, binding: &Binding, expected: &[String]) -> Result<(), Error> {
        binding.validate()?;
        if self.binding != *binding {
            return Err(Error::Stale(
                "workspace fence, target, features or toolchain changed",
            ));
        }
        if !self.admitted {
            return Err(Error::Blocked("runner lacks an admission grant"));
        }
        if self.argv != expected {
            return Err(Error::Unsupported(
                "runner argv is not the locked offline route",
            ));
        }
        if self.argv.len() > 96 {
            return Err(Error::Limit("Cargo argument count"));
        }
        Ok(())
    }
}

/// Exact command contract. The controlled worker supplies executable path,
/// cwd, closed environment, output caps, time limits, process-group termination,
/// and an isolated target dir. No arbitrary Cargo or libtest flags pass through.
pub fn metadata_argv(binding: &Binding, manifest_path: &str) -> Result<Vec<String>, Error> {
    binding.validate()?;
    check_member_path(&binding.workspace_root, manifest_path)?;
    if !manifest_path.ends_with("/Cargo.toml") {
        return Err(Error::Invalid("metadata manifest is not Cargo.toml"));
    }
    let mut args = vec![
        "metadata".into(),
        "--format-version".into(),
        "1".into(),
        "--locked".into(),
        "--offline".into(),
        "--manifest-path".into(),
        manifest_path.into(),
        "--filter-platform".into(),
        binding.target_triple.clone(),
    ];
    feature_argv(&binding.features, &mut args);
    Ok(args)
}

pub fn test_argv(
    binding: &Binding,
    manifest_path: &str,
    package: &str,
) -> Result<Vec<String>, Error> {
    binding.validate()?;
    check_member_path(&binding.workspace_root, manifest_path)?;
    if !manifest_path.ends_with("/Cargo.toml") || !valid_package_spec(package) {
        return Err(Error::Invalid("test manifest or package spec"));
    }
    let mut args = vec![
        "test".into(),
        "--locked".into(),
        "--offline".into(),
        "--manifest-path".into(),
        manifest_path.into(),
        "--target".into(),
        binding.target_triple.clone(),
        "--package".into(),
        package.into(),
        "--message-format=json".into(),
        "--color=never".into(),
    ];
    feature_argv(&binding.features, &mut args);
    args.extend(["--", "--format=pretty", "--color=never"].map(str::to_owned));
    Ok(args)
}

fn feature_argv(features: &Features, args: &mut Vec<String>) {
    if features.all {
        args.push("--all-features".into());
    } else {
        if features.no_default {
            args.push("--no-default-features".into());
        }
        if !features.selected.is_empty() {
            args.extend(["--features".into(), features.selected.join(",")]);
        }
    }
}

pub(crate) fn hash_field(h: &mut Sha256, bytes: &[u8]) {
    h.update((bytes.len() as u64).to_be_bytes());
    h.update(bytes);
}

pub fn sha256_hex(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

pub(crate) fn is_digest(text: &str, length: usize) -> bool {
    text.len() == length && text.bytes().all(|c| c.is_ascii_hexdigit())
}

fn safe_code(text: &str, limit: usize) -> bool {
    !text.is_empty()
        && text.len() <= limit
        && text
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'_' | b'-' | b'.'))
}

fn valid_feature(name: &str) -> bool {
    name.len() <= 128
        && !name.is_empty()
        && name.split('/').count() <= 2
        && name.split('/').all(|part| safe_code(part, 128))
}

fn valid_package_spec(name: &str) -> bool {
    name.len() <= 160
        && !name.is_empty()
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'@' | b'+'))
}

pub fn valid_absolute_path(path: &str) -> bool {
    if path.len() > 4096 || path.chars().any(|c| c.is_control() || c == '\\') {
        return false;
    }
    let (relative, windows) = match path.as_bytes() {
        [b'/', ..] => (&path[1..], false),
        [drive, b':', b'/', ..] if drive.is_ascii_uppercase() => (&path[3..], true),
        _ => return false,
    };
    relative.split('/').all(|part| {
        !part.is_empty() && part != "." && part != ".."
            && (!windows || valid_windows_component(part))
    })
}

fn valid_windows_component(part: &str) -> bool {
    if part.ends_with(['.', ' ']) || part.contains([':', '*', '?', '"', '<', '>', '|']) {
        return false;
    }
    let stem = part.split('.').next().unwrap_or_default().trim_end().to_ascii_uppercase();
    !matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL" | "CONIN$" | "CONOUT$")
        && !["COM", "LPT"].iter().any(|prefix| stem.strip_prefix(prefix)
            .is_some_and(|suffix| matches!(suffix, "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9" | "¹" | "²" | "³")))
}

/// Convert a path emitted by Cargo or another platform tool to the wire form.
/// Windows volumes are restricted to drive letters, including extended drive paths.
pub fn platform_path_to_wire(path: &str) -> Option<String> {
    #[cfg(windows)]
    let path = {
        let mut value = path.strip_prefix(r"\\?\").unwrap_or(path).replace('\\', "/");
        let drive = value.get(..1)?.to_ascii_uppercase();
        value.replace_range(..1, &drive);
        if !matches!(value.as_bytes(), [drive, b':', b'/', ..] if drive.is_ascii_uppercase()) {
            return None;
        }
        value
    };
    #[cfg(not(windows))]
    let path = path.to_owned();
    valid_absolute_path(&path).then_some(path)
}

pub(crate) fn check_member_path(root: &str, path: &str) -> Result<(), Error> {
    if !valid_absolute_path(path) || !path.starts_with(&format!("{root}/")) {
        return Err(Error::Invalid(
            "path is not a canonical member of workspace root",
        ));
    }
    Ok(())
}

#[cfg(test)]
mod path_tests {
    use super::*;

    #[test]
    fn wire_paths_reject_windows_aliases_and_ambiguous_spellings() {
        assert!(valid_absolute_path("C:/work dir/Cargo.toml"));
        assert!(valid_absolute_path("/workspace/Cargo.toml"));
        for path in ["C:foo", "C:/a/..", "C:/a/dir.", "C:/a/b:stream", "C:/a/AUX.rs",
            "C:/a/LPT¹.txt", "//server/share", "C:/a//b", r"C:\a\b", "/a/.."] {
            assert!(!valid_absolute_path(path), "{path}");
        }
        assert!(check_member_path("C:/work", "C:/work/Cargo.toml").is_ok());
        assert!(check_member_path("C:/work", "C:/work-other/Cargo.toml").is_err());
    }
}

pub(crate) fn json_object(
    obj: &serde_json::Value,
) -> Result<&serde_json::Map<String, serde_json::Value>, Error> {
    obj.as_object()
        .ok_or(Error::Malformed("expected JSON object"))
}

pub(crate) fn json_array(value: &serde_json::Value) -> Result<&Vec<serde_json::Value>, Error> {
    value
        .as_array()
        .ok_or(Error::Malformed("expected JSON array"))
}

pub(crate) fn string(value: &serde_json::Value, limit: usize) -> Result<String, Error> {
    let s = value
        .as_str()
        .ok_or(Error::Malformed("expected JSON string"))?;
    if s.len() > limit || s.chars().any(char::is_control) {
        return Err(Error::Limit(
            "string field is too long or contains a control character",
        ));
    }
    Ok(s.to_owned())
}
