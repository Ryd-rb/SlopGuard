//! Durable source-check admission and receipts owned by the native workspace writer.
use super::{bytes, directory, hash, invalid, linked, observe_workspace, store_snapshot};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use slophawk_public_shell::protocol::CheckerFence;
use slophawk_native_checks::contract::{
    Context, Language, Limits, MAX_SOURCE_BYTES, MAX_SOURCES, MAX_TOTAL_BYTES, Provenance,
    ScopeRequest, Source, Worker,
};
use std::collections::BTreeMap;
use std::fs;
use std::io::{self, ErrorKind};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::{self, JoinHandle};

const PROFILE_LIMIT: usize = 32 * 1024;
const RECEIPT_LIMIT: usize = 2 * 1024 * 1024;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Profile {
    version: u32,
    root_identity: String,
    context: Context,
    worker: Option<PinnedWorker>,
    #[serde(default)]
    near_miss_clones: bool,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PinnedWorker {
    dotnet: Pin,
    assembly: Pin,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Pin {
    path: PathBuf,
    sha256: String,
}

impl Pin {
    fn verify(&self, root: &Path) -> io::Result<String> {
        external(&self.path, root, false)?;
        if super::file_hash(&self.path)? != self.sha256 {
            return Err(invalid("scheduled checker tool does not match its pin"));
        }
        path_text(&self.path)
    }
}

fn external(path: &Path, root: &Path, is_directory: bool) -> io::Result<()> {
    slophawk_public_shell::identity::require_physical(path, is_directory)
        .map_err(io::Error::other)?;
    if path.canonicalize()?.starts_with(root) {
        return Err(invalid(
            "scheduled checker tools must be outside the checkout",
        ));
    }
    Ok(())
}

fn path_text(path: &Path) -> io::Result<String> {
    path.to_str()
        .map(str::to_owned)
        .ok_or_else(|| invalid("checker paths must be Unicode"))
}

#[derive(Deserialize, Serialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum Outcome {
    Running,
    Interrupted,
    Complete,
    Partial,
    Failed,
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Record {
    attempt: String,
    scope_sha256: String,
    outcome: Outcome,
    report: Option<Value>,
    error: Option<String>,
    sources: BTreeMap<String, String>,
}

pub(super) struct Scheduler {
    profile: PathBuf,
    ledger: PathBuf,
    epoch: String,
    latest: Option<Record>,
    // Hash only when loading or replacing the receipt, never on a status read.
    receipt_sha256: Option<String>,
    active_scope: Option<String>,
    enabled: bool,
    error: Option<String>,
}

struct Input {
    path: String,
    language: Language,
    digest: String,
    size: u64,
}

struct Observed {
    profile: Profile,
    profile_sha256: String,
    scope: String,
    inputs: Vec<Input>,
}

impl Scheduler {
    pub(super) fn open(state: &Path, epoch: &str) -> io::Result<Self> {
        let ledger = state.join(format!("native-{epoch}.checks.json"));
        let (latest, receipt_sha256) = match bytes(&ledger, RECEIPT_LIMIT) {
            Ok(value) => (
                Some(serde_json::from_slice::<Record>(&value).map_err(io::Error::other)?),
                Some(hash(&value)),
            ),
            Err(error) if error.kind() == ErrorKind::NotFound => (None, None),
            Err(error) => return Err(error),
        };
        let mut schedule = Self {
            profile: state.join("native-checkers.json"),
            ledger,
            epoch: epoch.into(),
            latest,
            receipt_sha256,
            active_scope: None,
            enabled: false,
            error: None,
        };
        // The writer acquired the lease and recovered the worker scope before opening this file.
        if let Some(mut latest) = schedule.latest.take() {
            if latest.outcome == Outcome::Running {
                latest.outcome = Outcome::Interrupted;
                latest.report = None;
                latest.error = Some("attempt_interrupted_before_receipt".into());
                schedule.receipt_sha256 = Some(schedule.write(&latest)?);
            }
            schedule.latest = Some(latest);
        }
        Ok(schedule)
    }

    fn write(&self, record: &Record) -> io::Result<String> {
        let data = serde_json::to_vec(record).map_err(io::Error::other)?;
        if data.len() > RECEIPT_LIMIT {
            return Err(invalid("scheduled check receipt exceeds its bound"));
        }
        store_snapshot(&self.ledger, &data, &super::resident_store::nonce()?)?;
        Ok(hash(&data))
    }

    // Keep the cheap, bounded policy observation separate from source and tool
    // validation. Status must observe revocation without scanning the checkout.
    fn read_profile(&self, root_identity: &str) -> io::Result<Option<(Profile, String)>> {
        let meta = match fs::symlink_metadata(&self.profile) {
            Ok(meta) => meta,
            Err(error) if error.kind() == ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error),
        };
        if !meta.is_file() || linked(&meta) {
            return Err(invalid("invalid scheduled checker profile"));
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            let owner =
                fs::metadata(self.profile.parent().expect("configured state parent"))?.uid();
            if meta.mode() & 0o077 != 0 || meta.uid() != owner {
                return Err(invalid("checker profile must be private"));
            }
        }
        let raw = bytes(&self.profile, PROFILE_LIMIT)?;
        let profile: Profile = serde_json::from_slice(&raw).map_err(io::Error::other)?;
        if profile.version != 1 || profile.root_identity != root_identity {
            return Err(invalid("checker profile does not authorize this root"));
        }
        Ok(Some((profile, hash(&raw))))
    }

    fn inspect(
        &self,
        root: &Path,
        root_identity: &str,
        revision: i64,
    ) -> io::Result<Option<Observed>> {
        let Some((profile, profile_sha256)) = self.read_profile(root_identity)? else {
            return Ok(None);
        };
        let (git, scan) = observe_workspace(root)?;
        if scan.partial {
            return Err(invalid("scheduled checker source inventory is incomplete"));
        }
        let mut inputs = Vec::new();
        for (path, source) in &scan.files {
            let language = match Path::new(path).extension().and_then(|ext| ext.to_str()) {
                Some("cs") => Language::Csharp,
                Some("py" | "pyi") => Language::Python,
                Some("rs") => Language::Rust,
                Some("fluxr") => Language::Fluxr,
                _ => continue,
            };
            // The one-shot checker does not admit empty source documents.
            if source.length > 0 {
                inputs.push(Input {
                    path: path.clone(),
                    language,
                    digest: source.digest.clone(),
                    size: source.length,
                });
            }
        }
        if inputs.len() > MAX_SOURCES
            || inputs.iter().map(|item| item.size).sum::<u64>() > MAX_TOTAL_BYTES as u64
        {
            return Err(invalid(
                "scheduled checker source closure exceeds its budget",
            ));
        }
        let ledger: Vec<_> = scan
            .files
            .iter()
            .map(|(path, item)| (path, &item.digest, item.length))
            .collect();
        let scope = hash(
            &serde_json::to_vec(&(
                slophawk_native_checks::SEMANTICS_VERSION,
                revision,
                root_identity,
                git.branch,
                git.commit,
                git.metadata_identity,
                &profile_sha256,
                ledger,
            ))
            .map_err(io::Error::other)?,
        );
        Ok(Some(Observed {
            profile,
            profile_sha256,
            scope,
            inputs,
        }))
    }

    pub(super) fn admit(
        &mut self,
        root: &Path,
        root_identity: &str,
        epoch: &str,
        revision: i64,
    ) -> Option<Work> {
        self.error = None;
        self.active_scope = None;
        match self.prepare(root, root_identity, epoch, revision) {
            Ok(work) => work,
            Err(error) => {
                self.enabled = true;
                self.error = Some(error.to_string());
                None
            }
        }
    }

    fn prepare(
        &mut self,
        root: &Path,
        root_identity: &str,
        epoch: &str,
        revision: i64,
    ) -> io::Result<Option<Work>> {
        let observed = self.inspect(root, root_identity, revision)?;
        self.enabled = observed.is_some();
        let Some(observed) = observed else {
            return Ok(None);
        };
        self.active_scope = Some(observed.scope.clone());
        if observed.inputs.is_empty()
            || self.latest.as_ref().is_some_and(|latest| {
                latest.scope_sha256 == observed.scope
                    && !matches!(latest.outcome, Outcome::Interrupted | Outcome::Running)
            })
        {
            return Ok(None);
        }
        let (sources, worker) = observed.prepare(root)?;
        let attempt = super::resident_store::nonce()?;
        let request = ScopeRequest {
            version: 1,
            workspace_epoch: epoch.into(),
            committed_revision: format!("{revision}-{}", observed.scope),
            workspace_root: path_text(root)?,
            linked_roots: Vec::new(),
            context: observed.profile.context,
            sources,
            worker,
            limits: Limits::default(),
            near_miss_clones: observed.profile.near_miss_clones,
        };
        let record = Record {
            attempt: attempt.clone(),
            scope_sha256: observed.scope,
            outcome: Outcome::Running,
            report: None,
            error: None,
            sources: request
                .sources
                .iter()
                .map(|source| (source.document_id.clone(), source.logical_path.clone()))
                .collect(),
        };
        let receipt_sha256 = self.write(&record)?;
        self.latest = Some(record);
        self.receipt_sha256 = Some(receipt_sha256);
        Ok(Some(Work { attempt, request }))
    }

    pub(super) fn finish(
        &mut self,
        root: &Path,
        root_identity: &str,
        revision: i64,
        finished: Finished,
    ) -> io::Result<()> {
        let Some(running) = &self.latest else {
            return Err(invalid("scheduled attempt disappeared"));
        };
        if running.attempt != finished.attempt || running.outcome != Outcome::Running {
            return Err(invalid("scheduled attempt is no longer current"));
        }
        let current = self
            .inspect(root, root_identity, revision)
            .and_then(|observed| {
                if let Some(observed) = observed {
                    observed.verify_worker(root)?;
                    return Ok(Some(observed.scope));
                }
                Ok(None)
            });
        let (outcome, report, error) = match finished.result {
            _ if current.as_ref().ok().and_then(|scope| scope.as_deref())
                != Some(running.scope_sha256.as_str()) =>
            {
                (
                    Outcome::Interrupted,
                    None,
                    Some("source_or_grant_changed_before_receipt".into()),
                )
            }
            Ok(report) => {
                let outcome = match report["status"].as_str() {
                    Some("complete") => Outcome::Complete,
                    Some("partial" | "unsupported") => Outcome::Partial,
                    _ => Outcome::Failed,
                };
                (outcome, Some(report), None)
            }
            Err(error) => (
                Outcome::Failed,
                None,
                Some(error.chars().take(240).collect()),
            ),
        };
        let record = Record {
            attempt: finished.attempt,
            scope_sha256: running.scope_sha256.clone(),
            outcome,
            report,
            error,
            sources: running.sources.clone(),
        };
        let receipt_sha256 = self.write(&record)?;
        self.latest = Some(record);
        self.receipt_sha256 = Some(receipt_sha256);
        Ok(())
    }

    pub(super) fn status(&self) -> Option<Value> {
        if !self.enabled {
            return None;
        }
        if let Some(error) = &self.error {
            return Some(json!({"state":"blocked","error":error}));
        }
        let current = self
            .latest
            .as_ref()
            .filter(|item| Some(&item.scope_sha256) == self.active_scope.as_ref());
        Some(match current {
            Some(record) => {
                json!({"state":record.outcome,"attempt":record.attempt,"scopeSha256":record.scope_sha256,
                "error":record.error,"report":record.report,"sources":record.sources})
            }
            None => json!({"state":"idle"}),
        })
    }

    pub(super) fn read_fence(&self, identity: &str) -> Option<CheckerFence> {
        let receipt_sha256 = self.receipt_sha256.as_ref()?;
        let (_, profile_sha256) = self.read_profile(identity).ok()??;
        Some(CheckerFence { version: 1, profile_sha256, receipt_sha256: receipt_sha256.clone() })
    }

    pub(super) fn current_report(&self, root: &Path, identity: &str, revision: i64)
        -> Option<(&Value, CheckerFence)> {
        // Avoid a full scan when there is no successful receipt to project.
        let record = self.latest.as_ref()?;
        if record.report.is_none() || !matches!(record.outcome, Outcome::Complete | Outcome::Partial) {
            return None;
        }
        let observed = self.inspect(root, identity, revision).ok()??;
        self.report_for_observation(root, identity, revision, &observed)
    }

    fn report_for_observation(&self, root: &Path, identity: &str, revision: i64, observed: &Observed)
        -> Option<(&Value, CheckerFence)> {
        let record = self.latest.as_ref()?;
        let report = record.report.as_ref()?;
        // A digest identifies bytes; it cannot make inconsistent receipt
        // headers applicable. These fields are supplied by the native producer
        // and are present in existing durable receipts, with no schema change.
        let committed_revision = format!("{revision}-{}", record.scope_sha256);
        if report.get("schema").and_then(Value::as_str) != Some("slophawk.native-shadow/1")
            || report.get("workspaceEpoch").and_then(Value::as_str) != Some(self.epoch.as_str())
            || report.get("committedRevision").and_then(Value::as_str) != Some(committed_revision.as_str())
            || !matches!((record.outcome, report.get("status").and_then(Value::as_str)),
                (Outcome::Complete, Some("complete")) | (Outcome::Partial, Some("partial" | "unsupported"))) {
            return None;
        }
        if observed.scope != record.scope_sha256 || observed.verify_worker(root).is_err() {
            return None;
        }
        // Bind the projection to the profile actually used by inspect, not a
        // replacement read after the scan. External profile writers do not
        // hold the workspace mutex. Detect a replacement still present after
        // validation; the public reader then checks both surrounding IPC reads.
        // Content equality does not prove an unobserved revoke/restore history.
        let fence = self.read_fence(identity)?;
        if fence.profile_sha256 != observed.profile_sha256 {
            return None;
        }
        Some((report, fence))
    }

    pub(super) fn failed(&mut self, error: io::Error) {
        self.error = Some(error.to_string());
    }
}

impl Observed {
    fn verify_worker(&self, root: &Path) -> io::Result<Option<Worker>> {
        if !self
            .inputs
            .iter()
            .any(|item| item.language == Language::Csharp)
        {
            return Ok(None);
        }
        let pin = self
            .profile
            .worker
            .as_ref()
            .ok_or_else(|| invalid("C# checks require a pinned Roslyn worker"))?;
        let pack = self
            .profile
            .context
            .reference_pack
            .as_ref()
            .ok_or_else(|| invalid("C# checks require an external reference pack"))?;
        external(Path::new(pack), root, true)?;
        Ok(Some(Worker {
            dotnet: pin.dotnet.verify(root)?,
            assembly: pin.assembly.verify(root)?,
        }))
    }

    fn prepare(&self, root: &Path) -> io::Result<(Vec<Source>, Option<Worker>)> {
        let worker = self.verify_worker(root)?;
        let mut sources = Vec::with_capacity(self.inputs.len());
        for input in &self.inputs {
            let path = root.join(&input.path);
            if !directory(path.parent().expect("source under a root"))?.starts_with(root) {
                return Err(invalid("scheduled checker source escaped the checkout"));
            }
            let data = bytes(&path, MAX_SOURCE_BYTES)?;
            if data.len() as u64 != input.size || !hash(&data).eq_ignore_ascii_case(&input.digest) {
                return Err(invalid("checker source changed after discovery"));
            }
            sources.push(Source {
                language: input.language,
                document_id: hash(input.path.as_bytes()),
                logical_path: input.path.clone(),
                physical_path: path_text(&path)?,
                provenance: Provenance::Normal,
                bytes_base64: slophawk_native_checks::base64::encode(&data),
                sha256: input.digest.clone(),
                byte_mapping: "exactUtf8".into(),
            });
        }
        Ok((sources, worker))
    }
}

pub(super) struct Work {
    attempt: String,
    request: ScopeRequest,
}

pub struct Finished {
    attempt: String,
    result: Result<Value, String>,
}

pub struct Running {
    cancel: Arc<AtomicBool>,
    worker: Option<JoinHandle<Finished>>,
}

impl Running {
    pub(super) fn start(work: Work) -> io::Result<Self> {
        let cancel = Arc::new(AtomicBool::new(false));
        let worker_cancel = Arc::clone(&cancel);
        let worker = thread::Builder::new()
            .name("slophawk-checks".into())
            .spawn(move || {
                let result = slophawk_native_checks::shadow(work.request, &worker_cancel)
                    .and_then(slophawk_native_checks::bounded_report)
                    .and_then(|data| {
                        serde_json::from_slice(&data).map_err(|error| error.to_string())
                    });
                Finished {
                    attempt: work.attempt,
                    result,
                }
            })?;
        Ok(Self {
            cancel,
            worker: Some(worker),
        })
    }

    pub fn completed(&mut self) -> Option<io::Result<Finished>> {
        if !self
            .worker
            .as_ref()
            .is_some_and(|worker| worker.is_finished())
        {
            return None;
        }
        Some(
            self.worker
                .take()
                .expect("finished worker present")
                .join()
                .map_err(|_| invalid("scheduled checker thread failed")),
        )
    }
}

impl Drop for Running {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Release);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

#[cfg(all(test, any(windows, target_os = "linux")))]
mod tests {
    use super::*;

    const IDENTITY: &str = "checker-fence-fixture";
    const REVISION: i64 = 7;
    const EPOCH: &str = "11111111111111111111111111111111";

    struct Fixture { base: PathBuf, root: PathBuf, state: PathBuf }

    impl Fixture {
        fn new() -> Self {
            let base = std::env::temp_dir().join(format!(
                "slophawk-checker-fence-{}", super::super::resident_store::nonce().unwrap()));
            fs::create_dir(&base).unwrap();
            let base = base.canonicalize().unwrap();
            let root = base.join("root");
            let state = base.join("state");
            fs::create_dir(&root).unwrap();
            fs::create_dir(&state).unwrap();
            #[cfg(unix)] {
                use std::os::unix::fs::PermissionsExt;
                fs::set_permissions(&state, fs::Permissions::from_mode(0o700)).unwrap();
            }
            fs::write(root.join("source.rs"), "fn value() -> i32 { 1 }\n").unwrap();
            let fixture = Self { base, root, state };
            fixture.profile(false);
            fixture
        }

        fn profile(&self, near: bool) {
            let mut profile = json!({"version":1,"rootIdentity":IDENTITY,
                "context":{"key":"fence-test","sourceClosureComplete":true}});
            // An old input omitting both optional members must retain defaults.
            if near { profile["nearMissClones"] = json!(true); }
            store_snapshot(&self.state.join("native-checkers.json"),
                &serde_json::to_vec(&profile).unwrap(),
                &super::super::resident_store::nonce().unwrap()).unwrap();
        }

        fn scheduler(&self) -> Scheduler { Scheduler::open(&self.state, EPOCH).unwrap() }

        fn admit(&self, scheduler: &mut Scheduler) -> Work {
            scheduler.admit(&self.root, IDENTITY, EPOCH, REVISION).unwrap()
        }

        fn finish(&self, scheduler: &mut Scheduler, work: Work, status: &str) {
            // Supply a receipt at the completion boundary; do not launch a worker.
            scheduler.finish(&self.root, IDENTITY, REVISION, Finished {
                attempt: work.attempt,
                result: Ok(json!({"schema":"slophawk.native-shadow/1",
                    "workspaceEpoch":work.request.workspace_epoch,
                    "committedRevision":work.request.committed_revision,
                    "status":status,"pairs":[],"documents":[]})),
            }).unwrap();
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) { let _ = fs::remove_dir_all(&self.base); }
    }

    #[test]
    fn checker_fence_binds_admission_completion_and_old_receipt_reload() {
        let fixture = Fixture::new();
        let mut scheduler = fixture.scheduler();
        assert!(scheduler.read_fence(IDENTITY).is_none());
        let work = fixture.admit(&mut scheduler);
        assert!(!work.request.near_miss_clones);
        assert!(work.request.worker.is_none());
        let running = scheduler.read_fence(IDENTITY).unwrap();
        assert!(scheduler.current_report(&fixture.root, IDENTITY, REVISION).is_none());
        fixture.finish(&mut scheduler, work, "complete");
        let (report, completed) = scheduler.current_report(&fixture.root, IDENTITY, REVISION).unwrap();
        assert_eq!(report["status"], "complete");
        assert_eq!(report["pairs"], json!([]));
        assert_eq!(completed.profile_sha256, running.profile_sha256);
        assert_ne!(completed.receipt_sha256, running.receipt_sha256);
        assert_eq!(completed.receipt_sha256, hash(&fs::read(&scheduler.ledger).unwrap()));
        let reloaded = fixture.scheduler();
        assert_eq!(reloaded.current_report(&fixture.root, IDENTITY, REVISION).unwrap().1, completed);
    }

    #[test]
    fn checker_fence_rejects_profile_replacement_after_source_inspection() {
        let fixture = Fixture::new();
        let mut scheduler = fixture.scheduler();
        let work = fixture.admit(&mut scheduler);
        fixture.finish(&mut scheduler, work, "complete");
        let observed = scheduler.inspect(&fixture.root, IDENTITY, REVISION).unwrap().unwrap();
        let before = scheduler.read_fence(IDENTITY).unwrap();
        fixture.profile(true); // The source and committed revision are unchanged.
        let after = scheduler.read_fence(IDENTITY).unwrap();
        assert_eq!(before.receipt_sha256, after.receipt_sha256);
        assert_ne!(before.profile_sha256, after.profile_sha256);
        assert!(scheduler.report_for_observation(&fixture.root, IDENTITY, REVISION, &observed).is_none());
        assert!(scheduler.current_report(&fixture.root, IDENTITY, REVISION).is_none());
        fs::remove_file(&scheduler.profile).unwrap();
        assert!(scheduler.read_fence(IDENTITY).is_none());
    }

    #[test]
    fn checker_fence_keeps_partial_receipts_but_withdraws_changed_or_incomplete_source() {
        let fixture = Fixture::new();
        let mut scheduler = fixture.scheduler();
        let work = fixture.admit(&mut scheduler);
        fixture.finish(&mut scheduler, work, "partial");
        assert_eq!(scheduler.current_report(&fixture.root, IDENTITY, REVISION).unwrap().0["status"], "partial");
        let before = scheduler.read_fence(IDENTITY).unwrap();
        fs::write(fixture.root.join("source.rs"), "fn value() -> i32 { 2 }\n").unwrap();
        assert!(scheduler.current_report(&fixture.root, IDENTITY, REVISION).is_none());
        // A fence alone never attests source validity: status does no source scan.
        assert_eq!(scheduler.read_fence(IDENTITY).unwrap(), before);
        fs::write(fixture.root.join("large.rs"), vec![b'x'; 2 * 1024 * 1024 + 1]).unwrap();
        assert!(observe_workspace(&fixture.root).unwrap().1.partial);
        assert!(scheduler.inspect(&fixture.root, IDENTITY, REVISION).is_err());
        assert!(scheduler.current_report(&fixture.root, IDENTITY, REVISION).is_none());
    }

    #[test]
    fn checker_fence_withholds_failed_and_recovered_running_receipts() {
        let fixture = Fixture::new();
        let mut scheduler = fixture.scheduler();
        let work = fixture.admit(&mut scheduler);
        let running = scheduler.read_fence(IDENTITY).unwrap();
        drop(work);
        drop(scheduler);
        let mut recovered = fixture.scheduler();
        let interrupted = recovered.read_fence(IDENTITY).unwrap();
        assert_ne!(running.receipt_sha256, interrupted.receipt_sha256);
        assert!(recovered.current_report(&fixture.root, IDENTITY, REVISION).is_none());
        let work = fixture.admit(&mut recovered);
        fixture.finish(&mut recovered, work, "failed");
        assert!(recovered.current_report(&fixture.root, IDENTITY, REVISION).is_none());
    }

    #[test]
    fn checker_fence_missing_invalid_and_oversized_profiles_are_unavailable() {
        let fixture = Fixture::new();
        let mut scheduler = fixture.scheduler();
        let work = fixture.admit(&mut scheduler);
        fixture.finish(&mut scheduler, work, "complete");
        assert!(scheduler.read_fence("another-root").is_none());
        for data in [b"{malformed".to_vec(), vec![b' '; PROFILE_LIMIT + 1]] {
            fs::write(&scheduler.profile, data).unwrap();
            assert!(scheduler.read_fence(IDENTITY).is_none());
            assert!(scheduler.current_report(&fixture.root, IDENTITY, REVISION).is_none());
        }
        fs::remove_file(&scheduler.profile).unwrap();
        assert!(scheduler.read_fence(IDENTITY).is_none());
    }

    #[test]
    fn checker_fence_does_not_bless_inconsistent_loaded_receipt_headers() {
        let fixture = Fixture::new();
        let mut scheduler = fixture.scheduler();
        let work = fixture.admit(&mut scheduler);
        fixture.finish(&mut scheduler, work, "complete");
        let ledger = scheduler.ledger.clone();
        let original: Value = serde_json::from_slice(&fs::read(&ledger).unwrap()).unwrap();
        drop(scheduler);
        for key in ["schema", "workspaceEpoch", "committedRevision", "status"] {
            for replacement in [None, Some(json!("wrong"))] {
                let mut record = original.clone();
                let report = record["report"].as_object_mut().unwrap();
                match replacement {
                    Some(value) => { report.insert(key.into(), value); }
                    None => { report.remove(key); }
                }
                fs::write(&ledger, serde_json::to_vec(&record).unwrap()).unwrap();
                let loaded = fixture.scheduler();
                assert!(loaded.read_fence(IDENTITY).is_some());
                assert!(loaded.current_report(&fixture.root, IDENTITY, REVISION).is_none(), "{key}");
            }
        }
    }
}
