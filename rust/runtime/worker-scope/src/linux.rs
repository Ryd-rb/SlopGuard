use super::*;
use std::collections::HashMap;
use std::os::fd::OwnedFd;
use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt};
use std::os::unix::net::UnixStream;
use std::process::{Child, Command, ExitStatus};

const SYSTEMD_RUN: &str = "/usr/bin/systemd-run";
const SYSTEMCTL: &str = "/usr/bin/systemctl";
const CGROUP_ROOT: &str = "/sys/fs/cgroup";
const HELPER_BUDGET: Duration = Duration::from_millis(700);

struct OutputPipe {
    reader: UnixStream,
    remaining: usize,
    closed: bool,
}

impl OutputPipe {
    fn open(cap: usize) -> std::io::Result<(Self, Stdio)> {
        let (reader, writer) = UnixStream::pair()?;
        reader.set_nonblocking(true)?;
        Ok((
            Self {
                reader,
                remaining: cap,
                closed: false,
            },
            Stdio::from(OwnedFd::from(writer)),
        ))
    }

    fn drain(&mut self, file: &mut File) -> Result<(), String> {
        if self.closed {
            return Ok(());
        }
        let mut bytes = [0; 32 * 1024];
        match self.reader.read(&mut bytes) {
            Ok(0) => self.closed = true,
            Ok(len) if len <= self.remaining => {
                self.remaining -= len;
                file.write_all(&bytes[..len])
                    .map_err(|error| error.to_string())?;
            }
            Ok(_) => return Err("worker exceeded transport output limit".into()),
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::Interrupted
                ) =>
            {
                ()
            }
            Err(error) => return Err(format!("read scope output: {error}")),
        }
        Ok(())
    }
}

struct Service {
    name: String,
    invocation: Option<String>,
    group: Option<String>,
}

fn runtime_directory() -> Result<PathBuf, String> {
    unsafe extern "C" {
        fn geteuid() -> u32;
    }
    let uid = unsafe { geteuid() };
    let base = PathBuf::from(format!("/run/user/{uid}"));
    let directory = base.join("slophawk-native-worker-v1");
    let check = |path: &Path| {
        let metadata = fs::symlink_metadata(path).map_err(|error| error.to_string())?;
        if !metadata.is_dir() || metadata.uid() != uid || metadata.mode() & 0o077 != 0 {
            return Err("worker recovery directory is not private to the current user".into());
        }
        Ok::<_, String>(())
    };
    check(&base)?;
    match fs::DirBuilder::new().mode(0o700).create(&directory) {
        Ok(()) => (),
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => (),
        Err(error) => return Err(error.to_string()),
    }
    check(&directory)?;
    Ok(directory)
}

fn user_manager(program: &str) -> Result<Command, String> {
    let directory = runtime_directory()?;
    let base = directory.parent().ok_or("no user runtime directory")?;
    let bus = base.join("bus");
    use std::os::unix::fs::FileTypeExt;
    if !fs::symlink_metadata(&bus)
        .map_err(|error| error.to_string())?
        .file_type()
        .is_socket()
    {
        return Err("systemd user bus is unavailable".into());
    }
    let mut command = Command::new(program);
    command
        .env_remove("DBUS_SYSTEM_BUS_ADDRESS")
        .env("XDG_RUNTIME_DIR", base)
        .env(
            "DBUS_SESSION_BUS_ADDRESS",
            format!("unix:path={}", bus.display()),
        );
    Ok(command)
}

fn set_record(path: &Path, state: &Service) -> Result<(), String> {
    let next = path.with_extension("new");
    let mut file = Files::open(&next)?;
    let result = (|| -> std::io::Result<()> {
        writeln!(file, "{}", state.name)?;
        writeln!(file, "{}", state.invocation.as_deref().unwrap_or(""))?;
        writeln!(file, "{}", state.group.as_deref().unwrap_or(""))?;
        file.sync_all()?;
        fs::rename(&next, path)?;
        File::open(path.parent().expect("fixed recovery root"))?.sync_all()
    })();
    if result.is_err() {
        let _ = fs::remove_file(next);
    }
    result.map_err(|error| format!("persist Linux worker scope: {error}"))
}

fn read_record(path: &Path) -> Result<Option<Service>, String> {
    let mut file = match OpenOptions::new()
        .read(true)
        .custom_flags(0x20000)
        .open(path)
    {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.to_string()),
    };
    let metadata = file.metadata().map_err(|error| error.to_string())?;
    if !metadata.is_file() || metadata.len() > 2200 || metadata.mode() & 0o077 != 0 {
        return Err("untrusted Linux worker recovery record".into());
    }
    let bytes = Files::read_limited(&mut file, 2200)?;
    let text = std::str::from_utf8(&bytes).map_err(|error| error.to_string())?;
    let mut lines = text.lines();
    let name = lines.next().ok_or("worker journal omitted unit")?;
    let invocation = lines.next().ok_or("worker journal omitted invocation")?;
    let group = lines.next().ok_or("worker journal omitted cgroup")?;
    let hex = |value: &str| value.len() == 32 && value.bytes().all(|b| b.is_ascii_hexdigit());
    if lines.next().is_some()
        || !name
            .strip_prefix("slophawk-shadow-")
            .and_then(|s| s.strip_suffix(".service"))
            .is_some_and(hex)
        || !invocation.is_empty() && !hex(invocation)
        || !group.is_empty() && (invocation.is_empty() || !safe_group(group, name))
    {
        return Err("invalid Linux worker recovery record".into());
    }
    Ok(Some(Service {
        name: name.into(),
        invocation: (!invocation.is_empty()).then(|| invocation.into()),
        group: (!group.is_empty()).then(|| group.into()),
    }))
}

fn recover_pending(path: &Path, transport: &Path, remaining: Duration) -> Result<(), String> {
    let Some(mut service) = read_record(path)? else {
        return Ok(());
    };
    let discovered = show(&service.name, transport, remaining)?;
    if discovered.load == "not-found" {
        // A planned launch may still be in flight after a crash. An absent
        // previously pinned invocation and cgroup can be safely retired.
        if service.invocation.is_none() {
            return Err("unconfirmed Linux worker launch; recovery blocked".into());
        }
        if let Some(group) = &service.group {
            if populated(group, &service.name)?.is_some() {
                return Err("Linux worker cgroup still exists without its manager identity".into());
            }
        }
    } else {
        verified(&mut service, &discovered)?;
        set_record(path, &service)?;
        if !drained(&mut service, transport, remaining)? {
            kill(&service, transport, remaining)?;
            until_drained(&mut service, transport, remaining)?;
        }
        remove(&service, transport);
    }
    fs::remove_file(path).map_err(|error| error.to_string())
}
#[derive(Debug)]
struct Observation {
    load: String,
    active: String,
    sub: String,
    exit_type: String,
    kill_mode: String,
    remain: String,
    invocation: String,
    group: String,
    result: String,
    main_code: String,
    main_status: String,
}

fn random_unit() -> Result<String, String> {
    let mut entropy = [0u8; 16];
    File::open("/dev/urandom")
        .and_then(|mut f| f.read_exact(&mut entropy))
        .map_err(|e| format!("cannot allocate authenticated scope nonce: {e}"))?;
    if entropy.iter().all(|b| *b == 0) {
        return Err("scope nonce was zero".into());
    }
    Ok(format!(
        "slophawk-shadow-{}.service",
        entropy
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect::<String>()
    ))
}
fn helper(
    program: &str,
    args: &[&str],
    directory: &Path,
    remaining: Duration,
) -> Result<(ExitStatus, String), String> {
    // Helpers have their own bounded lifetime and file-backed output; never use
    // Command::output(), which can hang on descendant-held pipes.
    let sequence = NEXT.fetch_add(1, Ordering::Relaxed);
    let stdout_path = directory.join(format!("helper-{sequence}-stdout"));
    let stderr_path = directory.join(format!("helper-{sequence}-stderr"));
    let mut out = Files::open(&stdout_path)?;
    let err = Files::open(&stderr_path)?;
    let result = (|| -> Result<(ExitStatus, String), String> {
        let mut child = user_manager(program)?
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::from(out.try_clone().map_err(|e| e.to_string())?))
            .stderr(Stdio::from(err.try_clone().map_err(|e| e.to_string())?))
            .spawn()
            .map_err(|e| format!("scope manager helper: {e}"))?;
        let until = Instant::now() + remaining.min(HELPER_BUDGET);
        let status = loop {
            match child.try_wait() {
                Ok(Some(s)) => break s,
                Ok(None) if Instant::now() < until => std::thread::sleep(POLL),
                Ok(None) => {
                    let _ = child.kill();
                    let _ = child.try_wait();
                    return Err("scope manager helper deadline expired".into());
                }
                Err(e) => {
                    let _ = child.kill();
                    let _ = child.try_wait();
                    return Err(format!("scope manager helper wait: {e}"));
                }
            }
        };
        let bytes = Files::read_limited(&mut out, 16 * 1024)?;
        let output =
            String::from_utf8(bytes).map_err(|_| "scope manager emitted non-UTF-8 text")?;
        Ok((status, output))
    })();
    drop(out);
    drop(err);
    let _ = fs::remove_file(stdout_path);
    let _ = fs::remove_file(stderr_path);
    result
}
fn show(service: &str, directory: &Path, remaining: Duration) -> Result<Observation, String> {
    let (status, lines) = helper(
        SYSTEMCTL,
        &[
            "--user",
            "--no-pager",
            "show",
            "--all",
            "--property=LoadState,ActiveState,SubState,InvocationID,ControlGroup,Result,ExecMainCode,ExecMainStatus,ExitType,KillMode,RemainAfterExit",
            service,
        ],
        directory,
        remaining,
    )?;
    let mut values = HashMap::new();
    for line in lines.lines() {
        let (key, value) = line
            .split_once('=')
            .ok_or("malformed systemctl scope observation")?;
        if values.insert(key, value).is_some() {
            return Err("duplicate systemctl scope field".into());
        }
    }
    let get = |k: &str| {
        values
            .get(k)
            .map(|v| (*v).to_owned())
            .ok_or_else(|| format!("missing systemctl scope property {k}"))
    };
    let load = get("LoadState")?;
    // systemctl returns a failure status for an otherwise authenticated
    // not-found observation. Missing manager output never implies absence.
    if !status.success() && load != "not-found" {
        return Err("systemctl show did not authenticate the worker scope".into());
    }
    let value = |key: &str| values.get(key).copied().unwrap_or_default().to_owned();
    Ok(Observation {
        load,
        active: value("ActiveState"),
        sub: value("SubState"),
        exit_type: value("ExitType"),
        kill_mode: value("KillMode"),
        remain: value("RemainAfterExit"),
        invocation: value("InvocationID"),
        group: value("ControlGroup"),
        result: value("Result"),
        main_code: value("ExecMainCode"),
        main_status: value("ExecMainStatus"),
    })
}
fn safe_group(group: &str, unit: &str) -> bool {
    group.starts_with('/')
        && group.ends_with(&format!("/{unit}"))
        && group.len() < 2048
        && !group.contains("//")
        && !group.chars().any(char::is_control)
        && !group.split('/').any(|c| c == "." || c == "..")
}
fn populated(group: &str, unit: &str) -> Result<Option<bool>, String> {
    if !safe_group(group, unit) {
        return Err("untrusted cgroup identity".into());
    }
    let path = Path::new(CGROUP_ROOT)
        .join(&group[1..])
        .join("cgroup.events");
    let contents = match fs::read_to_string(&path) {
        Ok(v) => v,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(format!("cannot verify worker cgroup drain: {e}")),
    };
    for line in contents.lines() {
        if let Some(value) = line.strip_prefix("populated ") {
            return match value {
                "0" => Ok(Some(false)),
                "1" => Ok(Some(true)),
                _ => Err("unrecognized cgroup population".into()),
            };
        }
    }
    Err("cgroup.events omitted populated field".into())
}
fn verified(service: &mut Service, o: &Observation) -> Result<(), String> {
    if o.load != "loaded"
        || o.invocation.len() != 32
        || !o.invocation.bytes().all(|b| b.is_ascii_hexdigit())
        || o.invocation.bytes().all(|b| b == b'0')
        || o.exit_type != "cgroup"
        || o.kill_mode != "control-group"
        || o.remain != "yes"
    {
        return Err("systemd unit lacked authenticated invocation identity".into());
    }
    if service
        .invocation
        .as_ref()
        .is_some_and(|v| v != &o.invocation)
    {
        return Err("worker scope invocation was replaced".into());
    }
    // After the same invocation terminates, systemd may clear ControlGroup.
    // It must have been pinned while the service was active, never inferred
    // from an empty or missing post-exit property.
    if !o.group.is_empty() && !safe_group(&o.group, &service.name) {
        return Err("systemd returned an untrusted cgroup identity".into());
    }
    if service
        .group
        .as_ref()
        .is_some_and(|v| !o.group.is_empty() && v != &o.group)
    {
        return Err("worker scope changed cgroup".into());
    }
    if service.group.is_none() && !o.group.is_empty() {
        service.group = Some(o.group.clone());
    }
    service
        .invocation
        .get_or_insert_with(|| o.invocation.clone());
    Ok(())
}
fn drained(service: &mut Service, directory: &Path, remaining: Duration) -> Result<bool, String> {
    let o = show(&service.name, directory, remaining)?;
    verified(service, &o)?;
    terminal_drained(service, &o)
}

fn terminal_drained(service: &Service, o: &Observation) -> Result<bool, String> {
    let terminal = matches!(
        (o.active.as_str(), o.sub.as_str()),
        ("inactive", "dead") | ("active", "exited") | ("failed", "failed")
    );
    if !terminal {
        return Ok(false);
    }
    // ExitType=cgroup makes the authenticated terminal authoritative even if
    // systemd has already removed the empty group of a very short-lived peer.
    match &service.group {
        Some(group) => Ok(populated(group, &service.name)? != Some(true)),
        None => Ok(true),
    }
}

fn kill(service: &Service, directory: &Path, deadline: Duration) -> Result<(), String> {
    let (status, _) = helper(
        SYSTEMCTL,
        &[
            "--user",
            "kill",
            "--kill-whom=all",
            "--signal=SIGKILL",
            &service.name,
        ],
        directory,
        deadline,
    )?;
    if status.success() {
        Ok(())
    } else {
        Err("systemd could not kill the full worker cgroup".into())
    }
}
fn remove(service: &Service, directory: &Path) {
    // The service was positively witnessed drained before this point.
    let _ = helper(
        SYSTEMCTL,
        &["--user", "stop", &service.name],
        directory,
        HELPER_BUDGET,
    );
    let _ = helper(
        SYSTEMCTL,
        &["--user", "reset-failed", &service.name],
        directory,
        HELPER_BUDGET,
    );
}
fn until_drained(
    service: &mut Service,
    directory: &Path,
    max_wait: Duration,
) -> Result<(), String> {
    let end = Instant::now() + max_wait;
    loop {
        let remaining = end.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Err("worker scope drainage unproven at deadline".into());
        }
        match drained(service, directory, remaining) {
            Ok(true) => return Ok(()),
            Ok(false) => std::thread::sleep(POLL.min(remaining)),
            Err(e) => return Err(e),
        }
    }
}
fn admission(directory: &Path, budget: &Budget) -> Result<Service, String> {
    if !Path::new("/sys/fs/cgroup/cgroup.controllers").is_file() {
        return Err("unsupported: Linux cgroup v2 is unavailable".into());
    }
    if !Path::new(SYSTEMD_RUN).is_file() || !Path::new(SYSTEMCTL).is_file() {
        return Err("unsupported: systemd user manager helpers absent".into());
    }
    let service = Service {
        name: random_unit()?,
        invocation: None,
        group: None,
    };
    let previous = show(&service.name, directory, budget.remaining())?;
    if previous.load != "not-found" {
        return Err("scope unit name was already allocated".into());
    }
    Ok(service)
}
pub(super) fn run(
    worker: &Invocation,
    files: &mut Files,
    stdout_cap: usize,
    stderr_cap: usize,
    budget: &Budget,
    cancel: &AtomicBool,
) -> Result<u32, String> {
    let (directory, _lease) = acquire(budget, cancel)?;
    let pending = directory.join("pending");
    recover_pending(&pending, &files.directory, budget.remaining())?;
    let result = run_service(
        worker, files, stdout_cap, stderr_cap, budget, cancel, &pending,
    );
    if result.is_ok() {
        fs::remove_file(&pending).map_err(|error| error.to_string())?;
    }
    result
}

fn run_service(
    worker: &Invocation,
    files: &mut Files,
    stdout_cap: usize,
    stderr_cap: usize,
    budget: &Budget,
    cancel: &AtomicBool,
    pending: &Path,
) -> Result<u32, String> {
    let mut service = admission(&files.directory, budget)?;
    if !budget.active(cancel) {
        return Err("worker cancelled before systemd admission".into());
    }
    let (mut out, stdout) = OutputPipe::open(stdout_cap).map_err(|error| error.to_string())?;
    let (mut err, stderr) = OutputPipe::open(stderr_cap).map_err(|error| error.to_string())?;
    let seconds = budget.remaining().as_secs().saturating_add(3).max(4);
    let runtime = format!("--property=RuntimeMaxSec={seconds}s");
    let unit = format!("--unit={}", service.name);
    let mut command = user_manager(SYSTEMD_RUN)?;
    for name in &worker.env {
        let mut setting = OsString::from("--setenv=");
        setting.push(name);
        command.arg(setting);
    }
    if let Some(directory) = &worker.cwd {
        let mut setting = OsString::from("--working-directory=");
        setting.push(directory);
        command.arg(setting);
    }
    set_record(pending, &service)?;
    let mut child: Child = match command
        .args([
            "--user",
            "--quiet",
            "--wait",
            "--pipe",
            "--service-type=exec",
            "--remain-after-exit",
            "--property=ExitType=cgroup",
            "--expand-environment=no",
            "--property=KillMode=control-group",
            "--property=SendSIGKILL=yes",
            runtime.as_str(),
            unit.as_str(),
            "--",
        ])
        .arg(&worker.program)
        .args(&worker.args)
        .stdin(files.stdin()?)
        .stdout(stdout)
        .stderr(stderr)
        .spawn()
    {
        Ok(child) => child,
        Err(error) => {
            fs::remove_file(pending).map_err(|error| error.to_string())?;
            return Err(format!("systemd worker service launch failed: {error}"));
        }
    };
    drop(command);
    let exchange = (|| -> Result<u32, String> {
        while budget.active(cancel) {
            out.drain(&mut files.output)?;
            err.drain(&mut files.error)?;
            let observation = show(&service.name, &files.directory, budget.remaining())?;
            if observation.load == "loaded" && observation.invocation.bytes().any(|b| b != b'0') {
                let before = (service.invocation.clone(), service.group.clone());
                verified(&mut service, &observation)?;
                if before != (service.invocation.clone(), service.group.clone()) {
                    set_record(pending, &service)?;
                }
                if terminal_drained(&service, &observation)? {
                    let code = observation
                        .main_status
                        .parse::<u32>()
                        .map_err(|_| "invalid scoped worker exit code")?;
                    if observation.main_code != "1"
                        || !matches!(observation.result.as_str(), "success" | "exit-code")
                    {
                        return Err("scoped worker did not exit normally".into());
                    }
                    // systemd-run --wait waits for the service to become
                    // inactive. Stop it only after saving its terminal proof.
                    remove(&service, &files.directory);
                    let mut launcher = None;
                    loop {
                        out.drain(&mut files.output)?;
                        err.drain(&mut files.error)?;
                        if launcher.is_none() {
                            launcher = child.try_wait().map_err(|e| e.to_string())?;
                        }
                        if let Some(status) = launcher.filter(|_| out.closed && err.closed) {
                            return if status.code().and_then(|code| u32::try_from(code).ok())
                                == Some(code)
                            {
                                Ok(code)
                            } else {
                                Err("worker and service exit identities differ".into())
                            };
                        }
                        if !budget.active(cancel) {
                            return Err(
                                "systemd launcher did not exit after the drained service stopped"
                                    .into(),
                            );
                        }
                        std::thread::sleep(POLL);
                    }
                }
            }
            if let Some(status) = child.try_wait().map_err(|e| e.to_string())? {
                return Err(format!(
                    "systemd launcher exited without a terminal scope: {status}"
                ));
            }
            std::thread::sleep(POLL);
        }
        Err("worker deadline, cancellation or authority revocation".into())
    })();
    if let Err(reason) = exchange {
        // On failure leave an unverified record in place. A later owner
        // must recover it under the same lease before a new scope can run.
        let recovery = recover_pending(pending, &files.directory, DRAIN_GRACE);
        let _ = child.kill();
        let _ = child.try_wait();
        return Err(format!(
            "{reason}; scope recovery: {}",
            recovery.err().as_deref().unwrap_or("verified")
        ));
    }
    exchange
}

fn acquire(budget: &Budget, cancel: &AtomicBool) -> Result<(PathBuf, File), String> {
    let directory = runtime_directory()?;
    let lease = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .custom_flags(0x20000)
        .open(directory.join("admission.lock"))
        .map_err(|error| error.to_string())?;
    let metadata = lease.metadata().map_err(|error| error.to_string())?;
    if !metadata.is_file() || metadata.mode() & 0o077 != 0 {
        return Err("Linux worker lease is not an ordinary private file".into());
    }
    while budget.active(cancel) {
        match lease.try_lock() {
            Ok(()) => break,
            Err(std::fs::TryLockError::WouldBlock) => std::thread::sleep(POLL),
            Err(error) => return Err(error.to_string()),
        }
    }
    if !budget.active(cancel) {
        return Err("Linux worker admission deadline or revocation".into());
    }
    Ok((directory, lease))
}

pub(super) fn recover(budget: &Budget, cancel: &AtomicBool) -> Result<(), String> {
    let (directory, _lease) = acquire(budget, cancel)?;
    let transport = Files::new(&[])?;
    recover_pending(
        &directory.join("pending"),
        &transport.directory,
        budget.remaining(),
    )
}
