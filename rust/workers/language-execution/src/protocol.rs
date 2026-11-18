//! Private migration-job/1 frame and identity contract. Not the daemon/MCP wire.
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use sha2::{Digest, Sha256};
use std::io::{self, Read, Write};

pub const PROTOCOL: &str = "slophawk.migration-job/1";
pub const MAX_FRAME: usize = 2_400_000;
pub const MAX_TOTAL: usize = 5_200_000;
pub const MAX_FRAMES: usize = 3;

pub fn sha256(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

/// Keep object key order independent of Cargo's unified serde_json features.
/// Array order remains part of the worker input identity.
pub fn ordered_json(mut value: serde_json::Value) -> serde_json::Result<Vec<u8>> {
    value.sort_all_objects();
    serde_json::to_vec(&value)
}
pub fn is_sha(s: &str) -> bool {
    s.len() == 64 && s.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
pub fn code(s: &str) -> bool {
    !s.is_empty() && s.len() <= 128 && s.bytes().all(|b| b.is_ascii_alphanumeric() || b"-_.".contains(&b))
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Identity {
    pub operation: String,
    pub job_id: String,
    pub attempt_id: String,
    pub root: String,
    pub context_sha256: String,
    pub epoch: String,
    pub committed_revision: u64,
    /// Test worker fence sequence; zero for jobs without a test lease.
    pub fence_sequence: u64,
    pub input_sha256: String,
    pub source_sha256: String,
    pub config_sha256: String,
    pub environment_sha256: String,
    pub tool_sha256: String,
    pub policy_sha256: String,
    pub grant_ref: String,
    pub budget_ref: String,
}
impl Identity {
    pub fn valid(&self) -> bool {
        [self.operation.as_str(), &self.job_id, &self.attempt_id, &self.epoch,
            &self.grant_ref, &self.budget_ref].into_iter().all(code)
            && slophawk_rust_project_context::valid_absolute_path(&self.root)
            && [&self.context_sha256, &self.input_sha256, &self.source_sha256,
                &self.config_sha256, &self.environment_sha256, &self.tool_sha256,
                &self.policy_sha256].into_iter().all(|s| is_sha(s))
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Budget {
    pub deadline_ms: u64,
    pub stdout_bytes: usize,
    pub stderr_bytes: usize,
    pub artifact_bytes: usize,
}
impl Budget {
    pub fn valid(&self) -> bool {
        (1..=120_000).contains(&self.deadline_ms)
            && (1..=256 * 1024).contains(&self.stdout_bytes)
            && (1..=64 * 1024).contains(&self.stderr_bytes)
            && (1..=2 * 1024 * 1024).contains(&self.artifact_bytes)
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Executable {
    pub path: String,
    pub sha256: String,
}
impl Executable {
    pub fn valid(&self) -> bool {
        slophawk_rust_project_context::valid_absolute_path(&self.path) && is_sha(&self.sha256)
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CargoVariant {
    /// Exact approved rustc binary; do not depend on inherited PATH/rustup.
    pub rustc: Executable,
    /// Cargo invokes rustdoc for doctests, independently of RUSTC.
    pub rustdoc: Executable,
    pub revision: u64,
    pub epoch: String,
    pub git_commit: String,
    pub target_triple: String,
    pub rustc_version: String,
    pub rustc_vv_sha256: String,
    pub environment_sha256: String,
    pub all_features: bool,
    pub no_default_features: bool,
    pub features: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub msvc: Option<MsvcToolchain>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MsvcToolchain {
    pub linker: Executable,
    pub library_paths: Vec<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FluxrModule {
    pub module_id: String,
    pub logical_document_id: String,
    pub relative_path: String,
    pub sha256: String,
    pub codec: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FluxrOperation {
    pub operation: String,
    pub module_id: String,
    pub export: String,
    pub input_kinds: Vec<String>,
    pub argument_kinds: Vec<String>,
    pub result_kind: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Operation {
    Pytest {
        interpreter: Executable,
        project_root: String,
        pytest_sha256: String,
        plugins_sha256: String,
        effective_config_sha256: Option<String>,
    },
    CargoMetadata {
        cargo: Executable,
        variant: CargoVariant,
        manifest_path: String,
    },
    CargoTest {
        cargo: Executable,
        variant: CargoVariant,
        manifest_path: String,
        package_spec: String,
        graph_sha256: String,
    },
    FluxrCheck {
        checked_worker: Executable,
        modules: Vec<FluxrModule>,
        operations: Vec<FluxrOperation>,
        cases: serde_json::Value,
        limits: serde_json::Value,
        output_scope: String,
        mode: String,
    },
}
impl Operation {
    pub fn name(&self) -> &'static str {
        match self {
            Self::Pytest { .. } => "pytest",
            Self::CargoMetadata { .. } => "cargo_metadata",
            Self::CargoTest { .. } => "cargo_test",
            Self::FluxrCheck { .. } => "fluxr_check",
        }
    }
    pub fn executable(&self) -> &Executable {
        match self {
            Self::Pytest { interpreter, .. } => interpreter,
            Self::CargoMetadata { cargo, .. } | Self::CargoTest { cargo, .. } => cargo,
            Self::FluxrCheck { checked_worker, .. } => checked_worker,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InputFile {
    pub relative_path: String,
    pub sha256: String,
    pub byte_count: u64,
}
impl InputFile {
    pub fn valid(&self) -> bool {
        !self.relative_path.is_empty() && self.relative_path.len() <= 1024
            && !self.relative_path.starts_with('/') && !self.relative_path.contains('\\')
            && !self.relative_path.contains(':')
            && self.relative_path.split('/').all(|s| !s.is_empty() && s != "." && s != "..")
            && is_sha(&self.sha256) && self.byte_count <= 4 * 1024 * 1024
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Job {
    pub format: String,
    pub identity: Identity,
    pub budget: Budget,
    /// Trusted owner profile separately pins the digest of this full ledger.
    pub inputs: Vec<InputFile>,
    pub operation: Operation,
}
impl Job {
    pub fn valid(&self) -> bool {
        self.format == PROTOCOL && self.identity.valid() && self.budget.valid()
            && self.identity.operation == self.operation.name()
            && self.operation.executable().valid()
            && match &self.operation {
                Operation::Pytest { pytest_sha256, plugins_sha256, .. } =>
                    is_sha(pytest_sha256) && is_sha(plugins_sha256),
                _ => true,
            }
            && self.identity.tool_sha256 == self.operation.executable().sha256
            && !self.inputs.is_empty() && self.inputs.len() <= 4096
            && self.inputs.iter().all(InputFile::valid)
            && self.inputs.iter().map(|f| f.byte_count).sum::<u64>() <= 32 * 1024 * 1024
            && self.inputs.iter().map(|f| &f.relative_path)
                .collect::<std::collections::BTreeSet<_>>().len() == self.inputs.len()
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    Complete,
    Partial,
    Unsupported,
    Deferred,
    Cancelled,
    Stale,
    Failed,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum Frame {
    Evidence { format: String, identity: Identity, payload: serde_json::Value },
    Terminal { format: String, identity: Identity, status: Status,
               reasons: Vec<String>, omissions: Vec<String> },
}
impl Frame {
    pub fn identity(&self) -> &Identity {
        match self { Self::Evidence { identity, .. } | Self::Terminal { identity, .. } => identity }
    }
    pub fn valid(&self) -> bool {
        let (format, reasons, omissions) = match self {
            Self::Evidence { format, .. } => (format.as_str(), &[][..], &[][..]),
            Self::Terminal { format, reasons, omissions, .. } =>
                (format.as_str(), reasons.as_slice(), omissions.as_slice()),
        };
        format == PROTOCOL && self.identity().valid()
            && reasons.len() <= 8 && omissions.len() <= 16
            && reasons.iter().chain(omissions.iter()).all(|s| {
                !s.is_empty() && s.len() <= 256 && !s.chars().any(char::is_control)
            })
    }
}

/// Exactly one little-endian u32 length prefix, checked before JSON allocation.
pub fn write_frame<T: Serialize>(writer: &mut impl Write, item: &T) -> io::Result<()> {
    let bytes = serde_json::to_vec(item).map_err(io::Error::other)?;
    if bytes.is_empty() || bytes.len() > MAX_FRAME {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "outbound frame limit"));
    }
    writer.write_all(&(bytes.len() as u32).to_le_bytes())?;
    writer.write_all(&bytes)
}

pub fn read_frame<T: DeserializeOwned>(reader: &mut impl Read) -> io::Result<Option<T>> {
    let mut len = [0u8; 4];
    match reader.read(&mut len[..1])? {
        0 => return Ok(None),
        1 => reader.read_exact(&mut len[1..])?,
        _ => unreachable!(),
    }
    let size = u32::from_le_bytes(len) as usize;
    if size == 0 || size > MAX_FRAME {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "inbound frame limit"));
    }
    let mut bytes = vec![0u8; size];
    reader.read_exact(&mut bytes)?;
    serde_json::from_slice(&bytes).map(Some).map_err(|_| {
        io::Error::new(io::ErrorKind::InvalidData, "malformed framed JSON")
    })
}

pub fn read_reply(mut input: impl Read) -> io::Result<Vec<Frame>> {
    let mut frames = Vec::new();
    let mut total = 0usize;
    while let Some(frame) = read_frame::<Frame>(&mut input)? {
        let size = serde_json::to_vec(&frame).map_err(io::Error::other)?.len();
        total = total.saturating_add(size + 4);
        if total > MAX_TOTAL || frames.len() >= MAX_FRAMES || !frame.valid() {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "invalid reply or aggregate limit"));
        }
        frames.push(frame);
    }
    Ok(frames)
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ReplyFailure {
    Invalid(&'static str),
    NonComplete { status: Status, reasons: Vec<String>, omissions: Vec<String> },
}

pub fn verify_terminal(frames: &[Frame], expected: &Identity) -> Result<serde_json::Value, ReplyFailure> {
    if frames.is_empty() || frames.len() > 2 || !expected.valid()
        || frames.iter().any(|f| !f.valid() || f.identity() != expected) {
        return Err(ReplyFailure::Invalid("absent, duplicate or mismatched reply identity"));
    }
    let Some(Frame::Terminal { status, reasons, omissions, .. }) = frames.last() else {
        return Err(ReplyFailure::Invalid("matching terminal missing"));
    };
    if *status != Status::Complete {
        if reasons.is_empty() { return Err(ReplyFailure::Invalid("noncomplete terminal lacks reason")); }
        return Err(ReplyFailure::NonComplete {
            status: status.clone(), reasons: reasons.clone(), omissions: omissions.clone(),
        });
    }
    if !reasons.is_empty() || !omissions.is_empty() || frames.len() != 2 {
        return Err(ReplyFailure::Invalid("complete terminal lacks sole evidence or contains omissions"));
    }
    let Frame::Evidence { payload, .. } = &frames[0] else {
        return Err(ReplyFailure::Invalid("evidence missing"));
    };
    Ok(payload.clone())
}
