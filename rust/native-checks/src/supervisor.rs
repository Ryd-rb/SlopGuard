//! Worker lifetime is an OS scope, not merely the PID returned by Command::spawn.
//!
//! Stdio is backed by private, seekable files: a peer that never reads stdin or
//! leaves stdout/stderr inherited by descendants cannot strand a writer/reader
//! thread. The supervisor has one wall-clock budget for admission, execution,
//! exit, observation, cancellation, and drain. No fact is returned without a
//! successful direct worker exit AND positive scope drainage evidence.
use crate::contract::Worker;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const POLL: Duration = Duration::from_millis(10);
const DRAIN_GRACE: Duration = Duration::from_secs(3);
static NEXT: AtomicU64 = AtomicU64::new(0);

pub(crate) struct Budget { started: Instant, limit: Duration }
impl Budget {
    fn new(milliseconds: u64) -> Self { Self { started: Instant::now(), limit: Duration::from_millis(milliseconds) } }
    fn active(&self, cancel: &AtomicBool) -> bool {
        !cancel.load(Ordering::SeqCst) && self.started.elapsed() < self.limit
    }
    fn remaining(&self) -> Duration { self.limit.saturating_sub(self.started.elapsed()) }
}

/// Private transient files prevent blocked pipe I/O from escaping the deadline.
/// They are only transport, never evidence of worker success on their own.
struct Files { directory: PathBuf, input: File, output: File, error: File }
impl Files {
    fn new(frame: &[u8]) -> Result<Self, String> {
        let mut directory = None;
        for _ in 0..16 {
            let sequence = NEXT.fetch_add(1, Ordering::Relaxed);
            let nanos = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_nanos();
            let candidate = std::env::temp_dir().join(format!("slophawk-shadow-{}-{nanos:x}-{sequence:x}", std::process::id()));
            #[cfg(unix)]
            let created = {
                use std::os::unix::fs::DirBuilderExt;
                let mut builder = fs::DirBuilder::new();
                builder.mode(0o700); builder.create(&candidate)
            };
            #[cfg(not(unix))]
            let created = fs::create_dir(&candidate);
            match created {
                Ok(()) => { directory = Some(candidate); break; }
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(e) => return Err(format!("private worker transport directory: {e}")),
            }
        }
        let directory = directory.ok_or("cannot reserve private worker transport directory")?;
        let opened = (|| -> Result<(File, File, File), String> {
            let mut input = Self::open(&directory.join("request"))?;
            input.write_all(frame).map_err(|e| format!("prepare worker request: {e}"))?;
            input.seek(SeekFrom::Start(0)).map_err(|e| e.to_string())?;
            let output = Self::open(&directory.join("stdout"))?;
            let error = Self::open(&directory.join("stderr"))?;
            Ok((input, output, error))
        })();
        match opened {
            Ok((input, output, error)) => Ok(Self { directory, input, output, error }),
            Err(e) => { let _ = fs::remove_file(directory.join("request"));
                let _ = fs::remove_file(directory.join("stdout"));
                let _ = fs::remove_file(directory.join("stderr"));
                let _ = fs::remove_dir(&directory); Err(e) }
        }
    }
    fn open(path: &Path) -> Result<File, String> {
        let mut options = OpenOptions::new();
        options.read(true).write(true).create_new(true);
        #[cfg(unix)] { use std::os::unix::fs::OpenOptionsExt; options.mode(0o600); }
        options.open(path).map_err(|e| format!("cannot create private worker file: {e}"))
    }
    fn stdio(&self) -> Result<(Stdio, Stdio, Stdio), String> {
        Ok((Stdio::from(self.input.try_clone().map_err(|e| e.to_string())?),
            Stdio::from(self.output.try_clone().map_err(|e| e.to_string())?),
            Stdio::from(self.error.try_clone().map_err(|e| e.to_string())?)))
    }
    fn read_limited(file: &mut File, cap: usize) -> Result<Vec<u8>, String> {
        if file.metadata().map_err(|e| e.to_string())?.len() > cap as u64 { return Err("worker output exceeded byte cap".into()); }
        file.seek(SeekFrom::Start(0)).map_err(|e| e.to_string())?;
        let mut bytes = Vec::new();
        file.take((cap as u64).saturating_add(1)).read_to_end(&mut bytes).map_err(|e| e.to_string())?;
        if bytes.len() > cap { return Err("worker output exceeded byte cap".into()); }
        Ok(bytes)
    }
    fn sizes_ok(&self, stdout_cap: usize, stderr_cap: usize) -> bool {
        self.output.metadata().is_ok_and(|m| m.len() <= stdout_cap as u64) &&
            self.error.metadata().is_ok_and(|m| m.len() <= stderr_cap as u64)
    }
    fn capture(&mut self, stdout_cap: usize, stderr_cap: usize) -> Result<(Vec<u8>, Vec<u8>), String> {
        Ok((Self::read_limited(&mut self.output, stdout_cap)?,
            Self::read_limited(&mut self.error, stderr_cap)?))
    }
}
impl Drop for Files {
    fn drop(&mut self) {
        for name in ["request", "stdout", "stderr"] { let _ = fs::remove_file(self.directory.join(name)); }
        let _ = fs::remove_dir(&self.directory);
    }
}

pub(crate) fn exchange(worker: &Worker, frame: &[u8], stdout_cap: usize, stderr_cap: usize,
    deadline_ms: u64, cancel: &AtomicBool) -> Result<(Vec<u8>, Vec<u8>), String>
{
    let budget = Budget::new(deadline_ms);
    let mut files = Files::new(frame)?;
    if !budget.active(cancel) { return Err("worker cancelled before admission".into()); }
    #[cfg(target_os = "linux")]
    let result = linux::run(worker, &mut files, stdout_cap, stderr_cap, &budget, cancel);
    #[cfg(target_os = "windows")]
    let result = windows::run(worker, &mut files, stdout_cap, stderr_cap, &budget, cancel);
    #[cfg(not(any(target_os = "linux", target_os = "windows")))]
    let result: Result<(), String> = Err("unsupported worker scope platform: no authenticated containment".into());
    result?;
    if !budget.active(cancel) { return Err("worker deadline or cancellation before fact publication".into()); }
    files.capture(stdout_cap, stderr_cap)
}

#[cfg(target_os = "linux")]
mod linux {
    use super::*;
    use std::collections::HashMap;
    use std::process::{Child, Command, ExitStatus};

    const SYSTEMD_RUN: &str = "/usr/bin/systemd-run";
    const SYSTEMCTL: &str = "/usr/bin/systemctl";
    const CGROUP_ROOT: &str = "/sys/fs/cgroup";
    const HELPER_BUDGET: Duration = Duration::from_millis(700);

    struct Service { name: String, invocation: Option<String>, group: Option<String> }
    #[derive(Debug)]
    struct Observation { load: String, active: String, sub: String, invocation: String,
        group: String, result: String, main_code: String, main_status: String }

    fn random_unit() -> Result<String, String> {
        let mut entropy = [0u8; 16];
        File::open("/dev/urandom").and_then(|mut f| f.read_exact(&mut entropy))
            .map_err(|e| format!("cannot allocate authenticated scope nonce: {e}"))?;
        if entropy.iter().all(|b| *b == 0) { return Err("scope nonce was zero".into()); }
        Ok(format!("slophawk-shadow-{}.service", entropy.iter().map(|b| format!("{b:02x}")).collect::<String>()))
    }
    fn helper(program: &str, args: &[&str], directory: &Path, remaining: Duration) -> Result<(ExitStatus, String), String> {
        // Helpers have their own bounded lifetime and file-backed output; never use
        // Command::output(), which can hang on descendant-held pipes.
        let sequence = NEXT.fetch_add(1, Ordering::Relaxed);
        let stdout_path = directory.join(format!("helper-{sequence}-stdout"));
        let stderr_path = directory.join(format!("helper-{sequence}-stderr"));
        let mut out = Files::open(&stdout_path)?;
        let err = Files::open(&stderr_path)?;
        let result = (|| -> Result<(ExitStatus, String), String> {
            let mut child = Command::new(program).args(args).stdin(Stdio::null())
                .stdout(Stdio::from(out.try_clone().map_err(|e| e.to_string())?))
                .stderr(Stdio::from(err.try_clone().map_err(|e| e.to_string())?))
                .spawn().map_err(|e| format!("scope manager helper: {e}"))?;
            let until = Instant::now() + remaining.min(HELPER_BUDGET);
            let status = loop {
                match child.try_wait() {
                    Ok(Some(s)) => break s,
                    Ok(None) if Instant::now() < until => std::thread::sleep(POLL),
                    Ok(None) => { let _ = child.kill(); let _ = child.try_wait(); return Err("scope manager helper deadline expired".into()); }
                    Err(e) => { let _ = child.kill(); let _ = child.try_wait(); return Err(format!("scope manager helper wait: {e}")); }
                }
            };
            let bytes = Files::read_limited(&mut out, 16 * 1024)?;
            let output = String::from_utf8(bytes).map_err(|_| "scope manager emitted non-UTF-8 text")?;
            Ok((status, output))
        })();
        drop(out); drop(err);
        let _ = fs::remove_file(stdout_path); let _ = fs::remove_file(stderr_path);
        result
    }
    fn show(service: &str, directory: &Path, remaining: Duration) -> Result<Observation, String> {
        let (status, lines) = helper(SYSTEMCTL, &["--user", "--no-pager", "show", "--property=LoadState,ActiveState,SubState,InvocationID,ControlGroup,Result,ExecMainCode,ExecMainStatus", service], directory, remaining)?;
        if !status.success() { return Err("systemctl show did not authenticate the worker scope".into()); }
        let mut values = HashMap::new();
        for line in lines.lines() {
            let (key, value) = line.split_once('=').ok_or("malformed systemctl scope observation")?;
            if values.insert(key, value).is_some() { return Err("duplicate systemctl scope field".into()); }
        }
        let get = |k: &str| values.get(k).map(|v| (*v).to_owned()).ok_or_else(|| format!("missing systemctl scope property {k}"));
        Ok(Observation {load: get("LoadState")?, active: get("ActiveState")?, sub: get("SubState")?,
            invocation: get("InvocationID")?, group: get("ControlGroup")?,
            result: get("Result")?, main_code: get("ExecMainCode")?, main_status: get("ExecMainStatus")? })
    }
    fn safe_group(group: &str, unit: &str) -> bool {
        group.starts_with('/') && group.ends_with(&format!("/{unit}")) && group.len() < 2048 &&
            !group.contains("//") && !group.chars().any(char::is_control) &&
            !group.split('/').any(|c| c == "." || c == "..")
    }
    fn populated(group: &str, unit: &str) -> Result<Option<bool>, String> {
        if !safe_group(group, unit) { return Err("untrusted cgroup identity".into()); }
        let path = Path::new(CGROUP_ROOT).join(&group[1..]).join("cgroup.events");
        let contents = match fs::read_to_string(&path) {
            Ok(v) => v,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(format!("cannot verify worker cgroup drain: {e}")),
        };
        for line in contents.lines() {
            if let Some(value) = line.strip_prefix("populated ") {
                return match value { "0" => Ok(Some(false)), "1" => Ok(Some(true)),
                    _ => Err("unrecognized cgroup population".into()) };
            }
        }
        Err("cgroup.events omitted populated field".into())
    }
    fn verified(service: &mut Service, o: &Observation) -> Result<(), String> {
        if o.load != "loaded" || o.invocation.len() != 32 || !o.invocation.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err("systemd unit lacked authenticated invocation identity".into());
        }
        if service.invocation.as_ref().is_some_and(|v| v != &o.invocation) {
            return Err("worker scope invocation was replaced".into());
        }
        // After the same invocation terminates, systemd may clear ControlGroup.
        // It must have been pinned while the service was active, never inferred
        // from an empty or missing post-exit property.
        if !o.group.is_empty() && !safe_group(&o.group, &service.name) {
            return Err("systemd returned an untrusted cgroup identity".into());
        }
        if service.group.as_ref().is_some_and(|v| !o.group.is_empty() && v != &o.group) {
            return Err("worker scope changed cgroup".into());
        }
        if service.group.is_none() {
            if !safe_group(&o.group, &service.name) { return Err("worker cgroup could not be pinned".into()); }
            service.group = Some(o.group.clone());
        }
        service.invocation.get_or_insert_with(|| o.invocation.clone());
        Ok(())
    }
    fn drained(service: &mut Service, directory: &Path, remaining: Duration) -> Result<bool, String> {
        let o = show(&service.name, directory, remaining)?;
        verified(service, &o)?;
        let terminal = (o.active == "inactive" && o.sub == "dead") ||
            (o.active == "active" && o.sub == "exited") || (o.active == "failed" && o.sub == "failed");
        if !terminal { return Ok(false); }
        // A missing *previously pinned* cgroup is a drainage witness only if
        // the same manager invocation is terminal. Missing before pinning fails.
        Ok(matches!(populated(service.group.as_deref().ok_or("unpinned worker cgroup")?, &service.name)?,
            Some(false) | None))
    }
    fn kill(service: &Service, directory: &Path, deadline: Duration) -> Result<(), String> {
        let (status, _) = helper(SYSTEMCTL, &["--user", "kill", "--kill-whom=all", "--signal=SIGKILL", &service.name], directory, deadline)?;
        if status.success() { Ok(()) } else { Err("systemd could not kill the full worker cgroup".into()) }
    }
    fn remove(service: &Service, directory: &Path) {
        // The service was positively witnessed drained before this point.
        let _ = helper(SYSTEMCTL, &["--user", "stop", &service.name], directory, HELPER_BUDGET);
        let _ = helper(SYSTEMCTL, &["--user", "reset-failed", &service.name], directory, HELPER_BUDGET);
    }
    fn until_drained(service: &mut Service, directory: &Path, max_wait: Duration) -> Result<(), String> {
        let end = Instant::now() + max_wait;
        loop {
            let remaining = end.saturating_duration_since(Instant::now());
            if remaining.is_zero() { return Err("worker scope drainage unproven at deadline".into()); }
            match drained(service, directory, remaining) {
                Ok(true) => return Ok(()),
                Ok(false) => std::thread::sleep(POLL.min(remaining)),
                Err(e) => return Err(e),
            }
        }
    }
    fn admission(directory: &Path, budget: &Budget) -> Result<Service, String> {
        if !Path::new("/sys/fs/cgroup/cgroup.controllers").is_file() { return Err("unsupported: Linux cgroup v2 is unavailable".into()); }
        if !Path::new(SYSTEMD_RUN).is_file() || !Path::new(SYSTEMCTL).is_file() { return Err("unsupported: systemd user manager helpers absent".into()); }
        let service = Service { name: random_unit()?, invocation: None, group: None };
        let previous = show(&service.name, directory, budget.remaining())?;
        if previous.load != "not-found" { return Err("scope unit name was already allocated".into()); }
        Ok(service)
    }
    pub(super) fn run(worker: &Worker, files: &mut Files, stdout_cap: usize, stderr_cap: usize,
        budget: &Budget, cancel: &AtomicBool) -> Result<(), String>
    {
        let mut service = admission(&files.directory, budget)?;
        if !budget.active(cancel) { return Err("worker cancelled before systemd admission".into()); }
        let (stdin, stdout, stderr) = files.stdio()?;
        let seconds = budget.remaining().as_secs().saturating_add(3).max(4);
        let runtime = format!("--property=RuntimeMaxSec={seconds}s");
        let fsize = format!("--property=LimitFSIZE={}", stdout_cap.max(stderr_cap));
        let unit = format!("--unit={}", service.name);
        let mut child: Child = Command::new(SYSTEMD_RUN)
            .args(["--user", "--quiet", "--wait", "--pipe",
                "--service-type=exec", "--property=KillMode=control-group",
                "--property=SendSIGKILL=yes", runtime.as_str(), fsize.as_str(), unit.as_str(),
                "--", worker.dotnet.as_str(), worker.assembly.as_str()])
            .stdin(stdin).stdout(stdout).stderr(stderr)
            .spawn().map_err(|e| format!("systemd worker service launch failed: {e}"))?;
        let mut pinned = false;
        let mut exit = None;
        let mut reason: Option<String> = None;
        while budget.active(cancel) {
            if !files.sizes_ok(stdout_cap, stderr_cap) { reason = Some("worker exceeded transport output limit".into()); break; }
            if !pinned {
                if let Ok(o) = show(&service.name, &files.directory, budget.remaining()) {
                    if o.load == "loaded" {
                        if let Err(e) = verified(&mut service, &o) { reason = Some(e); break; }
                        pinned = true;
                    }
                }
            }
            match child.try_wait() {
                Ok(Some(code)) => { exit = Some(code); break; }
                Ok(None) => std::thread::sleep(POLL),
                Err(e) => { reason = Some(format!("systemd-run wait: {e}")); break; }
            }
        }
        if !pinned {
            reason.get_or_insert_with(|| "missing authenticated worker cgroup/InvocationID".into());
        }
        if !budget.active(cancel) { reason.get_or_insert_with(|| "C# worker deadline or cancellation".into()); }
        if reason.is_some() || exit.is_none() {
            // Never treat process-group signaling as containment. Kill the unit,
            // then require manager+cgroup evidence; errors remain failures.
            let killed = kill(&service, &files.directory, DRAIN_GRACE);
            let _ = child.kill(); let _ = child.try_wait();
            let drain = if pinned { until_drained(&mut service, &files.directory, DRAIN_GRACE) }
                else { Err("scope identity never pinned; drain not proven".into()) };
            if drain.is_ok() { remove(&service, &files.directory); }
            return Err(format!("{}; scope kill: {}; scope drain: {}",
                reason.unwrap_or_else(|| "worker did not exit".into()),
                killed.err().unwrap_or_else(|| "sent".into()),
                drain.err().unwrap_or_else(|| "verified".into())));
        }
        let exit = exit.ok_or("worker scope manager lacked an exit status")?;
        if !pinned { return Err("worker scope identity not authenticated".into()); }
        if let Err(e) = until_drained(&mut service, &files.directory, budget.remaining()) {
            // The launcher may exit before descendants. Never strand the unit
            // when normal completion cannot prove process-tree drainage.
            let killed = kill(&service, &files.directory, DRAIN_GRACE);
            let drain = until_drained(&mut service, &files.directory, DRAIN_GRACE);
            if drain.is_ok() { remove(&service, &files.directory); }
            return Err(format!("{e}; recursive kill: {}; drain: {}",
                killed.err().unwrap_or_else(|| "sent".into()),
                drain.err().unwrap_or_else(|| "verified".into())));
        }
        let o = show(&service.name, &files.directory, budget.remaining())?;
        verified(&mut service, &o)?;
        let status_ok = o.result == "success" && o.main_code == "1" && o.main_status == "0";
        remove(&service, &files.directory);
        if !exit.success() || !status_ok { return Err("C# worker or scoped service exited unsuccessfully; staged facts discarded".into()); }
        if !files.sizes_ok(stdout_cap, stderr_cap) { return Err("worker output exceeded byte cap".into()); }
        Ok(())
    }
}

#[cfg(target_os = "windows")]
#[allow(unsafe_code)] // The Win32 Job Object / STARTUPINFOEX ABI is intentionally isolated here.
mod windows {
    use super::*;
    use std::ffi::c_void;
    use std::mem::{size_of, size_of_val};
    use std::os::windows::ffi::OsStrExt;
    use std::os::windows::io::AsRawHandle;
    use std::ptr::{null, null_mut};

    type RawHandle = *mut c_void;
    const KILL_ON_JOB_CLOSE: u32 = 0x00002000;
    const EXTENDED_JOB_LIMITS: i32 = 9;
    const BASIC_ACCOUNTING: i32 = 1;
    const HANDLE_LIST: usize = 0x0002_0002;
    const JOB_LIST: usize = 0x0002_000d;
    const EXTENDED_STARTUP: u32 = 0x0008_0000;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    const USE_STD_HANDLES: u32 = 0x0000_0100;
    const DUP_SAME_ACCESS: u32 = 0x0000_0002;
    const WAIT_OBJECT_0: u32 = 0;
    const WAIT_TIMEOUT: u32 = 258;

    #[repr(C)]
    #[derive(Default)]
    struct BasicLimits {
        process_time: i64, job_time: i64, flags: u32,
        min_working_set: usize, max_working_set: usize, active_process_limit: u32,
        affinity: usize, priority: u32, scheduling: u32,
    }
    #[repr(C)]
    #[derive(Default)]
    struct IoCounters { read_ops: u64, write_ops: u64, other_ops: u64,
        read_bytes: u64, write_bytes: u64, other_bytes: u64 }
    #[repr(C)]
    #[derive(Default)]
    struct ExtendedLimits {
        basic: BasicLimits, io: IoCounters,
        process_memory: usize, job_memory: usize, peak_process: usize, peak_job: usize,
    }
    #[repr(C)]
    #[derive(Default)]
    struct Accounting {
        user_time: i64, kernel_time: i64, period_user: i64, period_kernel: i64,
        page_faults: u32, total: u32, active: u32, terminated: u32,
    }
    #[repr(C)]
    struct StartupInfo {
        size: u32, reserved: *mut u16, desktop: *mut u16, title: *mut u16,
        x: u32, y: u32, width: u32, height: u32, columns: u32, rows: u32,
        fill: u32, flags: u32, show_window: u16, reserved_bytes: u16,
        reserved_data: *mut u8, stdin: RawHandle, stdout: RawHandle, stderr: RawHandle,
    }
    #[repr(C)]
    struct Startup { info: StartupInfo, attributes: *mut c_void }
    #[repr(C)]
    struct ProcessInfo { process: RawHandle, thread: RawHandle, pid: u32, tid: u32 }

    #[link(name="kernel32")]
    unsafe extern "system" {
        fn CreateJobObjectW(attributes: *mut c_void, name: *const u16) -> RawHandle;
        fn SetInformationJobObject(job: RawHandle, class: i32, info: *const c_void, len: u32) -> i32;
        fn QueryInformationJobObject(job: RawHandle, class: i32, info: *mut c_void,
            len: u32, out: *mut u32) -> i32;
        fn TerminateJobObject(job: RawHandle, exit_code: u32) -> i32;
        fn InitializeProcThreadAttributeList(list: *mut c_void, count: u32, flags: u32,
            size: *mut usize) -> i32;
        fn UpdateProcThreadAttribute(list: *mut c_void, flags: u32, key: usize,
            value: *mut c_void, size: usize, previous: *mut c_void, returned: *mut usize) -> i32;
        fn DeleteProcThreadAttributeList(list: *mut c_void);
        fn GetCurrentProcess() -> RawHandle;
        fn DuplicateHandle(from: RawHandle, source: RawHandle, to: RawHandle,
            duplicated: *mut RawHandle, access: u32, inherit: i32, options: u32) -> i32;
        fn CreateProcessW(executable: *const u16, arguments: *mut u16,
            process_attributes: *mut c_void, thread_attributes: *mut c_void,
            inherit: i32, creation_flags: u32, environment: *mut c_void, directory: *const u16,
            startup: *mut Startup, process: *mut ProcessInfo) -> i32;
        fn CloseHandle(handle: RawHandle) -> i32;
        fn WaitForSingleObject(handle: RawHandle, milliseconds: u32) -> u32;
        fn GetExitCodeProcess(handle: RawHandle, code: *mut u32) -> i32;
    }
    struct Handle(RawHandle);
    impl Handle {
        fn new(raw: RawHandle, context: &str) -> Result<Self, String> {
            if raw.is_null() || raw as isize == -1 {
                Err(format!("{context}: {}", std::io::Error::last_os_error()))
            } else { Ok(Self(raw)) }
        }
    }
    impl Drop for Handle {
        fn drop(&mut self) { unsafe { CloseHandle(self.0); } }
    }
    fn duplicate(file: &File) -> Result<Handle, String> {
        let mut result = null_mut();
        let self_process = unsafe { GetCurrentProcess() };
        let success = unsafe { DuplicateHandle(self_process, file.as_raw_handle() as RawHandle,
            self_process, &mut result, 0, 1, DUP_SAME_ACCESS) };
        if success == 0 { return Err(format!("inheritable worker transport handle: {}", std::io::Error::last_os_error())); }
        Handle::new(result, "duplicate worker handle")
    }
    fn wide(value: &std::ffi::OsStr) -> Vec<u16> { value.encode_wide().chain(std::iter::once(0)).collect() }
    // Windows CreateProcess command-line quoting (backslashes preceding a quote
    // and the final delimiter must be doubled).
    fn quote(value: &str) -> String {
        let mut result = String::from("\"");
        let mut slashes = 0;
        for ch in value.chars() {
            if ch == '\\' { slashes += 1; continue; }
            for _ in 0..(if ch == '"' { slashes * 2 + 1 } else { slashes }) { result.push('\\'); }
            if ch == '"' { result.push('"'); } else { result.push(ch); }
            slashes = 0;
        }
        for _ in 0..slashes * 2 { result.push('\\'); }
        result.push('"');
        result
    }
    fn active(job: &Handle) -> Result<u32, String> {
        let mut counts = Accounting::default();
        let success = unsafe { QueryInformationJobObject(job.0, BASIC_ACCOUNTING,
            (&mut counts as *mut Accounting).cast(), size_of::<Accounting>() as u32, null_mut()) };
        if success == 0 { return Err(format!("Windows job accounting: {}", std::io::Error::last_os_error())); }
        Ok(counts.active)
    }
    fn terminate(job: &Handle) -> Result<(), String> {
        if unsafe { TerminateJobObject(job.0, 1) } == 0 {
            Err(format!("Windows job termination failed: {}", std::io::Error::last_os_error()))
        } else { Ok(()) }
    }
    fn wait_drained(job: &Handle, remaining: Duration) -> Result<(), String> {
        let end = Instant::now() + remaining;
        loop {
            if active(job)? == 0 { return Ok(()); }
            if Instant::now() >= end { return Err("Windows worker job did not drain".into()); }
            std::thread::sleep(POLL);
        }
    }
    fn launch(worker: &Worker, files: &Files, job: &Handle) -> Result<Handle, String> {
        let stdin = duplicate(&files.input)?;
        let stdout = duplicate(&files.output)?;
        let stderr = duplicate(&files.error)?;
        // The attribute buffer is word-aligned and stays alive until CreateProcessW
        // has copied the handle whitelist and job assignment into the new process.
        let mut size = 0usize;
        unsafe { InitializeProcThreadAttributeList(null_mut(), 2, 0, &mut size); }
        if size == 0 { return Err("cannot size extended startup attributes".into()); }
        let mut buffer = vec![0usize; (size + size_of::<usize>() - 1) / size_of::<usize>()];
        let attribute_list = buffer.as_mut_ptr().cast::<c_void>();
        if unsafe { InitializeProcThreadAttributeList(attribute_list, 2, 0, &mut size) } == 0 {
            return Err(format!("initialize Windows worker launch attributes: {}", std::io::Error::last_os_error()));
        }
        let created = (|| -> Result<Handle, String> {
            let mut job_handle = job.0;
            let mut inherited = [stdin.0, stdout.0, stderr.0];
            if unsafe { UpdateProcThreadAttribute(attribute_list, 0, JOB_LIST,
                (&mut job_handle as *mut RawHandle).cast(), size_of::<RawHandle>(), null_mut(), null_mut()) } == 0 ||
               unsafe { UpdateProcThreadAttribute(attribute_list, 0, HANDLE_LIST,
                inherited.as_mut_ptr().cast(), size_of_val(&inherited), null_mut(), null_mut()) } == 0 {
                return Err(format!("assign job/stdio before worker exec: {}", std::io::Error::last_os_error()));
            }
            let mut startup = Startup {
                info: StartupInfo {
                    size: size_of::<Startup>() as u32, reserved: null_mut(), desktop: null_mut(),
                    title: null_mut(), x: 0, y: 0, width: 0, height: 0, columns: 0,
                    rows: 0, fill: 0, flags: USE_STD_HANDLES, show_window: 0, reserved_bytes: 0,
                    reserved_data: null_mut(), stdin: stdin.0, stdout: stdout.0, stderr: stderr.0,
                }, attributes: attribute_list,
            };
            let program = wide(std::ffi::OsStr::new(&worker.dotnet));
            let command = format!("{} {}", quote(&worker.dotnet), quote(&worker.assembly));
            let mut arguments = wide(std::ffi::OsStr::new(&command));
            let mut info = ProcessInfo { process: null_mut(), thread: null_mut(), pid: 0, tid: 0 };
            let success = unsafe { CreateProcessW(program.as_ptr(), arguments.as_mut_ptr(),
                null_mut(), null_mut(), 1, EXTENDED_STARTUP | CREATE_NO_WINDOW,
                null_mut(), null(), &mut startup, &mut info) };
            if success == 0 { return Err(format!("Windows scoped worker launch failed: {}", std::io::Error::last_os_error())); }
            let thread = Handle::new(info.thread, "Windows primary thread handle")?;
            let process = Handle::new(info.process, "Windows worker process handle")?;
            drop(thread);
            Ok(process)
        })();
        unsafe { DeleteProcThreadAttributeList(attribute_list); }
        created
    }
    pub(super) fn run(worker: &Worker, files: &mut Files, stdout_cap: usize, stderr_cap: usize,
        budget: &Budget, cancel: &AtomicBool) -> Result<(), String>
    {
        let job = Handle::new(unsafe { CreateJobObjectW(null_mut(), null()) }, "create Windows Job Object")?;
        let mut limits = ExtendedLimits::default();
        limits.basic.flags = KILL_ON_JOB_CLOSE;
        if unsafe { SetInformationJobObject(job.0, EXTENDED_JOB_LIMITS,
            (&limits as *const ExtendedLimits).cast(), size_of::<ExtendedLimits>() as u32) } == 0 {
            return Err(format!("set Windows kill-on-close job limit: {}", std::io::Error::last_os_error()));
        }
        if !budget.active(cancel) { return Err("worker cancelled before Windows job admission".into()); }
        let process = launch(worker, files, &job)?;
        let mut exit_code = None;
        let mut reason = None;
        while budget.active(cancel) {
            if !files.sizes_ok(stdout_cap, stderr_cap) { reason = Some("Windows worker output exceeded byte cap"); break; }
            match unsafe { WaitForSingleObject(process.0, 0) } {
                WAIT_OBJECT_0 => {
                    let mut code = 0;
                    if unsafe { GetExitCodeProcess(process.0, &mut code) } == 0 {
                        reason = Some("could not query Windows worker exit code");
                    } else { exit_code = Some(code); }
                    break;
                }
                WAIT_TIMEOUT => std::thread::sleep(POLL),
                _ => { reason = Some("Windows worker wait failed"); break; }
            }
        }
        if !budget.active(cancel) { reason.get_or_insert("Windows worker deadline/cancellation"); }
        if reason.is_some() || exit_code.is_none() {
            let signalled = terminate(&job);
            let drained = wait_drained(&job, DRAIN_GRACE);
            return Err(format!("{}; recursive terminate: {}; drained: {}",
                reason.unwrap_or("Windows worker exit missing"),
                signalled.err().unwrap_or_else(|| "yes".into()),
                drained.err().unwrap_or_else(|| "yes".into())));
        }
        match wait_drained(&job, budget.remaining()) {
            Ok(()) => (),
            Err(e) => { let _ = terminate(&job); let _ = wait_drained(&job, DRAIN_GRACE); return Err(e); }
        }
        if exit_code != Some(0) { return Err("Windows C# worker exited unsuccessfully; staged facts discarded".into()); }
        if !files.sizes_ok(stdout_cap, stderr_cap) { return Err("Windows C# worker output cap exceeded".into()); }
        Ok(())
    }
}
