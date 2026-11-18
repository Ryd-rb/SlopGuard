//! Bootstrap the Rust-owned workspace without running a managed application.
use super::*;
use slophawk_public_shell::identity::WorkspaceIdentity;

pub(super) fn profile() -> String {
    hash(b"slophawk-workspace-v2")
}

pub fn open(identity: &WorkspaceIdentity, create: bool) -> io::Result<(PathBuf, String)> {
    let (root, state) = state_paths(identity, create)?;
    let marker_path = state.join("migration-engine.json");
    if !create {
        let marker = authorized_marker(&state, &root)?;
        return Ok((state, marker.profile_sha256));
    }
    let _bootstrap = writer_lock(&state, "bootstrap", Duration::from_secs(3))?;
    if let Some(marker) = ordinary_marker(&state, &root)? {
        if marker.version == 1 {
            adopt(&marker_path, &state, &root, marker)?;
        }
    } else {
        if !absent(&state.join("inventory.sqlite3"))? {
            return Err(invalid("managed schema-v5 inventory is present; run the offline state importer before starting this workspace"));
        }
        let key = physical_root(&root)?;
        let mut snapshot = Snapshot {
            format: 1, inventory_schema: 5, root_identity: key.clone(), revision: 0, policy: None,
            tables: TABLES.iter().map(|(name, columns, _)| Table {
                name: (*name).to_owned(), columns: columns.split(',').map(str::to_owned).collect(),
                rows: Vec::new(),
            }).collect(),
        };
        snapshot.tables[0].rows.push(vec![
            Cell::integer(1), Cell::integer(0), Cell::text(key.clone()), Cell::nullable(String::new()),
            Cell::nullable(String::new()), Cell::nullable(String::new()),
            Cell::nullable(String::new()), Cell::nullable(String::new()),
        ]);
        install_snapshot(&identity.root, &state, key, snapshot)?;
    }
    let marker = authorized_marker(&state, &root)?;
    Ok((state, marker.profile_sha256))
}

pub fn import_legacy(identity: &WorkspaceIdentity, input: impl Read) -> io::Result<()> {
    let (root, state) = state_paths(identity, false)?;
    let _bootstrap = writer_lock(&state, "bootstrap", Duration::from_secs(3))?;
    if ordinary_marker(&state, &root)?.is_some() {
        return Err(invalid("this workspace already has a committed writer"));
    }
    let inventory = fs::symlink_metadata(state.join("inventory.sqlite3"))?;
    if !inventory.is_file() || linked(&inventory) {
        return Err(invalid("an ordinary existing SQLite inventory is required"));
    }
    let mut data = Vec::new();
    input.take(CAP as u64 + 1).read_to_end(&mut data)?;
    if data.len() > CAP {
        return Err(invalid("legacy workspace snapshot exceeds its bound"));
    }
    let snapshot: Snapshot = serde_json::from_slice(&data).map_err(io::Error::other)?;
    let key = physical_root(&root)?;
    let policy = hash(snapshot.policy.as_deref().unwrap_or("").as_bytes());
    validate_snapshot(&snapshot, &key, 0, &policy)?;
    slophawk_worker_scope::recover(Duration::from_secs(3)).map_err(io::Error::other)?;
    if physical_root(&root)? != key {
        return Err(invalid("the workspace was replaced during import"));
    }
    install_snapshot(&identity.root, &state, key, snapshot)
}

fn state_paths(identity: &WorkspaceIdentity, create: bool) -> io::Result<(PathBuf, PathBuf)> {
    let root = directory(&identity.root)?;
    let state = identity.state_directory().map_err(io::Error::other)?;
    if create {
        create_private(&state, &root)?;
    }
    let state = directory(&state)?;
    if state.starts_with(&root) {
        return Err(invalid("workspace state must live outside the checkout"));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let metadata = fs::metadata(&state)?;
        if metadata.mode() & 0o077 != 0 || metadata.uid() != unsafe { libc::geteuid() } {
            return Err(invalid("workspace state directory must belong to the current user and be private"));
        }
    }
    Ok((root, state))
}

fn ordinary_marker(state: &Path, root: &Path) -> io::Result<Option<Marker>> {
    if !absent(&state.join("migration-engine.json.pending"))? {
        return Err(invalid("a workspace state transfer is unfinished"));
    }
    let path = state.join("migration-engine.json");
    if absent(&path)? {
        return Ok(None);
    }
    let bytes = slophawk_language_execution::read_owner_document_bounded(&path, root, 4096)?;
    let marker: Marker = serde_json::from_slice(&bytes).map_err(io::Error::other)?;
    if !matches!(marker.version, 1 | 2) || marker.state != "native" || marker.root.canonicalize()? != root
        || marker.root_identity != physical_root(root)? || !hex(&marker.epoch, 32)
        || !hex(&marker.profile_sha256, 64) || !hex(&marker.native_sha256, 64)
        || !hex(&marker.backup_sha256, 64) || marker.revision < 0
        || !absent(&state.join(format!("native-{}.stopped", marker.epoch)))? {
        return Err(invalid("the workspace writer is incompatible or a transition is unfinished"));
    }
    Ok(Some(marker))
}

fn authorized_marker(state: &Path, root: &Path) -> io::Result<Marker> {
    ordinary_marker(state, root)?.ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "run slophawk start first"))
}

fn adopt(path: &Path, state: &Path, root: &Path, previous: Marker) -> io::Result<()> {
    // An already running Rust daemon retains its writer. With no holder,
    // validate the old snapshot before allowing future binary upgrades.
    let _writer = match writer_lock(state, &previous.epoch, Duration::ZERO) {
        Ok(lock) => lock,
        Err(error) if error.kind() == io::ErrorKind::WouldBlock => return Ok(()),
        Err(error) => return Err(error),
    };
    if ordinary_marker(state, root)?.as_ref() != Some(&previous) {
        return Err(invalid("the transferred writer changed during adoption"));
    }
    slophawk_worker_scope::recover(Duration::from_secs(3)).map_err(io::Error::other)?;
    let (_, _, checked) = resident_store::read_workspace_snapshot(state, &previous)?;
    let mut marker = previous;
    marker.version = 2;
    // Preserve the workspace epoch and durable receipt bindings across the
    // launcher change. The stored marker remains the single writer fence.
    if checked.revision < marker.revision {
        return Err(invalid("the workspace inventory regressed during adoption"));
    }
    let data = serde_json::to_vec(&marker).map_err(io::Error::other)?;
    store_snapshot(path, &data, &resident_store::nonce()?)
}

pub(super) fn install_snapshot(root: &Path, state: &Path, key: String, snapshot: Snapshot) -> io::Result<()> {
    let epoch = resident_store::nonce()?;
    let data = serde_json::to_vec(&snapshot).map_err(io::Error::other)?;
    if data.len() > CAP {
        return Err(invalid("workspace snapshot exceeds its bound"));
    }
    let marker = Marker {
        version: 2, root: root.to_owned(), root_identity: key, revision: snapshot.revision,
        native_sha256: file_hash(&std::env::current_exe()?)?, profile_sha256: profile(),
        state: "native".into(), backup_sha256: hash(&data), epoch: epoch.clone(),
    };
    let token = resident_store::nonce()?;
    install_once(&state.join(format!("migration-backup-{epoch}.json")), &data, &token)?;
    install_once(&state.join(format!("native-{epoch}.json")), &data, &token)?;
    let serialized = serde_json::to_vec(&marker).map_err(io::Error::other)?;
    install_once(&state.join("migration-engine.json"), &serialized, &token)
}

fn create_private(path: &Path, root: &Path) -> io::Result<()> {
    if !absent(path)? {
        directory(path)?;
        return Ok(());
    }
    let parent = path.parent().ok_or_else(|| invalid("workspace state has no parent"))?;
    create_private(parent, root)?;
    let parent = directory(parent)?;
    if parent.starts_with(root) {
        return Err(invalid("workspace state must live outside the checkout"));
    }
    let physical = parent.join(path.file_name().ok_or_else(|| invalid("missing workspace state name"))?);
    #[cfg(unix)]
    let created = {
        use std::os::unix::fs::DirBuilderExt;
        fs::DirBuilder::new().mode(0o700).create(&physical)
    };
    #[cfg(windows)]
    let created = slophawk_language_execution::create_owner_directory(&physical);
    match created {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => directory(path).map(|_| ()),
        Err(error) => Err(error),
    }
}

pub(super) fn absent(path: &Path) -> io::Result<bool> {
    match fs::symlink_metadata(path) {
        Ok(_) => Ok(false),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(true),
        Err(error) => Err(error),
    }
}
