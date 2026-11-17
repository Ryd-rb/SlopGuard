//! Bounded byte I/O and same-filesystem publication primitives.
use sha2::{Digest, Sha256};
use std::{fs::{self, File, OpenOptions}, io::{Read, Write}, path::{Path, PathBuf}};
use uuid::Uuid;

pub(crate) const MAX_DOCUMENT: usize = slophawk_probe_replay::MAX_SOURCE_BYTES;
pub(crate) const MAX_ASSEMBLY: u64 = 512 * 1024 * 1024;

pub(crate) fn read_limit(path: &Path, cap: usize) -> Result<Vec<u8>, String> {
    let mut file = File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
    if file.metadata().map_err(|e| e.to_string())?.len() > cap as u64 {
        return Err(format!("{} exceeds {cap} bytes", path.display()));
    }
    let mut content = Vec::new();
    file.take(cap as u64 + 1).read_to_end(&mut content)
        .map_err(|e| format!("{}: {e}", path.display()))?;
    if content.len() > cap { return Err(format!("{} exceeds {cap} bytes", path.display())); }
    Ok(content)
}

pub(crate) fn hash(bytes: &[u8]) -> String {
    format!("{:X}", Sha256::digest(bytes))
}

pub(crate) fn hash_assembly(path: &Path) -> Result<String, String> {
    let mut file = File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let length = file.metadata().map_err(|e| e.to_string())?.len();
    if length > MAX_ASSEMBLY { return Err("The candidate assembly exceeds the hash budget.".into()); }
    let mut hasher = Sha256::new();
    let mut read = 0_u64;
    let mut buffer = [0_u8; 32 * 1024];
    loop {
        let count = file.read(&mut buffer).map_err(|e| e.to_string())?;
        if count == 0 { break; }
        read += count as u64;
        if read > MAX_ASSEMBLY { return Err("The candidate assembly grew past the hash budget.".into()); }
        hasher.update(&buffer[..count]);
    }
    if read != length { return Err("The candidate assembly changed during hashing.".into()); }
    Ok(format!("{:X}", hasher.finalize()))
}

pub(crate) fn write_new(path: &Path, bytes: &[u8]) -> Result<(), String> {
    if bytes.len() > MAX_DOCUMENT { return Err("Checker artifact exceeds 8 MiB.".into()); }
    let mut f = OpenOptions::new().write(true).create_new(true).open(path)
        .map_err(|e| format!("{}: {e}", path.display()))?;
    f.write_all(bytes).and_then(|_| f.sync_all())
        .map_err(|e| format!("{}: {e}", path.display()))
}

pub(crate) fn new_id() -> String { Uuid::new_v4().simple().to_string() }

pub(crate) fn is_run_id(value: &str) -> bool {
    value.len() == 32 && value.bytes().all(|b| b.is_ascii_hexdigit()) &&
        Uuid::parse_str(value).is_ok()
}

pub(crate) fn checks_dir(workspace: &Path) -> PathBuf { workspace.join(".slophawk").join("checks") }

pub(crate) fn atomic_write(path: &Path, bytes: &[u8]) -> Result<(), String> {
    if bytes.len() > MAX_DOCUMENT { return Err("Checker publication exceeds 8 MiB.".into()); }
    let name = format!("{}.{}.pending", path.file_name().ok_or("Invalid publication path")?.to_string_lossy(), new_id());
    let temp = path.with_file_name(name);
    write_new(&temp, bytes)?;
    let outcome = replace_file(&temp, path);
    if outcome.is_err() { let _ = fs::remove_file(&temp); }
    outcome
}

#[cfg(not(windows))]
fn replace_file(from: &Path, to: &Path) -> Result<(), String> {
    fs::rename(from, to).map_err(|e| format!("Cannot publish {}: {e}", to.display()))
}

#[cfg(windows)]
fn replace_file(from: &Path, to: &Path) -> Result<(), String> {
    use std::os::windows::ffi::OsStrExt;
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn MoveFileExW(old: *const u16, new: *const u16, flags: u32) -> i32;
    }
    let old: Vec<u16> = from.as_os_str().encode_wide().chain(Some(0)).collect();
    let new: Vec<u16> = to.as_os_str().encode_wide().chain(Some(0)).collect();
    // MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH; no delete/recreate gap.
    let result = unsafe { MoveFileExW(old.as_ptr(), new.as_ptr(), 0x1 | 0x8) };
    if result != 0 { Ok(()) } else {
        Err(format!("Cannot publish {}: {}", to.display(), std::io::Error::last_os_error()))
    }
}

pub(crate) fn ensure_workspace(workspace: &str) -> Result<PathBuf, String> {
    let path = fs::canonicalize(workspace).map_err(|e| format!("Workspace {workspace}: {e}"))?;
    if !path.is_dir() { return Err("The workspace directory must already exist.".into()); }
    Ok(path)
}

pub(crate) fn read_json<T: serde::de::DeserializeOwned>(path: &Path) -> Result<T, String> {
    let bytes = read_limit(path, MAX_DOCUMENT)?;
    serde_json::from_slice(&bytes).map_err(|e| format!("{}: {e}", path.display()))
}

pub(crate) fn serialize<T: serde::Serialize>(value: &T) -> Result<Vec<u8>, String> {
    let bytes = serde_json::to_vec(value).map_err(|e| e.to_string())?;
    if bytes.len() > MAX_DOCUMENT { return Err("Checker JSON exceeds 8 MiB.".into()); }
    Ok(bytes)
}

/// Portable exclusive create; the lock survives crashes rather than silently
/// overwriting acknowledgments whose writer may not have drained.
pub(crate) struct Lock {
    path: PathBuf,
    file: Option<File>,
}
impl Lock {
    pub fn acquire(path: &Path) -> Result<Self, String> {
        let lock_path = PathBuf::from(format!("{}.lock", path.display()));
        let file = OpenOptions::new().write(true).create_new(true).open(&lock_path)
            .map_err(|e| format!("Acknowledgments are locked at {}: {e}", lock_path.display()))?;
        Ok(Self { path: lock_path, file: Some(file) })
    }
}
impl Drop for Lock {
    fn drop(&mut self) {
        drop(self.file.take());
        let _ = fs::remove_file(&self.path);
    }
}
