//! Opt-in reader for one owned, single-case, single-i64-equality fixture format.
//!
//! This is not a TRX/foreground converter, runner, or provenance authenticator.
//! The owner must independently qualify the producer and supply supervisor facts
//! for the exact attempt. Receipt fields and hashes cannot establish those facts.
//! No existing CLI, MCP, admission or review path invokes this reader.
//! See ../ASSERTION-RECEIPTS.md for the producer contract and trust boundary.

use crate::paired_regression::{self as paired, Arm, AssertionCapture, AssertionEvent,
    AssertionResult, Attempt, CaseKey, CaseOutcome, Digest, FailureKind, PreservedInputs,
    Provenance, RowKey, Selection, Target};
use serde::Deserialize;
use sha2::{Digest as _, Sha256};

pub const FORMAT: &str = "slophawk-owned-i64-equality-v1";
pub const MAX_RECEIPT_BYTES: usize = 16 * 1024;
pub const MAX_ASSERTIONS: usize = 8;

/// Owner-reviewed mapping of the actual instrumented adapter, source closure,
/// logical case/row and assertion to an independently specified i64 expectation.
/// Deliberately not deserializable from the receipt being assessed.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Qualification {
    pub record: Digest,
    pub adapter: Digest,
    pub target: Target,
    pub preserved: PreservedInputs,
    pub expected: i64,
}

/// Facts obtained outside the untrusted receipt. `capture_complete` means the
/// qualified supervisor observed one selected invocation through teardown and
/// closed its lossless capture, with no hidden retry or omitted event. It is not
/// inferred from process exit, TRX counters, a receipt flag or a content hash.
/// `provenance.receipt` is the supervisor's digest of these exact captured bytes.
/// Actual loaded inputs/environment, isolation and substitution review must have
/// been established independently; a self-reported JSON manifest is insufficient.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Supervisor {
    pub provenance: Provenance,
    pub arm: Arm,
    pub repetition: u8,
    pub selection: Selection,
    pub capture_complete: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HeaderField {
    Plan, Attempt, Arm, Repetition, Isolation, Snapshot, Build,
    SubstitutionReview, UnchangedInputs, Qualification, Adapter, Tests, Fixture,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Reason {
    UnqualifiedCapture, MissingProvenance, InvalidRepetition, IncompleteCapture,
    QualificationMismatch, AdapterMismatch, PreservedTestMismatch, FixtureMismatch,
    OracleMismatch, CaseMissing, CaseAmbiguous, CaseUnsupported, CaseMismatch,
    MissingReceipt, OversizedReceipt, ReceiptDigestMismatch, MalformedReceipt,
    UnsupportedFormat, HeaderMismatch(HeaderField), SetupIncomplete, TeardownIncomplete,
    OtherCaseFailure(FailureKind), CaseSkipped, CaseNotRun, CaseIncomplete,
    TooManyAssertions, AssertionNotReached, AssertionAmbiguous, AssertionMismatch,
    EventAttemptMismatch, EventCaseMismatch, EventIdentityMismatch,
    ContradictoryOutcome, UnrelatedAssertionFailure,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Validation {
    /// One bound attempt, not a paired verdict or a claim of semantic correctness.
    Complete(Attempt),
    /// Never supplies a partial Attempt that a caller could silently promote.
    Unknown(Reason),
}

/// Validate before constructing paired evidence. Retain every planned attempt;
/// if any validation is Unknown, the pair is unavailable, not a shorter retry set.
/// Successful attempts still require `paired_regression::compare` to check the
/// plan, complete arms, substitutions, actual environments, reuse and stability.
/// Error precedence is source order below; no untrusted text appears in reasons.
/// Input is capped before hashing/decoding. No I/O, execution or state mutation.
pub fn validate(bytes: Option<&[u8]>, qualification: Option<&Qualification>,
    supervisor: Option<&Supervisor>) -> Validation {
    match bind(bytes, qualification, supervisor) {
        Ok(attempt) => Validation::Complete(attempt),
        Err(reason) => Validation::Unknown(reason),
    }
}

/// Versioned oracle encoding: domain followed by signed i64 big-endian bytes.
pub fn oracle_id(expected: i64) -> Digest {
    let mut hash = Sha256::new();
    hash.update(b"slophawk-owned-i64-equality-v1/oracle\0");
    hash.update(expected.to_be_bytes());
    hash.finalize().into()
}

/// Semantic oracle input only; excludes timestamps, verdicts and attempt IDs.
pub fn observation_id(expected: i64, actual: i64) -> Digest {
    let mut hash = Sha256::new();
    hash.update(b"slophawk-owned-i64-equality-v1/observation\0");
    hash.update(expected.to_be_bytes());
    hash.update(actual.to_be_bytes());
    hash.finalize().into()
}

/// The one permitted occurrence is scoped to the attempt and complete target.
/// This key binds identity; only qualified complete capture proves occurrence.
pub fn event_id(attempt: Digest, target: Target) -> Digest {
    let mut hash = Sha256::new();
    hash.update(b"slophawk-owned-i64-equality-v1/event\0");
    hash.update(attempt);
    hash.update(target.case.test);
    match target.case.row {
        RowKey::Single => hash.update([0]),
        RowKey::Data(key) => { hash.update([1]); hash.update(key); },
    }
    hash.update(target.assertion);
    hash.update(target.oracle);
    hash.finalize().into()
}

fn bind(bytes: Option<&[u8]>, qualification: Option<&Qualification>,
    supervisor: Option<&Supervisor>) -> Result<Attempt, Reason> {
    let owner = qualification.ok_or(Reason::UnqualifiedCapture)?;
    let supervisor = supervisor.ok_or(Reason::MissingProvenance)?;
    let facts = supervisor.provenance;
    if supervisor.repetition >= paired::MAX_REPETITIONS { return Err(Reason::InvalidRepetition); }
    if !supervisor.capture_complete { return Err(Reason::IncompleteCapture); }
    if facts.qualification != owner.record { return Err(Reason::QualificationMismatch); }
    if facts.environment.adapter != owner.adapter { return Err(Reason::AdapterMismatch); }
    if facts.preserved.tests != owner.preserved.tests { return Err(Reason::PreservedTestMismatch); }
    if facts.preserved.fixture != owner.preserved.fixture { return Err(Reason::FixtureMismatch); }
    if owner.target.oracle != oracle_id(owner.expected) { return Err(Reason::OracleMismatch); }
    match supervisor.selection {
        Selection::Exact(case) if case == owner.target.case => (),
        Selection::Exact(_) => return Err(Reason::CaseMismatch),
        Selection::Missing => return Err(Reason::CaseMissing),
        Selection::Ambiguous => return Err(Reason::CaseAmbiguous),
        Selection::Unsupported => return Err(Reason::CaseUnsupported),
    }
    let bytes = bytes.ok_or(Reason::MissingReceipt)?;
    if bytes.len() > MAX_RECEIPT_BYTES { return Err(Reason::OversizedReceipt); }
    let digest: Digest = Sha256::digest(bytes).into();
    if digest != facts.receipt { return Err(Reason::ReceiptDigestMismatch); }
    let receipt: Receipt = serde_json::from_slice(bytes).map_err(|_| Reason::MalformedReceipt)?;
    if receipt.format != FORMAT { return Err(Reason::UnsupportedFormat); }
    receipt.header.validate(supervisor)?;
    if receipt.case.key()? != owner.target.case { return Err(Reason::CaseMismatch); }
    if !receipt.complete { return Err(Reason::IncompleteCapture); }
    // An assertion can have completed before cleanup fails. That is still Unknown.
    if receipt.setup == Phase::Failed { return Err(Reason::OtherCaseFailure(FailureKind::Setup)); }
    if receipt.teardown == Phase::Failed { return Err(Reason::OtherCaseFailure(FailureKind::Teardown)); }
    if receipt.setup != Phase::Completed { return Err(Reason::SetupIncomplete); }
    if receipt.teardown != Phase::Completed { return Err(Reason::TeardownIncomplete); }
    match receipt.terminal.kind {
        TerminalKind::Passed | TerminalKind::AssertionFailed => (),
        TerminalKind::SetupFailed => return Err(Reason::OtherCaseFailure(FailureKind::Setup)),
        TerminalKind::TeardownFailed => return Err(Reason::OtherCaseFailure(FailureKind::Teardown)),
        TerminalKind::Exception => return Err(Reason::OtherCaseFailure(FailureKind::Exception)),
        TerminalKind::BuildFailed => return Err(Reason::OtherCaseFailure(FailureKind::Build)),
        TerminalKind::RunnerFailed => return Err(Reason::OtherCaseFailure(FailureKind::Runner)),
        TerminalKind::Timeout => return Err(Reason::OtherCaseFailure(FailureKind::Timeout)),
        TerminalKind::Cancelled => return Err(Reason::OtherCaseFailure(FailureKind::Cancelled)),
        TerminalKind::Skipped => return Err(Reason::CaseSkipped),
        TerminalKind::NotRun => return Err(Reason::CaseNotRun),
        TerminalKind::Incomplete => return Err(Reason::CaseIncomplete),
    }
    if receipt.assertions.len() > MAX_ASSERTIONS { return Err(Reason::TooManyAssertions); }
    if receipt.assertions.is_empty() { return Err(Reason::AssertionNotReached); }
    if receipt.assertions.len() != 1 { return Err(Reason::AssertionAmbiguous); }
    let event = &receipt.assertions[0];
    if event.attempt != facts.attempt { return Err(Reason::EventAttemptMismatch); }
    if event.case.key()? != owner.target.case { return Err(Reason::EventCaseMismatch); }
    if event.assertion != owner.target.assertion || event.oracle != owner.target.oracle {
        return Err(Reason::AssertionMismatch);
    }
    if event.event != event_id(facts.attempt, owner.target) { return Err(Reason::EventIdentityMismatch); }
    let result = if event.actual == owner.expected { AssertionResult::Passed } else { AssertionResult::Failed };
    if (event.result == EventResult::Passed) != (result == AssertionResult::Passed) {
        return Err(Reason::ContradictoryOutcome);
    }
    let case_outcome = match (receipt.terminal.kind, receipt.terminal.event, result) {
        (TerminalKind::Passed, None, AssertionResult::Passed) => CaseOutcome::Passed,
        (TerminalKind::AssertionFailed, cause, _) if cause != Some(event.event) =>
            return Err(Reason::UnrelatedAssertionFailure),
        (TerminalKind::AssertionFailed, Some(cause), AssertionResult::Failed) => CaseOutcome::AssertionFailed(cause),
        _ => return Err(Reason::ContradictoryOutcome),
    };
    Ok(Attempt {
        repetition: supervisor.repetition, provenance: Some(facts),
        selection: supervisor.selection, case_outcome,
        assertion: AssertionCapture::ExactlyOne(AssertionEvent {
            target: owner.target, event: event.event,
            observation: observation_id(owner.expected, event.actual), result,
        }),
    })
}

// A closed schema with no aliases or extension map; only terminal.event may be
// omitted. Serde rejects duplicate/unknown fields, wrong types, invalid enum
// values and trailing documents. Fixed byte arrays reject malformed identities.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Receipt {
    format: String,
    header: Header,
    case: Case,
    complete: bool,
    setup: Phase,
    teardown: Phase,
    assertions: Vec<Event>,
    terminal: Terminal,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Header {
    plan: Digest, attempt: Digest, arm: WireArm, repetition: u8, isolation: Digest,
    snapshot: Digest, build: Digest, substitution_review: Digest, unchanged_inputs: Digest,
    qualification: Digest, adapter: Digest, tests: Digest, fixture: Digest,
}

impl Header {
    fn validate(&self, supervisor: &Supervisor) -> Result<(), Reason> {
        let facts = supervisor.provenance;
        let arm = match supervisor.arm { Arm::Baseline => WireArm::Baseline, Arm::Candidate => WireArm::Candidate };
        for (mismatch, field) in [
            (self.plan != facts.plan, HeaderField::Plan),
            (self.attempt != facts.attempt, HeaderField::Attempt),
            (self.arm != arm, HeaderField::Arm),
            (self.repetition != supervisor.repetition, HeaderField::Repetition),
            (self.isolation != facts.isolation, HeaderField::Isolation),
            (self.snapshot != facts.snapshot, HeaderField::Snapshot),
            (self.build != facts.build, HeaderField::Build),
            (self.substitution_review != facts.substitution_review, HeaderField::SubstitutionReview),
            (self.unchanged_inputs != facts.unchanged_inputs, HeaderField::UnchangedInputs),
            (self.qualification != facts.qualification, HeaderField::Qualification),
            (self.adapter != facts.environment.adapter, HeaderField::Adapter),
            (self.tests != facts.preserved.tests, HeaderField::Tests),
            (self.fixture != facts.preserved.fixture, HeaderField::Fixture),
        ] {
            if mismatch { return Err(Reason::HeaderMismatch(field)); }
        }
        Ok(())
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Case { test: Digest, row: Row, row_key: Digest }

impl Case {
    fn key(&self) -> Result<CaseKey, Reason> {
        let row = match self.row {
            Row::Single if self.row_key == [0; 32] => RowKey::Single,
            Row::Single => return Err(Reason::MalformedReceipt),
            Row::Data => RowKey::Data(self.row_key),
        };
        Ok(CaseKey { test: self.test, row })
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Event {
    attempt: Digest, case: Case, assertion: Digest, oracle: Digest, event: Digest,
    actual: i64, result: EventResult,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Terminal {
    kind: TerminalKind,
    // Omitted/null only for non-assertion outcomes; a failure requires a key.
    event: Option<Digest>,
}

#[derive(Clone, Copy, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
enum WireArm { Baseline, Candidate }

#[derive(Clone, Copy, Deserialize)]
#[serde(rename_all = "snake_case")]
enum Row { Single, Data }

#[derive(Clone, Copy, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
enum Phase { Completed, Failed, NotRun, Incomplete }

#[derive(Clone, Copy, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
enum EventResult { Passed, Failed }

#[derive(Clone, Copy, Deserialize)]
#[serde(rename_all = "snake_case")]
enum TerminalKind {
    Passed, AssertionFailed, SetupFailed, TeardownFailed, Exception, BuildFailed,
    RunnerFailed, Timeout, Cancelled, Skipped, NotRun, Incomplete,
}
