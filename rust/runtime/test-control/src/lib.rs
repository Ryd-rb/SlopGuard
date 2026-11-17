//! SG-MIG-11: native, *separate* v1 durable test-control partition.
//!
//! This crate does not attach to schema-v2 SourceInventory or start a daemon. A trusted
//! workspace owner supplies fences, evaluated graph facts, execution authority and a
//! contained executor. Source bytes, repository configuration and worker replies never
//! grant authority. There is deliberately no default executable runner.
//!
//! `bind` invalidates publication immediately, returning scopes which the owner must
//! revoke and drain. `admit` commits a scope *before* the peer executes. `finish` accepts
//! only an exactly bound, measured terminal after the authority has verified the owned
//! files and confirmed process-tree drainage. Reopening an interrupted store retains
//! its recovery barrier and retry history. Every state change is an IMMEDIATE SQLite
//! transaction in a private schema with FULL synchronous mode.

pub mod peer;

use rusqlite::{Connection, OptionalExtension, Transaction, TransactionBehavior, params};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use slophawk_test_impact as impact;
use slophawk_verification as verification;
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::path::Path;
use std::time::Duration;

pub const SCHEMA_VERSION: i64 = 1;
pub const PROTOCOL: &str = "slophawk.migration-job/1";
pub const MAX_FRAME: usize = 1024 * 1024;
const MAX_EXPORT: usize = 8 * 1024 * 1024;
const MAX_CAUSES: i64 = 8192;
const MAX_ATTEMPTS: i64 = 4096;
const MAX_RECEIPTS: i64 = 4096;
const MAX_EVIDENCE: i64 = 32784;
const MAX_TARGETS: usize = 16;
const MAX_ATTEMPTS_PER_PLAN: i64 = 3;

#[derive(Debug)]
pub enum ControlError {
    Sql(rusqlite::Error),
    Json(serde_json::Error),
    Io(std::io::Error),
    Selection(impact::SelectionError),
    Invalid(&'static str),
    Stale,
    Blocked(&'static str),
}
impl fmt::Display for ControlError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Sql(e) => write!(f, "store: {e}"),
            Self::Json(e) => write!(f, "bounded JSON: {e}"),
            Self::Io(e) => write!(f, "I/O: {e}"),
            Self::Selection(e) => write!(f, "selection: {e}"),
            Self::Invalid(e) => write!(f, "invalid test control: {e}"),
            Self::Stale => f.write_str("stale schedule authority"),
            Self::Blocked(e) => write!(f, "execution blocked: {e}"),
        }
    }
}
impl std::error::Error for ControlError {}
impl From<rusqlite::Error> for ControlError {
    fn from(value: rusqlite::Error) -> Self { Self::Sql(value) }
}
impl From<serde_json::Error> for ControlError {
    fn from(value: serde_json::Error) -> Self { Self::Json(value) }
}
impl From<std::io::Error> for ControlError {
    fn from(value: std::io::Error) -> Self { Self::Io(value) }
}
impl From<impact::SelectionError> for ControlError {
    fn from(value: impact::SelectionError) -> Self { Self::Selection(value) }
}
pub type Result<T> = std::result::Result<T, ControlError>;

fn hash(bytes: impl AsRef<[u8]>) -> String {
    format!("{:X}", Sha256::digest(bytes.as_ref()))
}
fn digest(s: &str) -> bool {
    s.len() == 64 && s.bytes().all(|b| b.is_ascii_hexdigit() && !b.is_ascii_lowercase())
}
fn field(s: &str, maximum: usize) -> bool {
    !s.trim().is_empty() && s.len() <= maximum && !s.contains('\0')
}
fn json<T: Serialize>(value: &T) -> Result<String> {
    let bytes = serde_json::to_vec(value)?;
    if bytes.len() > MAX_EXPORT { return Err(ControlError::Invalid("JSON quota")); }
    String::from_utf8(bytes).map_err(|_| ControlError::Invalid("UTF-8 encoder"))
}
fn from_json<T: for<'a> Deserialize<'a>>(s: &str) -> Result<T> {
    if s.len() > MAX_EXPORT { return Err(ControlError::Invalid("JSON quota")); }
    Ok(serde_json::from_str(s)?)
}

/// All fields originate from the trusted workspace owner, never the checkout.
/// Generation is a monotonically increasing writer incarnation across restarts;
/// sequence increases within it. Root/context/revision are independent identities.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Fence {
    pub root: String,
    pub context: String,
    pub epoch: String,
    pub generation: i64,
    pub sequence: i64,
    pub revision: i64,
    pub inputs: String,
    pub tool: String,
    pub policy: String,
    pub profile: String,
    pub grant: String,
    pub budget: String,
}
impl Fence {
    pub fn validate(&self) -> Result<()> {
        if !field(&self.root, 256) || !field(&self.context, 256)
            || !field(&self.epoch, 128) || !field(&self.grant, 128)
            || !field(&self.budget, 128) || self.generation <= 0
            || self.sequence < 0 || self.revision < 0
            || ![&self.inputs, &self.tool, &self.policy, &self.profile]
                .into_iter().all(|value| digest(value))
        {
            return Err(ControlError::Invalid("fence fields or exact digests"));
        }
        Ok(())
    }
    fn follows(&self, prior: &Self) -> bool {
        self.root == prior.root && self.context == prior.context
            && self.revision >= prior.revision
            && (self.generation > prior.generation
                || (self.generation == prior.generation && self.epoch == prior.epoch
                    && self.sequence > prior.sequence))
    }
}

/// Exact durable projection of the published pure selector's cause model.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Cause {
    pub revision: i64,
    pub path_key: String,
    pub old_path: Option<String>,
    pub old_digest: Option<String>,
    pub new_path: Option<String>,
    pub new_digest: Option<String>,
    pub predecessor: Option<String>,
    pub fallback_first_revision: Option<i64>,
}
impl Cause {
    pub fn id(&self) -> String { self.to_impact().id() }
    pub fn to_impact(&self) -> impact::TestCause {
        impact::TestCause {
            revision: self.revision, path_key: self.path_key.clone(),
            old_path: self.old_path.clone(), old_digest: self.old_digest.clone(),
            new_path: self.new_path.clone(), new_digest: self.new_digest.clone(),
            predecessor: self.predecessor.clone(),
            fallback_first_revision: self.fallback_first_revision,
        }
    }
    pub fn validate(&self) -> Result<()> {
        if self.revision < 0 || self.path_key.len() > 2048 || self.path_key.contains('\0')
            || [self.old_path.as_deref(), self.new_path.as_deref(), self.predecessor.as_deref()]
                .into_iter().flatten().any(|s| !field(s, 2048))
            || [self.old_digest.as_deref(), self.new_digest.as_deref()]
                .into_iter().flatten().any(|s| !digest(s))
            || (self.path_key.is_empty() && self.fallback_first_revision.is_none())
        {
            return Err(ControlError::Invalid("source cause"));
        }
        Ok(())
    }
}
impl From<&impact::TestCause> for Cause {
    fn from(x: &impact::TestCause) -> Self {
        Self { revision: x.revision, path_key: x.path_key.clone(),
            old_path: x.old_path.clone(), old_digest: x.old_digest.clone(),
            new_path: x.new_path.clone(), new_digest: x.new_digest.clone(),
            predecessor: x.predecessor.clone(), fallback_first_revision: x.fallback_first_revision }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Plan {
    pub key: String,
    pub target: String,
    pub fingerprint: String,
    pub causes: Vec<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct JobIdentity {
    pub protocol: String,
    pub operation: String,
    pub job: String,
    pub attempt: i64,
    pub number: i64,
    pub scope: String,
    pub fence: Fence,
    pub plan: Plan,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Job {
    pub identity: JobIdentity,
    /// Execution policy and output locations are granted by the trusted host,
    /// not transmitted as new authority by this request.
    pub max_response_bytes: u32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TerminalStatus { Complete, Partial, Unsupported, Deferred, Cancelled, Stale, Failed }
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunDisposition { Passed, Failed, Skipped, ZeroDiscovered, Incomplete, Unadmitted }
impl RunDisposition {
    pub fn as_verification(self) -> verification::typed::ProviderRunDisposition {
        use verification::typed::ProviderRunDisposition as P;
        match self {
            Self::Passed => P::Passed, Self::Failed => P::Failed,
            Self::Skipped => P::Skipped, Self::ZeroDiscovered => P::ZeroDiscovered,
            Self::Incomplete => P::Incomplete, Self::Unadmitted => P::Unadmitted,
        }
    }
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Counters {
    pub total: u32,
    pub passed: u32,
    pub failed: u32,
    pub skipped: u32,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RunReceipt {
    pub receipt_id: String,
    pub runner: String,
    pub disposition: RunDisposition,
    pub runner_completed: bool,
    pub artifact_complete: bool,
    pub artifact_digest: String,
    pub artifact_bytes: u64,
    pub collected_ids_digest: String,
    pub counters: Counters,
}
impl RunReceipt {
    fn validate(&self) -> Result<()> {
        let c = &self.counters;
        if !field(&self.receipt_id, 128) || !field(&self.runner, 128)
            || !self.runner_completed || !self.artifact_complete
            || !digest(&self.artifact_digest) || !digest(&self.collected_ids_digest)
            || self.artifact_bytes == 0 || self.artifact_bytes > 64 * 1024 * 1024
            || c.total == 0 || c.passed.checked_add(c.failed)
                .and_then(|x| x.checked_add(c.skipped)) != Some(c.total)
            || c.passed + c.failed == 0
            || !matches!(self.disposition, RunDisposition::Passed | RunDisposition::Failed)
            || (self.disposition == RunDisposition::Passed && c.failed != 0)
            || (self.disposition == RunDisposition::Failed && c.failed == 0)
        {
            return Err(ControlError::Invalid("no complete, counted, validated test receipt"));
        }
        Ok(())
    }
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Terminal {
    pub identity: JobIdentity,
    pub status: TerminalStatus,
    /// Distinguishes completed test failures from skips, zero discovery,
    /// incomplete runs and work never admitted. This is not evidence itself.
    pub observed: Option<RunDisposition>,
    pub reason: Option<String>,
    pub omissions: Vec<String>,
    pub receipt: Option<RunReceipt>,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AttemptObservation {
    pub status: TerminalStatus,
    pub disposition: Option<RunDisposition>,
    pub reason: Option<String>,
    pub omissions: Vec<String>,
}
impl Terminal {
    pub fn validate(&self, job: &JobIdentity) -> Result<()> {
        if &self.identity != job || self.omissions.len() > 16
            || self.reason.as_ref().is_some_and(|s| s.len() > 512)
            || (self.status != TerminalStatus::Complete
                && self.reason.as_ref().is_none_or(|s| !field(s, 512)))
            || self.omissions.iter().any(|s| !field(s, 256))
            || (self.status != TerminalStatus::Complete && self.receipt.is_some())
            || (self.status == TerminalStatus::Complete
                && (!self.omissions.is_empty() || self.receipt.is_none()
                    || self.observed != self.receipt.as_ref().map(|r| r.disposition)))
            || (self.status != TerminalStatus::Complete
                && matches!(self.observed, Some(RunDisposition::Passed | RunDisposition::Failed)))
        { return Err(ControlError::Invalid("terminal identity, completion or bounds")); }
        if let Some(r) = &self.receipt { r.validate()?; }
        Ok(())
    }
}

/// The host checks physical source bytes/profile/grant, artifact bytes, and the
/// independently owned process scope. A simulated authority is appropriate in
/// tests only. Implementations must not call into repository-controlled code.
pub trait Authority {
    fn current(&self, fence: &Fence) -> Result<()>;
    fn drained(&self, scope: &str) -> Result<()>;
    fn verify_receipt(&self, identity: &JobIdentity, receipt: &RunReceipt) -> Result<()>;
}

/// A supervisor-owned peer may execute only after a durable reservation.
/// It must watch authority changes *during* execution, revoke promptly and
/// recursively contain/drain descendants. No transport or command path is a grant.
pub trait Executor {
    fn reserve_scope(&mut self, fence: &Fence, plan: &Plan) -> Result<String>;
    fn execute(&mut self, job: &Job) -> Result<Terminal>;
    fn revoke_and_drain(&mut self, scope: &str) -> Result<()>;
    fn confirm_drained(&mut self, scope: &str) -> Result<()>;
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Admission { Ready(Job), Idle, AwaitingSelection, RetryAt(i64), Recovery(String), Exhausted, Blocked }
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BindResult { pub revoke_scopes: Vec<String>, pub newly_admitted_causes: usize }
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FinishResult { pub retired: Vec<String>, pub state: String }
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Pending {
    pub causes: Vec<Cause>,
    pub outstanding: Vec<Plan>,
    pub recovery_scopes: Vec<String>,
    pub latest: Option<AttemptObservation>,
    pub state: String,
}

// Explicitly versioned, portable migration export. A file copy of a live SQLite
// database is NOT an export; journal files must remain with the database.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RecoveryExport {
    pub schema: i64,
    pub fence: Option<Fence>,
    pub causes: Vec<(String, Cause)>,
    pub selection: Option<(Fence, bool)>,
    pub plans: Vec<Plan>,
    pub attempts: Vec<StoredAttempt>,
    pub evidence: Vec<(String, String, String, String, String)>,
    pub receipts: Vec<(String, i64, RunReceipt)>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StoredAttempt {
    pub identity: JobIdentity,
    pub state: String,
    pub retry_at: Option<i64>,
    pub drained: bool,
    pub observation: Option<AttemptObservation>,
}

pub struct Store { db: Connection }
impl Store {
    /// Caller supplies an owned, physical private state path outside the repository.
    /// `open` never touches schema-v2 C# state and refuses unknown native versions.
    pub fn open(path: &Path) -> Result<Self> {
        let db = Connection::open(path)?;
        db.busy_timeout(Duration::from_secs(2))?;
        db.execute_batch("PRAGMA foreign_keys=ON; PRAGMA synchronous=FULL;
            PRAGMA journal_mode=DELETE; PRAGMA max_page_count=16384;")?;
        let version: i64 = db.query_row("PRAGMA user_version", [], |r| r.get(0))?;
        if version != 0 && version != SCHEMA_VERSION {
            return Err(ControlError::Invalid("unsupported native test-control schema"));
        }
        if version == 0 {
            // Reject alien databases which happen to report version zero.
            let present: i64 = db.query_row(
                "SELECT count(*) FROM sqlite_master WHERE type='table' AND name NOT LIKE 'sqlite_%'",
                [], |r| r.get(0))?;
            if present != 0 { return Err(ControlError::Invalid("unknown existing database")); }
            db.execute_batch("BEGIN IMMEDIATE;
                CREATE TABLE binding (id INTEGER PRIMARY KEY CHECK (id=1), fence TEXT NOT NULL,
                    restored INTEGER NOT NULL DEFAULT 0);
                CREATE TABLE cause (id TEXT PRIMARY KEY, payload TEXT NOT NULL);
                CREATE TABLE selection (id INTEGER PRIMARY KEY CHECK (id=1), fence TEXT NOT NULL, complete INTEGER NOT NULL);
                CREATE TABLE plan (key TEXT PRIMARY KEY, target TEXT NOT NULL, fingerprint TEXT NOT NULL, causes TEXT NOT NULL);
                CREATE TABLE attempt (
                    id INTEGER PRIMARY KEY AUTOINCREMENT, plan_key TEXT NOT NULL, identity TEXT NOT NULL,
                    state TEXT NOT NULL, retry_at INTEGER, drained INTEGER NOT NULL DEFAULT 0,
                    observation TEXT);
                CREATE INDEX attempts_by_plan ON attempt(plan_key, id);
                CREATE TABLE evidence (
                    cause TEXT NOT NULL, target TEXT NOT NULL, fingerprint TEXT NOT NULL,
                    plan_key TEXT NOT NULL, receipt_id TEXT NOT NULL,
                    PRIMARY KEY(cause, target, fingerprint, plan_key),
                    FOREIGN KEY(cause) REFERENCES cause(id) ON DELETE CASCADE);
                CREATE TABLE receipt (receipt_id TEXT PRIMARY KEY, attempt_id INTEGER UNIQUE NOT NULL,
                    payload TEXT NOT NULL);
                PRAGMA user_version=1; COMMIT;")?;
        }
        let mut store = Self { db };
        // Never silently release any in-flight scope after a process dies.
        let tx = store.db.transaction_with_behavior(TransactionBehavior::Immediate)?;
        tx.execute("UPDATE attempt SET state='recovery', drained=0 WHERE state='running'", [])?;
        tx.commit()?;
        Ok(store)
    }

    fn fence_tx(tx: &Transaction<'_>) -> Result<Option<Fence>> {
        let value: Option<String> = tx.query_row("SELECT fence FROM binding WHERE id=1", [], |r| r.get(0)).optional()?;
        value.map(|text| from_json(&text)).transpose()
    }
    pub fn fence(&self) -> Result<Option<Fence>> {
        let value: Option<String> = self.db.query_row("SELECT fence FROM binding WHERE id=1", [], |r| r.get(0)).optional()?;
        value.map(|text| from_json(&text)).transpose()
    }
    fn require_fence(tx: &Transaction<'_>, fence: &Fence) -> Result<()> {
        if Self::fence_tx(tx)?.as_ref() != Some(fence) { return Err(ControlError::Stale); }
        let restored: i64 = tx.query_row("SELECT restored FROM binding WHERE id=1", [], |r| r.get(0))?;
        if restored != 0 { return Err(ControlError::Blocked("import requires fresh writer generation")); }
        Ok(())
    }
    fn all_causes(tx: &Transaction<'_>) -> Result<Vec<(String, Cause)>> {
        let mut statement = tx.prepare("SELECT id, payload FROM cause ORDER BY id")?;
        let rows = statement.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?;
        let mut causes = Vec::new();
        for row in rows {
            let (id, payload) = row?;
            let cause: Cause = from_json(&payload)?;
            if cause.id() != id { return Err(ControlError::Invalid("corrupted cause identity")); }
            causes.push((id, cause));
        }
        Ok(causes)
    }
    pub fn causes(&self) -> Result<Vec<Cause>> {
        let mut stmt = self.db.prepare("SELECT id, payload FROM cause ORDER BY id")?;
        let mut out = Vec::new();
        let rows = stmt.query_map([], |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)))?;
        for row in rows {
            let (id, value) = row?;
            let cause: Cause = from_json(&value)?;
            if cause.id() != id { return Err(ControlError::Invalid("corrupted cause identity")); }
            out.push(cause);
        }
        Ok(out)
    }

    /// Reconcile source observations together with the durable authority fence.
    /// Previously pending causes are not dropped by an incomplete source scan.
    /// Every returned scope must be revoked and drained by the owner before its
    /// output lease is released. Never call `bind` using raw repository data.
    pub fn bind(&mut self, fence: &Fence, incoming: &[Cause]) -> Result<BindResult> {
        fence.validate()?;
        if incoming.len() > MAX_CAUSES as usize { return Err(ControlError::Invalid("cause quota")); }
        let tx = self.db.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let prior = Self::fence_tx(&tx)?;
        if let Some(old) = &prior {
            if old != fence && !fence.follows(old) { return Err(ControlError::Stale); }
        }
        let restored: Option<i64> = tx.query_row("SELECT restored FROM binding WHERE id=1", [], |r| r.get(0)).optional()?;
        if restored == Some(1) && prior.as_ref().is_some_and(|old| fence.generation <= old.generation) {
            return Err(ControlError::Blocked("import requires fresh writer generation"));
        }
        let changed = prior.as_ref() != Some(fence);
        let mut scopes = Vec::new();
        if changed {
            scopes = Self::running_scopes(&tx)?;
            tx.execute("UPDATE attempt SET state='recovery', drained=0 WHERE state='running'", [])?;
            tx.execute("DELETE FROM evidence", [])?;
            tx.execute("DELETE FROM plan", [])?;
            tx.execute("DELETE FROM selection", [])?;
            tx.execute("INSERT INTO binding(id, fence, restored) VALUES(1, ?1, 0)
                ON CONFLICT(id) DO UPDATE SET fence=excluded.fence, restored=0", [json(fence)?])?;
        }
        let mut inserted = 0;
        for cause in incoming {
            cause.validate()?;
            if cause.revision > fence.revision { return Err(ControlError::Invalid("future cause revision")); }
            let id = cause.id();
            let payload = json(cause)?;
            let existing: Option<String> = tx.query_row("SELECT payload FROM cause WHERE id=?1", [&id], |r| r.get(0)).optional()?;
            if let Some(old) = existing {
                if old != payload { return Err(ControlError::Invalid("conflicting source cause")); }
            } else {
                tx.execute("INSERT INTO cause(id,payload) VALUES(?1,?2)", params![id, payload])?;
                inserted += 1;
            }
        }
        let total: i64 = tx.query_row("SELECT count(*) FROM cause", [], |r| r.get(0))?;
        if total > MAX_CAUSES { return Err(ControlError::Invalid("pending cause quota")); }
        if inserted > 0 && !changed {
            // An observation that changed without a new sequence is not a valid
            // complete selection; it must be fenced by the source owner first.
            return Err(ControlError::Stale);
        }
        tx.commit()?;
        Ok(BindResult { revoke_scopes: scopes, newly_admitted_causes: inserted })
    }

    /// Preferred source-monitor entry point: invalidate first, then revoke and
    /// drain every scope before permitting subsequent output-root reuse.
    /// If drainage fails, its recovery record remains durable and blocks work.
    pub fn bind_and_drain(&mut self, fence: &Fence, incoming: &[Cause],
        authority: &impl Authority, peer: &mut impl Executor) -> Result<BindResult> {
        let result = self.bind(fence, incoming)?;
        // Include scopes imported or recovered from a prior process, not only
        // the attempts which were running at this bind.
        for job in self.recoveries()? {
            peer.revoke_and_drain(&job.scope)?;
            authority.drained(&job.scope)?;
            self.confirm_drained(&job, authority)?;
        }
        Ok(result)
    }

    fn running_scopes(tx: &Transaction<'_>) -> Result<Vec<String>> {
        let mut stmt = tx.prepare("SELECT identity FROM attempt WHERE state='running' ORDER BY id")?;
        let rows = stmt.query_map([], |r| r.get::<_, String>(0))?;
        let mut out = Vec::new();
        for row in rows {
            let job: JobIdentity = from_json(&row?)?;
            out.push(job.scope);
        }
        Ok(out)
    }

    /// Selection always consumes the *entire current pending cause set*; a caller
    /// cannot obtain authoritative empty scope by submitting a subset.
    pub fn select(&mut self, fence: &Fence, input: &impact::SelectionInput,
        authority: &impl Authority) -> Result<(impact::TestSelection, Vec<String>)> {
        authority.current(fence)?;
        let tx = self.db.transaction_with_behavior(TransactionBehavior::Immediate)?;
        Self::require_fence(&tx, fence)?;
        let existing = Self::all_causes(&tx)?;
        let supplied = input.causes.iter().map(Cause::from)
            .map(|cause| (cause.id(), cause)).collect::<BTreeMap<_, _>>();
        if input.causes.len() != existing.len() || supplied.len() != existing.len()
            || existing.iter().any(|(id, cause)| supplied.get(id) != Some(cause))
        { return Err(ControlError::Invalid("selection omitted or changed pending cause")); }
        let selection = impact::select(input)?;
        if selection.targets.len() > MAX_TARGETS { return Err(ControlError::Invalid("selection target quota")); }
        let known = existing.iter().map(|(id, _)| id.as_str()).collect::<BTreeSet<_>>();
        let mut plans = Vec::new();
        let mut targets = BTreeSet::new();
        for target in &selection.targets {
            if !field(&target.id, 128) || !digest(&target.input_fingerprint)
                || !targets.insert(target.id.as_str()) || target.cause_ids.is_empty()
            { return Err(ControlError::Invalid("selected target identity")); }
            let causes = target.cause_ids.iter().cloned().collect::<BTreeSet<_>>();
            if causes.len() != target.cause_ids.len() || !causes.iter().all(|id| known.contains(id.as_str())) {
                return Err(ControlError::Invalid("selected cause identities"));
            }
            let causes = causes.into_iter().collect::<Vec<_>>();
            let fingerprint = hash(json(&(target.input_fingerprint.as_str(), &fence.inputs,
                &fence.tool, &fence.policy, &fence.profile, &fence.grant, &fence.budget))?);
            let key = hash(json(&(&fence.root, &fence.context, fence.revision,
                &fence.inputs, &fence.tool, &fence.policy, &fence.profile, &fence.grant,
                &fence.budget, &target.id, &fingerprint, &causes))?);
            plans.push(Plan { key, target: target.id.clone(), fingerprint, causes });
        }
        // Do not rewrite a still-running plan; selection changes require a new
        // schedule fence and drainage before their obligations can be replaced.
        let running: i64 = tx.query_row("SELECT count(*) FROM attempt WHERE state='running'", [], |r| r.get(0))?;
        if running != 0 { return Err(ControlError::Blocked("running attempt; drain before replanning")); }
        if Self::first_undrained(&tx)?.is_some() {
            return Err(ControlError::Blocked("recover interrupted scope before replanning"));
        }
        authority.current(fence)?;
        tx.execute("DELETE FROM plan", [])?;
        for plan in &plans {
            tx.execute("INSERT INTO plan(key,target,fingerprint,causes) VALUES(?1,?2,?3,?4)",
                params![plan.key, plan.target, plan.fingerprint, json(&plan.causes)?])?;
        }
        tx.execute("INSERT INTO selection(id,fence,complete) VALUES(1,?1,?2)
            ON CONFLICT(id) DO UPDATE SET fence=excluded.fence, complete=excluded.complete",
            params![json(fence)?, if selection.complete { 1_i64 } else { 0_i64 }])?;
        let retired = Self::retire(&tx)?;
        tx.commit()?;
        Ok((selection, retired))
    }

    fn plans(tx: &Transaction<'_>) -> Result<Vec<Plan>> {
        let mut stmt = tx.prepare("SELECT key,target,fingerprint,causes FROM plan ORDER BY target")?;
        let rows = stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?,
            r.get::<_, String>(2)?, r.get::<_, String>(3)?)))?;
        let mut plans = Vec::new();
        for row in rows {
            let (key,target,fingerprint,causes) = row?;
            plans.push(Plan { key,target,fingerprint,causes:from_json(&causes)? });
        }
        Ok(plans)
    }
    fn retire(tx: &Transaction<'_>) -> Result<Vec<String>> {
        let complete: Option<i64> = tx.query_row("SELECT complete FROM selection WHERE id=1", [], |r| r.get(0)).optional()?;
        if complete != Some(1) { return Ok(Vec::new()); }
        let plans = Self::plans(tx)?;
        let causes = Self::all_causes(tx)?;
        let mut retired = Vec::new();
        for (id, _) in causes {
            let required: Vec<&Plan> = plans.iter().filter(|p| p.causes.contains(&id)).collect();
            let mut satisfied = true;
            for plan in required {
                let evidence: Option<i64> = tx.query_row(
                    "SELECT 1 FROM evidence WHERE cause=?1 AND target=?2 AND fingerprint=?3 AND plan_key=?4",
                    params![id, plan.target, plan.fingerprint, plan.key], |r| r.get(0)).optional()?;
                if evidence.is_none() { satisfied = false; break; }
            }
            if satisfied {
                tx.execute("DELETE FROM cause WHERE id=?1", [&id])?;
                retired.push(id);
            }
        }
        Ok(retired)
    }

    /// Returns the first outstanding selected plan and its persisted retry gate.
    /// `scope` has to be reserved with the supervisor before `admit`; the host
    /// may not start the worker before `admit` has committed successfully.
    pub fn next(&mut self, fence: &Fence, now_ms: i64) -> Result<(Admission, Option<Plan>)> {
        let tx = self.db.transaction_with_behavior(TransactionBehavior::Immediate)?;
        Self::require_fence(&tx, fence)?;
        let causes = Self::all_causes(&tx)?.into_iter().map(|(id,_)| id).collect::<BTreeSet<_>>();
        if let Some(scope) = Self::first_undrained(&tx)? {
            return Ok((Admission::Recovery(scope),None));
        }
        if causes.is_empty() { return Ok((Admission::Idle,None)); }
        let running: i64 = tx.query_row("SELECT count(*) FROM attempt WHERE state='running'", [], |r| r.get(0))?;
        if running > 0 { return Ok((Admission::Blocked,None)); }
        let exists: Option<i64> = tx.query_row("SELECT complete FROM selection WHERE id=1", [], |r| r.get(0)).optional()?;
        if exists.is_none() { return Ok((Admission::AwaitingSelection,None)); }
        let mut retry_at: Option<i64> = None;
        let mut exhausted = false;
        let mut blocked = false;
        for plan in Self::plans(&tx)? {
            let mut outstanding = Vec::new();
            for cause in &plan.causes {
                if !causes.contains(cause) { continue; }
                let evidence: Option<i64> = tx.query_row("SELECT 1 FROM evidence WHERE cause=?1 AND target=?2 AND fingerprint=?3 AND plan_key=?4",
                    params![cause, plan.target, plan.fingerprint, plan.key], |r| r.get(0)).optional()?;
                if evidence.is_none() { outstanding.push(cause.clone()); }
            }
            if outstanding.is_empty() { continue; }
            let candidate = Plan { causes: outstanding, ..plan };
            let (state, _) = Self::prior_gate(&tx, &candidate.key, now_ms)?;
            match state {
                Admission::AwaitingSelection => return Ok((Admission::AwaitingSelection,Some(candidate))),
                Admission::RetryAt(time) => retry_at=Some(retry_at.map_or(time,|prior| prior.min(time))),
                Admission::Exhausted => exhausted=true,
                _ => blocked=true,
            }
        }
        let state = if let Some(when) = retry_at { Admission::RetryAt(when) }
            else if exhausted { Admission::Exhausted }
            else if blocked { Admission::Blocked }
            else { Admission::AwaitingSelection };
        Ok((state,None))
    }
    fn first_undrained(tx: &Transaction<'_>) -> Result<Option<String>> {
        let json: Option<String> = tx.query_row(
            "SELECT identity FROM attempt WHERE state='recovery' AND drained=0 ORDER BY id LIMIT 1",
            [], |r| r.get(0)).optional()?;
        json.map(|x| from_json::<JobIdentity>(&x).map(|j| j.scope)).transpose()
    }
    fn prior_gate(tx: &Transaction<'_>, key: &str, now_ms: i64) -> Result<(Admission, i64)> {
        let prev: Option<(i64, String, Option<i64>, i64, String)> = tx.query_row(
            "SELECT id,state,retry_at,drained,identity FROM attempt WHERE plan_key=?1 ORDER BY id DESC LIMIT 1",
            [key], |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?))).optional()?;
        let Some((_,state,retry_at,drained,identity)) = prev else { return Ok((Admission::AwaitingSelection,1)); };
        let job: JobIdentity = from_json(&identity)?;
        if state == "running" { return Ok((Admission::Blocked,0)); }
        if state == "recovery" && drained == 0 { return Ok((Admission::Recovery(job.scope),0)); }
        if job.number >= MAX_ATTEMPTS_PER_PLAN { return Ok((Admission::Exhausted,0)); }
        if matches!(state.as_str(), "blocked" | "completed") { return Ok((Admission::Blocked,0)); }
        if retry_at.is_some_and(|when| when > now_ms) {
            return Ok((Admission::RetryAt(retry_at.unwrap_or(now_ms)),0));
        }
        Ok((Admission::AwaitingSelection,job.number+1))
    }

    pub fn admit(&mut self, fence: &Fence, plan: &Plan, scope: &str,
        now_ms: i64, authority: &impl Authority) -> Result<Admission> {
        authority.current(fence)?;
        if !field(scope, 128) { return Err(ControlError::Invalid("owned scope ID")); }
        let tx = self.db.transaction_with_behavior(TransactionBehavior::Immediate)?;
        Self::require_fence(&tx, fence)?;
        if let Some(scope) = Self::first_undrained(&tx)? { return Ok(Admission::Recovery(scope)); }
        let active: i64 = tx.query_row("SELECT count(*) FROM attempt WHERE state='running'", [], |r| r.get(0))?;
        if active > 0 { return Ok(Admission::Blocked); }
        let current: Option<(String,String,String)> = tx.query_row(
            "SELECT target,fingerprint,causes FROM plan WHERE key=?1", [&plan.key],
            |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?))).optional()?;
        let Some((target,fingerprint,causes)) = current else { return Err(ControlError::Stale); };
        // The selected plan can contain causes previously satisfied; do not let
        // a caller mint a new plan with arbitrary omitted members.
        let selected: Vec<String> = from_json(&causes)?;
        let mut expected = Vec::new();
        for cause in selected {
            let active: Option<i64> = tx.query_row("SELECT 1 FROM cause WHERE id=?1", [&cause], |r| r.get(0)).optional()?;
            let evidence: Option<i64> = tx.query_row(
                "SELECT 1 FROM evidence WHERE cause=?1 AND target=?2 AND fingerprint=?3 AND plan_key=?4",
                params![cause,target,fingerprint,plan.key],|r| r.get(0)).optional()?;
            if active.is_some() && evidence.is_none() { expected.push(cause); }
        }
        if plan.target != target || plan.fingerprint != fingerprint || plan.causes != expected
            || plan.causes.is_empty()
        { return Err(ControlError::Invalid("admission does not match exact outstanding selection")); }
        let prior = Self::prior_gate(&tx, &plan.key, now_ms)?;
        if prior.1 == 0 { return Ok(prior.0); }
        let mut statement = tx.prepare("SELECT identity FROM attempt")?;
        let seen = statement.query_map([], |r| r.get::<_,String>(0))?;
        for entry in seen {
            if from_json::<JobIdentity>(&entry?)?.scope == scope {
                return Err(ControlError::Invalid("reused worker scope"));
            }
        }
        drop(statement);
        let count: i64 = tx.query_row("SELECT count(*) FROM attempt", [], |r| r.get(0))?;
        if count >= MAX_ATTEMPTS { return Err(ControlError::Blocked("attempt retention quota")); }
        authority.current(fence)?;
        tx.execute("INSERT INTO attempt(plan_key,identity,state,retry_at,drained)
            VALUES (?1,'{}','reserved',NULL,0)", [&plan.key])?;
        let id = tx.last_insert_rowid();
        let identity = JobIdentity { protocol: PROTOCOL.into(), operation: "test".into(),
            job: format!("test-attempt:{id}"), attempt: id, number: prior.1,
            scope: scope.into(), fence: fence.clone(), plan: plan.clone() };
        tx.execute("UPDATE attempt SET identity=?1,state='running' WHERE id=?2",
            params![json(&identity)?,id])?;
        tx.commit()?;
        Ok(Admission::Ready(Job { identity, max_response_bytes: MAX_FRAME as u32 }))
    }

    fn load_attempt(tx: &Transaction<'_>, job: &JobIdentity) -> Result<(String, bool)> {
        let row: Option<(String,String,i64)> = tx.query_row(
            "SELECT identity,state,drained FROM attempt WHERE id=?1", [job.attempt],
            |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?))).optional()?;
        let Some((serialized,state,drained)) = row else { return Err(ControlError::Stale); };
        if from_json::<JobIdentity>(&serialized)? != *job { return Err(ControlError::Stale); }
        Ok((state,drained!=0))
    }

    /// A positive drainage proof persists independently of the receipt. It is
    /// also required for failed/revoked attempts before any output lease reuse.
    pub fn confirm_drained(&mut self, job: &JobIdentity, authority: &impl Authority) -> Result<()> {
        authority.drained(&job.scope)?;
        let tx = self.db.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let (state,_) = Self::load_attempt(&tx, job)?;
        if !matches!(state.as_str(), "running" | "recovery") {
            return Err(ControlError::Invalid("attempt cannot be drained in this state"));
        }
        authority.drained(&job.scope)?;
        tx.execute("UPDATE attempt SET drained=1 WHERE id=?1", [job.attempt])?;
        tx.commit()?;
        Ok(())
    }
    pub fn recoveries(&self) -> Result<Vec<JobIdentity>> {
        let mut stmt = self.db.prepare("SELECT identity FROM attempt WHERE state='recovery' AND drained=0 ORDER BY id")?;
        let rows = stmt.query_map([], |r| r.get::<_, String>(0))?;
        let mut out = Vec::new();
        for row in rows { out.push(from_json(&row?)?); }
        Ok(out)
    }

    /// Outcome, receipt uniqueness, evidence and cause retirement are one commit.
    /// A failed transaction leaves the attempt running and every cause outstanding.
    pub fn finish(&mut self, terminal: &Terminal, now_ms: i64,
        authority: &impl Authority) -> Result<FinishResult> {
        let job = &terminal.identity;
        terminal.validate(job)?;
        authority.current(&job.fence)?;
        authority.drained(&job.scope)?;
        let tx = self.db.transaction_with_behavior(TransactionBehavior::Immediate)?;
        Self::require_fence(&tx, &job.fence)?;
        let (state,drained) = Self::load_attempt(&tx,job)?;
        if state != "running" || !drained { return Err(ControlError::Invalid("attempt not running and drained")); }
        let selection: Option<(String,i64)> = tx.query_row("SELECT fence,complete FROM selection WHERE id=1", [], |r| Ok((r.get(0)?,r.get(1)?))).optional()?;
        if selection.as_ref().is_none_or(|(f,_)| from_json::<Fence>(f).ok().as_ref()!=Some(&job.fence)) {
            return Err(ControlError::Stale);
        }
        let plans = Self::plans(&tx)?;
        if !plans.iter().any(|p| p.key==job.plan.key && p.target==job.plan.target
            && p.fingerprint==job.plan.fingerprint && job.plan.causes.iter().all(|id| p.causes.contains(id))) {
            return Err(ControlError::Stale);
        }
        let outcome;
        if terminal.status == TerminalStatus::Complete {
            let receipt = terminal.receipt.as_ref().ok_or(ControlError::Invalid("missing receipt"))?;
            receipt.validate()?;
            authority.verify_receipt(job,receipt)?;
            authority.current(&job.fence)?;
            authority.drained(&job.scope)?;
            // Receipt reuse is always rejected, including replays of an old attempt.
            tx.execute("INSERT INTO receipt(receipt_id,attempt_id,payload) VALUES(?1,?2,?3)",
                params![receipt.receipt_id,job.attempt,json(receipt)?])?;
            for cause in &job.plan.causes {
                tx.execute("INSERT INTO evidence(cause,target,fingerprint,plan_key,receipt_id) VALUES(?1,?2,?3,?4,?5)
                    ON CONFLICT(cause,target,fingerprint,plan_key) DO UPDATE SET receipt_id=excluded.receipt_id",
                    params![cause,job.plan.target,job.plan.fingerprint,job.plan.key,receipt.receipt_id])?;
            }
            let count: i64 = tx.query_row("SELECT count(*) FROM evidence", [], |r| r.get(0))?;
            if count > MAX_EVIDENCE { return Err(ControlError::Blocked("evidence retention quota")); }
            outcome = "completed";
        } else if matches!(terminal.status, TerminalStatus::Unsupported | TerminalStatus::Deferred) {
            outcome = "blocked";
        } else {
            outcome = if job.number >= MAX_ATTEMPTS_PER_PLAN { "exhausted" } else { "retryable" };
        }
        let retry_at = if outcome == "retryable" {
            Some(now_ms.saturating_add(250_i64.saturating_mul(1_i64 << (job.number-1).min(2))))
        } else { None };
        authority.current(&job.fence)?;
        authority.drained(&job.scope)?;
        let observation = AttemptObservation { status:terminal.status,
            disposition:terminal.observed,reason:terminal.reason.clone(),
            omissions:terminal.omissions.clone() };
        tx.execute("UPDATE attempt SET state=?1,retry_at=?2,observation=?3 WHERE id=?4",
            params![outcome,retry_at,json(&observation)?,job.attempt])?;
        let retired = Self::retire(&tx)?;
        tx.commit()?;
        Ok(FinishResult { retired, state: outcome.into() })
    }

    /// Failed protocol/transport: record retry *only after* process drainage.
    /// If authority changed, retain `recovery` for the new incarnation.
    pub fn interrupt(&mut self, job: &JobIdentity, now_ms: i64,
        authority: &impl Authority) -> Result<()> {
        authority.drained(&job.scope)?;
        let tx = self.db.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let (state,drained) = Self::load_attempt(&tx,job)?;
        if !drained { return Err(ControlError::Blocked("unconfirmed scope drainage")); }
        if state == "running" && Self::require_fence(&tx,&job.fence).is_ok()
            && authority.current(&job.fence).is_ok() {
            let (new_state,retry) = if job.number >= MAX_ATTEMPTS_PER_PLAN {
                ("exhausted",None)
            } else {
                ("retryable",Some(now_ms.saturating_add(250_i64 << (job.number-1).min(2))))
            };
            tx.execute("UPDATE attempt SET state=?1,retry_at=?2 WHERE id=?3",
                params![new_state,retry,job.attempt])?;
        } else if state == "running" {
            tx.execute("UPDATE attempt SET state='recovery' WHERE id=?1", [job.attempt])?;
        }
        tx.commit()?;
        Ok(())
    }

    pub fn pending(&mut self) -> Result<Pending> {
        let tx = self.db.transaction_with_behavior(TransactionBehavior::Deferred)?;
        let causes = Self::all_causes(&tx)?.into_iter().map(|(_,c)| c).collect::<Vec<_>>();
        let mut recovery_scopes = Vec::new();
        {
            let mut stmt = tx.prepare("SELECT identity FROM attempt WHERE state='recovery' AND drained=0 ORDER BY id")?;
            let rows = stmt.query_map([], |r| r.get::<_,String>(0))?;
            for row in rows { recovery_scopes.push(from_json::<JobIdentity>(&row?)?.scope); }
        }
        let mut outstanding = Vec::new();
        let cause_ids = causes.iter().map(Cause::id).collect::<BTreeSet<_>>();
        for mut plan in Self::plans(&tx)? {
            plan.causes.retain(|id| cause_ids.contains(id));
            if !plan.causes.is_empty() { outstanding.push(plan); }
        }
        let has_selection: Option<i64> = tx.query_row("SELECT complete FROM selection WHERE id=1", [], |r| r.get(0)).optional()?;
        let state = if !recovery_scopes.is_empty() { "waiting_recovery" }
            else if causes.is_empty() { "idle" }
            else if has_selection.is_none() { "awaiting_selection" }
            else if has_selection==Some(0) { "partial_selection" }
            else if outstanding.is_empty() { "awaiting_selection" }
            else { "pending" };
        let latest: Option<Option<String>> = tx.query_row(
            "SELECT observation FROM attempt ORDER BY id DESC LIMIT 1",
            [], |r| r.get(0)).optional()?;
        let latest = latest.flatten().map(|x| from_json(&x)).transpose()?;
        tx.commit()?;
        Ok(Pending { causes, outstanding, recovery_scopes, latest, state: state.into() })
    }

    /// Transport-neutral, bounded export of pending causes, completed receipts,
    /// evidence and retry history, including interrupted output scope identities.
    pub fn export(&mut self) -> Result<Vec<u8>> {
        let tx = self.db.transaction_with_behavior(TransactionBehavior::Deferred)?;
        let fence = Self::fence_tx(&tx)?;
        let causes = Self::all_causes(&tx)?;
        let selection: Option<(String,i64)> = tx.query_row("SELECT fence,complete FROM selection WHERE id=1", [], |r| Ok((r.get(0)?,r.get(1)?))).optional()?;
        let selection = selection.map(|(s, c)| -> Result<_> {
            Ok((from_json(&s)?, c != 0))
        }).transpose()?;
        let plans = Self::plans(&tx)?;
        let attempts = {
            let mut stmt = tx.prepare("SELECT identity,state,retry_at,drained,observation FROM attempt ORDER BY id")?;
            let rows = stmt.query_map([], |r| Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?,
                r.get(2)?,r.get::<_,i64>(3)?,r.get::<_,Option<String>>(4)?)))?;
            let mut list = Vec::new();
            for row in rows { let (v,state,retry_at,drained,observation) = row?;
                list.push(StoredAttempt { identity:from_json(&v)?,state,retry_at,drained:drained!=0,
                    observation:observation.map(|o| from_json(&o)).transpose()? }); }
            list
        };
        let evidence = {
            let mut stmt = tx.prepare("SELECT cause,target,fingerprint,plan_key,receipt_id FROM evidence ORDER BY cause,target")?;
            let rows = stmt.query_map([], |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?)))?;
            rows.collect::<std::result::Result<Vec<_>,_>>()?
        };
        let receipts = {
            let mut stmt = tx.prepare("SELECT receipt_id,attempt_id,payload FROM receipt ORDER BY attempt_id")?;
            let rows = stmt.query_map([], |r| Ok((r.get::<_,String>(0)?,r.get::<_,i64>(1)?,r.get::<_,String>(2)?)))?;
            let mut list = Vec::new();
            for row in rows { let (id, attempt, data) = row?;
                list.push((id,attempt,from_json(&data)?)); }
            list
        };
        let out = RecoveryExport { schema:SCHEMA_VERSION, fence, causes, selection, plans,
            attempts, evidence, receipts };
        let bytes = serde_json::to_vec(&out)?;
        if bytes.len() > MAX_EXPORT { return Err(ControlError::Invalid("recovery export quota")); }
        tx.commit()?;
        Ok(bytes)
    }

    /// Import is allowed only into a new empty native partition and only for
    /// a trusted matching physical root/context. Imported running attempts are
    /// quarantined for process-tree drainage. A new generation must `bind`
    /// before any imported selection can be trusted for execution/publication.
    pub fn import(&mut self, bytes: &[u8], root: &str, context: &str) -> Result<()> {
        if bytes.len() > MAX_EXPORT { return Err(ControlError::Invalid("recovery import quota")); }
        let snapshot: RecoveryExport = serde_json::from_slice(bytes)?;
        if snapshot.schema != SCHEMA_VERSION || snapshot.fence.as_ref().is_some_and(|f| f.root!=root || f.context!=context)
            || snapshot.causes.len() > MAX_CAUSES as usize
            || snapshot.attempts.len() > MAX_ATTEMPTS as usize || snapshot.receipts.len() > MAX_RECEIPTS as usize
            || snapshot.plans.len() > MAX_TARGETS || !field(root,256) || !field(context,256)
        { return Err(ControlError::Invalid("incompatible recovery export")); }
        if let Some(fence) = &snapshot.fence { fence.validate()?; }
        else if !snapshot.causes.is_empty() || !snapshot.attempts.is_empty()
            || !snapshot.plans.is_empty() || snapshot.selection.is_some()
            || !snapshot.evidence.is_empty() || !snapshot.receipts.is_empty()
        { return Err(ControlError::Invalid("unbound nonempty recovery export")); }
        let tx = self.db.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let exists: i64 = tx.query_row("SELECT count(*) FROM cause", [], |r| r.get(0))?;
        if exists!=0 || Self::fence_tx(&tx)?.is_some() {
            return Err(ControlError::Blocked("import requires an empty native partition"));
        }
        if let Some(fence) = &snapshot.fence {
            tx.execute("INSERT INTO binding(id,fence,restored) VALUES(1,?1,1)", [json(fence)?])?;
        }
        for (id,cause) in snapshot.causes {
            cause.validate()?;
            if cause.id()!=id { return Err(ControlError::Invalid("imported cause identity")); }
            tx.execute("INSERT INTO cause(id,payload) VALUES(?1,?2)",params![id,json(&cause)?])?;
        }
        // Imported projections and evidence are diagnostics only: no old
        // execution authority can make them current. Re-select under a fresh
        // bound epoch and independently re-verify any reusable artifact.
        if let Some((f,_)) = &snapshot.selection {
            if Some(f)!=snapshot.fence.as_ref() { return Err(ControlError::Invalid("selection fence")); }
        }
        for plan in snapshot.plans {
            if !digest(&plan.fingerprint) || !digest(&plan.key) || !field(&plan.target,128)
                || plan.causes.is_empty() { return Err(ControlError::Invalid("imported plan")); }
            // Intentionally not inserted into live plan table.
        }
        for attempt in snapshot.attempts {
            let job = &attempt.identity;
            if job.protocol!=PROTOCOL || job.operation!="test" || job.attempt<=0
                || job.job!=format!("test-attempt:{}",job.attempt)
                || job.number<=0 || job.number>MAX_ATTEMPTS_PER_PLAN
                || !field(&job.scope,128) || !digest(&job.plan.key) || !digest(&job.plan.fingerprint)
                || snapshot.fence.as_ref().is_some_and(|f| f.root!=job.fence.root || f.context!=job.fence.context)
            { return Err(ControlError::Invalid("imported attempt identity")); }
            let state = if attempt.state=="running" || !attempt.drained {
                "recovery"
            } else { attempt.state.as_str() };
            if !["recovery","retryable","exhausted","completed","blocked"].contains(&state) {
                return Err(ControlError::Invalid("imported attempt state"));
            }
            tx.execute("INSERT INTO attempt(id,plan_key,identity,state,retry_at,drained,observation)
                VALUES(?1,?2,?3,?4,?5,?6,?7)",
                params![job.attempt,job.plan.key,json(job)?,state,attempt.retry_at,
                    if state!="recovery" && attempt.drained { 1_i64 } else { 0_i64 },
                    attempt.observation.as_ref().map(json).transpose()?])?;
        }
        // Old evidence cannot retire work after import. Live evidence is
        // repopulated only by new, verified terminals.
        let _historical_evidence = snapshot.evidence;
        for (id,attempt,receipt) in snapshot.receipts {
            if id!=receipt.receipt_id { return Err(ControlError::Invalid("imported receipt identity")); }
            receipt.validate()?;
            tx.execute("INSERT INTO receipt(receipt_id,attempt_id,payload) VALUES(?1,?2,?3)",
                params![id,attempt,json(&receipt)?])?;
        }
        tx.commit()?;
        Ok(())
    }
}

/// A pure bridge to the shipped typed verification decision library. The
/// provider must supply independently validated evidence, scope and search facts;
/// a passing test alone does not establish branch/behavior coverage.
pub fn evaluate_verification(
    request: &verification::typed::TypedVerificationRequest,
    limits: Option<&verification::VerificationLimits>,
) -> std::result::Result<verification::VerificationBatch, verification::VerificationInputError> {
    verification::typed::evaluate_typed(request, limits)
}

/// Drives at most one admitted target. `Executor` is a pre-authorized bounded
/// peer supplied by a native OS process-tree supervisor. It must independently
/// revoke on authority changes *while* running; this synchronous function also
/// fences before publication and drains on every exit path.
pub fn drive_one(store: &mut Store, fence: &Fence, now_ms: i64,
    authority: &impl Authority, peer: &mut impl Executor) -> Result<Admission> {
    authority.current(fence)?;
    let (next,plan) = store.next(fence,now_ms)?;
    let Some(plan) = plan else { return Ok(next); };
    if next != Admission::AwaitingSelection { return Ok(next); }
    let scope = peer.reserve_scope(fence,&plan)?;
    let admission = match store.admit(fence,&plan,&scope,now_ms,authority) {
        Ok(admission) => admission,
        Err(error) => { peer.revoke_and_drain(&scope)?; return Err(error); }
    };
    let Admission::Ready(job) = &admission else {
        // Reservation never starts a worker, but still must be relinquished.
        peer.revoke_and_drain(&scope)?;
        return Ok(admission);
    };
    let result = peer.execute(job);
    // All output remains leased until recursive drainage is confirmed.
    let drain = match peer.confirm_drained(&job.identity.scope) {
        Ok(()) => Ok(()),
        Err(_) => peer.revoke_and_drain(&job.identity.scope),
    };
    drain?;
    store.confirm_drained(&job.identity,authority)?;
    let accepted = match result {
        Ok(terminal) => store.finish(&terminal,now_ms,authority),
        Err(e) => Err(e),
    };
    if accepted.is_err() {
        // A bad terminal or lost authority never writes evidence. Retain the
        // obligation; the store will use `recovery` for a superseded epoch.
        peer.revoke_and_drain(&job.identity.scope)?;
        store.interrupt(&job.identity,now_ms,authority)?;
    }
    accepted?;
    Ok(admission)
}
