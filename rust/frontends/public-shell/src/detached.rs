use std::{io, path::Path};

#[cfg(not(windows))]
pub struct Launch(std::process::Child);

#[cfg(not(windows))]
pub fn spawn(root: &Path, state: &Path, profile: &str) -> io::Result<Launch> {
    use std::process::{Command, Stdio};
    Command::new(std::env::current_exe()?)
        .arg("--native-resident-v1")
        .arg(root)
        .arg(state)
        .arg(profile)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map(Launch)
}

#[cfg(not(windows))]
impl Launch {
    pub fn exited(&mut self) -> io::Result<bool> {
        Ok(self.0.try_wait()?.is_some())
    }
}

#[cfg(windows)]
mod windows {
    use super::*;
    use std::{
        ffi::OsStr,
        os::windows::{
            ffi::OsStrExt,
            io::{AsRawHandle, FromRawHandle, OwnedHandle},
        },
        ptr::null,
    };
    use windows_sys::Win32::Foundation::{CloseHandle, WAIT_OBJECT_0, WAIT_TIMEOUT};
    use windows_sys::Win32::System::Threading::{
        CREATE_NEW_PROCESS_GROUP, CREATE_NO_WINDOW, CreateProcessW, PROCESS_INFORMATION,
        STARTUPINFOW, WaitForSingleObject,
    };

    pub struct Launch(OwnedHandle);

    impl Launch {
        pub fn exited(&mut self) -> io::Result<bool> {
            match unsafe { WaitForSingleObject(self.0.as_raw_handle(), 0) } {
                WAIT_OBJECT_0 => Ok(true),
                WAIT_TIMEOUT => Ok(false),
                _ => Err(io::Error::last_os_error()),
            }
        }
    }

    pub fn spawn(root: &Path, state: &Path, profile: &str) -> io::Result<Launch> {
        let executable = std::env::current_exe()?;
        let app: Vec<u16> = executable
            .as_os_str()
            .encode_wide()
            .chain(Some(0))
            .collect();
        let mut command = Vec::new();
        for value in [
            executable.as_os_str(),
            OsStr::new("--native-resident-v1"),
            root.as_os_str(),
            state.as_os_str(),
            OsStr::new(profile),
        ] {
            if !command.is_empty() {
                command.push(b' ' as u16);
            }
            quote(value, &mut command);
        }
        command.push(0);
        if command.len() > 32767 {
            return Err(io::Error::other("native daemon command line is oversized"));
        }
        let info = STARTUPINFOW {
            cb: std::mem::size_of::<STARTUPINFOW>() as u32,
            ..Default::default()
        };
        let mut process = PROCESS_INFORMATION::default();
        // Windows can keep other inheritable caller handles alive with ordinary
        // redirected Command spawning. FALSE excludes those handles here.
        let ok = unsafe {
            CreateProcessW(
                app.as_ptr(),
                command.as_mut_ptr(),
                null(),
                null(),
                0,
                CREATE_NO_WINDOW | CREATE_NEW_PROCESS_GROUP,
                null(),
                null(),
                &info,
                &mut process,
            )
        };
        if ok == 0 {
            return Err(io::Error::last_os_error());
        }
        unsafe {
            CloseHandle(process.hThread);
        }
        Ok(Launch(unsafe {
            OwnedHandle::from_raw_handle(process.hProcess)
        }))
    }

    fn quote(value: &OsStr, command: &mut Vec<u16>) {
        command.push(b'"' as u16);
        let mut slashes = 0;
        for c in value.encode_wide() {
            if c == b'\\' as u16 {
                slashes += 1;
                continue;
            }
            let count = if c == b'"' as u16 {
                2 * slashes + 1
            } else {
                slashes
            };
            command.extend(std::iter::repeat_n(b'\\' as u16, count));
            slashes = 0;
            command.push(c);
        }
        command.extend(std::iter::repeat_n(b'\\' as u16, 2 * slashes));
        command.push(b'"' as u16);
    }
}

#[cfg(windows)]
pub use windows::spawn;
