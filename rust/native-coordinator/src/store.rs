//! Versioned, checksummed *inventory + causes* store. This is intentionally not the C#
//! daemon's SQLite schema; publication and migration into it are forbidden.
use crate::sha256::{digest, to_hex};
use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};

const HEADER: &str = "SLOPHAWK_NATIVE_COORDINATOR_V2";
const POLICY: &str = "source-inventory/v2";
const MAX_STATE: usize = 8 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Source {
    pub digest: String,
    pub length: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Intent {
    pub id: u64,
    pub revision: u64,
    pub kind: String,
    pub old_path: String,
    pub new_path: String,
    pub old_digest: String,
    pub new_digest: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Attempt {
    pub id: u64,
    pub generation: u64,
    pub epoch: u64,
    pub revision: u64,
    pub pid: u32,
    pub start_ticks: u64,
    pub status: String,
    pub reason: String,
    pub causes: Vec<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Receipt {
    pub attempt: u64,
    pub revision: u64,
    pub outcome: String,
    pub coverage: String,
    pub reason: String,
    pub input_digest: String,
}

#[derive(Debug, Clone)]
pub struct State {
    pub root_id: String,
    pub generation: u64,
    pub epoch: u64,
    pub sequence: u64,
    pub revision: u64,
    pub projection: u64,
    pub next_id: u64,
    pub branch: String,
    pub commit: String,
    pub git_identity: String,
    pub retired_causes: usize,
    pub partial: bool,
    pub uncertain_scopes: BTreeSet<String>,
    pub inventory: BTreeMap<String, Source>,
    pub pending: Vec<Intent>,
    pub fallback: Option<(u64, u64)>,
    pub attempts: Vec<Attempt>,
    pub receipt: Option<Receipt>,
}

impl State {
    pub fn empty(root_id: String) -> Self {
        Self {
            root_id, generation: 0, epoch: 0, sequence: 0, revision: 0,
            projection: 0, next_id: 1, branch: String::new(), commit: String::new(),
            git_identity: String::new(), retired_causes: 0, partial: false, uncertain_scopes: BTreeSet::new(),
            inventory: BTreeMap::new(), pending: Vec::new(),
            fallback: None, attempts: Vec::new(), receipt: None,
        }
    }
    pub fn id(&mut self) -> io::Result<u64> {
        let value = self.next_id;
        self.next_id = self.next_id.checked_add(1).ok_or_else(|| invalid("ID exhausted"))?;
        Ok(value)
    }
    pub fn save(&self, dir: &Path) -> io::Result<()> {
        let mut body = String::new();
        body.push_str(HEADER);
        body.push('\n');
        for (key, val) in [
            ("root", hexstr(&self.root_id)),
            ("generation", self.generation.to_string()),
            ("epoch", self.epoch.to_string()),
            ("sequence", self.sequence.to_string()),
            ("revision", self.revision.to_string()),
            ("projection", self.projection.to_string()),
            ("next", self.next_id.to_string()),
            ("branch", hexstr(&self.branch)),
            ("commit", hexstr(&self.commit)),
            ("git_id", hexstr(&self.git_identity)),
            ("retired", self.retired_causes.to_string()),
            ("partial", u8::from(self.partial).to_string()),
            ("policy", POLICY.to_string()),
        ] {
            body.push_str(&format!("{key}\t{val}\n"));
        }
        for scope in &self.uncertain_scopes {
            body.push_str(&format!("scope\t{}\n", hexstr(scope)));
        }
        for (path, entry) in &self.inventory {
            body.push_str(&format!("source\t{}\t{}\t{}\n", hexstr(path), entry.digest, entry.length));
        }
        for item in &self.pending {
            body.push_str(&format!("intent\t{}\t{}\t{}\t{}\t{}\t{}\t{}\n",
                item.id, item.revision, item.kind, hexstr(&item.old_path),
                hexstr(&item.new_path), item.old_digest, item.new_digest));
        }
        if let Some((first, last)) = self.fallback {
            body.push_str(&format!("fallback\t{first}\t{last}\n"));
        }
        for item in &self.attempts {
            let causes = item.causes.iter().map(u64::to_string).collect::<Vec<_>>().join(",");
            body.push_str(&format!("attempt\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\n",
                item.id, item.generation, item.epoch, item.revision, item.pid,
                item.start_ticks, item.status, hexstr(&item.reason), causes));
        }
        if let Some(item) = &self.receipt {
            body.push_str(&format!("receipt\t{}\t{}\t{}\t{}\t{}\t{}\n",
                item.attempt, item.revision, item.outcome, item.coverage,
                hexstr(&item.reason), item.input_digest));
        }
        if body.len() > MAX_STATE - 100 {
            return Err(invalid("evidence exceeds the state budget"));
        }
        let result = format!("{body}checksum\t{}\n", digest(body.as_bytes()));
        atomic_replace(dir, "state.v2", result.as_bytes(), HEADER.as_bytes())
    }
    pub fn load(dir: &Path) -> io::Result<Option<Self>> {
        let path = dir.join("state.v2");
        let attr = match fs::symlink_metadata(&path) {
            Ok(meta) => meta,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(e),
        };
        if !attr.file_type().is_file() || attr.len() > MAX_STATE as u64 {
            return Err(invalid("unexpected evidence file type or length"));
        }
        let mut raw = Vec::new();
        use std::io::Read;
        File::open(&path)?.take(MAX_STATE as u64 + 1).read_to_end(&mut raw)?;
        if raw.len() > MAX_STATE { return Err(invalid("evidence grew beyond state budget")); }
        let text = std::str::from_utf8(&raw).map_err(|_| invalid("evidence UTF-8"))?;
        let (body, footer) = text.rsplit_once("checksum\t").ok_or_else(|| invalid("missing checksum"))?;
        if !footer.ends_with('\n') || footer.trim_end() != digest(body.as_bytes()) {
            return Err(invalid("evidence checksum mismatch"));
        }
        let mut lines = body.lines();
        if lines.next() != Some(HEADER) { return Err(invalid("unsupported state schema")); }
        let mut state = State::empty(String::new());
        let mut seen = std::collections::BTreeSet::new();
        for line in lines {
            let parts: Vec<&str> = line.split('\t').collect();
            let key = *parts.first().unwrap_or(&"");
            match (key, parts.as_slice()) {
                ("root", [_, v]) => state.root_id = unhex(v)?,
                ("generation", [_, v]) => state.generation = number(v)?,
                ("epoch", [_, v]) => state.epoch = number(v)?,
                ("sequence", [_, v]) => state.sequence = number(v)?,
                ("revision", [_, v]) => state.revision = number(v)?,
                ("projection", [_, v]) => state.projection = number(v)?,
                ("next", [_, v]) => state.next_id = number(v)?,
                ("branch", [_, v]) => state.branch = unhex(v)?,
                ("commit", [_, v]) => state.commit = unhex(v)?,
                ("git_id", [_, v]) => state.git_identity = unhex(v)?,
                ("retired", [_, v]) => state.retired_causes = number(v)?,
                ("partial", [_, "0"]) => state.partial = false,
                ("partial", [_, "1"]) => state.partial = true,
                ("policy", [_, POLICY]) => {},
                ("scope", [_, path]) => {
                    let scope = unhex(path)?;
                    if (!scope.is_empty() && !valid_relative(&scope)) ||
                       !state.uncertain_scopes.insert(scope) {
                        return Err(invalid("invalid or duplicate uncertain scope"));
                    }
                }
                ("source", [_, path, hash, size]) => {
                    let name = unhex(path)?;
                    if !valid_relative(&name) || hash.len() != 64 ||
                        !hash.bytes().all(|b| b.is_ascii_hexdigit()) ||
                        state.inventory.insert(name, Source { digest: (*hash).to_owned(), length: number(size)? }).is_some() {
                        return Err(invalid("duplicate or malformed source"));
                    }
                }
                ("intent", [_, id, rev, kind, old, new, old_hash, new_hash]) => {
                    let old_path = unhex(old)?;
                    let new_path = unhex(new)?;
                    if !old_path.is_empty() && !valid_relative(&old_path) ||
                        !new_path.is_empty() && !valid_relative(&new_path) {
                        return Err(invalid("unsafe intent path"));
                    }
                    state.pending.push(Intent {
                        id: number(id)?, revision: number(rev)?, kind: (*kind).to_string(),
                        old_path, new_path, old_digest: (*old_hash).to_string(),
                        new_digest: (*new_hash).to_string(),
                    });
                }
                ("fallback", [_, a, b]) => state.fallback = Some((number(a)?, number(b)?)),
                ("attempt", [_, id, generation, epoch, revision, pid, started, status, reason, causes]) => {
                    let cause_ids = if causes.is_empty() { Vec::new() } else {
                        causes.split(',').map(number).collect::<io::Result<Vec<_>>>()?
                    };
                    state.attempts.push(Attempt {
                        id: number(id)?, generation: number(generation)?, epoch: number(epoch)?,
                        revision: number(revision)?, pid: number(pid)?, start_ticks: number(started)?,
                        status: (*status).to_string(), reason: unhex(reason)?, causes: cause_ids,
                    });
                }
                ("receipt", [_, id, rev, outcome, coverage, reason, input_digest]) => {
                    state.receipt = Some(Receipt {
                        attempt: number(id)?, revision: number(rev)?, outcome: (*outcome).to_string(),
                        coverage: (*coverage).to_string(), reason: unhex(reason)?,
                        input_digest: (*input_digest).to_string(),
                    });
                }
                _ => return Err(invalid("unsupported or malformed state field")),
            }
            if !matches!(key, "source" | "scope" | "intent" | "attempt") && !seen.insert(key) {
                return Err(invalid("duplicate singleton state field"));
            }
        }
        for key in ["root", "generation", "epoch", "sequence", "revision", "projection", "next", "branch", "commit", "git_id", "retired", "partial", "policy"] {
            if !seen.contains(key) { return Err(invalid("missing mandatory state field")); }
        }
        if state.partial != !state.uncertain_scopes.is_empty() {
            return Err(invalid("inconsistent uncertainty frontier"));
        }
        if state.pending.len() > 2_048 || state.attempts.len() > 64 ||
           state.inventory.len() > 8_192 || state.uncertain_scopes.len() > 8_192 {
            return Err(invalid("state collection limit exceeded"));
        }
        Ok(Some(state))
    }
}

pub fn invalid(reason: &str) -> io::Error { io::Error::new(io::ErrorKind::InvalidData, reason) }
fn number<T: std::str::FromStr>(text: &str) -> io::Result<T> {
    text.parse().map_err(|_| invalid("invalid state integer"))
}
pub fn valid_relative(path: &str) -> bool {
    !path.is_empty() && !path.starts_with('/') && !path.contains('\\') &&
        path.split('/').all(|part| !part.is_empty() && part != "." && part != "..")
}
fn hexstr(value: &str) -> String { to_hex(value.as_bytes()) }
fn unhex(hex: &str) -> io::Result<String> {
    if hex.len() % 2 != 0 { return Err(invalid("odd hex field")); }
    let mut output = Vec::with_capacity(hex.len() / 2);
    for pair in hex.as_bytes().chunks_exact(2) {
        let s = std::str::from_utf8(pair).map_err(|_| invalid("non-hex field"))?;
        output.push(u8::from_str_radix(s, 16).map_err(|_| invalid("non-hex field"))?);
    }
    String::from_utf8(output).map_err(|_| invalid("non-UTF-8 field"))
}

/// Crash-safe single-file replacement: same-directory staging, sync, rename,
/// then directory sync on Unix. Readers must ALSO validate the generation and
/// committed evidence: state and report are not a two-file atomic transaction.
pub fn atomic_replace(dir: &Path, name: &str, data: &[u8], identity: &[u8]) -> io::Result<()> {
    atomic_replace_checked(dir, name, data, identity, || Ok(true))
}

/// Check the live authority AFTER staging/sync and directly before replacement.
pub fn atomic_replace_checked<F>(dir: &Path, name: &str, data: &[u8],
                                 identity: &[u8], mut live: F) -> io::Result<()>
where F: FnMut() -> io::Result<bool> {
    if data.len() > MAX_STATE { return Err(invalid("publication length exceeded")); }
    require_plain_dir(dir)?;
    let target = dir.join(name);
    let staging = dir.join(format!("{name}.pending"));
    if let Ok(metadata) = fs::symlink_metadata(&target) {
        if !metadata.is_file() || metadata.len() > MAX_STATE as u64 {
            return Err(invalid("unexpected target link, directory or size"));
        }
        let old = fs::read(&target)?;
        if old.len() < identity.len() || !old.starts_with(identity) {
            return Err(invalid("refusing to overwrite foreign publication"));
        }
    }
    if let Ok(metadata) = fs::symlink_metadata(&staging) {
        if !metadata.is_file() || metadata.len() > MAX_STATE as u64 {
            return Err(invalid("unexpected staging type or size"));
        }
        let old = fs::read(&staging)?;
        if !old.starts_with(identity) { return Err(invalid("foreign staging file")); }
        fs::remove_file(&staging)?; // abandoned, positively identified staging only
    }
    let mut writer = OpenOptions::new().write(true).create_new(true).open(&staging)?;
    writer.write_all(data)?;
    writer.sync_all()?;
    drop(writer);
    if !live()? {
        // Do not remove a replaced staging entry or an unexpected link.
        if staging_matches(&staging, data)? { fs::remove_file(&staging)?; }
        return Err(io::Error::new(io::ErrorKind::WouldBlock, "publication became stale after staging"));
    }
    if !staging_matches(&staging, data)? {
        return Err(invalid("staging contents or entry replaced before publication"));
    }
    replace_file(&staging, &target)?;
    sync_dir(dir)?;
    Ok(())
}

fn staging_matches(staging: &Path, expected: &[u8]) -> io::Result<bool> {
    let meta = fs::symlink_metadata(staging)?;
    if !meta.file_type().is_file() || meta.len() != expected.len() as u64 { return Ok(false); }
    Ok(fs::read(staging)? == expected)
}

pub fn withdraw(dir: &Path, name: &str, identity: &[u8]) -> io::Result<()> {
    require_plain_dir(dir)?;
    let path = dir.join(name);
    match fs::symlink_metadata(&path) {
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e),
        Ok(metadata) => {
            if !metadata.is_file() || metadata.len() > MAX_STATE as u64 ||
                !fs::read(&path)?.starts_with(identity) {
                return Err(invalid("refusing to remove foreign publication"));
            }
            fs::remove_file(&path)?;
            sync_dir(dir)
        }
    }
}

pub fn require_plain_dir(path: &Path) -> io::Result<()> {
    let metadata = fs::symlink_metadata(path)?;
    if !metadata.file_type().is_dir() { return Err(invalid("state directory is a symlink or not a directory")); }
    Ok(())
}

#[cfg(unix)]
fn sync_dir(path: &Path) -> io::Result<()> { File::open(path)?.sync_all() }
#[cfg(not(unix))]
fn sync_dir(_path: &Path) -> io::Result<()> { Ok(()) }

#[cfg(not(windows))]
fn replace_file(staging: &Path, target: &Path) -> io::Result<()> { fs::rename(staging, target) }

#[cfg(windows)]
fn replace_file(staging: &Path, target: &Path) -> io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    #[link(name = "kernel32")]
    extern "system" { fn MoveFileExW(from: *const u16, to: *const u16, flags: u32) -> i32; }
    let from: Vec<u16> = staging.as_os_str().encode_wide().chain(Some(0)).collect();
    let to: Vec<u16> = target.as_os_str().encode_wide().chain(Some(0)).collect();
    // Same-volume MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH.
    if unsafe { MoveFileExW(from.as_ptr(), to.as_ptr(), 0x1 | 0x8) } == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

pub fn state_directory(root: &Path) -> PathBuf { root.join(".slophawk-native-coordinator") }

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn state_codec_rejects_corruption() {
        let dir = std::env::temp_dir().join(format!("sh-native-store-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir(&dir).unwrap();
        let mut state = State::empty("root".to_owned());
        state.generation = 1;
        state.inventory.insert("p.cs".into(), Source { digest: digest(b"p"), length: 1 });
        state.uncertain_scopes.insert("p.cs".into());
        state.partial = true;
        state.save(&dir).unwrap();
        let got = State::load(&dir).unwrap().unwrap();
        assert_eq!(got.inventory, state.inventory);
        assert_eq!(got.uncertain_scopes, state.uncertain_scopes);
        fs::write(dir.join("state.v2"), "SLOPHAWK_NATIVE_FIXTURE_V1\nBROKEN\n").unwrap();
        assert!(State::load(&dir).is_err());
        fs::remove_dir_all(&dir).unwrap();
    }
}
