//! File locks, one per-user heavy-worker slot, and controlled worker-tree
//! supervision. Linux uses a new process group; it is not a substitute for a
//! delegated cgroup. Windows job-object atomic enrollment is NOT implemented:
//! process-backed work fails closed there until that cutover gate is met.
use crate::store::{invalid, require_plain_dir};
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::Path;
use std::process::Command;
#[cfg(target_os = "linux")]
use std::process::{Child, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;
#[cfg(target_os = "linux")]
use std::time::Instant;

pub struct Lock { _file: File }
impl Lock {
    pub fn try_acquire(path: &Path) -> io::Result<Self> {
        let parent = path.parent().ok_or_else(|| invalid("missing lock directory"))?;
        require_plain_dir(parent)?;
        if let Ok(meta) = fs::symlink_metadata(path) {
            if !meta.is_file() { return Err(invalid("lock path is not a plain file")); }
        }
        let file = OpenOptions::new().write(true).read(true).create(true).open(path)?;
        // Rust 1.99.1 File::try_lock uses flock on Unix and LockFileEx on
        // Windows. No PID-only or create_new lock is treated as a singleton.
        file.try_lock()?;
        Ok(Self { _file: file })
    }

    // This record survives the OS lock's release when its owner crashes.
    // Every workspace acquiring the per-user slot must prove the previous
    // process group is empty (or cancel its authenticated leader) first.
    fn recover_previous_scope(&mut self) -> io::Result<()> {
        self._file.seek(SeekFrom::Start(0))?;
        let mut raw = Vec::new();
        (&mut self._file).take(129).read_to_end(&mut raw)?;
        if raw.is_empty() { return Ok(()); }
        if raw.len() > 128 { return Err(invalid("oversized user worker reservation")); }
        let text = std::str::from_utf8(&raw).map_err(|_| invalid("invalid user worker reservation"))?;
        let (pid, ticks) = text.trim_end_matches('\n').split_once('\t')
            .ok_or_else(|| invalid("invalid user worker reservation"))?;
        let pid: u32 = pid.parse().map_err(|_| invalid("invalid worker pid"))?;
        let ticks: u64 = ticks.parse().map_err(|_| invalid("invalid worker start ticks"))?;
        if pid == 0 || ticks == 0 { return Err(invalid("unverifiable previous worker scope")); }
        if !recover_scope(pid, ticks)? {
            return Err(io::Error::new(io::ErrorKind::WouldBlock, "previous worker scope not drained"));
        }
        self.clear_worker()
    }

    pub fn record_worker(&mut self, pid: u32, start_ticks: u64) -> io::Result<()> {
        if pid == 0 || start_ticks == 0 { return Err(invalid("invalid worker identity")); }
        self.write_record(format!("{pid}\t{start_ticks}\n").as_bytes())
    }
    pub fn clear_worker(&mut self) -> io::Result<()> { self.write_record(&[]) }
    fn write_record(&mut self, bytes: &[u8]) -> io::Result<()> {
        self._file.set_len(0)?;
        self._file.seek(SeekFrom::Start(0))?;
        self._file.write_all(bytes)?;
        self._file.sync_all()
    }
}

pub fn private_dir(path: &Path) -> io::Result<()> {
    fs::create_dir_all(path)?;
    require_plain_dir(path)?;
    #[cfg(unix)] {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

pub fn heavy_slot() -> io::Result<Lock> {
    let variable = if cfg!(windows) { "LOCALAPPDATA" } else { "HOME" };
    let home = std::env::var_os(variable).ok_or_else(|| invalid("HOME unavailable for per-user admission"))?;
    let home = Path::new(&home);
    require_plain_dir(home)?;
    let cache = home.join(".cache");
    if !cache.exists() { fs::create_dir(&cache)?; }
    require_plain_dir(&cache)?; // Never chmod the user's pre-existing ~/.cache.
    let dir = cache.join("slophawk-native-coordinator");
    private_dir(&dir)?;
    let mut slot = Lock::try_acquire(&dir.join("heavy.lock"))?;
    slot.recover_previous_scope()?;
    Ok(slot)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FakeMode { Complete, Fail, Cancel }

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkerOutcome {
    pub status: &'static str,
    pub coverage: &'static str,
    pub reason: &'static str,
    pub pid: u32,
    pub start_ticks: u64,
    pub bytes: usize,
    pub duration: Duration,
}

pub fn child_fixture(mode: &str, input: &str) -> io::Result<i32> {
    let mut start = [0u8; 1];
    if io::stdin().read_exact(&mut start).is_err() || start[0] != 1 {
        return Ok(9); // No committed admission before launch.
    }
    if mode == "cancel" {
        // A controlled grandchild tests whether tree cancellation actually
        // applies to descendants and not just the parent process.
        let exe = std::env::current_exe()?;
        let _child = Command::new(exe).arg("fake-grandchild").spawn()?;
        std::thread::sleep(Duration::from_secs(10));
        return Ok(4);
    }
    if mode == "fail" { return Ok(7); }
    if mode != "complete" { return Ok(9); }
    let output = format!("{{\"providerId\":\"native-fixture\",\"schemaVersion\":1,\"toolVersion\":\"fake-1\",\"inputFingerprint\":\"{input}\",\"coverage\":{{\"state\":\"complete\",\"reason\":null}},\"facts\":[]}}\n");
    io::stdout().write_all(output.as_bytes())?;
    Ok(0)
}

pub fn grandchild_fixture() { std::thread::sleep(Duration::from_secs(10)); }

pub fn worker_start_ticks(pid: u32) -> io::Result<u64> {
    #[cfg(target_os = "linux")] {
        let text = fs::read_to_string(format!("/proc/{pid}/stat"))?;
        let suffix = text.rsplit_once(") ").ok_or_else(|| invalid("bad /proc stat"))?.1;
        let fields: Vec<_> = suffix.split_whitespace().collect();
        // /proc/<pid>/stat field 22, suffix begins with field 3.
        fields.get(19).ok_or_else(|| invalid("missing process starttime"))?
            .parse().map_err(|_| invalid("invalid process starttime"))
    }
    #[cfg(not(target_os = "linux"))] {
        let _ = pid;
        Err(io::Error::new(io::ErrorKind::Unsupported,
            "no verified process identity on this platform"))
    }
}

#[cfg(target_os = "linux")]
fn signal_group(pid: u32, signal: i32) -> io::Result<bool> {
    extern "C" { fn kill(pid: i32, sig: i32) -> i32; }
    if pid == 0 || pid > i32::MAX as u32 { return Err(invalid("invalid process group")); }
    let result = unsafe { kill(-(pid as i32), signal) };
    if result == 0 { return Ok(true); }
    let error = io::Error::last_os_error();
    if error.raw_os_error() == Some(3) { Ok(false) } else { Err(error) }
}

pub fn recover_scope(pid: u32, start_ticks: u64) -> io::Result<bool> {
    if pid == 0 { return Ok(true); } // child never passed the admission gate
    #[cfg(target_os = "linux")] {
        match worker_start_ticks(pid) {
            Ok(observed) if observed == start_ticks && start_ticks != 0 => {
                signal_group(pid, 9)?;
            }
            Ok(_) => return Ok(!signal_group(pid, 0)?),
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(e) => return Err(e),
        }
        for _ in 0..25 {
            if !signal_group(pid, 0)? { return Ok(true); }
            std::thread::sleep(Duration::from_millis(10));
        }
        Ok(false) // uncertain surviving descendants; keep reservation blocked
    }
    #[cfg(not(target_os = "linux"))] {
        let _ = (pid, start_ticks);
        Ok(false)
    }
}

pub fn run_fake<F>(mode: FakeMode, input: &str, initial_sequence: u64,
                   clock: &AtomicU64, mut admitted: F) -> io::Result<WorkerOutcome>
where F: FnMut(u32, u64) -> io::Result<()> {
    #[cfg(not(target_os = "linux"))] {
        let _ = (mode, input, initial_sequence, clock, &mut admitted);
        return Err(io::Error::new(io::ErrorKind::Unsupported,
            "process-backed fixture needs Linux process-group support"));
    }
    #[cfg(target_os = "linux")] {
        use std::os::unix::process::CommandExt;
        let started = Instant::now();
        let exe = std::env::current_exe()?;
        let mode_text = match mode { FakeMode::Complete => "complete", FakeMode::Fail => "fail", FakeMode::Cancel => "cancel" };
        let mut child = Command::new(exe)
            .arg("fake-worker").arg(mode_text).arg(input)
            .process_group(0)
            .stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null())
            .spawn()?;
        let pid = child.id();
        let start_ticks = match worker_start_ticks(pid) {
            Ok(ticks) => ticks,
            Err(error) => { cancel_and_wait(&mut child)?; return Err(error); }
        };
        if let Err(error) = admitted(pid, start_ticks) {
            cancel_and_wait(&mut child)?;
            return Err(error);
        }
        if let Some(mut stdin) = child.stdin.take() {
            // A launched child cannot execute or spawn until the durable
            // attempt has been updated with the exact process identity.
            if let Err(error) = stdin.write_all(&[1]) {
                cancel_and_wait(&mut child)?;
                return Err(error);
            }
        }
        let mut cancelled = false;
        let mut timed_out = false;
        let code = loop {
            // Give the controlled worker a brief opportunity to spawn its
            // grandchild before cancelling its entire process group.
            if clock.load(Ordering::Acquire) != initial_sequence ||
                (mode == FakeMode::Cancel && started.elapsed() >= Duration::from_millis(80)) {
                cancelled = true;
                cancel_and_wait(&mut child)?;
                break None;
            }
            if started.elapsed() >= Duration::from_secs(2) {
                timed_out = true;
                cancel_and_wait(&mut child)?;
                break None;
            }
            if let Some(exit) = child.try_wait()? { break exit.code(); }
            std::thread::sleep(Duration::from_millis(5));
        };
        // Reaping and bounding the output from our controlled fixture. The
        // child never emits more than 4 KiB, irrespective of source input.
        let mut output = Vec::new();
        if let Some(mut stdout) = child.stdout.take() {
            stdout.take(65_537).read_to_end(&mut output)?;
        }
        if output.len() > 65_536 { return Err(invalid("fake worker output exceeded limit")); }
        let (status, coverage, reason) = if timed_out {
            ("timed_out", "partial", "worker_timeout")
        } else if cancelled {
            ("cancelled", "partial", "worker_cancelled_or_obsolete")
        } else if code == Some(0) && output == format!("{{\"providerId\":\"native-fixture\",\"schemaVersion\":1,\"toolVersion\":\"fake-1\",\"inputFingerprint\":\"{input}\",\"coverage\":{{\"state\":\"complete\",\"reason\":null}},\"facts\":[]}}\n").as_bytes() {
            ("completed", "complete", "")
        } else {
            ("failed_to_run", "blocked", "fake_worker_exit_or_frame_error")
        };
        Ok(WorkerOutcome { status, coverage, reason, pid, start_ticks, bytes: output.len(), duration: started.elapsed() })
    }
}

#[cfg(target_os = "linux")]
fn cancel_and_wait(child: &mut Child) -> io::Result<()> {
    signal_group(child.id(), 9)?;
    child.wait()?;
    Ok(())
}
