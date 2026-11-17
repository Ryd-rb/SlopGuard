use super::*;
use serde_json::{Value, json};

pub struct Workspace {
    identity: Identity,
    marker: Marker,
    state: PathBuf,
    snapshot: Snapshot,
    root: PathBuf,
    _lease: File,
    revision: u64,
    epoch: String,
    report: Option<Vec<u8>>,
    error: Option<String>,
}

impl Workspace {
    pub fn open(root: &Path, state_dir: &Path, profile: &str) -> io::Result<Self> {
        let root = directory(root)?;
        let state_dir = directory(state_dir)?;
        if state_dir.starts_with(&root) {
            return Err(invalid("resident state must live outside the checkout"));
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            if fs::metadata(&state_dir)?.mode() & 0o077 != 0 {
                return Err(invalid("resident state directory must be private"));
            }
        }
        let marker = read_marker(&state_dir)?;
        if marker.version != 1
            || marker.state != "native"
            || marker.revision < 0
            || marker.root.canonicalize()? != root
            || physical_root(&root)? != marker.root_identity
            || marker.profile_sha256 != profile
            || !hex(profile, 64)
            || !hex(&marker.epoch, 32)
            || file_hash(&std::env::current_exe()?)? != marker.native_sha256
        {
            return Err(invalid(
                "resident daemon does not own the committed migration epoch",
            ));
        }
        slophawk_worker_scope::recover(Duration::from_secs(3)).map_err(io::Error::other)?;
        let lease = writer_lock(&state_dir, &marker.epoch, Duration::ZERO)?;
        let state = state_dir.join(format!("native-{}.json", marker.epoch));
        let backup = bytes(
            &state_dir.join(format!("migration-backup-{}.json", marker.epoch)),
            CAP,
        )?;
        if hash(&backup) != marker.backup_sha256 {
            return Err(invalid(
                "resident migration backup no longer matches the marker",
            ));
        }
        let policy: Snapshot = serde_json::from_slice(&backup).map_err(io::Error::other)?;
        let identity = Identity {
            protocol: PROTOCOL.into(),
            operation: "resident".into(),
            job: nonce()?,
            root: marker.root.clone(),
            root_identity: marker.root_identity.clone(),
            epoch: marker.epoch.clone(),
            committed_revision: marker.revision,
            input_sha256: hash(&[]),
            tool_sha256: marker.native_sha256.clone(),
            policy_sha256: hash(policy.policy.as_deref().unwrap_or_default().as_bytes()),
            budget_ref: "migration-state-16m-frame-128k".into(),
            grant_ref: profile.into(),
            state_directory: state_dir,
        };
        let snapshot = snapshot(&bytes(&state, CAP)?, &identity)?;
        let mut window = Self {
            identity,
            marker,
            state,
            snapshot,
            root,
            _lease: lease,
            revision: 1,
            epoch: epoch()?,
            report: None,
            error: None,
        };
        window.authorized()?;
        window.refresh();
        Ok(window)
    }

    pub fn authorized(&self) -> io::Result<()> {
        let directory = &self.identity.state_directory;
        if read_marker(directory)? != self.marker
            || physical_root(&self.root)? != self.identity.root_identity
        {
            return Err(invalid("the daemon's native writer authority changed"));
        }
        for path in [
            directory.join("migration-engine.json.pending"),
            directory.join(format!("native-{}.stopped", self.marker.epoch)),
        ] {
            match fs::symlink_metadata(path) {
                Ok(_) => return Err(invalid("the daemon's native writer is being retired")),
                Err(error) if error.kind() == io::ErrorKind::NotFound => (),
                Err(error) => return Err(error),
            }
        }
        Ok(())
    }

    pub fn refresh(&mut self) {
        if let Err(error) = self.reconcile() {
            self.error = Some(error.to_string());
            self.withdraw();
        }
    }

    fn reconcile(&mut self) -> io::Result<()> {
        self.authorized()?;
        // Rebuild changes in a proposed snapshot so a failed write leaves the
        // resident state at the last durable revision and allows a clean retry.
        let mut next = self.snapshot.clone();
        if reconcile(&self.root, &mut next)? {
            let data = serde_json::to_vec(&next).map_err(io::Error::other)?;
            if data.len() > CAP {
                return Err(invalid("resident state exceeds its migration budget"));
            }
            let _ = snapshot(&data, &self.identity)?;
            self.authorized()?;
            store_snapshot(&self.state, &data, &nonce()?)?;
            if self.snapshot.tables[0].rows[0][4..6] != next.tables[0].rows[0][4..6] {
                self.epoch = epoch()?;
            }
            self.snapshot = next;
            if self.report.is_some() {
                self.invalidate()?;
            }
        }
        if self.report.is_none() {
            let row = &self.snapshot.tables[0].rows[0];
            let value = json!({"schemaVersion":2,"producer":"SlopHawk","root":self.marker.root,
                "epoch":self.epoch,"revision":self.revision,"branch":row[4].value,"commit":row[5].value,
                "fileCount":self.snapshot.tables[1].rows.len(),"partial":false,"committedRevision":self.snapshot.revision});
            self.publish(serde_json::to_vec(&value).map_err(io::Error::other)?)?;
        }
        self.error = None;
        Ok(())
    }

    pub fn invalidate(&mut self) -> io::Result<u64> {
        self.revision = self
            .revision
            .checked_add(1)
            .ok_or_else(|| invalid("resident revision exhausted"))?;
        self.withdraw();
        Ok(self.revision)
    }

    pub fn status(&self) -> Value {
        let detail = &self.snapshot.tables[3].rows;
        let fallback = self.snapshot.tables[4].rows.first().map(|row| json!({
            "firstRevision": integer(&row[1]), "lastRevision": integer(&row[2]), "scope":"workspace"
        }));
        let last_fallback = self.snapshot.tables[4]
            .rows
            .first()
            .and_then(|row| integer(&row[2]));
        let first_fallback = self.snapshot.tables[4]
            .rows
            .first()
            .and_then(|row| integer(&row[1]));
        let retired = &self.snapshot.tables[0].rows[0];
        let retirement = integer(&retired[6]).map(|revision| {
            json!({
                "revision": revision,"count":integer(&retired[7]),"reason":"root_replaced"
            })
        });
        let ready = self.report.is_some();
        json!({"root":self.marker.root,"epoch":self.epoch,"revision":self.revision,
            "committedRevision":self.snapshot.revision,"reportSchemaVersion":2,
            "state": if ready { "ready" } else if self.error.is_some() { "degraded" } else { "pending" },
            "lastError":self.error,"fileCount":ready.then(||self.snapshot.tables[1].rows.len()),
            "partial":ready.then_some(false),"recordingError":null,
            "testScheduler":{"state":"awaiting_adapter","latest":null,"retryAt":null,"error":null},
            "executionProfile":{"path":self.identity.state_directory.join("execution.json"),"rootIdentity":self.identity.root_identity},
            "projectEvaluation":null,"pendingTestIntents":{"detailedCount":detail.len(),
                "oldestRevision":detail.first().and_then(|row|integer(&row[0])).or(first_fallback),
                "latestRevision":last_fallback.or_else(||detail.last().and_then(|row|integer(&row[0]))),
                "fallback":fallback,"state":if !detail.is_empty() || fallback.is_some() {"awaiting_selection"} else {"none"}},
            "retiredTestIntents":retirement})
    }

    pub fn results(&mut self) -> Option<Value> {
        self.refresh();
        let expected = self.report.as_ref()?;
        let actual = bytes(&self.root.join(".slophawk/results.json"), 8 * 1024 * 1024).ok()?;
        (actual == *expected)
            .then(|| serde_json::from_slice(expected).ok())
            .flatten()
    }

    fn publish(&mut self, data: Vec<u8>) -> io::Result<()> {
        let folder = self.root.join(".slophawk");
        match fs::create_dir(&folder) {
            Ok(()) => (),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => (),
            Err(error) => return Err(error),
        }
        directory(&folder)?;
        let folder_id = physical_root(&folder)?;
        let report = folder.join("results.json");
        verify_report(&folder, &report, &self.marker.root)?;
        let stage = self.root.join(format!(".slophawk-{}.pending", nonce()?));
        create_bound(&stage, &data)?;
        let result = (|| {
            self.authorized()?;
            directory(&folder)?;
            verify_report(&folder, &report, &self.marker.root)?;
            if physical_root(&folder)? != folder_id || bytes(&stage, 8 * 1024 * 1024)? != data {
                return Err(invalid("resident report staging or directory changed"));
            }
            fs::rename(&stage, &report)
        })();
        if result.is_err()
            && physical_root(&self.root).is_ok_and(|id| id == self.identity.root_identity)
            && bytes(&stage, 8 * 1024 * 1024).is_ok_and(|staged| staged == data)
        {
            let _ = fs::remove_file(stage);
        }
        result?;
        self.report = Some(data);
        Ok(())
    }

    fn withdraw(&mut self) {
        let Some(owned) = self.report.take() else {
            return;
        };
        let folder = self.root.join(".slophawk");
        let target = folder.join("results.json");
        if physical_root(&self.root).is_ok_and(|id| id == self.identity.root_identity)
            && directory(&folder).is_ok()
            && bytes(&target, 8 * 1024 * 1024).is_ok_and(|bytes| bytes == owned)
        {
            let _ = fs::remove_file(target);
        }
    }
}

impl Drop for Workspace {
    fn drop(&mut self) {
        self.withdraw();
    }
}

fn verify_report(folder: &Path, report: &Path, root: &Path) -> io::Result<()> {
    for path in fs::read_dir(folder)? {
        if path?.path() != report {
            return Err(invalid(".slophawk contains an unrecognized path"));
        }
    }
    match bytes(report, 8 * 1024 * 1024) {
        Ok(data) => {
            let document: Value = serde_json::from_slice(&data).map_err(io::Error::other)?;
            if document["producer"] != "SlopHawk"
                || document["schemaVersion"] != 2
                || document["root"].as_str().map(Path::new) != Some(root)
            {
                return Err(invalid(
                    "resident report path is owned by another producer or root",
                ));
            }
            Ok(())
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

fn integer(cell: &Cell) -> Option<i64> {
    cell.value.as_deref()?.parse().ok()
}

fn nonce() -> io::Result<String> {
    let mut bytes = [0u8; 16];
    getrandom::fill(&mut bytes).map_err(io::Error::other)?;
    Ok(slophawk_public_shell::identity::uppercase_hex(&bytes).to_ascii_lowercase())
}

fn epoch() -> io::Result<String> {
    let id = nonce()?;
    Ok(format!(
        "{}-{}-{}-{}-{}",
        &id[..8],
        &id[8..12],
        &id[12..16],
        &id[16..20],
        &id[20..]
    ))
}
