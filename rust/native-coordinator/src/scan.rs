//! Notification hints are never evidence. Each reconciliation scans the scoped
//! physical scratch root and compares stable content digests and relative names.
use crate::sha256::digest;
use crate::store::{invalid, Source};
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::fs::{self, File};
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

const MAX_FILES: usize = 8_192;
const MAX_DIRS: usize = 8_192;
const MAX_FILE_BYTES: u64 = 2 * 1024 * 1024;
const MAX_DEPTH: usize = 64;
const MAX_EVENTS: usize = 256;
const MAX_ENTRIES: usize = 50_000;
const MAX_TOTAL_SOURCE_BYTES: u64 = 64 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event {
    Add(String), Change(String), Delete(String), Rename { from: String, to: String },
    Git { branch: String, commit: String }, Overflow,
}

#[derive(Debug, Default)]
pub struct Coalescer {
    items: Vec<Event>,
    dirty: bool,
    first: Option<Instant>,
    last: Option<Instant>,
}

impl Coalescer {
    pub fn offer(&mut self, event: Event, now: Instant) {
        self.first.get_or_insert(now);
        self.last = Some(now);
        if self.items.len() >= MAX_EVENTS {
            self.items.clear();
            self.dirty = true;
        }
        if !self.dirty { self.items.push(event); }
    }
    pub fn due(&self, now: Instant) -> bool {
        matches!((self.first, self.last), (Some(first), Some(last)) if
            now.saturating_duration_since(last) >= Duration::from_millis(300) ||
            now.saturating_duration_since(first) >= Duration::from_secs(2))
    }
    pub fn take(&mut self) -> Vec<Event> {
        let mut events = std::mem::take(&mut self.items);
        if self.dirty { events.push(Event::Overflow); }
        self.first = None;
        self.last = None;
        self.dirty = false;
        events
    }
    pub fn is_empty(&self) -> bool { self.first.is_none() }
    pub fn len(&self) -> usize { self.items.len() }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Snapshot {
    pub files: BTreeMap<String, Source>,
    /// An empty scope means the root was not exhaustively enumerated.
    /// Other entries identify only the incomplete path/subtree. These are
    /// *frontiers*, not evidence that a previously known source was deleted.
    pub uncertain_scopes: BTreeSet<String>,
    pub partial: bool,
}
impl Snapshot {
    fn uncertain(&mut self, path: String) {
        self.uncertain_scopes.insert(path);
        self.partial = true;
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Delta {
    pub kind: &'static str,
    pub old_path: String,
    pub new_path: String,
    pub old_digest: String,
    pub new_digest: String,
}

pub fn root_identity(root: &Path) -> io::Result<String> {
    let canonical = root.canonicalize()?;
    if canonical != root { return Err(invalid("workspace root is no longer canonical")); }
    let meta = fs::symlink_metadata(root)?;
    if !meta.file_type().is_dir() || is_link(&meta) {
        return Err(invalid("workspace root replaced by link or file"));
    }
    #[cfg(unix)] {
        use std::os::unix::fs::MetadataExt;
        Ok(digest(format!("{}\0{}\0{}", root.display(), meta.dev(), meta.ino()).as_bytes()))
    }
    #[cfg(windows)] {
        windows_root_identity(root)
    }
    #[cfg(not(any(unix, windows)))] {
        Err(invalid("physical root identity not implemented on this platform"))
    }
}

fn is_link(metadata: &fs::Metadata) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        metadata.file_attributes() & 0x400 != 0
    }
    #[cfg(not(windows))]
    metadata.file_type().is_symlink()
}

#[cfg(windows)]
fn windows_root_identity(root: &Path) -> io::Result<String> {
    use std::os::windows::fs::OpenOptionsExt;
    let directory = fs::OpenOptions::new()
        .access_mode(0)
        .share_mode(7)
        .custom_flags(0x0220_0000) // Directory handle, without following reparse points.
        .open(root)?;
    let metadata = directory.metadata()?;
    if !metadata.is_dir() || is_link(&metadata) {
        return Err(invalid("workspace root replaced by link or file"));
    }
    let [volume, low, high] = windows_file_id(&directory)?;
    Ok(digest(format!("{root:?}\0{volume:016x}\0{low:016x}{high:016x}").as_bytes()))
}

#[cfg(windows)]
fn windows_file_id(file: &File) -> io::Result<[u64; 3]> {
    use std::ffi::c_void;
    use std::os::windows::io::AsRawHandle;
    #[repr(C)]
    #[derive(Default)]
    struct FileId {
        volume: u64,
        file: [u64; 2],
    }
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetFileInformationByHandleEx(handle: *mut c_void, class: i32,
            info: *mut FileId, size: u32) -> i32;
    }
    let mut id = FileId::default();
    // The opened handle and output buffer are valid for this operating-system call.
    if unsafe { GetFileInformationByHandleEx(file.as_raw_handle(), 18,
        &mut id, std::mem::size_of::<FileId>() as u32) } == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok([id.volume, id.file[0], id.file[1]])
}

#[cfg(windows)]
fn open_source(path: &Path) -> io::Result<File> {
    use std::os::windows::fs::OpenOptionsExt;
    let file = fs::OpenOptions::new().read(true)
        .share_mode(5) // Reading and renaming are allowed while the handle is held.
        .custom_flags(0x0020_0000) // Open the link itself rather than follow it.
        .open(path)?;
    let metadata = file.metadata()?;
    if !metadata.is_file() || is_link(&metadata) {
        return Err(invalid("source is not an ordinary file"));
    }
    Ok(file)
}

pub fn scan(root: &Path) -> io::Result<Snapshot> {
    let mut snapshot = Snapshot { files: BTreeMap::new(),
        uncertain_scopes: BTreeSet::new(), partial: false };
    let mut stack: VecDeque<(PathBuf, usize)> = VecDeque::from([(root.to_owned(), 0)]);
    let mut dirs = 0;
    let mut entries_seen = 0usize;
    let mut source_bytes = 0u64;
    while let Some((dir, depth)) = stack.pop_front() {
        dirs += 1;
        if dirs > MAX_DIRS || depth > MAX_DEPTH {
            snapshot.uncertain(String::new()); break;
        }
        let entries = match fs::read_dir(&dir) {
            Ok(entries) => entries,
            Err(_) => {
                let scope = dir.strip_prefix(root).unwrap_or(Path::new(""))
                    .to_string_lossy().replace('\\', "/");
                snapshot.uncertain(scope); continue;
            }
        };
        for result in entries {
            entries_seen += 1;
            if entries_seen > MAX_ENTRIES {
                snapshot.uncertain(String::new());
                break;
            }
            let entry = match result { Ok(value) => value, Err(_) => {
                let scope = dir.strip_prefix(root).unwrap_or(Path::new(""))
                    .to_string_lossy().replace('\\', "/");
                snapshot.uncertain(scope); continue;
            } };
            let path = entry.path();
            let relative = path.strip_prefix(root).map_err(|_| invalid("escaped root"))?;
            let mut parts = Vec::new();
            let mut excluded = false;
            for segment in relative.components() {
                let name = segment.as_os_str().to_string_lossy();
                if name.contains('\u{fffd}') || name.contains('\\') { excluded = true; break; }
                if is_excluded(&name) { excluded = true; break; }
                parts.push(name.into_owned());
            }
            if excluded {
                // Unrepresentable non-UTF8 source names cannot be classified
                // or safely mapped; do not infer deletions under this parent.
                if relative.to_string_lossy().contains('\u{fffd}') {
                    let scope = dir.strip_prefix(root).unwrap_or(Path::new(""))
                        .to_string_lossy().replace('\\', "/");
                    snapshot.uncertain(scope);
                }
                continue;
            }
            let name = parts.join("/");
            let metadata = match fs::symlink_metadata(&path) {
                Ok(m) => m,
                Err(_) => { snapshot.uncertain(name.clone()); continue; }
            };
            if is_link(&metadata) {
                // Symlinks never grant traversal/read authority; their
                // contents and any predecessor under this path remain unknown.
                snapshot.uncertain(name.clone()); continue;
            }
            if metadata.is_dir() { stack.push_back((path, depth + 1)); continue; }
            if !metadata.is_file() { continue; }
            if !is_source_input(&name) { continue; }
            if metadata.len() > MAX_FILE_BYTES || snapshot.files.len() >= MAX_FILES ||
               source_bytes.saturating_add(metadata.len()) > MAX_TOTAL_SOURCE_BYTES {
                snapshot.uncertain(name.clone());
                continue;
            }
            match stable_read(&path, &metadata) {
                Ok(Some(source)) => {
                    source_bytes = source_bytes.saturating_add(source.length);
                    snapshot.files.insert(name, source);
                }
                Ok(None) | Err(_) => { snapshot.uncertain(name); }
            }
        }
        if snapshot.uncertain_scopes.contains("") { break; }
    }
    Ok(snapshot)
}

fn is_excluded(name: &str) -> bool {
    [".git", ".slophawk", ".slophawk-native-coordinator", ".idea", ".vs",
        ".artifacts", "bin", "obj", "TestResults", "node_modules"]
        .iter().any(|v| name.eq_ignore_ascii_case(v))
}
/// Discovery is separate from authority. No syntax, import graph, or
/// cross-language edge is implied by a recognized extension.
pub fn language(name: &str) -> Option<&'static str> {
    if name.ends_with(".cs") { Some("csharp") }
    else if name.ends_with(".pyi") || name.ends_with(".py") { Some("python") }
    else if name.ends_with(".rs") { Some("rust") }
    else if name.ends_with(".fluxr") { Some("fluxr") }
    else { None }
}
fn is_source_input(name: &str) -> bool {
    language(name).is_some() ||
    [".csproj", ".sln", ".slnx", ".props", ".targets",
        "pyproject.toml", "pytest.ini", "setup.cfg", "tox.ini",
        "Cargo.toml", "Cargo.lock", "rust-toolchain", "rust-toolchain.toml", "fluxr.toml",
        "global.json", ".editorconfig"]
        .iter().any(|suffix| name.ends_with(suffix))
}
/// A provisional, explicit context. Evaluation adapters may replace this
/// with target-specific membership, but never infer cross-language impact.
pub fn context(path: &str, files: &BTreeMap<String, Source>) -> Option<String> {
    let language = language(path)?;
    let mut directory = path.rsplit_once('/').map_or("", |(dir, _)| dir);
    loop {
        let candidate = match language {
            "csharp" => files.keys().find(|key| {
                key.ends_with(".csproj") && key.rsplit_once('/').map_or("", |(d, _)| d) == directory
            }).cloned(),
            "python" => Some(if directory.is_empty() { "pyproject.toml".into() }
                            else { format!("{directory}/pyproject.toml") })
                .filter(|key| files.contains_key(key)),
            "rust" => Some(if directory.is_empty() { "Cargo.toml".into() }
                          else { format!("{directory}/Cargo.toml") })
                .filter(|key| files.contains_key(key)),
            "fluxr" => Some(if directory.is_empty() { "fluxr.toml".into() }
                           else { format!("{directory}/fluxr.toml") })
                .filter(|key| files.contains_key(key)),
            _ => None,
        };
        if let Some(project) = candidate { return Some(format!("{language}:manifest:{project}")); }
        if let Some((parent, _)) = directory.rsplit_once('/') { directory = parent; }
        else if directory.is_empty() { break; }
        else { directory = ""; }
    }
    let module = path.rsplit_once('/').map_or(".", |(dir, _)| dir);
    Some(format!("{language}:unresolved:{module}"))
}
pub fn is_test_trigger(name: &str) -> bool { name.to_ascii_lowercase().ends_with(".cs") }

fn stable_read(path: &Path, before: &fs::Metadata) -> io::Result<Option<Source>> {
    // Bounds apply before read and while streaming; a racing append cannot
    // force an unbounded allocation. A changing mtime/size is unknown evidence.
    #[cfg(windows)]
    let mut file = open_source(path)?;
    #[cfg(not(windows))]
    let mut file = File::open(path)?;
    let mut bytes = Vec::with_capacity(before.len().min(MAX_FILE_BYTES) as usize);
    let mut limited = file.by_ref().take(MAX_FILE_BYTES + 1);
    limited.read_to_end(&mut bytes)?;
    if bytes.len() as u64 > MAX_FILE_BYTES { return Ok(None); }
    let after = file.metadata()?;
    let current_entry = fs::symlink_metadata(path)?;
    if !current_entry.file_type().is_file() || is_link(&current_entry) ||
        current_entry.len() != after.len() || current_entry.modified()? != after.modified()? ||
        before.len() != after.len() || before.modified()? != after.modified()? ||
        after.len() != bytes.len() as u64 { return Ok(None); }
    #[cfg(unix)] {
        use std::os::unix::fs::MetadataExt;
        if (before.dev(), before.ino()) != (after.dev(), after.ino()) ||
           (after.dev(), after.ino()) != (current_entry.dev(), current_entry.ino()) {
            return Ok(None);
        }
    }
    #[cfg(windows)] {
        if windows_file_id(&file)? != windows_file_id(&open_source(path)?)? {
            return Ok(None);
        }
    }
    Ok(Some(Source { digest: digest(&bytes), length: bytes.len() as u64 }))
}

pub fn is_uncertain(scopes: &BTreeSet<String>, path: &str) -> bool {
    scopes.iter().any(|scope| scope.is_empty() || path == scope ||
        path.strip_prefix(scope).is_some_and(|rest| rest.starts_with('/')))
}

pub fn reconcile(old: &BTreeMap<String, Source>, observed: &Snapshot, hints: &[Event]) -> (BTreeMap<String, Source>, Vec<Delta>) {
    let mut new = observed.files.clone();
    if observed.partial {
        // Preserve only entries in incomplete subtrees. A failed read of
        // `broken/a.cs` must not hide a proven delete of `good/b.cs`.
        for (path, source) in old {
            if is_uncertain(&observed.uncertain_scopes, path) {
                new.entry(path.clone()).or_insert_with(|| source.clone());
            }
        }
    }
    let mut removed: BTreeSet<String> = old.keys().filter(|p| !new.contains_key(*p)).cloned().collect();
    let mut added: BTreeSet<String> = new.keys().filter(|p| !old.contains_key(*p)).cloned().collect();
    let mut changes = Vec::new();
    for (path, prior) in old {
        if let Some(later) = new.get(path) {
            if prior.digest != later.digest {
                changes.push(Delta {
                    kind: "change", old_path: path.clone(), new_path: path.clone(),
                    old_digest: prior.digest.clone(), new_digest: later.digest.clone(),
                });
            }
        }
    }
    // Explicit rename notifications pair changed-content moves too. Those
    // notifications remain hints: both endpoints must be proven by the scan.
    for event in hints {
        if let Event::Rename { from, to } = event {
            if removed.remove(from) && added.remove(to) {
                changes.push(Delta {
                    kind: "rename", old_path: from.clone(), new_path: to.clone(),
                    old_digest: old[from].digest.clone(), new_digest: new[to].digest.clone(),
                });
            }
        }
    }
    // Infer only unambiguous identical-content rename pairs; duplicate
    // content otherwise stays as separate add/delete causes.
    for from in removed.clone() {
        let candidates: Vec<_> = added.iter().filter(|to| new[*to].digest == old[&from].digest).cloned().collect();
        let same_old = removed.iter().filter(|p| old[*p].digest == old[&from].digest).count();
        if candidates.len() == 1 && same_old == 1 {
            let to = candidates[0].clone();
            removed.remove(&from); added.remove(&to);
            changes.push(Delta { kind: "rename", old_path: from.clone(), new_path: to,
                old_digest: old[&from].digest.clone(), new_digest: old[&from].digest.clone() });
        }
    }
    for path in removed {
        changes.push(Delta { kind: "delete", old_path: path.clone(), new_path: String::new(),
            old_digest: old[&path].digest.clone(), new_digest: String::new() });
    }
    for path in added {
        changes.push(Delta { kind: "add", old_path: String::new(), new_path: path.clone(),
            old_digest: String::new(), new_digest: new[&path].digest.clone() });
    }
    changes.sort_by(|a, b| (a.old_path.as_str(), a.new_path.as_str()).cmp(&(b.old_path.as_str(), b.new_path.as_str())));
    (new, changes)
}

pub fn inventory_digest(files: &BTreeMap<String, Source>) -> String {
    let mut body = Vec::new();
    for (path, source) in files {
        body.extend_from_slice(path.as_bytes()); body.push(0);
        body.extend_from_slice(source.digest.as_bytes()); body.push(0);
    }
    digest(&body)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn quiet_window_max_age_and_overflow() {
        let now = Instant::now();
        let mut queue = Coalescer::default();
        for i in 0..400 { queue.offer(Event::Change(format!("{i}.cs")), now + Duration::from_millis(i % 280)); }
        assert!(queue.len() <= 256);
        assert!(!queue.due(now + Duration::from_millis(400)));
        assert!(queue.due(now + Duration::from_secs(2)));
        assert!(queue.take().contains(&Event::Overflow));
        assert!(queue.is_empty());
    }
    #[test]
    fn digest_not_timestamp_and_unique_rename() {
        let source = Source { digest: digest(b"x"), length: 1 };
        let old = BTreeMap::from([("a.cs".into(), source.clone())]);
        let observed = Snapshot { files: BTreeMap::from([("b.cs".into(), source)]),
            uncertain_scopes: BTreeSet::new(), partial: false };
        let (_, changes) = reconcile(&old, &observed, &[]);
        assert_eq!(changes.len(), 1);
        assert_eq!(changes[0].kind, "rename");
    }
}
