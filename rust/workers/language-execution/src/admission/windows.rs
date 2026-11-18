use std::fs::{File, OpenOptions};
use std::io;
use std::os::windows::ffi::OsStrExt;
use std::os::windows::fs::{MetadataExt, OpenOptionsExt};
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
use std::path::Path;
use std::ptr::null_mut;
use windows_sys::Win32::Foundation::{
    GENERIC_ALL, GENERIC_WRITE, INVALID_HANDLE_VALUE, LocalFree,
};
use windows_sys::Win32::Security::Authorization::{ConvertSidToStringSidW,
    ConvertStringSecurityDescriptorToSecurityDescriptorW, GetSecurityInfo, SDDL_REVISION_1, SE_FILE_OBJECT};
use windows_sys::Win32::Security::{
    ACCESS_ALLOWED_ACE, ACE_HEADER, ACL, DACL_SECURITY_INFORMATION, EqualSid, GetAce,
    GetTokenInformation, INHERIT_ONLY_ACE, IsValidAcl, IsValidSid, IsWellKnownSid,
    OWNER_SECURITY_INFORMATION, PSECURITY_DESCRIPTOR, PSID, SECURITY_ATTRIBUTES, TOKEN_QUERY, TOKEN_USER, TokenUser,
    WinBuiltinAdministratorsSid, WinCreatorOwnerRightsSid, WinLocalSystemSid,
};
use windows_sys::Win32::Storage::FileSystem::{
    CREATE_NEW, CreateDirectoryW, CreateFileW, DELETE, FILE_APPEND_DATA, FILE_ATTRIBUTE_NORMAL,
    FILE_ATTRIBUTE_REPARSE_POINT, FILE_DELETE_CHILD,
    FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT, FILE_GENERIC_READ,
    FILE_WRITE_ATTRIBUTES, FILE_WRITE_DATA, FILE_WRITE_EA, READ_CONTROL, WRITE_DAC, WRITE_OWNER,
};
use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

const ACCESS_ALLOWED_ACE_TYPE: u8 = 0;
const ACCESS_DENIED_ACE_TYPE: u8 = 1;

struct Descriptor(PSECURITY_DESCRIPTOR);

impl Drop for Descriptor {
    fn drop(&mut self) {
        unsafe { LocalFree(self.0); }
    }
}

fn trusted_sid(sid: PSID, user: PSID, private: bool) -> bool {
    #[repr(C)]
    struct ServiceSid {
        header: [u8; 8],
        sub_authorities: [u32; 6],
    }
    // S-1-5-80-956008885-3418522649-1831038044-1853292631-2271478464.
    // The MSVC and SDK installation trees allow the Windows servicing account.
    static TRUSTED_INSTALLER: ServiceSid = ServiceSid {
        header: [1, 6, 0, 0, 0, 0, 0, 5],
        sub_authorities: [80, 956008885, 3418522649, 1831038044, 1853292631, 2271478464],
    };
    unsafe {
        EqualSid(sid, user) != 0
            || IsWellKnownSid(sid, WinLocalSystemSid) != 0
            || IsWellKnownSid(sid, WinBuiltinAdministratorsSid) != 0
            || IsWellKnownSid(sid, WinCreatorOwnerRightsSid) != 0
            || !private && EqualSid(sid, (&raw const TRUSTED_INSTALLER).cast_mut().cast()) != 0
    }
}

fn current_user() -> io::Result<Vec<usize>> {
    let mut token = null_mut();
    if unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) } == 0 {
        return Err(io::Error::last_os_error());
    }
    let token = unsafe { OwnedHandle::from_raw_handle(token) };
    let mut needed = 0;
    unsafe { GetTokenInformation(token.as_raw_handle(), TokenUser, null_mut(), 0, &mut needed); }
    if needed == 0 || needed > 4096 {
        return Err(io::Error::last_os_error());
    }
    // TOKEN_USER contains a pointer into the same aligned buffer.
    let mut buffer = vec![0usize; (needed as usize).div_ceil(size_of::<usize>())];
    if unsafe { GetTokenInformation(token.as_raw_handle(), TokenUser,
        buffer.as_mut_ptr().cast(), needed, &mut needed) } == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(buffer)
}

fn with_owner_security<T>(directory: bool, action: impl FnOnce(&SECURITY_ATTRIBUTES) -> io::Result<T>) -> io::Result<T> {
    let buffer = current_user()?;
    // The token SID points into the live, aligned token-information buffer.
    let user = unsafe { (*buffer.as_ptr().cast::<TOKEN_USER>()).User.Sid };
    let mut sid_text = null_mut();
    if unsafe { ConvertSidToStringSidW(user, &mut sid_text) } == 0 {
        return Err(io::Error::last_os_error());
    }
    let mut length = 0;
    while unsafe { *sid_text.add(length) } != 0 { length += 1; }
    let sid = String::from_utf16(unsafe { std::slice::from_raw_parts(sid_text, length) });
    unsafe { LocalFree(sid_text.cast()); }
    let sid = sid.map_err(io::Error::other)?;
    let inheritance = if directory { "OICI" } else { "" };
    let sddl: Vec<u16> = format!("O:{sid}D:P(A;{inheritance};FA;;;{sid})(A;{inheritance};FA;;;SY)(A;{inheritance};FA;;;BA)")
        .encode_utf16().chain([0]).collect();
    let mut descriptor = Descriptor(null_mut());
    if unsafe { ConvertStringSecurityDescriptorToSecurityDescriptorW(sddl.as_ptr(),
        SDDL_REVISION_1, &mut descriptor.0, null_mut()) } == 0 {
        return Err(io::Error::last_os_error());
    }
    let attributes = SECURITY_ATTRIBUTES { nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
        lpSecurityDescriptor: descriptor.0, bInheritHandle: 0 };
    action(&attributes)
}

pub(super) fn create_private_directory(path: &Path) -> io::Result<()> {
    let name: Vec<u16> = path.as_os_str().encode_wide().chain([0]).collect();
    with_owner_security(true, |attributes| {
        if unsafe { CreateDirectoryW(name.as_ptr(), attributes) } == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    })
}

pub(super) fn create_private_file(path: &Path) -> io::Result<File> {
    let parent = path.parent().ok_or(io::ErrorKind::InvalidInput)?.canonicalize()?;
    let physical = parent.join(path.file_name().ok_or(io::ErrorKind::InvalidInput)?);
    let name: Vec<u16> = physical.as_os_str().encode_wide().chain([0]).collect();
    with_owner_security(false, |attributes| {
        let handle = unsafe { CreateFileW(name.as_ptr(), GENERIC_WRITE, 0, attributes,
            CREATE_NEW, FILE_ATTRIBUTE_NORMAL, null_mut()) };
        if handle == INVALID_HANDLE_VALUE {
            return Err(io::Error::last_os_error());
        }
        Ok(unsafe { File::from_raw_handle(handle) })
    })
}

fn owner_and_dacl(file: &File, private: bool) -> io::Result<()> {
    let buffer = current_user()?;
    let user = unsafe { (*buffer.as_ptr().cast::<TOKEN_USER>()).User.Sid };
    let mut security = Descriptor(null_mut());
    let mut acl: *mut ACL = null_mut();
    let mut owner = null_mut();
    let result = unsafe { GetSecurityInfo(file.as_raw_handle(), SE_FILE_OBJECT,
        OWNER_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION, &mut owner, null_mut(),
        &mut acl, null_mut(), &mut security.0) };
    if result != 0 {
        return Err(io::Error::from_raw_os_error(result as i32));
    }
    let denied = || io::Error::new(io::ErrorKind::PermissionDenied, "unsafe Windows owner or DACL");
    if owner.is_null() || acl.is_null() || unsafe { IsValidAcl(acl) } == 0
        || if private { unsafe { EqualSid(owner, user) == 0 } }
            else { !trusted_sid(owner, user, false) } {
        return Err(denied());
    }
    let write_mask = FILE_WRITE_DATA | FILE_APPEND_DATA | FILE_WRITE_ATTRIBUTES | FILE_WRITE_EA
        | FILE_DELETE_CHILD | DELETE | WRITE_DAC | WRITE_OWNER | GENERIC_ALL | GENERIC_WRITE;
    for index in 0..u32::from(unsafe { (*acl).AceCount }) {
        let mut entry = null_mut();
        if unsafe { GetAce(acl, index, &mut entry) } == 0 || entry.is_null() {
            return Err(denied());
        }
        let header = unsafe { &*entry.cast::<ACE_HEADER>() };
        if header.AceFlags & INHERIT_ONLY_ACE as u8 != 0
            || header.AceType == ACCESS_DENIED_ACE_TYPE {
            continue;
        }
        if header.AceType != ACCESS_ALLOWED_ACE_TYPE
            || usize::from(header.AceSize) < size_of::<ACCESS_ALLOWED_ACE>() {
            return Err(denied());
        }
        let entry = unsafe { &*entry.cast::<ACCESS_ALLOWED_ACE>() };
        let sid = (&raw const entry.SidStart).cast_mut().cast();
        if unsafe { IsValidSid(sid) } == 0 || !trusted_sid(sid, user, private)
            && (private && entry.Mask != 0 || entry.Mask & write_mask != 0) {
            return Err(denied());
        }
    }
    Ok(())
}

pub(super) fn open_checked(path: &Path, directory: bool, private: bool) -> io::Result<File> {
    let file = OpenOptions::new()
        .access_mode(READ_CONTROL | if directory { 0 } else { FILE_GENERIC_READ })
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT)
        .open(path)?;
    let meta = file.metadata()?;
    if meta.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
        || if directory { !meta.is_dir() } else { !meta.is_file() } {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "unexpected Windows file type"));
    }
    owner_and_dacl(&file, private)?;
    Ok(file)
}
