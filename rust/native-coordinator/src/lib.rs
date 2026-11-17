//! Bounded native observation coordinator. No compiler or repository code is
//! executed by the automatic path; .NET daemon state and IPC remain owned by
//! the shipped C# daemon until an explicitly verified transfer.
mod git;
mod protocol;
pub mod execution;
mod scan;
mod sha256;
mod store;
mod supervisor;
pub mod ipc;

pub use git::GitSnapshot;
pub use protocol::{JobIdentity, JobRequest, Outcome, WorkerFrame, JOB_PROTOCOL,
    MAX_FRAME as MAX_JOB_FRAME, checked_output, checked_request, read_frame, write_frame};

pub use scan::{Coalescer, Delta, Event, Snapshot};
pub use store::{Attempt, Intent, Receipt, Source};
pub use supervisor::{FakeMode, WorkerOutcome};

use scan::{inventory_digest, root_identity};
use std::fs;
use std::io::{self, ErrorKind, Read};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Instant;
use store::{atomic_replace_checked, invalid, state_directory, withdraw, State};
use supervisor::{heavy_slot, private_dir, Lock};

const PRODUCER: &str = "SlopHawk-Native-Coordinator";
const REPORT_PREFIX: &[u8] = b"{\"producer\":\"SlopHawk-Native-Coordinator\"";
const MAX_INTENTS: usize = 2_048;
const MAX_HISTORY: usize = 64;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Fence {
    pub generation: u64,
    pub epoch: u64,
    pub sequence: u64,
    pub revision: u64,
    pub input_digest: String,
    pub root_identity: String,
    pub git_identity: String,
    pub policy_digest: String,
}

pub struct Admission {
    _slot: Lock,
    pub attempt_id: u64,
    pub causes: Vec<u64>,
    pub fence: Fence,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FixtureResult { Complete, Fail, Cancel }

/// The only owner of the fixture projection, guarded by an OS singleton lock.
/// `volatile_cache` is never written to the evidence store.
pub struct Coordinator {
    root: PathBuf,
    state_dir: PathBuf,
    state_dir_id: String,
    _writer: Lock,
    state: State,
    queue: Coalescer,
    observed_sequence: Arc<AtomicU64>,
    volatile_cache: Vec<(String, String)>,
}

impl Coordinator {
    pub fn open(path: impl AsRef<Path>) -> io::Result<Self> {
        let root = fs::canonicalize(path)?;
        let identity = root_identity(&root)?;
        let git = git::observe(&root)?;
        let dir = state_directory(&root);
        private_dir(&dir)?;
        let dir_id = root_identity(&dir)?;
        let writer = Lock::try_acquire(&dir.join("writer.lock"))?;
        let mut state = State::load(&dir)?.unwrap_or_else(|| State::empty(identity.clone()));
        if state.root_id != identity {
            // Physical checkout replacement retires, rather than inheriting,
            // outstanding causes and findings from an unrelated root.
            let mut fresh = State::empty(identity.clone());
            fresh.generation = state.generation;
            fresh.epoch = state.epoch;
            fresh.sequence = state.sequence;
            fresh.revision = state.revision;
            fresh.retired_causes = state.retired_causes
                .saturating_add(state.pending.len())
                .saturating_add(usize::from(state.fallback.is_some()));
            state = fresh;
        }
        state.branch = git.branch;
        state.commit = git.commit;
        state.git_identity = git.metadata_identity;
        // An old process must not resurrect a raw report from before this
        // incarnation. Withdraw while holding the singleton writer lease.
        withdraw(&dir, "results.json", REPORT_PREFIX)?;
        state.generation = plus(state.generation)?;
        state.epoch = plus(state.epoch)?;
        state.sequence = plus(state.sequence)?;
        let mut unknown_scope = false;
        for attempt in &mut state.attempts {
            if attempt.status == "running" || attempt.status == "admitted" {
                // A PID and start tick in repository-writable state is not
                // authenticated process-scope ownership. Never kill or reap
                // from that record. Only a never-started admitted attempt
                // (pid=0) can be safely marked interrupted here.
                let never_started = attempt.pid == 0;
                attempt.status = if never_started { "interrupted" } else { "scope_uncertain" }.into();
                attempt.reason = if never_started { "interrupted_before_launch" }
                    else { "unverified_process_scope_on_restart" }.into();
                unknown_scope |= !never_started;
            }
        }
        state.save(&dir)?;
        let seq = Arc::new(AtomicU64::new(state.sequence));
        let mut coordinator = Self {
            root, state_dir: dir, state_dir_id: dir_id,
            _writer: writer, state, queue: Coalescer::default(),
            observed_sequence: seq, volatile_cache: Vec::new(),
        };
        // Startup observation never invents a new automatic source intent.
        // Prior pending cause IDs survive the new process generation intact.
        coordinator.reconcile_inner(true, &[])?;
        if unknown_scope { coordinator.withdraw()?; }
        Ok(coordinator)
    }

    pub fn root(&self) -> &Path { &self.root }
    pub fn state_path(&self) -> PathBuf { self.state_dir.join("state.v2") }
    pub fn report_path(&self) -> PathBuf { self.state_dir.join("results.json") }
    pub fn sequence_clock(&self) -> Arc<AtomicU64> { Arc::clone(&self.observed_sequence) }
    pub fn pending(&self) -> &[Intent] { &self.state.pending }
    pub fn attempts(&self) -> &[Attempt] { &self.state.attempts }
    pub fn inventory(&self) -> &std::collections::BTreeMap<String, Source> { &self.state.inventory }
    pub fn epoch(&self) -> u64 { self.state.epoch }
    pub fn generation(&self) -> u64 { self.state.generation }
    pub fn revision(&self) -> u64 { self.state.revision }
    pub fn partial(&self) -> bool { self.state.partial }
    pub fn queue_len(&self) -> usize { self.queue.len() }
    pub fn uncertainty(&self) -> &std::collections::BTreeSet<String> {
        &self.state.uncertain_scopes
    }

    /// Bounded polling fallback. File-system notices are hints; even with no
    /// notices a changed Git binding or bytes invalidates current output.
    /// A producer should call this at a finite cadence (CLI: 750 ms).
    pub fn poll(&mut self) -> io::Result<bool> {
        if root_identity(&self.root)? != self.state.root_id {
            self.withdraw()?;
            return Err(invalid("physical workspace root replaced"));
        }
        let git = git::observe(&self.root)?;
        let is_git_change = git.branch != self.state.branch ||
            git.commit != self.state.commit ||
            git.metadata_identity != self.state.git_identity;
        let observed = scan::scan(&self.root)?;
        let (projected, _) = scan::reconcile(&self.state.inventory, &observed, &[]);
        let changed = projected != self.state.inventory ||
            observed.uncertain_scopes != self.state.uncertain_scopes;
        if is_git_change {
            self.notify(Event::Git { branch: git.branch, commit: git.commit }, Instant::now())?;
        } else if changed {
            self.notify(Event::Overflow, Instant::now())?;
        } else { return Ok(false); }
        self.reconcile_now()?;
        Ok(true)
    }

    fn state_dir_matches(&self) -> io::Result<bool> {
        Ok(root_identity(&self.state_dir)? == self.state_dir_id)
    }
    fn withdraw(&self) -> io::Result<()> {
        if !self.state_dir_matches()? {
            return Err(invalid("native state directory replaced"));
        }
        withdraw(&self.state_dir, "results.json", REPORT_PREFIX)
    }

    /// Events and Git changes invalidate before the 300ms/2s coalescing window.
    /// Paths are hint-only; reconciliation verifies old and new digests.
    pub fn notify(&mut self, event: Event, at: Instant) -> io::Result<()> {
        match &event {
            Event::Add(p) | Event::Change(p) | Event::Delete(p) => {
                if !store::valid_relative(p) { return Err(invalid("unsafe event path")); }
            }
            Event::Rename { from, to } => {
                if !store::valid_relative(from) || !store::valid_relative(to) {
                    return Err(invalid("unsafe rename paths"));
                }
            }
            Event::Git { .. } | Event::Overflow => {}
        }
        self.withdraw()?;
        // The stdin callback may have already advanced this atomic clock
        // while an admitted worker was running. Never decrease that fence.
        let mut next = self.state.clone();
        next.sequence = self.observed_sequence.fetch_add(1, Ordering::AcqRel)
            .checked_add(1).ok_or_else(|| invalid("invalidation clock exhausted"))?;
        if matches!(event, Event::Git { .. }) {
            // Supplied event labels are never trusted as Git authority. They
            // only force an epoch transition; reconciliation reads live HEAD.
            next.epoch = plus(next.epoch)?;
        }
        next.save(&self.state_dir)?;
        self.state = next;
        self.queue.offer(event, at);
        self.volatile_cache.clear();
        Ok(())
    }

    pub fn tick(&mut self, at: Instant) -> io::Result<bool> {
        if !self.queue.due(at) { return Ok(false); }
        let hints = self.queue.take();
        self.reconcile_inner(false, &hints)?;
        Ok(true)
    }
    pub fn recheck(&mut self) -> io::Result<Vec<Delta>> {
        self.notify(Event::Overflow, Instant::now())?;
        let hints = self.queue.take();
        self.reconcile_inner(false, &hints)
    }
    pub fn reconcile_now(&mut self) -> io::Result<Vec<Delta>> {
        let hints = self.queue.take();
        self.reconcile_inner(false, &hints)
    }

    fn reconcile_inner(&mut self, baseline: bool, hints: &[Event]) -> io::Result<Vec<Delta>> {
        let begin_seq = self.observed_sequence.load(Ordering::Acquire);
        if begin_seq != self.state.sequence {
            return Err(io::Error::new(ErrorKind::WouldBlock, "unprocessed notification"));
        }
        let begin_id = root_identity(&self.root)?;
        if begin_id != self.state.root_id { self.withdraw()?; return Err(invalid("root replaced")); }
        let begin_git = git::observe(&self.root)?;
        let seen = scan::scan(&self.root)?;
        if self.observed_sequence.load(Ordering::Acquire) != begin_seq ||
            root_identity(&self.root)? != begin_id ||
            git::observe(&self.root)? != begin_git {
            self.withdraw()?;
            return Err(io::Error::new(ErrorKind::WouldBlock, "stale scan discarded"));
        }
        let (files, changes) = scan::reconcile(&self.state.inventory, &seen, hints);
        let mut next = self.state.clone();
        // First stable observation is a baseline. A cross-incarnation edit
        // never becomes a new test intent, but existing intents are retained.
        if !baseline {
            for change in &changes {
                let source = if change.new_path.is_empty() { &change.old_path } else { &change.new_path };
                if !scan::is_test_trigger(source) && !scan::is_test_trigger(&change.old_path) { continue; }
                if next.pending.len() >= MAX_INTENTS {
                    let next_revision = plus(next.revision)?;
                    let first = next.fallback.map_or(next_revision, |v| v.0);
                    next.fallback = Some((first, next_revision));
                    continue;
                }
                let id = next.id()?;
                next.pending.push(Intent {
                    id, revision: plus(next.revision)?, kind: change.kind.to_string(),
                    old_path: change.old_path.clone(), new_path: change.new_path.clone(),
                    old_digest: change.old_digest.clone(), new_digest: change.new_digest.clone(),
                });
            }
        }
        if begin_git.metadata_identity != next.git_identity {
            next.epoch = plus(next.epoch)?;
        }
        next.branch = begin_git.branch.clone();
        next.commit = begin_git.commit.clone();
        next.git_identity = begin_git.metadata_identity.clone();
        next.inventory = files;
        next.uncertain_scopes = seen.uncertain_scopes;
        next.partial = seen.partial;
        next.revision = plus(next.revision)?;
        if !self.state_dir_matches()? {
            return Err(invalid("state writer lost its physical directory"));
        }
        if self.observed_sequence.load(Ordering::Acquire) != begin_seq ||
           root_identity(&self.root)? != begin_id ||
           git::observe(&self.root)? != begin_git {
            self.withdraw()?;
            return Err(io::Error::new(ErrorKind::WouldBlock, "stale commit discarded"));
        }
        next.save(&self.state_dir)?; // inventory, uncertainty and causes atomically
        self.state = next;
        self.publish()?;
        Ok(changes)
    }

    /// A user-scoped worker lock and a durable admitted attempt are acquired
    /// together before the fake subprocess is allowed to start.
    pub fn admit(&mut self) -> io::Result<Option<Admission>> {
        if self.state.pending.is_empty() && self.state.fallback.is_none() { return Ok(None); }
        if self.state.attempts.iter().any(|a| a.status == "scope_uncertain") {
            return Err(io::Error::new(ErrorKind::WouldBlock, "worker_scope_uncertain"));
        }
        let slot = heavy_slot()?;
        self.check_live()?;
        let causes = self.state.pending.iter().map(|cause| cause.id).collect::<Vec<_>>();
        let fence = self.fence();
        let mut next = self.state.clone();
        let id = next.id()?;
        next.attempts.push(Attempt {
            id, generation: fence.generation, epoch: fence.epoch, revision: fence.revision,
            pid: 0, start_ticks: 0, status: "admitted".into(), reason: String::new(),
            causes: causes.clone(),
        });
        if next.attempts.len() > MAX_HISTORY { next.attempts.remove(0); }
        next.save(&self.state_dir)?;
        self.state = next;
        Ok(Some(Admission { _slot: slot, attempt_id: id, causes, fence }))
    }

    pub fn mark_worker_started(&mut self, admitted: &mut Admission, pid: u32, start_ticks: u64) -> io::Result<()> {
        let mut next = self.state.clone();
        let item = next.attempts.iter_mut().find(|v| v.id == admitted.attempt_id)
            .ok_or_else(|| invalid("unknown admitted attempt"))?;
        if item.status != "admitted" { return Err(invalid("attempt already started")); }
        // Persist the per-user reservation FIRST. A crash between the two
        // durable writes cannot allow another workspace to steal a live slot.
        admitted._slot.record_worker(pid, start_ticks)?;
        item.pid = pid;
        item.start_ticks = start_ticks;
        item.status = "running".into();
        next.save(&self.state_dir)?;
        self.state = next;
        Ok(())
    }

    pub fn finish(&mut self, admitted: Admission, result: FixtureResult) -> io::Result<bool> {
        let (status, coverage, reason) = match result {
            FixtureResult::Complete => ("completed", "complete", ""),
            FixtureResult::Fail => ("failed_to_run", "blocked", "fake_worker_failure"),
            FixtureResult::Cancel => ("cancelled", "partial", "worker_cancelled"),
        };
        self.finish_status(admitted, status, coverage, reason)
    }
    fn finish_status(&mut self, mut admitted: Admission, status: &str, coverage: &str, reason: &str) -> io::Result<bool> {
        let live = self.is_fence_live(&admitted.fence)?;
        let mut next = self.state.clone();
        let item = next.attempts.iter_mut().find(|v| v.id == admitted.attempt_id)
            .ok_or_else(|| invalid("missing attempt"))?;
        if item.status != "admitted" && item.status != "running" {
            return Err(invalid("attempt is not active"));
        }
        item.status = if live { status } else { "superseded" }.into();
        item.reason = if live { reason } else { "stale_epoch_or_inputs" }.into();
        if live {
            next.receipt = Some(Receipt {
                attempt: admitted.attempt_id, revision: admitted.fence.revision,
                outcome: status.into(), coverage: coverage.into(), reason: reason.into(),
                input_digest: admitted.fence.input_digest.clone(),
            });
            if status == "completed" && coverage == "complete" {
                next.pending.retain(|cause| !admitted.causes.contains(&cause.id));
                // Fallback is retained: a detailed selection cannot resolve a
                // workspace-wide fallback cause without a matching scope proof.
            }
        }
        next.save(&self.state_dir)?; // disposition and receipt commit together
        self.state = next;
        admitted._slot.clear_worker()?;
        if live { self.publish()?; }
        Ok(live && status == "completed" && coverage == "complete")
    }

    pub fn run_fake(&mut self, mode: FakeMode) -> io::Result<Option<(bool, WorkerOutcome)>> {
        let mut admitted = match self.admit()? { Some(value) => value, None => return Ok(None) };
        let input = admitted.fence.input_digest.clone();
        let sequence = admitted.fence.sequence;
        let clock = Arc::clone(&self.observed_sequence);
        let outcome = supervisor::run_fake(mode, &input, sequence, &clock,
            |pid, started| self.mark_worker_started(&mut admitted, pid, started));
        match outcome {
            Ok(outcome) => {
                let result = match outcome.status {
                    "completed" => FixtureResult::Complete,
                    "cancelled" | "timed_out" => FixtureResult::Cancel,
                    _ => FixtureResult::Fail,
                };
                let accepted = self.finish(admitted, result)?;
                Ok(Some((accepted, outcome)))
            }
            Err(error) => {
                self.finish(admitted, FixtureResult::Fail)?;
                Err(error)
            }
        }
    }

    pub fn fence(&self) -> Fence {
        Fence {
            generation: self.state.generation, epoch: self.state.epoch,
            sequence: self.state.sequence, revision: self.state.revision,
            input_digest: inventory_digest(&self.state.inventory),
            root_identity: self.state.root_id.clone(),
            git_identity: self.state.git_identity.clone(),
            policy_digest: sha256::digest(b"source-inventory/v2"),
        }
    }
    fn is_fence_live(&self, expected: &Fence) -> io::Result<bool> {
        if self.fence() != *expected || self.observed_sequence.load(Ordering::Acquire) != expected.sequence ||
            root_identity(&self.root)? != self.state.root_id ||
            git::observe(&self.root)?.metadata_identity != self.state.git_identity {
            return Ok(false);
        }
        let observed = scan::scan(&self.root)?;
        if observed.partial || self.state.partial { return Ok(false); }
        Ok(observed.files == self.state.inventory && self.observed_sequence.load(Ordering::Acquire) == expected.sequence)
    }
    fn check_live(&self) -> io::Result<()> {
        if self.is_fence_live(&self.fence())? { Ok(()) }
        else { Err(io::Error::new(ErrorKind::WouldBlock, "inputs changed before admission")) }
    }

    fn publish(&mut self) -> io::Result<()> {
        let fence = self.fence();
        // State and report use separate atomic replacements. A reader must
        // validate both against live inputs; startup withdraws orphan output.
        if self.observed_sequence.load(Ordering::Acquire) != fence.sequence ||
            root_identity(&self.root)? != self.state.root_id ||
            !self.state_dir_matches()? ||
            git::observe(&self.root)?.metadata_identity != self.state.git_identity {
            return Ok(());
        }
        self.state.projection = plus(self.state.projection)?;
        self.state.save(&self.state_dir)?;
        let report = self.report_json();
        if report.len() > 8 * 1024 * 1024 { return Err(invalid("report exceeds budget")); }
        let observed = scan::scan(&self.root)?;
        if !self.scan_agrees(&observed) { return Ok(()); }
        if self.fence() != fence || self.observed_sequence.load(Ordering::Acquire) != fence.sequence ||
            root_identity(&self.root)? != self.state.root_id ||
            !self.state_dir_matches()? ||
            git::observe(&self.root)?.metadata_identity != self.state.git_identity {
            return Ok(());
        }
        atomic_replace_checked(&self.state_dir, "results.json", report.as_bytes(), REPORT_PREFIX, || {
            Ok(self.fence() == fence &&
                self.observed_sequence.load(Ordering::Acquire) == fence.sequence &&
                root_identity(&self.root)? == self.state.root_id &&
                self.state_dir_matches()? &&
                git::observe(&self.root)?.metadata_identity == self.state.git_identity &&
                self.scan_agrees(&scan::scan(&self.root)?))
        })
    }
    fn scan_agrees(&self, observed: &Snapshot) -> bool {
        if !observed.partial && !self.state.partial { observed.files == self.state.inventory }
        else {
            observed.uncertain_scopes == self.state.uncertain_scopes &&
            observed.files.iter().all(|(p, f)| self.state.inventory.get(p) == Some(f)) &&
            self.state.inventory.iter().all(|(p, f)| {
                observed.files.get(p) == Some(f) ||
                scan::is_uncertain(&observed.uncertain_scopes, p)
            })
        }
    }

    pub fn read_current(&self) -> io::Result<Option<String>> {
        if !self.state_dir_matches()? { return Ok(None); }
        let path = self.report_path();
        let meta = match fs::symlink_metadata(&path) {
            Ok(meta) => meta,
            Err(error) if error.kind() == ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error),
        };
        if !meta.file_type().is_file() || meta.len() > 8 * 1024 * 1024 {
            return Err(invalid("report must be a bounded ordinary file"));
        }
        let mut raw = Vec::new();
        fs::File::open(&path)?.take(8 * 1024 * 1024 + 1).read_to_end(&mut raw)?;
        if raw.len() as u64 != meta.len() { return Ok(None); }
        if !raw.starts_with(REPORT_PREFIX) { return Err(invalid("foreign or invalid report")); }
        let fence = self.fence();
        if root_identity(&self.root)? != self.state.root_id ||
            git::observe(&self.root)?.metadata_identity != self.state.git_identity ||
            self.observed_sequence.load(Ordering::Acquire) != fence.sequence ||
            !self.scan_agrees(&scan::scan(&self.root)?) { return Ok(None); }
        let expected = self.report_json();
        if raw != expected.as_bytes() ||
           self.observed_sequence.load(Ordering::Acquire) != fence.sequence ||
           root_identity(&self.root)? != self.state.root_id ||
           git::observe(&self.root)?.metadata_identity != self.state.git_identity ||
           !self.state_dir_matches()? {
            return Ok(None);
        }
        Ok(Some(expected))
    }

    pub fn status_json(&self) -> String {
        let (coverage, reason) = self.coverage();
        let pending = self.state.pending.len();
        let (oldest, latest) = self.state.pending.iter().fold((None, None), |(a, b), v| {
            (Some(a.unwrap_or(v.revision).min(v.revision)), Some(b.unwrap_or(v.revision).max(v.revision)))
        });
        let fallback = match self.state.fallback {
            Some((a, b)) => format!("{{\"scope\":\"workspace\",\"firstRevision\":{a},\"lastRevision\":{b}}}"),
            None => "null".into(),
        };
        let active = self.state.attempts.iter().any(|a| a.status == "running" || a.status == "admitted");
        format!("{{\"root\":{},\"epoch\":{},\"revision\":{},\"committedRevision\":{},\"reportSchemaVersion\":2,\"state\":{},\"fileCount\":{},\"partial\":{},\"testScheduler\":{{\"state\":\"awaiting_adapter\"}},\"pendingTestIntents\":{{\"state\":{},\"detailedCount\":{pending},\"oldestRevision\":{},\"latestRevision\":{},\"fallback\":{fallback}}},\"coverage\":{{\"state\":{},\"reason\":{}}},\"admitted\":{},\"generation\":{}}}",
            quote(&self.root.display().to_string()), quote(&epoch_id(self.state.epoch)), self.state.sequence,
            self.state.revision, quote(if self.state.partial ||
                self.state.attempts.iter().any(|a| a.status == "scope_uncertain") {
                "pending" } else { "ready" }),
            self.state.inventory.len(), self.state.partial,
            quote(if pending > 0 || self.state.fallback.is_some() { "awaiting_selection" } else { "none" }),
            json_option(oldest), json_option(latest), quote(coverage), quote(reason), active, self.state.generation)
    }

    /// The isolated schema-v2 projection is deliberately not `.slophawk/results.json`.
    /// No adapter is registered and `list_checkers` remains empty. Contexts
    /// are provisional discovery facts, never inferred impact edges.
    pub fn report_json(&self) -> String {
        let (coverage, reason) = self.coverage();
        let receipt = match self.state.receipt.as_ref().filter(|r| r.revision == self.state.revision) {
            Some(r) => format!("{{\"attempt\":{},\"revision\":{},\"outcome\":{},\"coverage\":{},\"reason\":{},\"inputFingerprint\":{}}}",
                r.attempt, r.revision, quote(&r.outcome), quote(&r.coverage), quote(&r.reason), quote(&r.input_digest)),
            None => "null".into(),
        };
        let sources = self.state.inventory.iter().filter_map(|(path, source)| {
            let language = scan::language(path)?;
            let context = scan::context(path, &self.state.inventory)?;
            Some(format!("{{\"path\":{},\"digest\":{},\"language\":{},\"context\":{}}}",
                quote(path), quote(&source.digest), quote(language), quote(&context)))
        }).collect::<Vec<_>>().join(",");
        let scopes = self.state.uncertain_scopes.iter()
            .map(|s| quote(s)).collect::<Vec<_>>().join(",");
        format!("{{\"producer\":{},\"schemaVersion\":2,\"root\":{},\"incarnation\":{},\"epoch\":{},\"revision\":{},\"committedRevision\":{},\"configurationRevision\":1,\"projectionRevision\":{},\"branch\":{},\"commit\":{},\"fileCount\":{},\"partial\":{},\"sourceInventory\":{{\"schemaVersion\":2,\"policy\":\"source-inventory/v2\",\"files\":[{}],\"uncertainScopes\":[{}],\"crossLanguageImpact\":false,\"executionAuthorized\":false}},\"findings\":[],\"checkers\":[],\"nativeCoordinator\":{{\"coverage\":{{\"state\":{},\"reason\":{}}},\"fixtureReceipt\":{}}},\"projectionTruncated\":false}}",
            quote(PRODUCER), quote(&self.root.display().to_string()), quote(&format!("native-{}", self.state.generation)),
            quote(&epoch_id(self.state.epoch)), self.state.sequence, self.state.revision,
            self.state.projection, nullable(&self.state.branch), nullable(&self.state.commit),
            self.state.inventory.len(), self.state.partial, sources, scopes,
            quote(coverage), quote(reason), receipt)
    }

    fn coverage(&self) -> (&'static str, &'static str) {
        if self.state.partial { return ("partial", "incomplete_source_enumeration"); }
        if self.state.attempts.iter().any(|a| a.status == "scope_uncertain") {
            return ("blocked", "worker_scope_uncertain");
        }
        if let Some(receipt) = self.state.receipt.as_ref().filter(|r| r.revision == self.state.revision) {
            if receipt.coverage == "complete" && self.state.pending.is_empty() && self.state.fallback.is_none() {
                // A controlled fixture outcome is not a production checker
                // or selected test receipt: never claim complete coverage.
                return ("partial", "fixture_only_no_production_checker");
            }
            if receipt.coverage == "blocked" { return ("blocked", "worker_failure"); }
            if receipt.coverage == "partial" { return ("partial", "worker_cancelled_or_incomplete"); }
        }
        if !self.state.pending.is_empty() || self.state.fallback.is_some() {
            ("pending", "awaiting_fixture_worker")
        } else { ("partial", "fixture_only_no_production_checker") }
    }

    pub fn reply_json(&self, operation: &str) -> String {
        let payload = match operation {
            "hello" => Some("{\"wireVersion\":1,\"reportSchemaVersion\":2,\"capabilities\":[\"status\",\"results\",\"recheck\",\"stop\",\"list_checkers\"]}".to_string()),
            "status" | "workspace_status" => Some(self.status_json()),
            "results" | "get_findings" => match self.read_current() {
                Ok(result) => result,
                Err(error) => return failure("publication_failed", &error.to_string()),
            },
            "list_checkers" => Some("[]".to_string()),
            _ => return failure("unknown_command", "Unknown fixture command."),
        };
        match payload {
            Some(value) => format!("{{\"ok\":true,\"payload\":{value},\"error\":null}}"),
            None => failure("no_current_report", "No validated current report."),
        }
    }
}

impl Drop for Coordinator {
    fn drop(&mut self) {
        // Old writers must never remove the new root's report after a root or
        // state-directory replacement. A reader fences stale old bytes.
        if root_identity(&self.root).ok().as_deref() == Some(self.state.root_id.as_str()) &&
           self.state_dir_matches().unwrap_or(false) {
            let _ = self.withdraw();
        }
    }
}

fn plus(value: u64) -> io::Result<u64> {
    value.checked_add(1).ok_or_else(|| invalid("revision counter exhausted"))
}
fn epoch_id(epoch: u64) -> String { format!("00000000-0000-4000-8000-{epoch:012x}") }
fn json_option(value: Option<u64>) -> String { value.map_or("null".into(), |n| n.to_string()) }
fn nullable(value: &str) -> String { if value.is_empty() { "null".into() } else { quote(value) } }
pub fn quote(value: &str) -> String {
    let mut out = String::from("\"");
    for c in value.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if c < '\u{20}' => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}
pub fn failure(code: &str, reason: &str) -> String {
    format!("{{\"ok\":false,\"payload\":null,\"error\":{},\"code\":{}}}", quote(reason), quote(code))
}

/// Controlled subprocess entry points used solely by `src/main.rs`.
pub fn fake_worker(mode: &str, input: &str) -> io::Result<i32> {
    supervisor::child_fixture(mode, input)
}
pub fn fake_grandchild() { supervisor::grandchild_fixture(); }

#[cfg(test)]
mod crash_recovery_tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn unauthenticated_running_pid_blocks_recovery_and_publication() -> io::Result<()> {
        let nonce = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();
        let root = std::env::temp_dir().join(format!("native-scope-test-{}-{nonce}", std::process::id()));
        fs::create_dir(&root)?;
        fs::write(root.join("item.cs"), "before")?;
        {
            let mut c = Coordinator::open(&root)?;
            let revision = c.revision();
            let generation = c.generation();
            let epoch = c.epoch();
            c.state.pending.push(Intent { id: 99, revision, kind: "change".into(),
                old_path: "item.cs".into(), new_path: "item.cs".into(),
                old_digest: String::new(), new_digest: String::new() });
            c.state.attempts.push(Attempt { id: 100, generation, epoch,
                revision, pid: 12_345, start_ticks: 900, status: "running".into(),
                reason: String::new(), causes: vec![99] });
            c.state.save(&c.state_dir)?;
        }
        let mut recovered = Coordinator::open(&root)?;
        assert_eq!(recovered.pending().len(), 1, "causes survive restart");
        assert!(recovered.attempts().iter().any(|a| a.status == "scope_uncertain"));
        assert!(recovered.read_current()?.is_none(), "report must stay withdrawn");
        assert!(matches!(recovered.admit(), Err(ref e) if e.kind() == ErrorKind::WouldBlock),
            "an unverified PID must never be treated as authenticated drain evidence");
        drop(recovered);
        fs::remove_dir_all(root)?;
        Ok(())
    }
}
