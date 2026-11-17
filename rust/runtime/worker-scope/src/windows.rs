use super::*;
use std::ffi::c_void;
use std::mem::{size_of, size_of_val};
use std::os::windows::ffi::OsStrExt;
use std::os::windows::fs::{MetadataExt, OpenOptionsExt};
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
const ALREADY_EXISTS: u32 = 183;
const JOB_QUERY: u32 = 4;
const JOB_TERMINATE: u32 = 8;
const REPARSE_POINT: u32 = 0x400;
const OPEN_REPARSE_POINT: u32 = 0x0020_0000;

#[link(name = "bcrypt")]
unsafe extern "system" {
    fn BCryptGenRandom(algorithm: *mut c_void, buffer: *mut u8, size: u32, flags: u32) -> i32;
}

#[repr(C)]
#[derive(Default)]
struct BasicLimits {
    process_time: i64,
    job_time: i64,
    flags: u32,
    min_working_set: usize,
    max_working_set: usize,
    active_process_limit: u32,
    affinity: usize,
    priority: u32,
    scheduling: u32,
}
#[repr(C)]
#[derive(Default)]
struct IoCounters {
    read_ops: u64,
    write_ops: u64,
    other_ops: u64,
    read_bytes: u64,
    write_bytes: u64,
    other_bytes: u64,
}
#[repr(C)]
#[derive(Default)]
struct ExtendedLimits {
    basic: BasicLimits,
    io: IoCounters,
    process_memory: usize,
    job_memory: usize,
    peak_process: usize,
    peak_job: usize,
}
#[repr(C)]
#[derive(Default)]
struct Accounting {
    user_time: i64,
    kernel_time: i64,
    period_user: i64,
    period_kernel: i64,
    page_faults: u32,
    total: u32,
    active: u32,
    terminated: u32,
}
#[repr(C)]
struct StartupInfo {
    size: u32,
    reserved: *mut u16,
    desktop: *mut u16,
    title: *mut u16,
    x: u32,
    y: u32,
    width: u32,
    height: u32,
    columns: u32,
    rows: u32,
    fill: u32,
    flags: u32,
    show_window: u16,
    reserved_bytes: u16,
    reserved_data: *mut u8,
    stdin: RawHandle,
    stdout: RawHandle,
    stderr: RawHandle,
}
#[repr(C)]
struct Startup {
    info: StartupInfo,
    attributes: *mut c_void,
}
#[repr(C)]
struct ProcessInfo {
    process: RawHandle,
    thread: RawHandle,
    pid: u32,
    tid: u32,
}

#[link(name = "kernel32")]
unsafe extern "system" {
    fn CreateJobObjectW(attributes: *mut c_void, name: *const u16) -> RawHandle;
    fn OpenJobObjectW(access: u32, inherit: i32, name: *const u16) -> RawHandle;
    fn GetLastError() -> u32;
    fn SetInformationJobObject(job: RawHandle, class: i32, info: *const c_void, len: u32) -> i32;
    fn QueryInformationJobObject(
        job: RawHandle,
        class: i32,
        info: *mut c_void,
        len: u32,
        out: *mut u32,
    ) -> i32;
    fn TerminateJobObject(job: RawHandle, exit_code: u32) -> i32;
    fn InitializeProcThreadAttributeList(
        list: *mut c_void,
        count: u32,
        flags: u32,
        size: *mut usize,
    ) -> i32;
    fn UpdateProcThreadAttribute(
        list: *mut c_void,
        flags: u32,
        key: usize,
        value: *mut c_void,
        size: usize,
        previous: *mut c_void,
        returned: *mut usize,
    ) -> i32;
    fn DeleteProcThreadAttributeList(list: *mut c_void);
    fn GetCurrentProcess() -> RawHandle;
    fn DuplicateHandle(
        from: RawHandle,
        source: RawHandle,
        to: RawHandle,
        duplicated: *mut RawHandle,
        access: u32,
        inherit: i32,
        options: u32,
    ) -> i32;
    fn CreateProcessW(
        executable: *const u16,
        arguments: *mut u16,
        process_attributes: *mut c_void,
        thread_attributes: *mut c_void,
        inherit: i32,
        creation_flags: u32,
        environment: *mut c_void,
        directory: *const u16,
        startup: *mut Startup,
        process: *mut ProcessInfo,
    ) -> i32;
    fn CloseHandle(handle: RawHandle) -> i32;
    fn WaitForSingleObject(handle: RawHandle, milliseconds: u32) -> u32;
    fn GetExitCodeProcess(handle: RawHandle, code: *mut u32) -> i32;
}
struct Handle(RawHandle);
impl Handle {
    fn new(raw: RawHandle, context: &str) -> Result<Self, String> {
        if raw.is_null() || raw as isize == -1 {
            Err(format!("{context}: {}", std::io::Error::last_os_error()))
        } else {
            Ok(Self(raw))
        }
    }
}
impl Drop for Handle {
    fn drop(&mut self) {
        unsafe {
            CloseHandle(self.0);
        }
    }
}
fn duplicate(file: &File) -> Result<Handle, String> {
    let mut result = null_mut();
    let self_process = unsafe { GetCurrentProcess() };
    let success = unsafe {
        DuplicateHandle(
            self_process,
            file.as_raw_handle() as RawHandle,
            self_process,
            &mut result,
            0,
            1,
            DUP_SAME_ACCESS,
        )
    };
    if success == 0 {
        return Err(format!(
            "inheritable worker transport handle: {}",
            std::io::Error::last_os_error()
        ));
    }
    Handle::new(result, "duplicate worker handle")
}
fn wide(value: &std::ffi::OsStr) -> Vec<u16> {
    value.encode_wide().chain(std::iter::once(0)).collect()
}
// Windows CreateProcess command-line quoting (backslashes preceding a quote
// and the final delimiter must be doubled).
fn quote(value: &str) -> String {
    let mut result = String::from("\"");
    let mut slashes = 0;
    for ch in value.chars() {
        if ch == '\\' {
            slashes += 1;
            continue;
        }
        for _ in 0..(if ch == '"' { slashes * 2 + 1 } else { slashes }) {
            result.push('\\');
        }
        if ch == '"' {
            result.push('"');
        } else {
            result.push(ch);
        }
        slashes = 0;
    }
    for _ in 0..slashes * 2 {
        result.push('\\');
    }
    result.push('"');
    result
}
fn active(job: &Handle) -> Result<u32, String> {
    let mut counts = Accounting::default();
    let success = unsafe {
        QueryInformationJobObject(
            job.0,
            BASIC_ACCOUNTING,
            (&mut counts as *mut Accounting).cast(),
            size_of::<Accounting>() as u32,
            null_mut(),
        )
    };
    if success == 0 {
        return Err(format!(
            "Windows job accounting: {}",
            std::io::Error::last_os_error()
        ));
    }
    Ok(counts.active)
}
fn terminate(job: &Handle) -> Result<(), String> {
    if unsafe { TerminateJobObject(job.0, 1) } == 0 {
        Err(format!(
            "Windows job termination failed: {}",
            std::io::Error::last_os_error()
        ))
    } else {
        Ok(())
    }
}
fn wait_drained(job: &Handle, remaining: Duration) -> Result<(), String> {
    let end = Instant::now() + remaining;
    loop {
        if active(job)? == 0 {
            return Ok(());
        }
        if Instant::now() >= end {
            return Err("Windows worker job did not drain".into());
        }
        std::thread::sleep(POLL);
    }
}
fn launch(worker: &Invocation, files: &Files, job: &Handle) -> Result<Handle, String> {
    let stdin = duplicate(&files.input)?;
    let stdout = duplicate(&files.output)?;
    let stderr = duplicate(&files.error)?;
    // The attribute buffer is word-aligned and stays alive until CreateProcessW
    // has copied the handle whitelist and job assignment into the new process.
    let mut size = 0usize;
    unsafe {
        InitializeProcThreadAttributeList(null_mut(), 2, 0, &mut size);
    }
    if size == 0 {
        return Err("cannot size extended startup attributes".into());
    }
    let mut buffer = vec![0usize; (size + size_of::<usize>() - 1) / size_of::<usize>()];
    let attribute_list = buffer.as_mut_ptr().cast::<c_void>();
    if unsafe { InitializeProcThreadAttributeList(attribute_list, 2, 0, &mut size) } == 0 {
        return Err(format!(
            "initialize Windows worker launch attributes: {}",
            std::io::Error::last_os_error()
        ));
    }
    let created = (|| -> Result<Handle, String> {
        let mut job_handle = job.0;
        let mut inherited = [stdin.0, stdout.0, stderr.0];
        if unsafe {
            UpdateProcThreadAttribute(
                attribute_list,
                0,
                JOB_LIST,
                (&mut job_handle as *mut RawHandle).cast(),
                size_of::<RawHandle>(),
                null_mut(),
                null_mut(),
            )
        } == 0
            || unsafe {
                UpdateProcThreadAttribute(
                    attribute_list,
                    0,
                    HANDLE_LIST,
                    inherited.as_mut_ptr().cast(),
                    size_of_val(&inherited),
                    null_mut(),
                    null_mut(),
                )
            } == 0
        {
            return Err(format!(
                "assign job/stdio before worker exec: {}",
                std::io::Error::last_os_error()
            ));
        }
        let mut startup = Startup {
            info: StartupInfo {
                size: size_of::<Startup>() as u32,
                reserved: null_mut(),
                desktop: null_mut(),
                title: null_mut(),
                x: 0,
                y: 0,
                width: 0,
                height: 0,
                columns: 0,
                rows: 0,
                fill: 0,
                flags: USE_STD_HANDLES,
                show_window: 0,
                reserved_bytes: 0,
                reserved_data: null_mut(),
                stdin: stdin.0,
                stdout: stdout.0,
                stderr: stderr.0,
            },
            attributes: attribute_list,
        };
        let program = wide(&worker.program);
        let command = std::iter::once(&worker.program)
            .chain(&worker.args)
            .map(|arg| {
                arg.to_str()
                    .filter(|text| !text.contains('\0'))
                    .map(quote)
                    .ok_or("worker argument is not UTF-8 or contains a NUL")
            })
            .collect::<Result<Vec<_>, _>>()?
            .join(" ");
        let directory = worker.cwd.as_ref().map(|path| wide(path.as_os_str()));
        let mut arguments = wide(std::ffi::OsStr::new(&command));
        let mut info = ProcessInfo {
            process: null_mut(),
            thread: null_mut(),
            pid: 0,
            tid: 0,
        };
        let success = unsafe {
            CreateProcessW(
                program.as_ptr(),
                arguments.as_mut_ptr(),
                null_mut(),
                null_mut(),
                1,
                EXTENDED_STARTUP | CREATE_NO_WINDOW,
                null_mut(),
                directory.as_ref().map_or(null(), |value| value.as_ptr()),
                &mut startup,
                &mut info,
            )
        };
        if success == 0 {
            return Err(format!(
                "Windows scoped worker launch failed: {}",
                std::io::Error::last_os_error()
            ));
        }
        let thread = Handle::new(info.thread, "Windows primary thread handle")?;
        let process = Handle::new(info.process, "Windows worker process handle")?;
        drop(thread);
        Ok(process)
    })();
    unsafe {
        DeleteProcThreadAttributeList(attribute_list);
    }
    created
}
pub(super) fn run_recoverable(
    worker: &Invocation,
    files: &mut Files,
    limits: Limits,
    budget: &Budget,
    cancel: &AtomicBool,
) -> Result<u32, String> {
    let (directory, _lease) = acquire(budget, cancel)?;
    let pending = directory.join("pending");
    recover_pending(&pending, budget.remaining())?;
    let mut entropy = [0u8; 16];
    if unsafe { BCryptGenRandom(null_mut(), entropy.as_mut_ptr(), 16, 2) } != 0 {
        return Err("could not generate a Windows worker identity".into());
    }
    let identity = entropy
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    let name = job_name(&identity);
    let raw = unsafe { CreateJobObjectW(null_mut(), name.as_ptr()) };
    let error = unsafe { GetLastError() };
    let job = Handle::new(raw, "create Windows Job Object")?;
    if error == ALREADY_EXISTS {
        return Err("Windows worker job name already exists".into());
    }
    {
        let mut record = open_record(&pending, true)?;
        record
            .write_all(identity.as_bytes())
            .and_then(|()| record.sync_all())
            .map_err(|error| format!("persist worker recovery record: {error}"))?;
    }
    let result = run(
        worker,
        files,
        limits.stdout,
        limits.stderr,
        budget,
        cancel,
        &job,
    );
    drop(job);
    recover_pending(&pending, budget.remaining().min(DRAIN_GRACE))?;
    result
}

fn recovery_directory() -> Result<PathBuf, String> {
    let home = PathBuf::from(std::env::var_os("LOCALAPPDATA").ok_or("LOCALAPPDATA unavailable")?);
    if !home.is_absolute() {
        return Err("worker state root is not absolute".into());
    }
    let directory = home.join("SlopHawk").join("native-worker-scope-v1");
    for path in [&home, &home.join("SlopHawk"), &directory] {
        if !path.exists() {
            match fs::create_dir(path) {
                Ok(()) => (),
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => (),
                Err(error) => return Err(error.to_string()),
            }
        }
        let metadata = fs::symlink_metadata(path).map_err(|error| error.to_string())?;
        if !metadata.is_dir() || metadata.file_attributes() & REPARSE_POINT != 0 {
            return Err("worker state directory is a reparse point or non-directory".into());
        }
    }
    Ok(directory)
}

fn open_record(path: &Path, new: bool) -> Result<File, String> {
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(!new)
        .create_new(new)
        .custom_flags(OPEN_REPARSE_POINT)
        .open(path)
        .map_err(|error| format!("open worker state: {error}"))?;
    let metadata = file.metadata().map_err(|error| error.to_string())?;
    if !metadata.is_file() || metadata.file_attributes() & REPARSE_POINT != 0 {
        return Err("worker state file is not an ordinary file".into());
    }
    Ok(file)
}

fn job_name(identity: &str) -> Vec<u16> {
    wide(std::ffi::OsStr::new(&format!(
        r"Global\SlopHawkNativeWorker-{identity}"
    )))
}

fn recover_pending(path: &Path, remaining: Duration) -> Result<(), String> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(value) => value,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error.to_string()),
    };
    if !metadata.is_file()
        || metadata.file_attributes() & REPARSE_POINT != 0
        || metadata.len() != 32
    {
        return Err("uncertain Windows worker recovery record".into());
    }
    let mut identity = [0u8; 32];
    OpenOptions::new()
        .read(true)
        .custom_flags(OPEN_REPARSE_POINT)
        .open(path)
        .and_then(|mut file| file.read_exact(&mut identity))
        .map_err(|error| error.to_string())?;
    if !identity
        .iter()
        .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(byte))
    {
        return Err("invalid Windows worker recovery identity".into());
    }
    let name = job_name(std::str::from_utf8(&identity).map_err(|error| error.to_string())?);
    let raw = unsafe { OpenJobObjectW(JOB_QUERY | JOB_TERMINATE, 0, name.as_ptr()) };
    if raw.is_null() {
        // A named job is gone only after its handles close and every
        // associated process has terminated. Other errors prove nothing.
        if unsafe { GetLastError() } != 2 {
            return Err(format!(
                "reopen Windows worker job: {}",
                std::io::Error::last_os_error()
            ));
        }
    } else {
        let job = Handle(raw);
        if active(&job)? != 0 {
            terminate(&job)?;
            wait_drained(&job, remaining)?;
        }
    }
    fs::remove_file(path).map_err(|error| format!("retire worker recovery record: {error}"))
}

fn run(
    worker: &Invocation,
    files: &mut Files,
    stdout_cap: usize,
    stderr_cap: usize,
    budget: &Budget,
    cancel: &AtomicBool,
    job: &Handle,
) -> Result<u32, String> {
    let mut limits = ExtendedLimits::default();
    limits.basic.flags = KILL_ON_JOB_CLOSE;
    if unsafe {
        SetInformationJobObject(
            job.0,
            EXTENDED_JOB_LIMITS,
            (&limits as *const ExtendedLimits).cast(),
            size_of::<ExtendedLimits>() as u32,
        )
    } == 0
    {
        return Err(format!(
            "set Windows kill-on-close job limit: {}",
            std::io::Error::last_os_error()
        ));
    }
    if !budget.active(cancel) {
        return Err("worker cancelled before Windows job admission".into());
    }
    let process = launch(worker, files, job)?;
    let mut exit_code = None;
    let mut reason = None;
    while budget.active(cancel) {
        if !files.sizes_ok(stdout_cap, stderr_cap) {
            reason = Some("Windows worker output exceeded byte cap");
            break;
        }
        match unsafe { WaitForSingleObject(process.0, 0) } {
            WAIT_OBJECT_0 => {
                let mut code = 0;
                if unsafe { GetExitCodeProcess(process.0, &mut code) } == 0 {
                    reason = Some("could not query Windows worker exit code");
                } else {
                    exit_code = Some(code);
                }
                break;
            }
            WAIT_TIMEOUT => std::thread::sleep(POLL),
            _ => {
                reason = Some("Windows worker wait failed");
                break;
            }
        }
    }
    if !budget.active(cancel) {
        reason.get_or_insert("Windows worker deadline/cancellation");
    }
    if reason.is_some() || exit_code.is_none() {
        let signalled = terminate(job);
        let drained = wait_drained(job, DRAIN_GRACE);
        return Err(format!(
            "{}; recursive terminate: {}; drained: {}",
            reason.unwrap_or("Windows worker exit missing"),
            signalled.err().unwrap_or_else(|| "yes".into()),
            drained.err().unwrap_or_else(|| "yes".into())
        ));
    }
    match wait_drained(job, budget.remaining()) {
        Ok(()) => (),
        Err(e) => {
            let _ = terminate(job);
            let _ = wait_drained(job, DRAIN_GRACE);
            return Err(e);
        }
    }
    if !files.sizes_ok(stdout_cap, stderr_cap) {
        return Err("Windows C# worker output cap exceeded".into());
    }
    exit_code.ok_or_else(|| "Windows worker exit missing".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[ignore = "child launched inside the recovery test job"]
    fn worker_fixture() {
        std::thread::sleep(Duration::from_secs(15));
    }

    #[test]
    fn stale_named_job_must_drain_before_its_record_is_retired() {
        let files = Files::new(&[]).unwrap();
        let mut entropy = [0u8; 16];
        assert_eq!(
            unsafe { BCryptGenRandom(null_mut(), entropy.as_mut_ptr(), 16, 2) },
            0
        );
        let identity = entropy
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>();
        let name = job_name(&identity);
        let job = Handle::new(
            unsafe { CreateJobObjectW(null_mut(), name.as_ptr()) },
            "test job",
        )
        .unwrap();
        let path = files.directory.join("pending");
        fs::write(&path, &identity).unwrap();
        let worker = Invocation {
            program: std::env::current_exe().unwrap().into_os_string(),
            args: ["--ignored", "--exact", "windows::tests::worker_fixture"]
                .into_iter()
                .map(Into::into)
                .collect(),
            cwd: None,
        };
        let child = launch(&worker, &files, &job).unwrap();
        assert_eq!(active(&job).unwrap(), 1);
        recover_pending(&path, Duration::from_secs(3)).unwrap();
        assert_eq!(active(&job).unwrap(), 0);
        assert_eq!(unsafe { WaitForSingleObject(child.0, 3000) }, WAIT_OBJECT_0);
        assert!(!path.exists());
    }
}

fn acquire(budget: &Budget, cancel: &AtomicBool) -> Result<(PathBuf, File), String> {
    let directory = recovery_directory()?;
    let lease = open_record(&directory.join("admission.lock"), false)?;
    while budget.active(cancel) {
        match lease.try_lock() {
            Ok(()) => break,
            Err(std::fs::TryLockError::WouldBlock) => std::thread::sleep(POLL),
            Err(error) => return Err(format!("worker admission lock: {error}")),
        }
    }
    if !budget.active(cancel) {
        return Err("Windows worker admission deadline/cancellation".into());
    }
    Ok((directory, lease))
}

pub(super) fn recover(budget: &Budget, cancel: &AtomicBool) -> Result<(), String> {
    let (directory, _lease) = acquire(budget, cancel)?;
    recover_pending(&directory.join("pending"), budget.remaining())
}
