//! Windows overlapped named-pipe client with a fixed I/O deadline, bounded
//! response allocation, one ACK and verified same-user server process token.
//! Uses OS FFI rather than adding a new unpinned Windows dependency.
#![cfg(windows)]
use crate::protocol::read_reply_and_ack;
use crate::{Result, failure};
use std::ffi::{OsStr, c_void};
use std::io::{self, Read, Write};
use std::os::windows::ffi::OsStrExt;
use std::ptr::{null, null_mut};
use std::time::{Duration, Instant};

type Handle = *mut c_void;
const INVALID_HANDLE_VALUE: Handle = -1isize as Handle;
const GENERIC_READ_WRITE: u32 = 0xC000_0000;
const OPEN_EXISTING: u32 = 3;
const FILE_FLAG_OVERLAPPED: u32 = 0x4000_0000;
const ERROR_FILE_NOT_FOUND: u32 = 2;
const ERROR_PIPE_BUSY: u32 = 231;
const ERROR_IO_PENDING: u32 = 997;
const WAIT_OBJECT_0: u32 = 0;
const WAIT_TIMEOUT: u32 = 258;
const PROCESS_QUERY_LIMITED_INFORMATION: u32 = 0x1000;
const TOKEN_QUERY: u32 = 0x0008;
const TOKEN_USER: u32 = 1;
const MAX_TOKEN_BYTES: u32 = 32 * 1024;
const INFINITE: u32 = 0xFFFF_FFFF;

#[repr(C)]
struct Overlapped {
    internal: usize,
    internal_high: usize,
    offset: u32,
    offset_high: u32,
    h_event: Handle,
}

#[repr(C)]
#[derive(Copy, Clone)]
struct SidAndAttributes { sid: *mut c_void, attributes: u32 }
#[repr(C)]
#[derive(Copy, Clone)]
struct TokenUser { user: SidAndAttributes }

#[link(name = "Kernel32")]
unsafe extern "system" {
    fn CreateFileW(name: *const u16, access: u32, share: u32, security: *const c_void,
        disposition: u32, flags: u32, template: Handle) -> Handle;
    fn WaitNamedPipeW(name: *const u16, timeout: u32) -> i32;
    fn GetNamedPipeServerProcessId(pipe: Handle, process: *mut u32) -> i32;
    fn OpenProcess(access: u32, inherit: i32, process: u32) -> Handle;
    fn GetCurrentProcess() -> Handle;
    fn CreateEventW(security: *const c_void, manual: i32, initial: i32, name: *const u16) -> Handle;
    fn ReadFile(handle: Handle, buffer: *mut c_void, requested: u32, received: *mut u32,
        overlapped: *mut Overlapped) -> i32;
    fn WriteFile(handle: Handle, buffer: *const c_void, requested: u32, written: *mut u32,
        overlapped: *mut Overlapped) -> i32;
    fn WaitForSingleObject(handle: Handle, timeout: u32) -> u32;
    fn GetOverlappedResult(handle: Handle, overlapped: *mut Overlapped,
        transferred: *mut u32, wait: i32) -> i32;
    fn CancelIoEx(handle: Handle, overlapped: *mut Overlapped) -> i32;
    fn CloseHandle(handle: Handle) -> i32;
    fn GetLastError() -> u32;
}
#[link(name = "Advapi32")]
unsafe extern "system" {
    fn OpenProcessToken(process: Handle, access: u32, token: *mut Handle) -> i32;
    fn GetTokenInformation(token: Handle, class: u32, info: *mut c_void, length: u32,
        required: *mut u32) -> i32;
    fn EqualSid(first: *const c_void, second: *const c_void) -> i32;
}

struct OwnedHandle(Handle);
impl OwnedHandle {
    fn new(raw: Handle) -> io::Result<Self> {
        if raw.is_null() || raw == INVALID_HANDLE_VALUE { Err(io::Error::last_os_error()) }
        else { Ok(Self(raw)) }
    }
}
impl Drop for OwnedHandle {
    fn drop(&mut self) { unsafe { CloseHandle(self.0); } }
}

struct Pipe {
    handle: OwnedHandle,
    deadline: Instant,
}

pub fn exchange(instance: &str, request: &[u8]) -> Result<Option<Vec<u8>>> {
    let path = format!(r"\\.\pipe\{instance}");
    let wide: Vec<u16> = OsStr::new(&path).encode_wide().chain(Some(0)).collect();
    let Some(mut pipe) = connect(&wide)? else { return Ok(None); };
    pipe.write_all(request).map_err(|e| failure("daemon_io", e))?;
    let reply = read_reply_and_ack(&mut pipe)?;
    Ok(Some(reply))
}

fn connect(path: &[u16]) -> Result<Option<Pipe>> {
    let until = Instant::now() + Duration::from_millis(1_500);
    loop {
        // SAFETY: NUL-terminated path, CreateFileW owns returned handle.
        let raw = unsafe { CreateFileW(path.as_ptr(), GENERIC_READ_WRITE, 0, null(),
            OPEN_EXISTING, FILE_FLAG_OVERLAPPED, null_mut()) };
        if raw != INVALID_HANDLE_VALUE {
            let handle = OwnedHandle::new(raw).map_err(|e| failure("daemon_io", e))?;
            verify_server_user(handle.0)?;
            return Ok(Some(Pipe { handle, deadline: Instant::now() + Duration::from_secs(6) }));
        }
        let error = unsafe { GetLastError() };
        if error == ERROR_FILE_NOT_FOUND { return Ok(None); }
        if error != ERROR_PIPE_BUSY {
            return Err(failure("daemon_io", io::Error::from_raw_os_error(error as i32)));
        }
        let remaining = until.saturating_duration_since(Instant::now());
        if remaining.is_zero() { return Err(failure("daemon_busy", "Daemon pipe is busy; it is not safe to start another listener.")); }
        let timeout = remaining.as_millis().min(100).max(1) as u32;
        unsafe { WaitNamedPipeW(path.as_ptr(), timeout); }
    }
}

fn verify_server_user(pipe: Handle) -> Result<()> {
    let mut server = 0u32;
    if unsafe { GetNamedPipeServerProcessId(pipe, &mut server) } == 0 || server == 0 {
        return Err(failure("peer_identity", "Cannot identify the named-pipe server process."));
    }
    let proc = OwnedHandle::new(unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, server) })
        .map_err(|e| failure("peer_identity", e))?;
    let current = user_sid(unsafe { GetCurrentProcess() })?;
    let remote = user_sid(proc.0)?;
    // SAFETY: both pointers point into live token-information buffers here.
    if unsafe { EqualSid(current.1 as *const c_void, remote.1 as *const c_void) } == 0 {
        return Err(failure("peer_identity", "Named-pipe peer is not running as the current user."));
    }
    Ok(())
}

fn user_sid(process: Handle) -> Result<(Vec<u8>, *mut c_void)> {
    let mut token: Handle = null_mut();
    if unsafe { OpenProcessToken(process, TOKEN_QUERY, &mut token) } == 0 {
        return Err(failure("peer_identity", io::Error::last_os_error()));
    }
    let token = OwnedHandle::new(token).map_err(|e| failure("peer_identity", e))?;
    let mut count = 0u32;
    unsafe { GetTokenInformation(token.0, TOKEN_USER, null_mut(), 0, &mut count); }
    if count == 0 || count > MAX_TOKEN_BYTES {
        return Err(failure("peer_identity", "Invalid peer token size."));
    }
    let mut buffer = vec![0u8; count as usize];
    if unsafe { GetTokenInformation(token.0, TOKEN_USER, buffer.as_mut_ptr().cast(),
        count, &mut count) } == 0 {
        return Err(failure("peer_identity", io::Error::last_os_error()));
    }
    // SAFETY: TOKEN_USER starts with SID_AND_ATTRIBUTES and the buffer remains
    // live until after the subsequent EqualSid call. read_unaligned avoids an
    // alignment assumption about the Win32-written byte buffer.
    let header = unsafe { std::ptr::read_unaligned(buffer.as_ptr().cast::<TokenUser>()) };
    if header.user.sid.is_null() {
        return Err(failure("peer_identity", "Peer token lacks a user SID."));
    }
    Ok((buffer, header.user.sid))
}

impl Pipe {
    fn io(&self, buffer: *mut c_void, length: usize, is_read: bool) -> io::Result<usize> {
        let event = OwnedHandle::new(unsafe { CreateEventW(null(), 1, 0, null()) })?;
        let mut op = Overlapped { internal: 0, internal_high: 0, offset: 0,
            offset_high: 0, h_event: event.0 };
        let mut transferred = 0u32;
        let length = length.min(u32::MAX as usize) as u32;
        // SAFETY: buffer remains allocated until the completion/cancellation is
        // drained, including a timed-out overlapped operation.
        let ok = unsafe {
            if is_read {
                ReadFile(self.handle.0, buffer, length, null_mut(), &mut op)
            } else {
                WriteFile(self.handle.0, buffer, length, null_mut(), &mut op)
            }
        };
        if ok == 0 && unsafe { GetLastError() } != ERROR_IO_PENDING {
            return Err(io::Error::last_os_error());
        }
        let duration = self.deadline.saturating_duration_since(Instant::now());
        let ms = duration.as_millis().min(u32::MAX as u128) as u32;
        let wait = unsafe { WaitForSingleObject(event.0, ms) };
        if wait != WAIT_OBJECT_0 {
            // Windows cancellation is asynchronous: always drain the pending
            // completion before reusing/dropping the request's buffer or event.
            unsafe { CancelIoEx(self.handle.0, &mut op); }
            unsafe { WaitForSingleObject(event.0, INFINITE); }
            if wait == WAIT_TIMEOUT { return Err(io::Error::new(io::ErrorKind::TimedOut, "Daemon pipe I/O timed out.")); }
            return Err(io::Error::other("Daemon pipe wait failed."));
        }
        if unsafe { GetOverlappedResult(self.handle.0, &mut op, &mut transferred, 0) } == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(transferred as usize)
    }
}

impl Read for Pipe {
    fn read(&mut self, destination: &mut [u8]) -> io::Result<usize> {
        if destination.is_empty() { return Ok(0); }
        self.io(destination.as_mut_ptr().cast(), destination.len(), true)
    }
}
impl Write for Pipe {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if bytes.is_empty() { return Ok(0); }
        self.io(bytes.as_ptr() as *mut c_void, bytes.len(), false)
    }
    fn flush(&mut self) -> io::Result<()> { Ok(()) }
}
