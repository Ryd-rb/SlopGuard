//! Source-only artifact reader. A lease/read-scope attestation originates from
//! an execution supervisor, never from repository content or Cobertura XML.
//! Linux uses descriptor-relative O_NOFOLLOW walks; other OSes fail closed.
use crate::{Check, Problem, Status, MAX_COVERAGE_BYTES, sha256_hex, logical_path, field};
use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ReadScopeFacts {
    pub root_identity: String,
    pub epoch: String,
    pub attempt_id: i64,
    pub target: String,
    pub build_identity: String,
    pub owned_and_drained: bool,
    pub mapping_trusted: bool,
    pub runtime_closure_complete: bool,
    /// Attested attempt/lease times, Unix epoch milliseconds, not file-provided clocks.
    pub started_ms: i64,
    pub finished_ms: i64,
    pub lease_created_ms: i64,
    pub now_ms: i64,
    /// The supervisor already resolved authorization outside this untrusted tree.
    pub authorization_reference: String,
}
impl ReadScopeFacts {
    pub fn complete(&self) -> bool {
        self.owned_and_drained && self.mapping_trusted && self.runtime_closure_complete &&
        self.attempt_id>0 && self.started_ms>0 && self.started_ms<=self.finished_ms &&
        self.lease_created_ms >= self.started_ms.saturating_sub(120_000) &&
        self.lease_created_ms <= self.finished_ms &&
        self.finished_ms<=self.now_ms.saturating_add(120_000) &&
        self.now_ms>=self.started_ms && self.now_ms-self.started_ms <= 86_400_000 &&
        [self.root_identity.as_str(), &self.epoch, &self.target,
            &self.build_identity, &self.authorization_reference]
            .iter().all(|s|field(s,256))
    }
}

/// Immutable owned bytes from the exact verified file descriptor. Construction is
/// deliberately private: a caller cannot turn arbitrary XML into an attested read.
#[derive(Clone, Debug)]
pub struct PhysicalSnapshot {
    bytes: Box<[u8]>, sha256: String, pub facts: ReadScopeFacts,
}
impl PhysicalSnapshot {
    pub fn bytes(&self) -> &[u8] { &self.bytes }
    pub fn digest(&self) -> &str { &self.sha256 }
}

pub fn capture_coverage(root: &Path, relative_path: &str, facts: &ReadScopeFacts) -> Check<PhysicalSnapshot> {
    if !facts.complete() { return Err(Problem::new(Status::Unknown,"read_scope_not_attested")); }
    if logical_path(relative_path,1024).as_deref()!=Some(relative_path) || !root.is_absolute() {
        return Err(Problem::new(Status::Malformed,"artifact_path_not_canonical"));
    }
    let bytes = capture_no_follow(root, relative_path, MAX_COVERAGE_BYTES,
        facts.started_ms, facts.finished_ms)?;
    let digest = sha256_hex(&bytes);
    Ok(PhysicalSnapshot { bytes:bytes.into_boxed_slice(), sha256:digest, facts:facts.clone() })
}

#[cfg(not(target_os="linux"))]
fn capture_no_follow(_root:&Path, _relative:&str, _limit:usize,
    _started_ms:i64, _finished_ms:i64) -> Check<Vec<u8>> {
    Err(Problem::new(Status::Unsupported,"physical_identity_not_supported_on_platform"))
}

#[cfg(target_os="linux")]
fn capture_no_follow(root: &Path, relative: &str, limit: usize,
    started_ms:i64, finished_ms:i64) -> Check<Vec<u8>> {
    capture_no_follow_with_hook(root,relative,limit,started_ms,finished_ms,&||{})
}

/// Deterministic source-only race-test seam; trusted callers never supply a hook.
#[cfg(target_os="linux")]
fn capture_no_follow_with_hook(root: &Path, relative: &str, limit: usize,
    started_ms:i64, finished_ms:i64, after_first_read: &dyn Fn()) -> Check<Vec<u8>> {
    use std::ffi::{CString, OsStr};
    use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
    use std::os::unix::ffi::OsStrExt;
    use std::os::unix::fs::{FileExt, MetadataExt};
    use std::{fs::File, mem};

    fn failure() -> Problem { Problem::new(Status::Mismatch,"artifact_link_or_race") }
    fn open_child(parent:i32, name:&OsStr, dir:bool) -> Check<OwnedFd> {
        let name=CString::new(name.as_bytes()).map_err(|_|failure())?;
        let flags=libc::O_RDONLY|libc::O_CLOEXEC|libc::O_NOFOLLOW|libc::O_NONBLOCK|
            (if dir { libc::O_DIRECTORY } else { 0 });
        // SAFETY: name is nul-terminated, parent is a live OwnedFd for a directory.
        let raw=unsafe { libc::openat(parent,name.as_ptr(),flags) };
        if raw<0 { return Err(failure()); }
        // SAFETY: successful openat transfers ownership of a fresh descriptor.
        Ok(unsafe { OwnedFd::from_raw_fd(raw) })
    }
    fn identity(fd:&OwnedFd, directory:bool) -> Check<(u64,u64,u64)> {
        // SAFETY: zeroed C stat is initialized by fstat on success.
        let mut st:libc::stat=unsafe { mem::zeroed() };
        if unsafe { libc::fstat(fd.as_raw_fd(),&mut st) }!=0 ||
            (st.st_mode & libc::S_IFMT)!=if directory { libc::S_IFDIR } else { libc::S_IFREG } ||
            st.st_ino==0 || st.st_nlink==0 || (!directory && st.st_nlink!=1) { return Err(failure()); }
        Ok((st.st_dev as u64,st.st_ino as u64,st.st_nlink as u64))
    }
    // Walk from the filesystem root. Reject parent traversal, dot components and links.
    // These descriptors pin every directory while the leaf is captured.
    let mut path_parts=Vec::new();
    for component in root.components() {
        match component {
            std::path::Component::RootDir => {},
            std::path::Component::Normal(v) => path_parts.push(v.to_os_string()),
            _ => return Err(Problem::new(Status::Malformed,"root_not_canonical")),
        }
    }
    if path_parts.is_empty() { return Err(Problem::new(Status::Malformed,"root_is_filesystem_root")); }
    // SAFETY: CString literal is valid, / exists on Linux.
    let raw=unsafe { libc::open(c"/".as_ptr(),libc::O_RDONLY|libc::O_DIRECTORY|libc::O_CLOEXEC|libc::O_NOFOLLOW) };
    if raw<0 { return Err(failure()); }
    let mut directories=vec![unsafe { OwnedFd::from_raw_fd(raw) }];
    let mut ids=vec![identity(&directories[0],true)?];
    for part in &path_parts {
        let fd=open_child(directories.last().unwrap().as_raw_fd(),part,true)?;
        ids.push(identity(&fd,true)?); directories.push(fd);
    }
    let rel_segments:Vec<_>=relative.split('/').collect();
    for part in &rel_segments[..rel_segments.len()-1] {
        let fd=open_child(directories.last().unwrap().as_raw_fd(),OsStr::new(part),true)?;
        ids.push(identity(&fd,true)?); directories.push(fd);
    }
    let leaf=rel_segments[rel_segments.len()-1];
    let parent=directories.last().unwrap().as_raw_fd();
    let fd=open_child(parent,OsStr::new(leaf),false)?;
    let file_id=identity(&fd,false)?;
    let file=File::from(fd);
    let before=file.metadata().map_err(|_|failure())?;
    let modified_ms=before.mtime().checked_mul(1000)
        .and_then(|ms|ms.checked_add(before.mtime_nsec()/1_000_000)).ok_or_else(failure)?;
    if modified_ms<started_ms || modified_ms>finished_ms.saturating_add(120_000) {
        return Err(Problem::new(Status::Expired,"coverage_artifact_not_fresh"));
    }
    let len=before.len();
    if len==0 || len>limit as u64 { return Err(Problem::new(Status::Malformed,"artifact_size_invalid")); }
    let mut first=vec![0u8;len as usize]; let mut second=vec![0u8;len as usize];
    for (index,buf) in [&mut first,&mut second].into_iter().enumerate() {
        if index==1 { after_first_read(); }
        let mut cursor=0;
        while cursor<buf.len() {
            let amount=file.read_at(&mut buf[cursor..],cursor as u64).map_err(|_|failure())?;
            if amount==0 { return Err(failure()); }
            cursor+=amount;
        }
    }
    let after=file.metadata().map_err(|_|failure())?;
    if first!=second || file_id!=(after.dev(),after.ino(),after.nlink()) ||
        before.len()!=after.len() || before.mtime()!=after.mtime() ||
        before.mtime_nsec()!=after.mtime_nsec() || before.ctime()!=after.ctime() ||
        before.ctime_nsec()!=after.ctime_nsec() { return Err(failure()); }
    let again=open_child(parent,OsStr::new(leaf),false)?;
    if identity(&again,false)?!=file_id { return Err(failure()); }
    for (fd,id) in directories.iter().zip(&ids) {
        if identity(fd,true)? != *id { return Err(failure()); }
    }
    // Verify the *names* still resolve to the same pinned directory identities;
    // fstat on held descriptors alone would miss a parent renamed/symlink-swapped.
    let fs_root=&directories[0];
    let mut replayed=Vec::new();
    for part in path_parts.iter().map(|p|p.as_os_str()).chain(
        rel_segments[..rel_segments.len()-1].iter().map(|p|OsStr::new(p))) {
        let current=replayed.last().unwrap_or(fs_root);
        let next=open_child(current.as_raw_fd(),part,true)?;
        let depth=replayed.len()+1;
        if identity(&next,true)?!=ids[depth] {return Err(failure());}
        replayed.push(next);
    }
    let replay_parent=replayed.last().unwrap_or(fs_root);
    let replay_leaf=open_child(replay_parent.as_raw_fd(),OsStr::new(leaf),false)?;
    if identity(&replay_leaf,false)?!=file_id {return Err(failure());}
    Ok(first)
}

#[cfg(all(test,target_os="linux"))]
mod race_tests {
    use super::*;
    use std::fs;
    use std::time::{SystemTime,UNIX_EPOCH};
    fn directory() -> std::path::PathBuf {
        let nonce=SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();
        let root=std::env::temp_dir().join(format!("sg-mig-13-races-{}-{nonce}",std::process::id()));
        fs::create_dir_all(&root).unwrap();root
    }
    #[test]
    fn in_place_change_between_reads_cannot_keep_first_digest() {
        let root=directory();
        fs::write(root.join("artifact.xml"),b"snapshot-a").unwrap();
        let now=SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_millis() as i64;
        let outcome=capture_no_follow_with_hook(&root,"artifact.xml",1024,now-60_000,now+2_000,&|| {
            fs::write(root.join("artifact.xml"),b"snapshot-b").unwrap();
        });
        assert!(outcome.is_err());
        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn stale_mtime_cannot_be_rebranded_as_current_lease_artifact() {
        let root=directory();
        fs::write(root.join("artifact.xml"),b"previous-attempt-bytes").unwrap();
        let now=SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_millis() as i64;
        let outcome=capture_no_follow_with_hook(&root,"artifact.xml",1024,
            now+300_000,now+301_000,&||{});
        assert_eq!(Status::Expired,outcome.unwrap_err().status);
        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn renamed_root_cannot_keep_read_authority() {
        let root=directory();
        fs::write(root.join("artifact.xml"),b"snapshot-a").unwrap();
        let moved=root.with_extension("moved");
        let now=SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_millis() as i64;
        let outcome=capture_no_follow_with_hook(&root,"artifact.xml",1024,now-60_000,now+2_000,&|| {
            fs::rename(&root,&moved).unwrap();
            fs::create_dir(&root).unwrap();
            fs::write(root.join("artifact.xml"),b"snapshot-a").unwrap();
        });
        assert!(outcome.is_err());
        fs::remove_dir_all(root).unwrap();
        fs::remove_dir_all(moved).unwrap();
    }
}
