//! OS-user admission and recoverable worker scopes.
use crate::protocol::{self, Budget, Executable, Job};
use serde::Deserialize;
use std::fs::{self, File};
use std::io::{self, Read};
use std::path::{Path, PathBuf};
#[cfg(any(target_os = "linux", windows))]
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

#[cfg(windows)]
mod windows;

pub(crate) fn physical(path: &Path) -> io::Result<bool> {
    let canonical = fs::canonicalize(path)?;
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        use slophawk_rust_project_context::platform_path_to_wire;
        let wire = |p: &Path| p.to_str().and_then(platform_path_to_wire);
        let Some(path_text) = wire(path) else { return Ok(false); };
        if Some(path_text) != wire(&canonical) { return Ok(false); }
        for ancestor in path.ancestors().filter(|p| p.has_root()) {
            if fs::symlink_metadata(ancestor)?.file_attributes() & 0x400 != 0 {
                return Ok(false);
            }
        }
        Ok(true)
    }
    #[cfg(not(windows))]
    Ok(canonical == path)
}

fn open_private(path: &Path, directory: bool) -> io::Result<File> {
    if !physical(path)? {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "nonphysical private path"));
    }
    #[cfg(windows)]
    return windows::open_checked(path, directory, true);
    #[cfg(target_os = "linux")]
    {
        use std::os::unix::fs::{MetadataExt, PermissionsExt};
        let file = File::open(path)?;
        let meta = file.metadata()?;
        if (if directory { !meta.is_dir() } else { !meta.is_file() })
            || meta.uid() != unsafe { linux::geteuid() }
            || meta.permissions().mode() & 0o077 != 0 {
            return Err(io::Error::new(io::ErrorKind::PermissionDenied, "path is not private to the OS user"));
        }
        Ok(file)
    }
    #[cfg(not(any(target_os = "linux", windows)))]
    {
        let _ = directory;
        Err(io::Error::new(io::ErrorKind::Unsupported, "OS user admission unavailable"))
    }
}

fn read_private_profile(path: &Path) -> io::Result<Vec<u8>> {
    let dir = path.parent().ok_or(io::ErrorKind::InvalidInput)?;
    let _directory = open_private(dir, true)?;
    let file = open_private(path, false)?;
    read_bounded(file, 32 * 1024)
}

/// Read an owner-authenticated scheduler configuration outside the checkout.
pub fn read_owner_document(path: &Path, root: &Path) -> io::Result<Vec<u8>> {
    if path.canonicalize()?.starts_with(root.canonicalize()?) {
        return Err(io::Error::new(io::ErrorKind::PermissionDenied, "checkout cannot grant execution"));
    }
    read_private_profile(path)
}

fn verify_profile(path: &Path, job: &Job) -> Result<(Profile, String), Refusal> {
    if !job.valid() { return Err(Refusal::Denied("invalid job identity or budget")); }
        let bytes = read_private_profile(path)
            .map_err(|_| Refusal::Denied("profile is not a bounded private regular file"))?;
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
            || !physical(Path::new(&profile.root)).map_err(|_| Refusal::Stale("workspace missing"))?
            || inside(path, Path::new(&profile.root))?
        {
            return Err(Refusal::Denied("scope, tool, policy, grant or budget not authorized"));
        }
        let output = Path::new(&profile.artifacts_root);
        if inside(output, Path::new(&profile.root))? || open_private(output, true).is_err() {
            return Err(Refusal::Denied("artifact root not owned and private outside checkout"));
        }
        verify_executable(&profile.worker)?;
        verify_executable(&profile.tool)?;
        match &job.operation {
            crate::protocol::Operation::CargoMetadata { variant, .. }
            | crate::protocol::Operation::CargoTest { variant, .. } => {
                verify_executable(&variant.rustc)?;
                verify_executable(&variant.rustdoc)?;
                if let Some(msvc) = &variant.msvc {
                    verify_executable(&msvc.linker)?;
                    for lib in &msvc.library_paths {
                        let dir = Path::new(lib);
                        if !physical(dir).unwrap_or(false) || !dir.is_dir()
                            || inside(dir, Path::new(&profile.root))? {
                            return Err(Refusal::Denied("MSVC library directory is not physical and external"));
                        }
                        #[cfg(windows)]
                        windows::open_checked(dir, true, false)
                            .map_err(|_| Refusal::Denied("MSVC library directory ownership check failed"))?;
                    }
                }
            },
            _ => {}
        }
        Ok((profile, protocol::sha256(&bytes)))
}

fn inside(path: &Path, root: &Path) -> Result<bool, Refusal> {
    let denied = |_| Refusal::Denied("scope path unavailable");
    Ok(path.canonicalize().map_err(denied)?.starts_with(root.canonicalize().map_err(denied)?))
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
    #[cfg(windows)]
    let file = {
        if !physical(path).map_err(|_| Refusal::Denied("executable unavailable"))? {
            return Err(Refusal::Denied("executable is not a physical Windows path"));
        }
        windows::open_checked(path, false, false)
    };
    #[cfg(not(windows))]
    let file = File::open(path);
    let bytes = read_bounded(file.map_err(|_| Refusal::Denied("executable read or ownership check failed"))?, 64 * 1024 * 1024)
        .map_err(|_| Refusal::Denied("executable exceeds cap"))?;
    if protocol::sha256(&bytes) != executable.sha256 { return Err(Refusal::Stale("executable digest mismatch")); }
    Ok(())
}

/// Recheck only the OS-private profile's bytes on supervision ticks. Executable
/// bytes are checked fully before launch and after the worker exits.
fn current_fingerprint(path: &Path) -> Option<String> {
    Some(protocol::sha256(&read_private_profile(path).ok()?))
}

fn verify_inputs(job: &Job) -> Result<(), Refusal> {
    for input in &job.inputs {
        if !input.valid() { return Err(Refusal::Denied("invalid source ledger item")); }
        let path = Path::new(&job.identity.root).join(&input.relative_path);
        let meta = fs::symlink_metadata(&path).map_err(|_| Refusal::Stale("declared source missing"))?;
        if !meta.file_type().is_file() || meta.len() != input.byte_count
            || !physical(&path).map_err(|_| Refusal::Stale("source unavailable"))? {
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
    run_admitted_guarded(profile_path, job, cancelled, &|| true)
}

/// Also hold a coordinator-owned execution fence throughout the attempt.
pub fn run_admitted_guarded(
    profile_path: &Path,
    job: Job,
    cancelled: &AtomicBool,
    coordinator_current: &dyn Fn() -> bool,
) -> Result<Verified, Refusal> {
    let (profile, fingerprint) = verify_profile(profile_path, &job)?;
    verify_inputs(&job)?;
    #[cfg(not(any(target_os = "linux", windows)))]
    { let _ = (profile, fingerprint, cancelled, coordinator_current); return Err(Refusal::Unsupported("OS worker supervision unavailable")); }
    #[cfg(any(target_os = "linux", windows))]
    {
        if cancelled.load(Ordering::Acquire) { return Err(Refusal::WorkerStatus {
            status: protocol::Status::Cancelled, reasons: vec!["cancelled_before_admission".into()],
            omissions: vec!["not_launched".into()],
        }); }
        let artifact_dir = Path::new(&profile.artifacts_root).join(format!("{}-{}", job.identity.job_id, job.identity.attempt_id));
        let mut builder = fs::DirBuilder::new();
        #[cfg(target_os = "linux")]
        let directory = {
            use std::os::unix::fs::DirBuilderExt;
            builder.mode(0o700)
        };
        #[cfg(windows)]
        let directory = &mut builder;
        directory.create(&artifact_dir)
            .map_err(|_| Refusal::Denied("attempt directory must be new and exclusive"))?;
        if open_private(&artifact_dir, true).is_err() {
            let _ = fs::remove_dir(&artifact_dir);
            return Err(Refusal::Denied("attempt directory inherited unsafe permissions"));
        }
        #[cfg(target_os = "linux")]
        let run_scope = slophawk_worker_scope::run_guarded;
        #[cfg(windows)]
        let run_scope = slophawk_worker_scope::run_guarded_clean;
        #[cfg(target_os = "linux")]
        let mut command = {
            // systemd starts the fixed environment tool inside the worker scope.
            let mut command = Command::new("/usr/bin/env");
            command.args(["-i", "LC_ALL=C", "LANG=C"]).arg(&profile.worker.path);
            command
        };
        #[cfg(windows)]
        let mut command = {
            let mut command = Command::new(&profile.worker.path);
            crate::runner::controlled_env(&mut command)
                .map_err(|_| Refusal::Failed("Windows environment unavailable"))?;
            command
        };
        command.current_dir(&job.identity.root).arg("--migration-job-1").arg(&artifact_dir);
        let mut request = Vec::new();
        protocol::write_frame(&mut request, &job).map_err(|_| Refusal::Failed("invalid worker request"))?;
        let started_ms = unix_ms();
        let current = || coordinator_current()
            && current_fingerprint(profile_path).as_deref() == Some(fingerprint.as_str());
        let output = match run_scope(&command, &request,
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
        if result.is_err() || !coordinator_current()
            || !matches!(verify_profile(profile_path, &job), Ok((_, digest)) if digest == fingerprint)
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
