//! Read-only, bounded Git HEAD observation. This is not Git execution: no
//! subprocess, filters, hooks, repository commands, or credentials are used.
//! Unknown/corrupt metadata fails closed rather than granting a fresh report.
use crate::sha256::digest;
use crate::store::{invalid, valid_relative};
use std::fs::{self, File};
use std::io::{self, ErrorKind, Read};
use std::path::{Path, PathBuf};

const MAX_HEAD: u64 = 8 * 1024;
const MAX_PACKED: u64 = 4 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitSnapshot {
    pub branch: String,
    pub commit: String,
    pub metadata_identity: String,
}
impl GitSnapshot {
    fn empty() -> Self {
        Self { branch: String::new(), commit: String::new(),
            metadata_identity: "unversioned".into() }
    }
}

fn bounded(path: &Path, limit: u64) -> io::Result<Vec<u8>> {
    let meta = fs::symlink_metadata(path)?;
    if !meta.file_type().is_file() || meta.len() > limit {
        return Err(invalid("Git metadata must be a bounded ordinary file"));
    }
    let mut bytes = Vec::new();
    File::open(path)?.take(limit + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > limit { return Err(invalid("Git metadata exceeds limit")); }
    Ok(bytes)
}
fn text(path: &Path, limit: u64) -> io::Result<String> {
    String::from_utf8(bounded(path, limit)?).map_err(|_| invalid("non-UTF8 Git metadata"))
}
fn hash_valid(oid: &str) -> bool {
    (oid.len() == 40 || oid.len() == 64) && oid.bytes().all(|b| b.is_ascii_hexdigit())
}
fn read_oid(path: &Path) -> io::Result<Option<String>> {
    match text(path, MAX_HEAD) {
        Ok(oid) => {
            let oid = oid.trim();
            if !hash_valid(oid) { return Err(invalid("invalid Git object ID")); }
            Ok(Some(oid.to_ascii_lowercase()))
        }
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}
fn subdirectory_of(root: &Path, path: PathBuf) -> io::Result<PathBuf> {
    let actual = fs::canonicalize(&path)?;
    if !actual.starts_with(root) { return Err(invalid("Git ref escaped metadata root")); }
    Ok(actual)
}
fn git_dir(root: &Path) -> io::Result<Option<PathBuf>> {
    let marker = root.join(".git");
    let meta = match fs::symlink_metadata(&marker) {
        Ok(meta) => meta,
        Err(e) if e.kind() == ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e),
    };
    if meta.file_type().is_symlink() { return Err(invalid(".git link is not authorized")); }
    if meta.is_dir() { return Ok(Some(marker)); }
    if !meta.is_file() { return Err(invalid("invalid .git entry type")); }
    let contents = text(&marker, MAX_HEAD)?;
    let raw = contents.trim().strip_prefix("gitdir: ")
        .ok_or_else(|| invalid("unsupported .git indirection"))?;
    let candidate = Path::new(raw);
    if raw.is_empty() || raw.contains('\0') { return Err(invalid("invalid .git path")); }
    let joined = if candidate.is_absolute() { candidate.to_owned() }
                 else { root.join(candidate) };
    let actual = fs::canonicalize(joined)?;
    let meta = fs::symlink_metadata(&actual)?;
    if !meta.file_type().is_dir() { return Err(invalid("Git directory is not physical")); }
    Ok(Some(actual))
}

#[cfg(target_os = "linux")]
pub fn read_head_for_poll(root: &Path) -> io::Result<Option<String>> {
    git_dir(root)?
        .map(|directory| text(&directory.join("HEAD"), MAX_HEAD))
        .transpose()
}
fn common_dir(git: &Path) -> io::Result<PathBuf> {
    let path = git.join("commondir");
    let raw = match text(&path, MAX_HEAD) {
        Ok(s) => s,
        Err(e) if e.kind() == ErrorKind::NotFound => return Ok(git.to_owned()),
        Err(e) => return Err(e),
    };
    let path = Path::new(raw.trim());
    if path.as_os_str().is_empty() { return Err(invalid("empty Git common directory")); }
    let candidate = if path.is_absolute() { path.to_owned() } else { git.join(path) };
    let actual = fs::canonicalize(candidate)?;
    if !fs::symlink_metadata(&actual)?.file_type().is_dir() {
        return Err(invalid("Git common directory must be physical"));
    }
    Ok(actual)
}
fn packed_oid(common: &Path, reference: &str) -> io::Result<Option<String>> {
    let packed = match text(&common.join("packed-refs"), MAX_PACKED) {
        Ok(value) => value,
        Err(e) if e.kind() == ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e),
    };
    let mut found = None;
    for line in packed.lines() {
        if line.starts_with('#') || line.starts_with('^') || line.is_empty() { continue; }
        let (oid, key) = line.split_once(' ')
            .ok_or_else(|| invalid("malformed packed Git refs"))?;
        if key == reference {
            if !hash_valid(oid) || found.is_some() { return Err(invalid("ambiguous packed Git ref")); }
            found = Some(oid.to_ascii_lowercase());
        }
    }
    Ok(found)
}
pub fn observe(root: &Path) -> io::Result<GitSnapshot> {
    let git = match git_dir(root)? { Some(path) => path, None => return Ok(GitSnapshot::empty()) };
    let common = common_dir(&git)?;
    let head = text(&git.join("HEAD"), MAX_HEAD)?;
    let head = head.trim();
    let (branch, commit) = if let Some(reference) = head.strip_prefix("ref: ") {
        if !reference.starts_with("refs/") || !valid_relative(reference) ||
           reference.contains('\0') || reference.contains('\n') {
            return Err(invalid("invalid symbolic Git HEAD"));
        }
        let oid_path = common.join(reference);
        let commit = match fs::symlink_metadata(&oid_path) {
            Ok(meta) if meta.file_type().is_file() => {
                let verified = subdirectory_of(&common, oid_path)?;
                read_oid(&verified)?
            }
            Ok(_) => return Err(invalid("symbolic Git ref is not an ordinary file")),
            Err(e) if e.kind() == ErrorKind::NotFound => packed_oid(&common, reference)?,
            Err(e) => return Err(e),
        };
        (reference.to_owned(), commit.unwrap_or_default())
    } else if hash_valid(head) {
        (String::new(), head.to_ascii_lowercase())
    } else {
        return Err(invalid("unknown Git HEAD format"));
    };
    // Identity includes Git storage binding, not only the display commit:
    // rebinding worktree metadata invalidates even identical ref values.
    let metadata_identity = digest(format!("{}\0{}\0{}\0{}",
        git.display(), common.display(), branch, commit).as_bytes());
    Ok(GitSnapshot { branch, commit, metadata_identity })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn missing_git_is_an_explicit_unversioned_context() {
        let root = std::env::temp_dir().join(format!("sg-mig-03-git-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir(&root).unwrap();
        assert_eq!(observe(&root).unwrap(), GitSnapshot::empty());
        fs::remove_dir(&root).unwrap();
    }
    #[test]
    fn checked_ref_names() {
        assert!(!valid_relative("refs/heads/../victim"));
        assert!(hash_valid("a367ef58076f7e29d2ae475a3b593daad04c4998"));
        assert!(!hash_valid("not-an-oid"));
    }
}
