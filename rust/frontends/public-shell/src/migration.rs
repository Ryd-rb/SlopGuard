use base64::{Engine, engine::general_purpose::STANDARD};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use slophawk_native_coordinator::observe_workspace;
use std::collections::BTreeMap;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, atomic::{AtomicBool, Ordering}};
use std::time::Duration;

const PROTOCOL: &str = "slophawk.migration-job/1";
const FRAME: usize = 128 * 1024;
const CHUNK: usize = 72 * 1024;
const CAP: usize = 16 * 1024 * 1024;

#[path = "resident_store.rs"]
pub mod resident_store;
#[path = "scheduled_checks.rs"]
mod scheduled_checks;
#[path = "scheduled_tests.rs"]
mod scheduled_tests;
#[path = "scheduled_csharp.rs"]
mod scheduled_csharp;
#[path = "test_handoff.rs"]
mod test_handoff;

#[derive(Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Identity {
    protocol: String,
    operation: String,
    job: String,
    root: PathBuf,
    root_identity: String,
    epoch: String,
    committed_revision: i64,
    input_sha256: String,
    tool_sha256: String,
    policy_sha256: String,
    budget_ref: String,
    grant_ref: String,
    state_directory: PathBuf,
}

#[derive(Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum Packet {
    Request {
        identity: Identity,
        bytes: usize,
        sha256: String,
    },
    Chunk {
        index: usize,
        data: String,
    },
    End {
        identity: Identity,
    },
}

#[derive(Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Marker {
    version: u32,
    root: PathBuf,
    root_identity: String,
    revision: i64,
    epoch: String,
    native_sha256: String,
    profile_sha256: String,
    state: String,
    backup_sha256: String,
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Snapshot {
    format: u32,
    inventory_schema: u32,
    root_identity: String,
    revision: i64,
    policy: Option<String>,
    tables: Vec<Table>,
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Table {
    name: String,
    columns: Vec<String>,
    rows: Vec<Vec<Cell>>,
}

#[derive(Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct Cell {
    #[serde(rename = "type")]
    kind: String,
    value: Option<String>,
}

impl Cell {
    fn text(value: String) -> Self {
        Self {
            kind: "text".into(),
            value: Some(value),
        }
    }
    fn integer(value: i64) -> Self {
        Self {
            kind: "integer".into(),
            value: Some(value.to_string()),
        }
    }
    fn nullable(value: String) -> Self {
        if value.is_empty() {
            Self {
                kind: "null".into(),
                value: None,
            }
        } else {
            Self::text(value)
        }
    }
}

const TABLES: [(&str, &str, usize); 9] = [
    (
        "workspace",
        "id,revision,root_identity,policy,branch,commit_id,retired_revision,retired_intents",
        1,
    ),
    ("source", "path_key,path,digest,verified", 100_000),
    ("scope", "kind,path", 100_000),
    (
        "test_intent",
        "revision,path_key,old_path,old_digest,new_path,new_digest,predecessor",
        2048,
    ),
    ("intent_fallback", "id,first_revision,last_revision", 1),
    (
        "test_attempt",
        "id,number,revision,epoch,target,fingerprint,causes,plan_key,state,outcome,retry_at,lease_id,profile_revision",
        64,
    ),
    ("test_evidence", "cause,target,fingerprint", 32_784),
    (
        "project_graph",
        "revision,root_identity,complete,payload",
        128,
    ),
    (
        "test_run_receipt",
        "attempt_id,format,revision,epoch,fence_sequence,git_fingerprint,root_identity,target,fingerprint,profile_revision,lease_id,build_domain,build_sha256,runner_sha256,artifact_sha256,artifact_bytes,run_id,platform,runner,artifact,execution,total,executed,passed,failed,skipped,other,reason_code",
        64,
    ),
];

fn invalid(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}

fn hash(bytes: &[u8]) -> String {
    format!("{:X}", Sha256::digest(bytes))
}

fn hex(value: &str, len: usize) -> bool {
    value.len() == len && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn frame(input: &mut impl Read) -> io::Result<Packet> {
    let mut prefix = [0; 4];
    input.read_exact(&mut prefix)?;
    let len = u32::from_le_bytes(prefix) as usize;
    if len == 0 || len > FRAME {
        return Err(invalid("migration frame exceeds its limit"));
    }
    let mut bytes = vec![0; len];
    input.read_exact(&mut bytes)?;
    serde_json::from_slice(&bytes).map_err(io::Error::other)
}

fn emit(output: &mut impl Write, value: &serde_json::Value) -> io::Result<()> {
    let bytes = serde_json::to_vec(value).map_err(io::Error::other)?;
    if bytes.len() > FRAME {
        return Err(invalid("migration output exceeds its frame limit"));
    }
    output.write_all(&(bytes.len() as u32).to_le_bytes())?;
    output.write_all(&bytes)
}

fn receive(input: &mut impl Read) -> io::Result<(Identity, Vec<u8>)> {
    let Packet::Request {
        identity,
        bytes,
        sha256,
    } = frame(input)?
    else {
        return Err(invalid("migration request header missing"));
    };
    if identity.protocol != PROTOCOL
        || identity.committed_revision < 0
        || !hex(&identity.epoch, 32)
        || !hex(&identity.job, 32)
        || !hex(&identity.tool_sha256, 64)
        || !hex(&identity.policy_sha256, 64)
        || identity.budget_ref != "migration-state-16m-frame-128k"
        || identity.grant_ref.is_empty()
        || identity.grant_ref.len() > 128
        || identity.grant_ref.chars().any(char::is_control)
        || bytes > CAP
        || sha256 != identity.input_sha256
    {
        return Err(invalid("migration identity is invalid"));
    }
    let mut payload = Vec::with_capacity(bytes);
    for expected in 0..bytes.div_ceil(CHUNK) {
        let Packet::Chunk { index, data } = frame(input)? else {
            return Err(invalid("migration chunk missing"));
        };
        let block = STANDARD.decode(data).map_err(io::Error::other)?;
        if index != expected || block.len() != CHUNK.min(bytes - payload.len()) {
            return Err(invalid(
                "migration chunk is out of order or has an invalid length",
            ));
        }
        payload.extend_from_slice(&block);
    }
    let Packet::End { identity: end } = frame(input)? else {
        return Err(invalid("migration request terminal missing"));
    };
    if end != identity || hash(&payload) != sha256
        || identity.operation != "managed_tests_v1" && input.read(&mut [0])? != 0 {
        return Err(invalid(
            "migration request has stale identity or extra data",
        ));
    }
    Ok((identity, payload))
}

fn linked(m: &fs::Metadata) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        m.file_attributes() & 0x400 != 0
    }
    #[cfg(not(windows))]
    m.file_type().is_symlink()
}

fn directory(path: &Path) -> io::Result<PathBuf> {
    if !path.is_absolute() {
        return Err(invalid("migration requires an absolute state path"));
    }
    for parent in path.ancestors() {
        let info = fs::symlink_metadata(parent)?;
        if !info.is_dir() || linked(&info) {
            return Err(invalid("migration directory is linked or missing"));
        }
    }
    path.canonicalize()
}

fn bytes(path: &Path, cap: usize) -> io::Result<Vec<u8>> {
    let metadata = fs::symlink_metadata(path)?;
    if !metadata.is_file() || linked(&metadata) || metadata.len() > cap as u64 {
        return Err(invalid("migration member is not a bounded ordinary file"));
    }
    let mut data = Vec::new();
    File::open(path)?
        .take(cap as u64 + 1)
        .read_to_end(&mut data)?;
    if data.len() > cap {
        return Err(invalid("migration member grew after admission"));
    }
    Ok(data)
}

fn snapshot(data: &[u8], identity: &Identity) -> io::Result<Snapshot> {
    let value: Snapshot = serde_json::from_slice(data).map_err(io::Error::other)?;
    if value.format != 1
        || value.inventory_schema != 5
        || value.root_identity != identity.root_identity
        || value.revision < identity.committed_revision
        || hash(value.policy.as_deref().unwrap_or("").as_bytes()) != identity.policy_sha256
        || value.tables.len() != TABLES.len()
    {
        return Err(invalid(
            "migration snapshot identity or schema is unsupported",
        ));
    }
    for (table, (name, columns, limit)) in value.tables.iter().zip(TABLES) {
        if table.name != name
            || table
                .columns
                .iter()
                .map(String::as_str)
                .ne(columns.split(','))
            || table.rows.len() > limit
        {
            return Err(invalid("migration snapshot contains incompatible tables"));
        }
        for row in &table.rows {
            if row.len() != table.columns.len()
                || row
                    .iter()
                    .any(|cell| !match (cell.kind.as_str(), &cell.value) {
                        ("null", None) | ("text", Some(_)) => true,
                        ("integer", Some(value)) => value.parse::<i64>().is_ok(),
                        ("blob", Some(value)) => STANDARD.decode(value).is_ok(),
                        _ => false,
                    })
            {
                return Err(invalid("migration snapshot contains invalid storage cells"));
            }
        }
    }
    let [workspace] = value.tables[0].rows.as_slice() else {
        return Err(invalid("migration workspace row is missing"));
    };
    if workspace[0].value.as_deref() != Some("1")
        || workspace[1]
            .value
            .as_deref()
            .and_then(|v| v.parse::<i64>().ok())
            != Some(value.revision)
        || workspace[2].value.as_deref() != Some(identity.root_identity.as_str())
        || workspace[3].value != value.policy
        || value.tables[5]
            .rows
            .iter()
            .any(|r| r[8].value.as_deref() == Some("running"))
        || value.tables[8].rows.iter().any(|r| {
            r[1].value.as_deref() != Some("1")
                || r[6].value.as_deref() != Some(identity.root_identity.as_str())
        })
    {
        return Err(invalid(
            "migration snapshot contains a conflicting workspace or receipt",
        ));
    }
    Ok(value)
}

pub(crate) fn create_bound(path: &Path, data: &[u8]) -> io::Result<()> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(path)?;
    file.write_all(data)?;
    file.sync_all()?;
    #[cfg(unix)]
    File::open(
        path.parent()
            .ok_or_else(|| invalid("migration path omitted parent"))?,
    )?
    .sync_all()?;
    Ok(())
}

fn install_once(path: &Path, data: &[u8], job: &str) -> io::Result<()> {
    match bytes(path, CAP) {
        Ok(previous) if previous == data => return Ok(()),
        Ok(_) => {
            return Err(invalid(
                "migration state has already been committed with different bytes",
            ));
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => (),
        Err(error) => return Err(error),
    }
    let next = path.with_extension(format!("{job}.pending"));
    create_bound(&next, data)?;
    fs::rename(&next, path)?;
    #[cfg(unix)]
    File::open(
        path.parent()
            .ok_or_else(|| invalid("migration path omitted parent"))?,
    )?
    .sync_all()?;
    Ok(())
}

fn file_hash(path: &Path) -> io::Result<String> {
    let file = File::open(path)?;
    let mut sha = Sha256::new();
    let read = io::copy(&mut file.take(128 * 1024 * 1024 + 1), &mut sha)?;
    if read > 128 * 1024 * 1024 {
        return Err(invalid("migration peer exceeds its binary budget"));
    }
    Ok(format!("{:X}", sha.finalize()))
}

#[cfg(target_os = "linux")]
fn physical_root(path: &Path) -> io::Result<String> {
    use std::ffi::{CString, c_char, c_int, c_uint, c_void};
    use std::os::unix::ffi::OsStrExt;

    unsafe extern "C" {
        fn statx(
            dir: c_int,
            path: *const c_char,
            flags: c_int,
            mask: c_uint,
            info: *mut c_void,
        ) -> c_int;
    }
    #[repr(align(8))]
    struct Statx([u8; 256]);
    let mut raw = Statx([0; 256]);
    let path = CString::new(path.as_os_str().as_bytes()).map_err(io::Error::other)?;
    // The Linux statx ABI has a fixed 256-byte layout. Use the same fields
    // as the managed RootIdentity implementation, including birth time.
    if unsafe { statx(-100, path.as_ptr(), 0x100, 0x901, raw.0.as_mut_ptr().cast()) } != 0 {
        return Err(io::Error::last_os_error());
    }
    let word = |offset| u32::from_ne_bytes(raw.0[offset..offset + 4].try_into().unwrap());
    let wide = |offset| u64::from_ne_bytes(raw.0[offset..offset + 8].try_into().unwrap());
    let mode = u16::from_ne_bytes(raw.0[28..30].try_into().unwrap());
    if word(0) & 0x101 != 0x101 || mode & 0xf000 != 0x4000 {
        return Err(invalid("migration root is no longer a physical directory"));
    }
    Ok(format!(
        "linux:{}:{}:{}:{}:{}",
        word(136),
        word(140),
        wide(32),
        wide(80) as i64,
        word(88)
    ))
}

#[cfg(windows)]
fn physical_root(path: &Path) -> io::Result<String> {
    use std::ffi::c_void;
    use std::os::windows::{fs::OpenOptionsExt, io::AsRawHandle};

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetFileInformationByHandleEx(
            handle: *mut c_void,
            class: i32,
            info: *mut c_void,
            size: u32,
        ) -> i32;
    }
    let root = OpenOptions::new()
        .access_mode(0)
        .share_mode(7)
        .custom_flags(0x0220_0000)
        .open(path)?;
    let mut id = [0u64; 3];
    if linked(&root.metadata()?) || !root.metadata()?.is_dir() {
        return Err(invalid("migration root is no longer a physical directory"));
    }
    if unsafe { GetFileInformationByHandleEx(root.as_raw_handle(), 18, id.as_mut_ptr().cast(), 24) }
        == 0
    {
        return Err(io::Error::last_os_error());
    }
    let [volume, low, high] = id;
    Ok(format!("windows:{volume:016x}:{low:016x}{high:016x}"))
}

#[cfg(not(any(windows, target_os = "linux")))]
fn physical_root(_: &Path) -> io::Result<String> {
    Err(invalid("migration root identity is unsupported on this OS"))
}

fn summarize(state: &Snapshot, identity: &Identity) -> io::Result<Vec<u8>> {
    serde_json::to_vec(&serde_json::json!({
        "engine": "rust", "epoch": identity.epoch, "committedRevision": state.revision,
        "sourceCount": state.tables[1].rows.len(), "pendingCauses": state.tables[3].rows.len(),
        "pendingWorkspaceFallback": !state.tables[4].rows.is_empty(),
    }))
    .map_err(io::Error::other)
}

fn reconcile(root: &Path, state: &mut Snapshot) -> io::Result<bool> {
    let (git, scan) = observe_workspace(root)?;
    // Preserve all predecessor evidence when the current observation is incomplete.
    // A complete four-language scan supplies the C# projection of schema v5.
    if scan.partial || !state.tables[2].rows.is_empty() {
        return Err(invalid(
            "migration reconciliation requires a complete source observation",
        ));
    }
    if state.tables[1].rows.iter().any(|row| {
        row[1].value.as_deref().is_none_or(|path| {
            path.contains('\\')
                || path
                    .split('/')
                    .any(|part| part.eq_ignore_ascii_case(".slophawk-native-coordinator"))
        })
    }) {
        return Err(invalid(
            "migration reconciliation cannot prove the legacy source scope",
        ));
    }
    let mut rows = BTreeMap::new();
    for (path, fact) in scan.files {
        if !path.to_ascii_lowercase().ends_with(".cs") {
            continue;
        }
        if cfg!(windows) && !path.is_ascii() {
            return Err(invalid(
                "migration cannot reconcile non-ASCII Windows source keys",
            ));
        }
        let key = if cfg!(windows) {
            path.to_ascii_uppercase()
        } else {
            path.clone()
        };
        let row = vec![
            Cell::text(key.clone()),
            Cell::text(path),
            Cell::text(fact.digest.to_ascii_uppercase()),
            Cell::integer(1),
        ];
        if rows.insert(key, row).is_some() {
            return Err(invalid(
                "migration reconciliation has ambiguous source keys",
            ));
        }
    }
    let rows: Vec<_> = rows.into_values().collect();
    let source_changed = rows != state.tables[1].rows;
    let branch = Cell::nullable(git.branch);
    let commit = Cell::nullable(git.commit);
    let workspace = &mut state.tables[0].rows[0];
    if !source_changed && workspace[4] == branch && workspace[5] == commit {
        return Ok(false);
    }
    let revision = state
        .revision
        .checked_add(1)
        .ok_or_else(|| invalid("migration revision exhausted"))?;
    state.revision = revision;
    workspace[1] = Cell::integer(revision);
    workspace[4] = branch;
    workspace[5] = commit;
    if source_changed {
        state.tables[1].rows = rows;
        let fallback = &mut state.tables[4].rows;
        let superseded = match fallback.as_mut_slice() {
            [] => {
                fallback.push(vec![
                    Cell::integer(1),
                    Cell::integer(revision),
                    Cell::integer(revision),
                ]);
                None
            }
            [row] => {
                let prior = row[2]
                    .value
                    .clone()
                    .ok_or_else(|| invalid("missing fallback revision"))?;
                row[2] = Cell::integer(revision);
                Some(hash(format!("{prior}:").as_bytes()))
            }
            _ => return Err(invalid("ambiguous migration workspace fallback")),
        };
        if let Some(prior) = superseded {
            state.tables[6]
                .rows
                .retain(|row| row[0].value.as_ref() != Some(&prior));
        }
    }
    Ok(true)
}

fn read_marker(directory: &Path) -> io::Result<Marker> {
    serde_json::from_slice(&bytes(&directory.join("migration-engine.json"), 4096)?)
        .map_err(io::Error::other)
}

fn writer_lock(directory: &Path, epoch: &str, budget: Duration) -> io::Result<File> {
    let path = directory.join(format!("native-{epoch}.lock"));
    if fs::symlink_metadata(&path).is_ok_and(|value| !value.is_file() || linked(&value)) {
        return Err(invalid("migration writer lock is linked"));
    }
    let lock = OpenOptions::new().read(true).write(true).create(true).truncate(false).open(path)?;
    let deadline = std::time::Instant::now() + budget;
    loop {
        match lock.try_lock() {
            Ok(()) => return Ok(lock),
            Err(std::fs::TryLockError::WouldBlock) if std::time::Instant::now() < deadline =>
                std::thread::sleep(Duration::from_millis(30)),
            Err(error) => return Err(error.into()),
        }
    }
}

fn store_snapshot(state: &Path, data: &[u8], job: &str) -> io::Result<()> {
    let next = state.with_extension(format!("{job}.pending"));
    create_bound(&next, data)?;
    fs::rename(&next, state)?;
    #[cfg(unix)]
    File::open(state.parent().ok_or_else(|| invalid("migration state directory missing"))?)?.sync_all()?;
    Ok(())
}

fn exchange(identity: &Identity, data: &[u8], owner_alive: &AtomicBool) -> io::Result<(Vec<u8>, bool)> {
    let root = directory(&identity.root)?;
    let directory = directory(&identity.state_directory)?;
    if directory.starts_with(&root)
        || physical_root(&root)? != identity.root_identity
        || file_hash(&std::env::current_exe()?)? != identity.tool_sha256
    {
        return Err(invalid(
            "migration writer or state location differs from the pinned identity",
        ));
    }
    if identity.operation == "managed_tests_drain_v1" && data.is_empty()
        && identity.epoch == managed_test_epoch(&identity.root_identity) {
        let _lock = writer_lock(&directory, &identity.epoch, Duration::from_secs(2))?;
        slophawk_worker_scope::recover(Duration::from_secs(3)).map_err(io::Error::other)?;
        return Ok((Vec::new(), false));
    }
    if identity.operation == "managed_tests_v1" {
        return managed_tests(identity, &root, &directory, data, owner_alive).map(|result| (result, false));
    }
    let marker = read_marker(&directory)?;
    if marker.version != 1
        || marker.root.canonicalize()? != root
        || marker.root_identity != identity.root_identity
        || marker.revision != identity.committed_revision
        || marker.epoch != identity.epoch
        || marker.native_sha256 != identity.tool_sha256
        || !hex(&marker.profile_sha256, 64)
        || !hex(&marker.backup_sha256, 64)
    {
        return Err(invalid("migration marker does not bind this writer"));
    }
    let wait = if identity.operation == "export_and_stop_v1" { Duration::from_secs(8) } else { Duration::ZERO };
    let _lock = writer_lock(&directory, &identity.epoch, wait)?;
    slophawk_worker_scope::recover(Duration::from_secs(3)).map_err(io::Error::other)?;
    if read_marker(&directory)? != marker || physical_root(&root)? != identity.root_identity {
        return Err(invalid("migration writer changed while taking its lease"));
    }
    let state = directory.join(format!("native-{}.json", identity.epoch));
    let stopped = directory.join(format!("native-{}.stopped", identity.epoch));
    match (identity.operation.as_str(), marker.state.as_str()) {
        ("import_state_v1", "transition-uncertain") => {
            if fs::symlink_metadata(&stopped).is_ok()
                || hash(data) != marker.backup_sha256
                || snapshot(data, identity)?.revision != identity.committed_revision
            {
                return Err(invalid("migration import no longer owns this epoch"));
            }
            install_once(&state, data, &identity.job)?;
            Ok((Vec::new(), true))
        }
        ("export_and_stop_v1", "native" | "rollback-uncertain") if data.is_empty() => {
            let data = bytes(&state, CAP)?;
            let mut exported = snapshot(&data, identity)?;
            // The stopped seal commits an immutable rollback payload on retry.
            let data = if fs::symlink_metadata(&stopped).is_ok() {
                data
            } else {
                let tests = scheduled_tests::Scheduler::open(&directory, &root, &identity.epoch)?;
                if tests.project(&root, &mut exported)? {
                    let next = serde_json::to_vec(&exported).map_err(io::Error::other)?;
                    snapshot(&next, identity)?;
                    if next.len() > CAP || read_marker(&directory)? != marker
                        || physical_root(&root)? != identity.root_identity {
                        return Err(invalid("rollback test projection lost its authority or budget"));
                    }
                    store_snapshot(&state, &next, &identity.job)?;
                    next
                } else {
                    data
                }
            };
            install_once(&stopped, hash(&data).as_bytes(), &identity.job)?;
            Ok((data, false))
        }
        ("abort_import_v1", "transition-uncertain") if data.is_empty() => {
            // No engine can operate an uncertain epoch. Seal it permanently
            // before letting the previous writer resume.
            install_once(&stopped, b"aborted", &identity.job)?;
            Ok((Vec::new(), false))
        }
        ("inspect_state_v1" | "reconcile_state_v1", "native") if data.is_empty() => {
            if fs::symlink_metadata(&stopped).is_ok()
                || fs::symlink_metadata(directory.join("migration-engine.json.pending")).is_ok()
            {
                return Err(invalid(
                    "native writer is stopped or has an unfinished transition",
                ));
            }
            let mut value = snapshot(&bytes(&state, CAP)?, identity)?;
            if identity.operation == "reconcile_state_v1" && reconcile(&root, &mut value)? {
                let data = serde_json::to_vec(&value).map_err(io::Error::other)?;
                snapshot(&data, identity)?;
                if data.len() > CAP
                    || read_marker(&directory)? != marker
                    || physical_root(&root)? != identity.root_identity
                {
                    return Err(invalid(
                        "native reconciliation lost its writer or snapshot budget",
                    ));
                }
                store_snapshot(&state, &data, &identity.job)?;
            }
            Ok((summarize(&value, identity)?, true))
        }
        _ => Err(invalid(
            "migration operation is unsupported for this writer phase",
        )),
    }
}

fn managed_test_epoch(root: &str) -> String {
    hash(format!("managed-test-ledger\0{root}").as_bytes())[..32].to_ascii_lowercase()
}

fn managed_tests(identity: &Identity, root: &Path, state: &Path, data: &[u8],
    owner_alive: &AtomicBool) -> io::Result<Vec<u8>> {
    let binding = state.join("managed-test-peer.json");
    let current = || {
        owner_alive.load(Ordering::Acquire) && ["migration-engine.json", "migration-engine.json.pending"]
            .iter()
            .all(|name| fs::symlink_metadata(state.join(name)).is_err_and(|error| {
                error.kind() == io::ErrorKind::NotFound
            }))
            && physical_root(root).is_ok_and(|value| value == identity.root_identity)
            && slophawk_language_execution::read_owner_document(&binding, root)
                .is_ok_and(|value| hash(&value) == identity.grant_ref)
    };
    if identity.epoch != managed_test_epoch(&identity.root_identity) || !current() {
        return Err(invalid("the managed test peer lost its owner binding"));
    }
    let _lock = writer_lock(state, &identity.epoch, Duration::ZERO)?;
    slophawk_worker_scope::recover(Duration::from_secs(3)).map_err(io::Error::other)?;
    let mut snapshot = snapshot(data, identity)?;
    if snapshot.revision != identity.committed_revision || !current() {
        return Err(invalid("the managed test peer received a stale inventory"));
    }

    let mut tests = scheduled_tests::Scheduler::open(state, root, &identity.epoch)?
        .for_managed(&identity.grant_ref);
    let mut projected = tests.project(root, &mut snapshot)?;
    if !projected {
        let causes = if tests.configured() { test_handoff::causes(&snapshot)? } else { Vec::new() };
        if let Some(mut attempt) = tests.start_with_causes(root, &identity.root_identity,
            &identity.epoch, snapshot.revision, causes) {
            loop {
                if !current() {
                    return Err(invalid("the managed test peer lost its owner binding"));
                }
                if let Some(finished) = attempt.completed() {
                    tests.finish(root, &identity.root_identity, snapshot.revision, finished?)?;
                    break;
                }
                std::thread::sleep(Duration::from_millis(50));
            }
            projected = tests.project(root, &mut snapshot)?;
        }
    }
    let tables = projected.then(|| {
        snapshot.tables.into_iter().filter(|table| matches!(table.name.as_str(),
            "test_intent" | "intent_fallback" | "test_attempt" | "test_evidence" | "test_run_receipt"))
            .collect::<Vec<_>>()
    });
    if !current() {
        return Err(invalid("the managed test peer lost its owner binding"));
    }
    let payload = serde_json::to_vec(&serde_json::json!({"status":tests.status(),"tables":tables}))
        .map_err(io::Error::other)?;
    if payload.len() > CAP {
        return Err(invalid("managed test results exceed the interchange budget"));
    }
    Ok(payload)
}

pub fn run_stdio() -> io::Result<()> {
    let (identity, data) = receive(&mut io::stdin().lock())?;
    let owner_alive = Arc::new(AtomicBool::new(true));
    if identity.operation == "managed_tests_v1" {
        let alive = Arc::clone(&owner_alive);
        // Only the current managed writer holds this pipe. EOF fences the
        // running attempt even if the daemon exits without sending Stop.
        std::thread::Builder::new().name("managed-writer-lifetime".into()).spawn(move || {
            let _ = io::stdin().lock().read(&mut [0]);
            alive.store(false, Ordering::Release);
        })?;
    }
    let (export, claimed) = exchange(&identity, &data, &owner_alive)?;
    let mut output = io::stdout().lock();
    for (index, chunk) in export.chunks(CHUNK).enumerate() {
        emit(
            &mut output,
            &serde_json::json!({"kind":"chunk","index":index,"data":STANDARD.encode(chunk)}),
        )?;
    }
    emit(
        &mut output,
        &serde_json::json!({
            "kind":"terminal","identity":identity,"status":"complete","reason":"ok","omissions":[],
            "payloadSha256":hash(&export),"drained":true,"writerClaimed":claimed,
        }),
    )?;
    output.flush()
}
