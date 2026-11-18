//! Opt-in comparison of already captured evidence for one regression assertion.
//!
//! This module does not read receipts, execute an adapter, or change review state.
//! Existing foreground JSON and TRX dispositions cannot construct these facts on
//! their own. A qualified caller must bind complete captures to the declared plan,
//! immutable inputs, actual execution context and reviewed production substitution.
//! Digests are caller-supplied SHA-256 identities, not proofs verified here.
//! `Target::oracle` binds the independently specified expectation, its observation
//! encoding and assertion semantics. No inference from error text is permitted.

pub type Digest = [u8; 32];
pub const MIN_REPETITIONS: u8 = 2;
pub const MAX_REPETITIONS: u8 = 8;
pub const MAX_SUBSTITUTIONS: usize = 16;
pub const MAX_PATH_BYTES: usize = 512;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RowKey { Single, Data(Digest) }

/// Logical identity qualified across builds; never a display name or row ordinal.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CaseKey { pub test: Digest, pub row: RowKey }

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Target { pub case: CaseKey, pub assertion: Digest, pub oracle: Digest }

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PreservedInputs {
    /// Complete test source, helpers and instrumentation manifest, including modes.
    pub tests: Digest,
    /// Fixture code/data and selected input values, including deterministic seeds.
    pub fixture: Digest,
}

/// Actual identities, not inferred versions from a configuration file. Each
/// digest covers a versioned manifest; the owner retains the readable manifests.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Environment {
    pub sdk: Digest,
    pub runner: Digest,
    pub adapter: Digest,
    pub target: Digest,
    pub dependencies: Digest,
    pub settings: Digest,
    /// Runtime, OS/architecture, culture, timezone and relevant external state.
    pub runtime_environment: Digest,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EnvironmentField { Sdk, Runner, Adapter, Target, Dependencies, Settings, RuntimeEnvironment }

/// A replacement of an existing production file, with its mode unchanged.
/// Additions, deletions, renames, generated inputs and mode changes need a later
/// contract. Paths are logical repository-relative names, never opened here.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ProductionEdit<'a> { pub path: &'a str, pub before: Digest, pub after: Digest }

#[derive(Clone, Copy, Debug)]
pub struct Substitution<'a> {
    /// Identity of an owner-reviewed record binding BOTH snapshots, the COMPLETE
    /// production-only delta and unchanged inputs. This grants no execution rights.
    pub review: Option<Digest>,
    pub baseline_snapshot: Digest,
    pub candidate_snapshot: Digest,
    pub unchanged_inputs: Digest,
    /// Strict bytewise path order; an empty set requires identical snapshots.
    pub edits: &'a [ProductionEdit<'a>],
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Provenance {
    pub plan: Digest,
    pub attempt: Digest,
    pub receipt: Digest,
    /// Fresh isolated instance/reset record for this execution, not a source hash.
    pub isolation: Digest,
    pub snapshot: Digest,
    /// Complete manifest of actual loaded executable bytes, excluding volatile IDs.
    pub build: Digest,
    pub substitution_review: Digest,
    pub unchanged_inputs: Digest,
    pub preserved: PreservedInputs,
    pub environment: Environment,
    pub qualification: Digest,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Selection { Exact(CaseKey), Missing, Ambiguous, Unsupported }

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AssertionResult { Passed, Failed }

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AssertionEvent {
    pub target: Target,
    /// Event key within the bound attempt; terminal failure must name this event.
    pub event: Digest,
    /// Complete semantic input to the deterministic oracle under its versioned
    /// encoding. Excludes the assertion verdict and volatile event IDs/timestamps;
    /// no other normalization is allowed implicitly.
    pub observation: Digest,
    pub result: AssertionResult,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AssertionCapture {
    /// Complete capture proves exactly one target occurrence in the selected row.
    ExactlyOne(AssertionEvent),
    NotReached,
    Ambiguous,
    Unsupported,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FailureKind { Setup, Teardown, Exception, Build, Runner, Timeout, Cancelled }

/// Outcome of the exact selected case/row, never the containing run or suite.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CaseOutcome {
    Passed,
    AssertionFailed(Digest),
    OtherFailure(FailureKind),
    Skipped,
    NotRun,
    Incomplete,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Attempt {
    pub repetition: u8,
    pub provenance: Option<Provenance>,
    pub selection: Selection,
    pub case_outcome: CaseOutcome,
    pub assertion: AssertionCapture,
}

#[derive(Clone, Copy, Debug)]
pub struct ArmEvidence<'a> {
    /// Every planned attempt, including failed/partial attempts; no best-of retries.
    pub attempts: &'a [Attempt],
    pub complete: bool,
}

#[derive(Clone, Copy, Debug)]
pub struct PairEvidence<'a> {
    /// Predeclared plan binding target, preserved inputs and repetition policy.
    pub plan: Digest,
    pub target: Target,
    pub preserved: PreservedInputs,
    /// Owner qualification of capture/row mapping, not inferred from a runner name.
    pub qualification: Option<Digest>,
    pub repetitions: u8,
    pub substitution: Option<Substitution<'a>>,
    pub baseline: ArmEvidence<'a>,
    pub candidate: ArmEvidence<'a>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Arm { Baseline, Candidate }

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Reason {
    InvalidRepetitionBudget, TooManyAttempts, IncompleteArm, RepetitionSet,
    UnqualifiedCapture, MissingSubstitution, UnreviewedSubstitution, TooManyEdits,
    InvalidEditPath, InvalidEditSet, MissingProvenance, PlanMismatch,
    SnapshotMismatch, SubstitutionMismatch, PreservedTestMismatch, FixtureMismatch,
    OtherInputsMismatch, QualificationMismatch, EnvironmentMismatch(EnvironmentField),
    BuildChanged, ReusedAttempt, ReusedReceipt, ReusedIsolation,
    CaseMissing, CaseAmbiguous, CaseUnsupported, CaseMismatch,
    AssertionNotReached, AssertionAmbiguous, AssertionUnsupported, AssertionMismatch,
    UnrelatedAssertionFailure, OtherCaseFailure(FailureKind), CaseSkipped, CaseNotRun,
    CaseIncomplete, ContradictoryOutcome, UnstableObservations, CandidateFailed, BothArmsFailed,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Problem { pub reason: Reason, pub arm: Option<Arm>, pub repetition: Option<u8> }

impl Problem {
    fn global(reason: Reason) -> Self { Self { reason, arm: None, repetition: None } }
    fn at(reason: Reason, arm: Arm, repetition: Option<u8>) -> Self {
        Self { reason, arm: Some(arm), repetition }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Outcome {
    /// Intended old-arm assertion failure and candidate pass for this target only.
    Discriminating,
    /// Both arms passed this target; actionability requires an independent bug claim.
    NoDiscrimination,
    Unknown(Problem),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Limitation {
    SuppliedFactsNotVerified, OneCaseRowAssertionOnly, OracleMeaningNotVerified,
    FiniteRepetitionsOnly, FirstProblemOnly,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Support {
    pub baseline_first_receipt: Digest,
    pub candidate_first_receipt: Digest,
    pub baseline_observation: Digest,
    pub candidate_observation: Digest,
    pub repetitions: u8,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Comparison {
    pub plan: Digest,
    pub target: Target,
    pub outcome: Outcome,
    /// Raw supplied counts, including on an early budget rejection.
    pub baseline_attempts: usize,
    pub candidate_attempts: usize,
    /// First receipt per arm links into the complete retained input, not a summary
    /// that permits discarding later attempts. Absent for an unknown comparison.
    pub support: Option<Support>,
    pub limitations: [Limitation; 5],
}

/// Pure, bounded comparison. Input allocation/decoding is the caller's boundary.
/// No heap allocation, I/O, recursion, clock, runner, mutation or state publication.
/// Unknown precedence: budgets/completeness/counts, qualification/substitution,
/// ordinal sets, provenance (baseline then candidate, ordinal order), observations in that
/// same order, then paired verdict. Within a stage the source order is authoritative.
/// Reordering attempts does not change the result; edits must be canonical already.
pub fn compare(input: &PairEvidence<'_>) -> Comparison {
    let (outcome, support) = match assess(input) {
        Ok((outcome, support)) => (outcome, Some(support)),
        Err(problem) => (Outcome::Unknown(problem), None),
    };
    Comparison {
        plan: input.plan, target: input.target, outcome,
        baseline_attempts: input.baseline.attempts.len(),
        candidate_attempts: input.candidate.attempts.len(), support,
        limitations: [Limitation::SuppliedFactsNotVerified, Limitation::OneCaseRowAssertionOnly,
            Limitation::OracleMeaningNotVerified, Limitation::FiniteRepetitionsOnly,
            Limitation::FirstProblemOnly],
    }
}

fn assess(input: &PairEvidence<'_>) -> Result<(Outcome, Support), Problem> {
    if !(MIN_REPETITIONS..=MAX_REPETITIONS).contains(&input.repetitions) {
        return Err(Problem::global(Reason::InvalidRepetitionBudget));
    }
    let arms = [(Arm::Baseline, input.baseline), (Arm::Candidate, input.candidate)];
    for (arm, evidence) in arms {
        if evidence.attempts.len() > usize::from(MAX_REPETITIONS) {
            return Err(Problem::at(Reason::TooManyAttempts, arm, None));
        }
        if !evidence.complete { return Err(Problem::at(Reason::IncompleteArm, arm, None)); }
        if evidence.attempts.len() != usize::from(input.repetitions) {
            return Err(Problem::at(Reason::RepetitionSet, arm, None));
        }
    }
    let qualification = input.qualification.ok_or(Problem::global(Reason::UnqualifiedCapture))?;
    let substitution = input.substitution.ok_or(Problem::global(Reason::MissingSubstitution))?;
    let review = substitution.review.ok_or(Problem::global(Reason::UnreviewedSubstitution))?;
    validate_edits(&substitution)?;
    for (arm, evidence) in arms {
        let mut seen = [false; MAX_REPETITIONS as usize];
        for attempt in evidence.attempts {
            let ordinal = usize::from(attempt.repetition);
            if attempt.repetition >= input.repetitions || seen[ordinal] {
                return Err(Problem::at(Reason::RepetitionSet, arm, None));
            }
            seen[ordinal] = true;
        }
    }
    let baseline_first = provenance(attempt(input.baseline, Arm::Baseline, 0)?, Arm::Baseline)?;
    let mut seen = [([0; 32], [0; 32], [0; 32]); 2 * MAX_REPETITIONS as usize];
    let mut seen_count = 0;
    for (arm, evidence) in arms {
        let first = provenance(attempt(evidence, arm, 0)?, arm)?;
        for ordinal in 0..input.repetitions {
            let item = attempt(evidence, arm, ordinal)?;
            let facts = provenance(item, arm)?;
            let fail = |reason| Problem::at(reason, arm, Some(ordinal));
            let snapshot = match arm {
                Arm::Baseline => substitution.baseline_snapshot,
                Arm::Candidate => substitution.candidate_snapshot,
            };
            if facts.plan != input.plan { return Err(fail(Reason::PlanMismatch)); }
            if facts.snapshot != snapshot { return Err(fail(Reason::SnapshotMismatch)); }
            if facts.substitution_review != review { return Err(fail(Reason::SubstitutionMismatch)); }
            if facts.preserved.tests != input.preserved.tests { return Err(fail(Reason::PreservedTestMismatch)); }
            if facts.preserved.fixture != input.preserved.fixture { return Err(fail(Reason::FixtureMismatch)); }
            if facts.unchanged_inputs != substitution.unchanged_inputs { return Err(fail(Reason::OtherInputsMismatch)); }
            if facts.qualification != qualification { return Err(fail(Reason::QualificationMismatch)); }
            if let Some(field) = environment_difference(&baseline_first.environment, &facts.environment) {
                return Err(fail(Reason::EnvironmentMismatch(field)));
            }
            if facts.build != first.build { return Err(fail(Reason::BuildChanged)); }
            if seen[..seen_count].iter().any(|prior| prior.0 == facts.attempt) {
                return Err(fail(Reason::ReusedAttempt));
            }
            if seen[..seen_count].iter().any(|prior| prior.1 == facts.receipt) {
                return Err(fail(Reason::ReusedReceipt));
            }
            if seen[..seen_count].iter().any(|prior| prior.2 == facts.isolation) {
                return Err(fail(Reason::ReusedIsolation));
            }
            seen[seen_count] = (facts.attempt, facts.receipt, facts.isolation);
            seen_count += 1;
        }
    }
    let baseline = observe_arm(input, Arm::Baseline, input.baseline)?;
    let candidate = observe_arm(input, Arm::Candidate, input.candidate)?;
    if baseline.1 == candidate.1 && baseline.0 != candidate.0 {
        return Err(Problem::global(Reason::ContradictoryOutcome));
    }
    let candidate_first = provenance(attempt(input.candidate, Arm::Candidate, 0)?, Arm::Candidate)?;
    let outcome = match (baseline.0, candidate.0) {
        (AssertionResult::Passed, AssertionResult::Passed) => Outcome::NoDiscrimination,
        (AssertionResult::Failed, AssertionResult::Passed) => {
            if substitution.edits.is_empty() || baseline_first.build == candidate_first.build {
                return Err(Problem::global(Reason::UnstableObservations));
            }
            Outcome::Discriminating
        },
        (AssertionResult::Passed, AssertionResult::Failed) =>
            return Err(Problem::at(Reason::CandidateFailed, Arm::Candidate, None)),
        (AssertionResult::Failed, AssertionResult::Failed) =>
            return Err(Problem::global(Reason::BothArmsFailed)),
    };
    Ok((outcome, Support {
        baseline_first_receipt: baseline_first.receipt, candidate_first_receipt: candidate_first.receipt,
        baseline_observation: baseline.1, candidate_observation: candidate.1,
        repetitions: input.repetitions,
    }))
}

fn validate_edits(substitution: &Substitution<'_>) -> Result<(), Problem> {
    if substitution.edits.len() > MAX_SUBSTITUTIONS {
        return Err(Problem::global(Reason::TooManyEdits));
    }
    if substitution.edits.is_empty() != (substitution.baseline_snapshot == substitution.candidate_snapshot) {
        return Err(Problem::global(Reason::InvalidEditSet));
    }
    let mut prior: Option<&str> = None;
    for edit in substitution.edits {
        let path = edit.path;
        if path.is_empty() || path.len() > MAX_PATH_BYTES ||
            !path.bytes().all(|b| b.is_ascii_alphanumeric() || b"/._-".contains(&b)) ||
            path.split('/').any(|part| part.is_empty() || part == "." || part == ".." || part.ends_with('.')) {
            return Err(Problem::global(Reason::InvalidEditPath));
        }
        if edit.before == edit.after || prior.is_some_and(|previous| previous >= path) {
            return Err(Problem::global(Reason::InvalidEditSet));
        }
        prior = Some(path);
    }
    Ok(())
}

fn environment_difference(a: &Environment, b: &Environment) -> Option<EnvironmentField> {
    [
        (a.sdk != b.sdk, EnvironmentField::Sdk), (a.runner != b.runner, EnvironmentField::Runner),
        (a.adapter != b.adapter, EnvironmentField::Adapter), (a.target != b.target, EnvironmentField::Target),
        (a.dependencies != b.dependencies, EnvironmentField::Dependencies),
        (a.settings != b.settings, EnvironmentField::Settings),
        (a.runtime_environment != b.runtime_environment, EnvironmentField::RuntimeEnvironment),
    ].into_iter().find_map(|(different, field)| different.then_some(field))
}

fn attempt<'a>(evidence: ArmEvidence<'a>, arm: Arm, ordinal: u8) -> Result<&'a Attempt, Problem> {
    evidence.attempts.iter().find(|item| item.repetition == ordinal)
        .ok_or(Problem::at(Reason::RepetitionSet, arm, Some(ordinal)))
}

fn provenance(item: &Attempt, arm: Arm) -> Result<&Provenance, Problem> {
    item.provenance.as_ref().ok_or(Problem::at(Reason::MissingProvenance, arm, Some(item.repetition)))
}

fn observe_arm(input: &PairEvidence<'_>, arm: Arm, evidence: ArmEvidence<'_>)
    -> Result<(AssertionResult, Digest), Problem> {
    let mut first = None;
    for ordinal in 0..input.repetitions {
        let item = attempt(evidence, arm, ordinal)?;
        let fail = |reason| Problem::at(reason, arm, Some(ordinal));
        match item.selection {
            Selection::Exact(case) if case == input.target.case => (),
            Selection::Exact(_) => return Err(fail(Reason::CaseMismatch)),
            Selection::Missing => return Err(fail(Reason::CaseMissing)),
            Selection::Ambiguous => return Err(fail(Reason::CaseAmbiguous)),
            Selection::Unsupported => return Err(fail(Reason::CaseUnsupported)),
        }
        match item.case_outcome {
            CaseOutcome::OtherFailure(kind) => return Err(fail(Reason::OtherCaseFailure(kind))),
            CaseOutcome::Skipped => return Err(fail(Reason::CaseSkipped)),
            CaseOutcome::NotRun => return Err(fail(Reason::CaseNotRun)),
            CaseOutcome::Incomplete => return Err(fail(Reason::CaseIncomplete)),
            _ => (),
        }
        let event = match item.assertion {
            AssertionCapture::ExactlyOne(event) => event,
            AssertionCapture::NotReached => return Err(fail(Reason::AssertionNotReached)),
            AssertionCapture::Ambiguous => return Err(fail(Reason::AssertionAmbiguous)),
            AssertionCapture::Unsupported => return Err(fail(Reason::AssertionUnsupported)),
        };
        if event.target != input.target { return Err(fail(Reason::AssertionMismatch)); }
        match (item.case_outcome, event.result) {
            (CaseOutcome::Passed, AssertionResult::Passed) => (),
            (CaseOutcome::AssertionFailed(cause), _) if cause != event.event =>
                return Err(fail(Reason::UnrelatedAssertionFailure)),
            (CaseOutcome::AssertionFailed(_), AssertionResult::Failed) => (),
            _ => return Err(fail(Reason::ContradictoryOutcome)),
        }
        let observation = (event.result, event.observation);
        if first.is_some_and(|previous| previous != observation) {
            return Err(fail(Reason::UnstableObservations));
        }
        first = Some(observation);
    }
    first.ok_or(Problem::at(Reason::RepetitionSet, arm, None))
}
