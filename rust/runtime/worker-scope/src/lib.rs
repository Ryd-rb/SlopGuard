//! Worker lifetime is an OS scope, not merely the PID returned by Command::spawn.
//!
//! Stdio is backed by private, seekable files: a peer that never reads stdin or
//! leaves stdout/stderr inherited by descendants cannot strand a writer/reader
//! thread. The supervisor has one wall-clock budget for admission, execution,
//! exit, observation, cancellation, and drain. No fact is returned without a
//! successful direct worker exit AND positive scope drainage evidence.
use std::ffi::OsString;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::process::Command;
#[cfg(target_os = "linux")]
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const POLL: Duration = Duration::from_millis(10);
const DRAIN_GRACE: Duration = Duration::from_secs(3);
static NEXT: AtomicU64 = AtomicU64::new(0);

struct Budget<'a> {
    started: Instant,
    limit: Duration,
    current: &'a dyn Fn() -> bool,
}
impl Budget<'_> {
    fn active(&self, cancel: &AtomicBool) -> bool {
        !cancel.load(Ordering::SeqCst) && self.started.elapsed() < self.limit && (self.current)()
    }
    fn remaining(&self) -> Duration {
        self.limit.saturating_sub(self.started.elapsed())
    }
}

/// Private transient files prevent blocked pipe I/O from escaping the deadline.
/// They are only transport, never evidence of worker success on their own.
struct Files {
    directory: PathBuf,
    input: File,
    output: File,
    error: File,
}
impl Files {
    fn new(frame: &[u8]) -> Result<Self, String> {
        let mut directory = None;
        for _ in 0..16 {
            let sequence = NEXT.fetch_add(1, Ordering::Relaxed);
            let nanos = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos();
            let candidate = std::env::temp_dir().join(format!(
                "slophawk-shadow-{}-{nanos:x}-{sequence:x}",
                std::process::id()
            ));
            #[cfg(unix)]
            let created = {
                use std::os::unix::fs::DirBuilderExt;
                let mut builder = fs::DirBuilder::new();
                builder.mode(0o700);
                builder.create(&candidate)
            };
            #[cfg(not(unix))]
            let created = fs::create_dir(&candidate);
            match created {
                Ok(()) => {
                    directory = Some(candidate);
                    break;
                }
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(e) => return Err(format!("private worker transport directory: {e}")),
            }
        }
        let directory = directory.ok_or("cannot reserve private worker transport directory")?;
        let opened = (|| -> Result<(File, File, File), String> {
            let mut input = Self::open(&directory.join("request"))?;
            input
                .write_all(frame)
                .map_err(|e| format!("prepare worker request: {e}"))?;
            input.seek(SeekFrom::Start(0)).map_err(|e| e.to_string())?;
            let output = Self::open(&directory.join("stdout"))?;
            let error = Self::open(&directory.join("stderr"))?;
            Ok((input, output, error))
        })();
        match opened {
            Ok((input, output, error)) => Ok(Self {
                directory,
                input,
                output,
                error,
            }),
            Err(e) => {
                let _ = fs::remove_file(directory.join("request"));
                let _ = fs::remove_file(directory.join("stdout"));
                let _ = fs::remove_file(directory.join("stderr"));
                let _ = fs::remove_dir(&directory);
                Err(e)
            }
        }
    }
    fn open(path: &Path) -> Result<File, String> {
        let mut options = OpenOptions::new();
        options.read(true).write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        options
            .open(path)
            .map_err(|e| format!("cannot create private worker file: {e}"))
    }
    #[cfg(target_os = "linux")]
    fn stdio(&self) -> Result<(Stdio, Stdio, Stdio), String> {
        Ok((
            Stdio::from(self.input.try_clone().map_err(|e| e.to_string())?),
            Stdio::from(self.output.try_clone().map_err(|e| e.to_string())?),
            Stdio::from(self.error.try_clone().map_err(|e| e.to_string())?),
        ))
    }
    fn read_limited(file: &mut File, cap: usize) -> Result<Vec<u8>, String> {
        if file.metadata().map_err(|e| e.to_string())?.len() > cap as u64 {
            return Err("worker output exceeded byte cap".into());
        }
        file.seek(SeekFrom::Start(0)).map_err(|e| e.to_string())?;
        let mut bytes = Vec::new();
        file.take((cap as u64).saturating_add(1))
            .read_to_end(&mut bytes)
            .map_err(|e| e.to_string())?;
        if bytes.len() > cap {
            return Err("worker output exceeded byte cap".into());
        }
        Ok(bytes)
    }
    fn sizes_ok(&self, stdout_cap: usize, stderr_cap: usize) -> bool {
        self.output
            .metadata()
            .is_ok_and(|m| m.len() <= stdout_cap as u64)
            && self
                .error
                .metadata()
                .is_ok_and(|m| m.len() <= stderr_cap as u64)
    }
    fn capture(
        &mut self,
        stdout_cap: usize,
        stderr_cap: usize,
    ) -> Result<(Vec<u8>, Vec<u8>), String> {
        Ok((
            Self::read_limited(&mut self.output, stdout_cap)?,
            Self::read_limited(&mut self.error, stderr_cap)?,
        ))
    }
}
impl Drop for Files {
    fn drop(&mut self) {
        for name in ["request", "stdout", "stderr"] {
            let _ = fs::remove_file(self.directory.join(name));
        }
        let _ = fs::remove_dir(&self.directory);
    }
}

/// Exact executable, arguments and optional working directory. Repository data
/// must not supply an executable to this boundary. Callers retain grant and
/// digest checks and must validate the returned protocol terminal.
#[derive(Debug)]
pub struct Output {
    pub code: u32,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
}

#[derive(Clone, Copy)]
pub struct Limits {
    pub stdout: usize,
    pub stderr: usize,
    pub deadline: Duration,
}

/// Resolve the caller's configured tool before using scoped launch. The current
/// directory is not searched implicitly.
pub fn find_on_path(name: &str) -> Result<PathBuf, String> {
    let name = if cfg!(windows) {
        format!("{name}.exe")
    } else {
        name.to_owned()
    };
    let path = std::env::var_os("PATH").ok_or("PATH is unavailable")?;
    std::env::split_paths(&path)
        .filter(|root| root.is_absolute())
        .map(|root| root.join(&name))
        .find(|candidate| candidate.is_file())
        .ok_or_else(|| format!("{name} was not found on PATH"))
}

/// Launch in an authenticated OS scope and return output only after it drains.
/// Standard input is staged before launch; no pipe writer can delay the deadline.
pub fn run(
    command: &Command,
    input: &[u8],
    limits: Limits,
    cancel: &AtomicBool,
) -> Result<Output, String> {
    run_guarded(command, input, limits, cancel, &|| true)
}

/// Also stop the worker when the caller's independent grant or input fence is revoked.
pub fn run_guarded(
    command: &Command,
    input: &[u8],
    limits: Limits,
    cancel: &AtomicBool,
    current: &dyn Fn() -> bool,
) -> Result<Output, String> {
    if !Path::new(command.get_program()).is_absolute()
        || command
            .get_current_dir()
            .is_some_and(|path| !path.is_absolute())
    {
        return Err("worker executable and working directory must be absolute".into());
    }
    if command
        .get_envs()
        .any(|(name, value)| std::env::var_os(name).as_deref() != value || value.is_none())
    {
        return Err("only explicitly forwarded caller environment is supported".into());
    }
    const MAX_TRANSPORT: usize = 32 * 1024 * 1024;
    if input.len() > MAX_TRANSPORT
        || limits.stdout > MAX_TRANSPORT
        || limits.stderr > MAX_TRANSPORT
        || limits.deadline.is_zero()
        || limits.deadline > Duration::from_secs(1800)
    {
        return Err("worker transport limits exceeded".into());
    }
    let budget = Budget {
        started: Instant::now(),
        limit: limits.deadline,
        current,
    };
    if !budget.active(cancel) {
        return Err("worker cancelled before admission".into());
    }
    let mut files = Files::new(input)?;
    let worker = Invocation {
        program: command.get_program().to_owned(),
        args: command.get_args().map(ToOwned::to_owned).collect(),
        cwd: command.get_current_dir().map(ToOwned::to_owned),
        #[cfg(target_os = "linux")]
        env: command
            .get_envs()
            .map(|(name, _)| name.to_owned())
            .collect(),
    };
    #[cfg(target_os = "linux")]
    let code = linux::run(
        &worker,
        &mut files,
        limits.stdout,
        limits.stderr,
        &budget,
        cancel,
    )?;
    #[cfg(target_os = "windows")]
    let code = windows::run_recoverable(&worker, &mut files, limits, &budget, cancel)?;
    #[cfg(not(any(target_os = "linux", target_os = "windows")))]
    let code = return Err("unsupported worker scope platform: no authenticated containment".into());
    if !budget.active(cancel) {
        return Err("worker deadline or cancellation before fact publication".into());
    }
    let (stdout, stderr) = files.capture(limits.stdout, limits.stderr)?;
    Ok(Output {
        code,
        stdout,
        stderr,
    })
}

struct Invocation {
    program: OsString,
    args: Vec<OsString>,
    cwd: Option<PathBuf>,
    #[cfg(target_os = "linux")]
    env: Vec<OsString>,
}

#[cfg(target_os = "linux")]
mod linux;

#[cfg(target_os = "windows")]
#[allow(unsafe_code)] // The Win32 Job Object / STARTUPINFOEX ABI is intentionally isolated here.
mod windows;
