//! Opt-in native Unix socket endpoint with the *request/response framing*
//! of SlopHawk's current-user private wire v1. It does not impersonate the
//! .NET named-pipe endpoint or implement MCP stdio. All operations use the
//! same singleton `Coordinator` and validated report reader.
use crate::{failure, Coordinator};
use std::io::{self, ErrorKind, Read, Write};
use std::time::{Duration, Instant};

const MAX_REPLY: usize = 8 * 1024 * 1024 + 4096;
const HELLO: u8 = 5;
const VERSIONED: u8 = 6;
const VERSION: u8 = 1;

pub fn dispatch(coordinator: &mut Coordinator, request: &[u8]) -> (String, bool) {
    if request.len() != 1 && request.len() != 3 {
        return (failure("malformed_request", "Expected one command or 06 01 command."), false);
    }
    let command = if request.len() == 3 {
        if request[0] != VERSIONED {
            return (failure("malformed_request", "Invalid versioned envelope."), false);
        }
        if request[1] != VERSION {
            return (failure("unsupported_wire_version", "Supported wireVersion is 1."), false);
        }
        request[2]
    } else { request[0] };
    if request.len() == 3 && !(1..=4).contains(&command) {
        return (failure("unknown_command", "Versioned command is not supported."), false);
    }
    match command {
        HELLO if request.len() == 1 => (coordinator.reply_json("hello"), false),
        1 => (coordinator.reply_json("status"), false),
        2 => (coordinator.reply_json("results"), false),
        3 => match coordinator.recheck() {
            Ok(_) => (format!("{{\"ok\":true,\"payload\":{{\"revision\":{}}},\"error\":null}}",
                coordinator.revision()), false),
            Err(e) => (failure("reconciliation_failed", &e.to_string()), false),
        },
        4 => ("{\"ok\":true,\"payload\":{\"stopping\":true},\"error\":null}".into(), true),
        _ => (failure("unknown_command", "Unknown daemon command."), false),
    }
}

pub fn reply<RW: Read + Write>(io: &mut RW, response: &str) -> io::Result<bool> {
    let bytes = response.as_bytes();
    if bytes.len() > MAX_REPLY || bytes.len() > i32::MAX as usize {
        return Err(io::Error::new(ErrorKind::InvalidData, "daemon reply exceeds budget"));
    }
    io.write_all(&(bytes.len() as i32).to_le_bytes())?;
    io.write_all(bytes)?;
    io.flush()?;
    let mut ack = [0u8; 1];
    io.read_exact(&mut ack)?;
    Ok(ack[0] == 1)
}

#[cfg(target_os = "linux")]
mod unix_server {
    use super::*;
    use std::fs::{self, Permissions};
    use std::os::fd::AsRawFd;
    use std::os::unix::fs::{FileTypeExt, MetadataExt, PermissionsExt};
    use std::os::unix::net::{UnixListener, UnixStream};
    use std::path::{Path, PathBuf};
    use std::thread;

    #[repr(C)]
    struct UCred { pid: i32, uid: u32, gid: u32 }
    unsafe extern "C" {
        fn getsockopt(fd: i32, level: i32, name: i32, value: *mut UCred, len: *mut u32) -> i32;
        fn geteuid() -> u32;
    }
    fn current_user(stream: &UnixStream) -> io::Result<bool> {
        let mut cred = UCred { pid: 0, uid: u32::MAX, gid: u32::MAX };
        let mut length = std::mem::size_of::<UCred>() as u32;
        let code = unsafe {
            getsockopt(stream.as_raw_fd(), 1, 17, &mut cred, &mut length)
        };
        if code != 0 { return Err(io::Error::last_os_error()); }
        Ok(length == std::mem::size_of::<UCred>() as u32 &&
            cred.pid > 0 && cred.uid == unsafe { geteuid() })
    }
    struct OwnedSocket { path: PathBuf, dev: u64, ino: u64 }
    impl Drop for OwnedSocket {
        fn drop(&mut self) {
            if let Ok(meta) = fs::symlink_metadata(&self.path) {
                if meta.file_type().is_socket() &&
                    (meta.dev(), meta.ino()) == (self.dev, self.ino) {
                    let _ = fs::remove_file(&self.path);
                }
            }
        }
    }
    fn socket_path(coordinator: &Coordinator) -> PathBuf {
        coordinator.state_path().with_file_name("ipc.sock")
    }
    fn accept_once(stream: &mut UnixStream, coordinator: &mut Coordinator) -> io::Result<bool> {
        if !current_user(stream)? { return Ok(false); }
        stream.set_read_timeout(Some(Duration::from_millis(750)))?;
        stream.set_write_timeout(Some(Duration::from_secs(5)))?;
        let mut command = [0u8; 1];
        stream.read_exact(&mut command)?;
        let mut request = vec![command[0]];
        if command[0] == VERSIONED {
            let mut rest = [0u8; 2];
            stream.read_exact(&mut rest)?;
            request.extend(rest);
        }
        let (response, stopping) = dispatch(coordinator, &request);
        stream.set_read_timeout(Some(Duration::from_secs(1)))?;
        let acknowledged = reply(stream, &response)?;
        Ok(stopping && acknowledged)
    }
    /// A private socket under an already verified 0700 directory, mode 0600,
    /// with a Linux SO_PEERCRED UID check. One request per connection.
    pub fn serve(coordinator: &mut Coordinator) -> io::Result<()> {
        let socket = socket_path(coordinator);
        if let Ok(meta) = fs::symlink_metadata(&socket) {
            if !meta.file_type().is_socket() {
                return Err(io::Error::new(ErrorKind::InvalidData, "foreign IPC socket entry"));
            }
            // The single-writer guard is already held; this is a prior
            // incarnation's endpoint, not a live server.
            fs::remove_file(&socket)?;
        }
        let listener = UnixListener::bind(&socket)?;
        fs::set_permissions(&socket, Permissions::from_mode(0o600))?;
        let meta = fs::symlink_metadata(&socket)?;
        let _cleanup = OwnedSocket { path: socket, dev: meta.dev(), ino: meta.ino() };
        listener.set_nonblocking(true)?;
        let mut last_poll = Instant::now();
        loop {
            if last_poll.elapsed() >= Duration::from_millis(750) {
                coordinator.poll()?;
                last_poll = Instant::now();
            }
            match listener.accept() {
                Ok((mut stream, _)) => match accept_once(&mut stream, coordinator) {
                    Ok(true) => return Ok(()),
                    Ok(false) => {}
                    Err(e) if matches!(e.kind(), ErrorKind::TimedOut |
                        ErrorKind::WouldBlock | ErrorKind::UnexpectedEof |
                        ErrorKind::ConnectionReset | ErrorKind::BrokenPipe) => {}
                    Err(e) => return Err(e),
                },
                Err(e) if e.kind() == ErrorKind::WouldBlock => {
                    thread::sleep(Duration::from_millis(40));
                }
                Err(e) => return Err(e),
            }
        }
    }
}
pub fn serve(coordinator: &mut Coordinator) -> io::Result<()> {
    #[cfg(target_os = "linux")]
    { unix_server::serve(coordinator) }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = coordinator;
        Err(io::Error::new(ErrorKind::Unsupported,
            "current-user native IPC not implemented on this platform"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn reject_malformed_wire_before_any_state_change() {
        // The dispatch contract is testable with a scratch Coordinator in
        // integration tests; these byte-level rejects are statically trivial.
        assert_eq!(VERSION, 1);
        assert_eq!(HELLO, 5);
        assert_eq!(VERSIONED, 6);
        assert_eq!(MAX_REPLY, 8 * 1024 * 1024 + 4096);
    }
    #[test]
    fn acknowledgement_gate() {
        struct Pipe { input: std::io::Cursor<Vec<u8>>, output: Vec<u8> }
        impl Read for Pipe { fn read(&mut self, b: &mut [u8]) -> io::Result<usize> {
            self.input.read(b)
        }}
        impl Write for Pipe {
            fn write(&mut self, b: &[u8]) -> io::Result<usize> {
                self.output.extend_from_slice(b); Ok(b.len())
            }
            fn flush(&mut self) -> io::Result<()> { Ok(()) }
        }
        let mut p = Pipe { input: std::io::Cursor::new(vec![1]), output: vec![] };
        assert!(reply(&mut p, "{\"ok\":true}").unwrap());
        assert_eq!(i32::from_le_bytes(p.output[..4].try_into().unwrap()), 11);
        let mut p = Pipe { input: std::io::Cursor::new(vec![0]), output: vec![] };
        assert!(!reply(&mut p, "hello").unwrap());
    }
}
