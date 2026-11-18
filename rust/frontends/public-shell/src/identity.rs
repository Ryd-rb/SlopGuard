//! Workspace and state identity shared with the C# tool adapters.
use crate::{Result, failure};
use sha2::{Digest, Sha256};
use std::fs;
use std::path::{Component, Path, PathBuf};

#[derive(Clone, Debug)]
pub struct WorkspaceIdentity {
    pub root: PathBuf,
    pub instance_name: String,
}

impl WorkspaceIdentity {
    pub fn resolve(path: &Path) -> Result<Self> {
        let absolute = std::path::absolute(path)
            .map_err(|e| failure("invalid_root", e))?;
        // Path.TrimEndingDirectorySeparator(Path.GetFullPath(...)) in C#.
        // Rebuild components to drop a trailing separator except for root.
        let mut root = PathBuf::new();
        for part in absolute.components() { root.push(part.as_os_str()); }
        require_physical(&root, true)?;
        let user = current_user()?;
        let text = root.to_str().ok_or_else(|| failure("invalid_root", "Workspace paths must be Unicode."))?;
        let mut key = user.into_bytes();
        key.push(0);
        #[cfg(windows)]
        let normalized: String = text.chars().map(|ch| {
            let mut upper = ch.to_uppercase();
            let mapped = upper.next().unwrap_or(ch);
            if upper.next().is_some() { ch } else { mapped }
        }).collect();
        #[cfg(not(windows))]
        let normalized = text;
        key.extend_from_slice(normalized.as_bytes());
        let digest = Sha256::digest(key);
        let name = format!("SlopHawk-{}", uppercase_hex(&digest[..16]));
        Ok(Self { root, instance_name: name })
    }

    pub fn state_directory(&self) -> Result<PathBuf> {
        #[cfg(windows)]
        let local = std::env::var_os("LOCALAPPDATA").map(PathBuf::from);
        #[cfg(target_os = "linux")]
        let local = std::env::var_os("XDG_DATA_HOME").map(PathBuf::from)
            .filter(|path| path.is_absolute())
            .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".local/share")));
        #[cfg(not(any(windows, target_os = "linux")))]
        let local: Option<PathBuf> = None;
        let path = local.filter(|path| path.is_absolute())
            .ok_or_else(|| failure("state_unavailable", "The per-user application data directory is unavailable."))?
            .join("SlopHawk").join("state").join(&self.instance_name);
        if path.starts_with(&self.root) {
            return Err(failure("invalid_state", "The application state directory must be outside the checkout."));
        }
        Ok(path)
    }
}

pub fn uppercase_hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789ABCDEF";
    let mut result = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        result.push(DIGITS[(b >> 4) as usize] as char);
        result.push(DIGITS[(b & 15) as usize] as char);
    }
    result
}

/// Check every existing component, not just the leaf: the C# workspace rejects
/// linked/reparse ancestors as well as linked roots. When leaf_is_dir is false,
/// the leaf must instead be an ordinary file.
pub fn require_physical(path: &Path, leaf_is_dir: bool) -> Result<()> {
    if !path.is_absolute() || path.components().any(|c|
        matches!(c, Component::ParentDir | Component::CurDir)) {
        return Err(failure("invalid_path", "A normalized absolute physical path is required."));
    }
    let mut prefix = PathBuf::new();
    for component in path.components() {
        prefix.push(component.as_os_str());
        if matches!(component, Component::Prefix(_)) || !prefix.has_root() {
            continue;
        }
        let meta = fs::symlink_metadata(&prefix)
            .map_err(|e| failure("invalid_path", format!("{}: {e}", prefix.display())))?;
        if meta.file_type().is_symlink() || is_reparse_point(&meta) {
            return Err(failure("invalid_path", format!("Linked path is not trusted: {}", prefix.display())));
        }
        if prefix != path && !meta.is_dir() {
            return Err(failure("invalid_path", "A path ancestor is not a directory."));
        }
        if prefix == path && meta.is_dir() != leaf_is_dir {
            return Err(failure("invalid_path", "Unexpected file type at the path leaf."));
        }
    }
    Ok(())
}

#[cfg(windows)]
fn is_reparse_point(meta: &fs::Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;
    (meta.file_attributes() & 0x400) != 0
}
#[cfg(not(windows))]
fn is_reparse_point(_: &fs::Metadata) -> bool { false }

#[cfg(windows)]
fn current_user() -> Result<String> {
    use std::os::windows::ffi::OsStringExt;
    #[link(name = "Advapi32")]
    unsafe extern "system" {
        fn GetUserNameW(name: *mut u16, count: *mut u32) -> i32;
    }
    let mut buffer = vec![0u16; 1024];
    let mut length = buffer.len() as u32;
    // SAFETY: Windows writes at most `length` UTF-16 code units into the buffer.
    if unsafe { GetUserNameW(buffer.as_mut_ptr(), &mut length) } == 0 || length < 2 {
        return Err(failure("identity_unavailable", "Unable to resolve the current Windows account."));
    }
    Ok(std::ffi::OsString::from_wide(&buffer[..length as usize - 1])
        .to_string_lossy().into_owned())
}

#[cfg(target_os = "linux")]
fn current_user() -> Result<String> {
    use std::{ffi::CStr, mem::MaybeUninit, ptr};
    let mut entry = MaybeUninit::<libc::passwd>::uninit();
    let mut buffer = vec![0u8; 64 * 1024];
    let mut result = ptr::null_mut();
    // SAFETY: The result and all strings point into the supplied live buffers.
    let code = unsafe { libc::getpwuid_r(libc::geteuid(), entry.as_mut_ptr(),
        buffer.as_mut_ptr().cast(), buffer.len(), &mut result) };
    if code != 0 || result.is_null() {
        return Err(failure("identity_unavailable", "Unable to resolve the current Linux account."));
    }
    // SAFETY: A successful passwd result has a terminated name in `buffer`.
    unsafe { CStr::from_ptr((*result).pw_name) }.to_str().map(str::to_owned)
        .map_err(|error| failure("identity_unavailable", error))
}

#[cfg(not(any(windows, target_os = "linux")))]
fn current_user() -> Result<String> {
    Err(failure("unsupported_os", "SlopHawk supports Windows and Linux."))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn workspace_pipe_hash_is_uppercase_and_sixteen_bytes() {
        let name = uppercase_hex(&[0, 15, 160, 255]);
        assert_eq!(name, "000FA0FF");
    }
}
