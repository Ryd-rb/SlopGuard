//! Linux OS-user admission and cgroup supervision. Never read grants from a checkout.
use crate::protocol::{self, Budget, Executable, Job};
use serde::Deserialize;
use std::fs::{self, File};
use std::io::{self, Read};
use std::path::{Path, PathBuf};
#[cfg(target_os = "linux")]
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Debug)]
pub enum Refusal {
    Unsupported(&'static str),
    Denied(&'static str),
    Stale(&'static str),
    Failed(&'static str),
    WorkerStatus { status: protocol::Status, reasons: Vec<String>, omissions: Vec<String> },
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct Profile {
    root: String,
    context_sha256: String,
    epoch: String,
    committed_revision: u64,
    fence_sequence: u64,
    input_sha256: String,
    source_sha256: String,
    input_files_sha256: String,
    operation_sha256: String,
    config_sha256: String,
    environment_sha256: String,
    policy_sha256: String,
    grant_ref: String,
    budget_ref: String,
    operations: Vec<String>,
    /// Cargo test is a build-and-execute action; repository code extensions
    /// require distinct owner grants, never flags from Cargo.toml.
    allow_build: bool,
    allow_build_scripts: bool,
    allow_proc_macros: bool,
    tool: Executable,
    worker: Executable,
    artifacts_root: String,
    budget: Budget,
}

/// Only a verified, current worker result may be passed to the source classifiers.
/// The staged directory is not daemon publication; its lifecycle belongs to the caller.
#[derive(Debug)]
pub struct Verified {
    job: Job,
    payload: serde_json::Value,
    started_ms: u64,
    finished_ms: u64,
    artifact_dir: PathBuf,
}
impl Verified {
    pub fn job(&self) -> &Job { &self.job }
    pub fn payload(&self) -> &serde_json::Value { &self.payload }
    pub fn started_ms(&self) -> u64 { self.started_ms }
    pub fn finished_ms(&self) -> u64 { self.finished_ms }
    pub fn artifact_dir(&self) -> &Path { &self.artifact_dir }
}

pub fn unix_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_millis() as u64
}

#[cfg(target_os = "linux")]
mod linux {
    unsafe extern "C" { pub fn geteuid() -> u32; }
}

fn verify_profile(path: &Path, job: &Job) -> Result<(Profile, String), Refusal> {
    if !job.valid() { return Err(Refusal::Denied("invalid job identity or budget")); }
    #[cfg(not(target_os = "linux"))]
    { let _ = path; return Err(Refusal::Unsupported("OS-authenticated process-group admission is Linux only")); }
    #[cfg(target_os = "linux")]
    {
        use std::os::unix::fs::{MetadataExt, PermissionsExt};
        let uid = unsafe { linux::geteuid() };
        let dir = path.parent().ok_or(Refusal::Denied("profile has no parent"))?;
        let dir_meta = fs::symlink_metadata(dir).map_err(|_| Refusal::Denied("profile directory unavailable"))?;
        if !dir_meta.is_dir() || dir_meta.uid() != uid || dir_meta.permissions().mode() & 0o077 != 0 {
            return Err(Refusal::Denied("profile directory is not private to OS user"));
        }
        let meta = fs::symlink_metadata(path).map_err(|_| Refusal::Denied("profile unavailable"))?;
        if !meta.file_type().is_file() || meta.uid() != uid || meta.permissions().mode() & 0o077 != 0 || meta.len() > 32 * 1024 {
            return Err(Refusal::Denied("profile is not a private regular file"));
        }
        let bytes = read_bounded(File::open(path).map_err(|_| Refusal::Denied("profile cannot be opened"))?, 32 * 1024)
            .map_err(|_| Refusal::Denied("profile exceeds limit"))?;
        let profile: Profile = serde_json::from_slice(&bytes).map_err(|_| Refusal::Denied("invalid profile JSON"))?;
        let i = &job.identity;
        if profile.root != i.root || profile.context_sha256 != i.context_sha256
            || profile.epoch != i.epoch || profile.committed_revision != i.committed_revision
            || profile.fence_sequence != i.fence_sequence
            || profile.input_sha256 != i.input_sha256 || profile.source_sha256 != i.source_sha256
            || profile.input_files_sha256 != protocol::sha256(&serde_json::to_vec(&job.inputs)
                .map_err(|_| Refusal::Denied("input ledger serialization failed"))?)
            || profile.operation_sha256 != protocol::sha256(&serde_json::to_vec(&job.operation)
                .map_err(|_| Refusal::Denied("operation serialization failed"))?)
            || profile.config_sha256 != i.config_sha256 || profile.environment_sha256 != i.environment_sha256
            || profile.policy_sha256 != i.policy_sha256 || profile.grant_ref != i.grant_ref
            || profile.budget_ref != i.budget_ref || !profile.operations.contains(&i.operation)
            || profile.operations.is_empty() || profile.operations.len() > 4
            || matches!(&job.operation, crate::protocol::Operation::CargoTest { .. })
                && !(profile.allow_build && profile.allow_build_scripts && profile.allow_proc_macros)
            || !profile.tool.valid() || &profile.tool != job.operation.executable()
            || !profile.worker.valid() || !profile.budget.valid()
            || job.budget.deadline_ms > profile.budget.deadline_ms
            || job.budget.stdout_bytes > profile.budget.stdout_bytes
            || job.budget.stderr_bytes > profile.budget.stderr_bytes
            || job.budget.artifact_bytes > profile.budget.artifact_bytes
            || !Path::new(&profile.root).is_absolute()
            || fs::canonicalize(&profile.root).map_err(|_| Refusal::Stale("workspace missing"))?
                != Path::new(&profile.root)
            || path.starts_with(&profile.root)
        {
            return Err(Refusal::Denied("scope, tool, policy, grant or budget not authorized"));
        }
        let output = Path::new(&profile.artifacts_root);
        let out_meta = fs::symlink_metadata(output).map_err(|_| Refusal::Denied("artifact root missing"))?;
        if !output.is_absolute() || output.starts_with(&profile.root)
            || !out_meta.is_dir() || out_meta.uid() != uid
            || out_meta.permissions().mode() & 0o077 != 0
            || fs::canonicalize(output).map_err(|_| Refusal::Denied("artifact root missing"))? != output
        {
            return Err(Refusal::Denied("artifact root not owned and private outside checkout"));
        }
        verify_executable(&profile.worker)?;
        verify_executable(&profile.tool)?;
        match &job.operation {
            crate::protocol::Operation::CargoMetadata { variant, .. }
            | crate::protocol::Operation::CargoTest { variant, .. } => {
                verify_executable(&variant.rustc)?;
                verify_executable(&variant.rustdoc)?;
            },
            _ => {}
        }
        Ok((profile, protocol::sha256(&bytes)))
    }
}

pub(crate) fn read_bounded(mut input: impl Read, limit: usize) -> io::Result<Vec<u8>> {
    let mut data = Vec::new();
    let mut chunk = [0u8; 8192];
    loop {
        let n = input.read(&mut chunk)?;
        if n == 0 { return Ok(data); }
        if n > limit.saturating_sub(data.len()) {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "input exceeded cap"));
        }
        data.extend_from_slice(&chunk[..n]);
    }
}

fn verify_executable(executable: &Executable) -> Result<(), Refusal> {
    if !executable.valid() { return Err(Refusal::Denied("invalid executable identity")); }
    let path = Path::new(&executable.path);
    let link = fs::symlink_metadata(path).map_err(|_| Refusal::Denied("executable unavailable"))?;
    let meta = fs::metadata(path).map_err(|_| Refusal::Denied("executable target unavailable"))?;
    // Virtualenv Python executables are frequently symlinks. Pin both the
    // selected path (profile) and the resolved target bytes (SHA-256).
    if !(link.file_type().is_symlink() || link.file_type().is_file())
        || !meta.file_type().is_file() || meta.len() > 64 * 1024 * 1024
    { return Err(Refusal::Denied("executable must resolve to a bounded regular file")); }
    #[cfg(target_os = "linux")]
    {
        use std::os::unix::fs::{MetadataExt, PermissionsExt};
        let owner = meta.uid();
        if owner != 0 && owner != unsafe { linux::geteuid() } || meta.permissions().mode() & 0o022 != 0 {
            return Err(Refusal::Denied("executable is writable by another OS principal"));
        }
    }
    let bytes = read_bounded(File::open(path).map_err(|_| Refusal::Denied("executable read failed"))?, 64 * 1024 * 1024)
        .map_err(|_| Refusal::Denied("executable exceeds cap"))?;
    if protocol::sha256(&bytes) != executable.sha256 { return Err(Refusal::Stale("executable digest mismatch")); }
    Ok(())
}

/// Recheck only the OS-private profile's bytes on supervision ticks. Executable
/// bytes are checked fully before launch and after the worker exits.
fn current_fingerprint(path: &Path) -> Option<String> {
    #[cfg(not(target_os = "linux"))]
    { let _ = path; None }
    #[cfg(target_os = "linux")]
    {
        use std::os::unix::fs::{MetadataExt, PermissionsExt};
        let uid = unsafe { linux::geteuid() };
        let d = fs::symlink_metadata(path.parent()?).ok()?;
        let m = fs::symlink_metadata(path).ok()?;
        if !d.is_dir() || d.uid() != uid || d.permissions().mode() & 0o077 != 0
            || !m.file_type().is_file() || m.uid() != uid || m.permissions().mode() & 0o077 != 0
            || m.len() > 32 * 1024 { return None; }
        let bytes = read_bounded(File::open(path).ok()?, 32 * 1024).ok()?;
        Some(protocol::sha256(&bytes))
    }
}

fn verify_inputs(job: &Job) -> Result<(), Refusal> {
    for input in &job.inputs {
        if !input.valid() { return Err(Refusal::Denied("invalid source ledger item")); }
        let path = Path::new(&job.identity.root).join(&input.relative_path);
        let meta = fs::symlink_metadata(&path).map_err(|_| Refusal::Stale("declared source missing"))?;
        if !meta.file_type().is_file() || meta.len() != input.byte_count
            || fs::canonicalize(&path).map_err(|_| Refusal::Stale("source unavailable"))? != path {
            return Err(Refusal::Stale("source identity or canonical path changed"));
        }
        let bytes = read_bounded(File::open(&path).map_err(|_| Refusal::Stale("source unreadable"))?, 4 * 1024 * 1024)
            .map_err(|_| Refusal::Stale("source exceeded byte budget"))?;
        if protocol::sha256(&bytes) != input.sha256 {
            return Err(Refusal::Stale("source content digest changed"));
        }
    }
    Ok(())
}

/// Verify the private profile before launching and before accepting the receipt.
/// The shared manager owns the complete process tree and the exchange deadline.
pub fn run_admitted(profile_path: &Path, job: Job, cancelled: &AtomicBool) -> Result<Verified, Refusal> {
    let (profile, fingerprint) = verify_profile(profile_path, &job)?;
    verify_inputs(&job)?;
    #[cfg(not(target_os = "linux"))]
    { let _ = (profile, fingerprint, cancelled); return Err(Refusal::Unsupported("Linux process supervision required")); }
    #[cfg(target_os = "linux")]
    {
        use std::os::unix::fs::DirBuilderExt;
        if cancelled.load(Ordering::Acquire) { return Err(Refusal::WorkerStatus {
            status: protocol::Status::Cancelled, reasons: vec!["cancelled_before_admission".into()],
            omissions: vec!["not_launched".into()],
        }); }
        let artifact_dir = Path::new(&profile.artifacts_root).join(format!("{}-{}", job.identity.job_id, job.identity.attempt_id));
        fs::DirBuilder::new().mode(0o700).create(&artifact_dir)
            .map_err(|_| Refusal::Denied("attempt directory must be new and exclusive"))?;
        // The fixed env tool establishes a clean locale for the admitted worker.
        // It executes inside the same cgroup before any repository code can run.
        let mut command = Command::new("/usr/bin/env");
        command.args(["-i", "LC_ALL=C", "LANG=C"]).arg(&profile.worker.path)
            .current_dir(&job.identity.root).arg("--migration-job-1").arg(&artifact_dir);
        let mut request = Vec::new();
        protocol::write_frame(&mut request, &job).map_err(|_| Refusal::Failed("invalid worker request"))?;
        let started_ms = unix_ms();
        let current = || current_fingerprint(profile_path).as_deref() == Some(fingerprint.as_str());
        let output = match slophawk_worker_scope::run_guarded(&command, &request,
            slophawk_worker_scope::Limits {
                stdout: protocol::MAX_TOTAL,
                stderr: 4096,
                deadline: std::time::Duration::from_millis(job.budget.deadline_ms),
            }, cancelled, &current) {
            Ok(output) => output,
            Err(reason) => {
                if std::env::var_os("SLOPHAWK_SCOPE_DIAGNOSTICS").is_some() {
                    eprintln!("language worker scope: {}", reason.chars().take(600).collect::<String>());
                }
                if slophawk_worker_scope::recover(std::time::Duration::from_secs(3)).is_ok() {
                    let _ = fs::remove_dir_all(&artifact_dir);
                }
                if cancelled.load(Ordering::Acquire) {
                    return Err(Refusal::WorkerStatus {
                        status: protocol::Status::Cancelled, reasons: vec!["cancelled_during_execution".into()],
                        omissions: vec!["receipt_discarded".into()],
                    });
                }
                if !current() { return Err(Refusal::Stale("authority revoked during execution")); }
                return Err(Refusal::Failed("worker launch, deadline or scope drainage failed"));
            }
        };
        let result = (|| {
            if output.code != 0 {
                return Err(Refusal::Failed("worker peer failed"));
            }
            let frames = protocol::read_reply(output.stdout.as_slice())
                .map_err(|_| Refusal::Failed("bad or oversized worker frames"))?;
            protocol::verify_terminal(&frames, &job.identity).map_err(|error| match error {
                protocol::ReplyFailure::NonComplete { status, reasons, omissions } =>
                    Refusal::WorkerStatus { status, reasons, omissions },
                protocol::ReplyFailure::Invalid(_) => Refusal::Failed("missing or invalid terminal"),
            })
        })();
        let finished_ms = unix_ms();
        // Never publish evidence or retain attempt output on an invalid peer or fence.
        if result.is_err() || !matches!(verify_profile(profile_path, &job), Ok((_, digest)) if digest == fingerprint)
            || verify_inputs(&job).is_err() || cancelled.load(Ordering::Acquire)
        {
            let _ = fs::remove_dir_all(&artifact_dir);
            return Err(result.err().unwrap_or_else(|| {
                if cancelled.load(Ordering::Acquire) {
                    Refusal::WorkerStatus { status: protocol::Status::Cancelled,
                        reasons: vec!["cancelled_before_commit".into()],
                        omissions: vec!["receipt_discarded".into()] }
                } else { Refusal::Stale("authority or source changed before commit") }
            }));
        }
        Ok(Verified { job, payload: result.unwrap(), started_ms, finished_ms, artifact_dir })
    }
}

/// A successful scope is still staged; the owner may explicitly discard it.
pub fn discard_staged(receipt: &Verified) -> io::Result<()> {
    fs::remove_dir_all(&receipt.artifact_dir)
}
