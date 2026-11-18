//! The native daemon's authenticated local transport. The command and reply
//! envelopes are shared with the existing CLI and MCP projections.
use crate::migration::resident_store::Workspace;
use hmac::{Hmac, Mac};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::Sha256;
use slophawk_public_shell::protocol::{
    Command, DaemonClient, DaemonReply, MAX_REPLY_BYTES, Transport,
};
use slophawk_public_shell::{Result, failure, mcp};
use std::fs::{self, File};
use std::io::{self, Read, Write};
use std::net::{Ipv4Addr, SocketAddrV4, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
};
use std::time::{Duration, Instant};

type Auth = Hmac<Sha256>;
const POLL: Duration = Duration::from_millis(500);
const CONCURRENT: usize = 8;

#[derive(Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct Record {
    port: u16,
    key: [u8; 32],
}

struct Session(PathBuf);
impl Session {
    fn read(&self) -> io::Result<Record> {
        let meta = fs::symlink_metadata(&self.0)?;
        if !meta.is_file() || meta.len() > 1024 {
            return Err(io::Error::other("invalid native session descriptor"));
        }
        slophawk_public_shell::identity::require_physical(&self.0, false)
            .map_err(io::Error::other)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            if meta.mode() & 0o077 != 0 {
                return Err(io::Error::other("native descriptor is not private"));
            }
        }
        let mut bytes = Vec::new();
        File::open(&self.0)?.take(1025).read_to_end(&mut bytes)?;
        if bytes.len() > 1024 {
            return Err(io::Error::other("native descriptor exceeded its bound"));
        }
        serde_json::from_slice(&bytes).map_err(io::Error::other)
    }
}

impl Transport for Session {
    fn exchange(&self, request: &[u8]) -> Result<Option<Vec<u8>>> {
        let record = match self.read() {
            Ok(value) => value,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(failure("daemon_io", error)),
        };
        let address = SocketAddrV4::new(Ipv4Addr::LOCALHOST, record.port);
        let socket = match TcpStream::connect_timeout(&address.into(), Duration::from_millis(500)) {
            Ok(socket) => socket,
            Err(error) if error.kind() == io::ErrorKind::ConnectionRefused => return Ok(None),
            Err(error) => return Err(failure("daemon_io", error)),
        };
        client_exchange(socket, &record, request)
            .map(Some)
            .map_err(|e| failure("daemon_io", e))
    }
}

fn authenticate(
    key: &[u8; 32],
    channel: &[u8],
    server: &[u8; 32],
    client: &[u8; 32],
    data: &[u8],
) -> Auth {
    let mut mac = Auth::new_from_slice(key).expect("fixed HMAC key length");
    mac.update(channel);
    mac.update(server);
    mac.update(client);
    mac.update(data);
    mac
}

struct Connection {
    stream: TcpStream,
    deadline: Instant,
}
impl Connection {
    fn limit(&mut self, duration: Duration) {
        self.deadline = Instant::now() + duration;
    }
    fn remaining(&self) -> io::Result<Duration> {
        self.deadline
            .checked_duration_since(Instant::now())
            .filter(|left| !left.is_zero())
            .ok_or_else(|| io::Error::new(io::ErrorKind::TimedOut, "native IPC deadline expired"))
    }
}
impl Read for Connection {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        self.stream.set_read_timeout(Some(self.remaining()?))?;
        self.stream.read(buffer)
    }
}
impl Write for Connection {
    fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
        self.stream.set_write_timeout(Some(self.remaining()?))?;
        self.stream.write(buffer)
    }
    fn flush(&mut self) -> io::Result<()> {
        self.stream.flush()
    }
}

fn random() -> io::Result<[u8; 32]> {
    let mut bytes = [0; 32];
    getrandom::fill(&mut bytes).map_err(io::Error::other)?;
    Ok(bytes)
}

fn client_exchange(stream: TcpStream, record: &Record, request: &[u8]) -> io::Result<Vec<u8>> {
    if !matches!(request.len(), 1 | 3) {
        return Err(io::Error::other("invalid workspace command size"));
    }
    stream.set_nodelay(true)?;
    let mut io = Connection {
        stream,
        deadline: Instant::now() + Duration::from_secs(6),
    };
    let mut challenge = [0; 32];
    io.read_exact(&mut challenge)?;
    let client = random()?;
    let signature = authenticate(&record.key, b"command", &challenge, &client, request)
        .finalize()
        .into_bytes();
    io.write_all(&client)?;
    io.write_all(&[request.len() as u8])?;
    io.write_all(request)?;
    io.write_all(&signature)?;
    let mut length = [0; 4];
    io.read_exact(&mut length)?;
    let size = u32::from_le_bytes(length) as usize;
    if !(1..=MAX_REPLY_BYTES).contains(&size) {
        return Err(io::Error::other("invalid daemon reply size"));
    }
    let mut bytes = vec![0; size];
    let mut signature = [0; 32];
    io.read_exact(&mut bytes)?;
    io.read_exact(&mut signature)?;
    authenticate(&record.key, b"reply", &challenge, &client, &bytes)
        .verify_slice(&signature)
        .map_err(|_| io::Error::other("native daemon reply authentication failed"))?;
    io.write_all(&[1])?;
    Ok(bytes)
}

fn success(payload: Value) -> DaemonReply {
    DaemonReply {
        ok: true,
        payload: Some(payload),
        error: None,
        code: None,
    }
}
fn refused(code: &str, error: &str) -> DaemonReply {
    DaemonReply {
        ok: false,
        payload: None,
        error: Some(error.into()),
        code: Some(code.into()),
    }
}

fn request(
    stream: TcpStream,
    key: [u8; 32],
    store: &Mutex<Workspace>,
    stopping: &AtomicBool,
    recheck: &AtomicBool,
) -> io::Result<()> {
    // Windows accepted sockets inherit the listener's nonblocking mode.
    // Each connection has its own thread and bounded blocking I/O deadlines.
    stream.set_nonblocking(false)?;
    stream.set_nodelay(true)?;
    let mut io = Connection {
        stream,
        deadline: Instant::now() + Duration::from_millis(750),
    };
    let challenge = random()?;
    io.write_all(&challenge)?;
    let mut client = [0; 32];
    let mut size = [0];
    let mut signature = [0; 32];
    io.read_exact(&mut client)?;
    io.read_exact(&mut size)?;
    let mut frame = [0; 3];
    let command = match size[0] {
        1 => &mut frame[..1],
        3 => &mut frame[..],
        _ => return Ok(()),
    };
    io.read_exact(command)?;
    io.read_exact(&mut signature)?;
    authenticate(&key, b"command", &challenge, &client, command)
        .verify_slice(&signature)
        .map_err(|_| io::Error::other("native daemon command authentication failed"))?;
    let (reply, stop) = {
        let mut state = store
            .lock()
            .map_err(|_| io::Error::other("native workspace lock was poisoned"))?;
        state.authorized()?;
        match command {
            [5] => (
                success(json!({"wireVersion":1,"reportSchemaVersion":2,
                "capabilities":["status","results","recheck","stop","list_checkers"]})),
                false,
            ),
            [6, 1, 1] => (success(state.status()), false),
            [6, 1, 2] => (
                state.results().map(success).unwrap_or_else(|| {
                    refused(
                        "no_current_report",
                        "No current inventory report. Check workspace status.",
                    )
                }),
                false,
            ),
            [6, 1, 3] => {
                let revision = state.invalidate()?;
                recheck.store(true, Ordering::Release);
                (success(json!({"revision":revision})), false)
            }
            [6, 1, 4] => (success(json!({"stopping":true})), true),
            [6, _, _] => (
                refused(
                    "unsupported_wire_version",
                    "Expected daemon protocol version 1.",
                ),
                false,
            ),
            _ => (refused("unknown_command", "Unknown daemon command."), false),
        }
    };
    let bytes = serde_json::to_vec(&reply).map_err(io::Error::other)?;
    if bytes.len() > MAX_REPLY_BYTES {
        return Err(io::Error::other("native reply is oversized"));
    }
    let signature = authenticate(&key, b"reply", &challenge, &client, &bytes)
        .finalize()
        .into_bytes();
    io.limit(Duration::from_secs(5));
    io.write_all(&(bytes.len() as u32).to_le_bytes())?;
    io.write_all(&bytes)?;
    io.write_all(&signature)?;
    io.limit(Duration::from_secs(1));
    let mut ack = [0];
    io.read_exact(&mut ack)?;
    if stop && ack[0] == 1 {
        stopping.store(true, Ordering::Release);
    }
    Ok(())
}

pub fn serve(root: &Path, state: &Path, profile: &str) -> io::Result<()> {
    let workspace = Workspace::open(root, state, profile)?;
    let session = Session(state.join("native-daemon.json"));
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))?;
    listener.set_nonblocking(true)?;
    let record = Record {
        port: listener.local_addr()?.port(),
        key: random()?,
    };
    let stage = state.join(format!("native-daemon-{}.pending", record.port));
    if fs::symlink_metadata(&session.0).is_ok() {
        session.read()?;
    }
    super::migration::create_bound(
        &stage,
        &serde_json::to_vec(&record).map_err(io::Error::other)?,
    )?;
    fs::rename(&stage, &session.0)?;
    let workspace = Arc::new(Mutex::new(workspace));
    let stopping = Arc::new(AtomicBool::new(false));
    let recheck = Arc::new(AtomicBool::new(false));
    let mut checker: Option<crate::migration::resident_store::CheckerWorker> = None;
    let result = std::thread::scope(|threads| -> io::Result<()> {
        let mut active = Vec::<std::thread::ScopedJoinHandle<'_, ()>>::new();
        let mut next_scan = Instant::now() + POLL;
        while !stopping.load(Ordering::Acquire) {
            active.retain(|handle| !handle.is_finished());
            {
                let mut state = workspace
                    .lock()
                    .map_err(|_| io::Error::other("native workspace lock was poisoned"))?;
                if state.authorized().is_err() {
                    break;
                }
                if let Some(finished) = checker.as_mut().and_then(|worker| worker.completed()) {
                    state.finish_checks(finished);
                    checker = None;
                }
                if recheck.swap(false, Ordering::AcqRel) || Instant::now() >= next_scan {
                    state.refresh();
                    if checker.is_none() {
                        checker = state.schedule_checks();
                    }
                    next_scan = Instant::now() + POLL;
                }
            }
            if active.len() < CONCURRENT {
                match listener.accept() {
                    Ok((stream, _)) => {
                        let key = record.key;
                        let workspace = Arc::clone(&workspace);
                        let stopping = Arc::clone(&stopping);
                        let recheck = Arc::clone(&recheck);
                        active.push(threads.spawn(move || {
                            if let Err(error) =
                                request(stream, key, &workspace, &stopping, &recheck)
                            {
                                if std::env::var_os("SLOPHAWK_NATIVE_DIAGNOSTICS").is_some() {
                                    eprintln!("Native daemon connection failed: {error}");
                                }
                            }
                        }));
                        continue;
                    }
                    Err(error) if error.kind() == io::ErrorKind::WouldBlock => (),
                    Err(error) => return Err(error),
                }
            }
            std::thread::sleep(Duration::from_millis(15));
        }
        Ok(())
    });
    drop(checker);
    if session.read().is_ok_and(|current| current == record) {
        fs::remove_file(&session.0)?;
    }
    result
}

pub fn cli(root: &Path, state: &Path, profile: &str, verb: &str) -> Result<i32> {
    let client = DaemonClient::new(
        Session(state.join("native-daemon.json")),
        root.display().to_string(),
    );
    match verb {
        "status" => super::print_reply(&client.command(Command::Status)?),
        "results" => super::print_reply(&client.command(Command::Results)?),
        "recheck" => super::print_reply(&client.command(Command::Recheck)?),
        "stop" => super::print_reply(&client.command(Command::Stop)?),
        "start" | "mcp" => {
            let ready = start(&client, root, state, profile)?;
            if verb == "start" || !ready.ok {
                return super::print_reply(&ready);
            }
            mcp::serve(&client, io::stdin().lock(), io::stdout().lock())?;
            Ok(0)
        }
        _ => Err(failure(
            "invalid_arguments",
            "Unknown native workspace command",
        )),
    }
}

fn start(
    client: &DaemonClient<Session>,
    root: &Path,
    state: &Path,
    profile: &str,
) -> Result<DaemonReply> {
    match client.command(Command::Status) {
        Ok(reply) => return Ok(reply),
        Err(error) if error.code == "daemon_unavailable" => (),
        Err(error) => return Err(error),
    }
    let mut child =
        crate::detached::spawn(root, state, profile).map_err(|e| failure("failed_launch", e))?;
    let deadline = Instant::now() + Duration::from_secs(8);
    loop {
        match client.command(Command::Status) {
            Ok(reply) => return Ok(reply),
            Err(error) if error.code == "daemon_unavailable" => (),
            Err(error) => return Err(error),
        }
        if child.exited().map_err(|e| failure("failed_launch", e))? {
            return Err(failure(
                "failed_launch",
                "Native daemon exited before readiness.",
            ));
        }
        if Instant::now() >= deadline {
            return Err(failure(
                "daemon_timeout",
                "Native daemon did not become ready",
            ));
        }
        std::thread::sleep(Duration::from_millis(30));
    }
}
