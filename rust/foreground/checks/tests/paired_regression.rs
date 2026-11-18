//! Draft snapshot cases for SG-CHECK-R03; deliberately not executed by the packet.
//! Repeated-byte digests below are synthetic identity placeholders, not hashes of
//! repository files or evidence that any runner has been qualified.
use slophawk_foreground_checks::paired_regression::*;

const PLAN: Digest = [1; 32];
const BASELINE: Digest = [10; 32];
const CANDIDATE: Digest = [11; 32];
const PRESERVED: PreservedInputs = PreservedInputs { tests: [12; 32], fixture: [13; 32] };
const UNCHANGED: Digest = [14; 32];
const QUALIFICATION: Digest = [15; 32];
const REVIEW: Digest = [16; 32];
const EDITS: [ProductionEdit<'static>; 1] = [ProductionEdit {
    path: "src/Counter.cs", before: [40; 32], after: [41; 32],
}];

fn target() -> Target {
    Target { case: CaseKey { test: [2; 32], row: RowKey::Single }, assertion: [3; 32], oracle: [4; 32] }
}

fn environment() -> Environment {
    Environment {
        sdk: [20; 32], runner: [21; 32], adapter: [22; 32], target: [23; 32],
        dependencies: [24; 32], settings: [25; 32], runtime_environment: [26; 32],
    }
}

fn captured(arm: Arm, repetition: u8, target: Target) -> Attempt {
    let offset = match arm { Arm::Baseline => 0, Arm::Candidate => 10 };
    Attempt {
        repetition,
        provenance: Some(Provenance {
            plan: PLAN, attempt: [70 + offset + repetition; 32],
            receipt: [90 + offset + repetition; 32], isolation: [110 + offset + repetition; 32],
            snapshot: if arm == Arm::Baseline { BASELINE } else { CANDIDATE },
            build: if arm == Arm::Baseline { [17; 32] } else { [18; 32] },
            substitution_review: REVIEW, unchanged_inputs: UNCHANGED, preserved: PRESERVED,
            environment: environment(), qualification: QUALIFICATION,
        }),
        selection: Selection::Exact(target.case),
        case_outcome: CaseOutcome::Passed,
        assertion: AssertionCapture::ExactlyOne(AssertionEvent {
            target, event: [50 + offset + repetition; 32],
            observation: if arm == Arm::Baseline { [30; 32] } else { [31; 32] },
            result: AssertionResult::Passed,
        }),
    }
}

struct Fixture {
    target: Target,
    repetitions: u8,
    baseline: Vec<Attempt>,
    candidate: Vec<Attempt>,
}

impl Fixture {
    fn new(repetitions: u8) -> Self {
        let target = target();
        Self {
            target, repetitions,
            baseline: (0..repetitions).map(|i| captured(Arm::Baseline, i, target)).collect(),
            candidate: (0..repetitions).map(|i| captured(Arm::Candidate, i, target)).collect(),
        }
    }

    fn input(&self) -> PairEvidence<'_> {
        PairEvidence {
            plan: PLAN, target: self.target, preserved: PRESERVED,
            qualification: Some(QUALIFICATION), repetitions: self.repetitions,
            substitution: Some(Substitution {
                review: Some(REVIEW), baseline_snapshot: BASELINE, candidate_snapshot: CANDIDATE,
                unchanged_inputs: UNCHANGED, edits: &EDITS,
            }),
            baseline: ArmEvidence { attempts: &self.baseline, complete: true },
            candidate: ArmEvidence { attempts: &self.candidate, complete: true },
        }
    }

    fn select(&mut self, target: Target) {
        self.target = target;
        for item in self.baseline.iter_mut().chain(self.candidate.iter_mut()) {
            item.selection = Selection::Exact(target.case);
            event(item).target = target;
        }
    }
}

fn facts(item: &mut Attempt) -> &mut Provenance { item.provenance.as_mut().unwrap() }

fn event(item: &mut Attempt) -> &mut AssertionEvent {
    match &mut item.assertion {
        AssertionCapture::ExactlyOne(event) => event,
        _ => panic!("fixture must contain one assertion event"),
    }
}

fn fail_at_target(attempts: &mut [Attempt]) {
    for item in attempts {
        let event = event(item);
        event.result = AssertionResult::Failed;
        let cause = event.event;
        item.case_outcome = CaseOutcome::AssertionFailed(cause);
    }
}

fn unknown(input: PairEvidence<'_>, reason: Reason, arm: Option<Arm>, repetition: Option<u8>) {
    let comparison = compare(&input);
    assert_eq!(comparison.outcome, Outcome::Unknown(Problem { reason, arm, repetition }));
    assert_eq!(comparison.support, None);
}

#[test]
fn pass_pass_only_reports_no_discrimination_for_the_measured_target() {
    // Different observations may both satisfy a weak oracle, such as x > 0.
    let fixture = Fixture::new(MIN_REPETITIONS);
    let result = compare(&fixture.input());
    assert_eq!(result.outcome, Outcome::NoDiscrimination);
    assert_eq!(result.target, fixture.target);
    let support = result.support.unwrap();
    assert_eq!(support.baseline_observation, [30; 32]);
    assert_eq!(support.candidate_observation, [31; 32]);
    assert_eq!(support.repetitions, MIN_REPETITIONS);
    assert_eq!(result.limitations, [
        Limitation::SuppliedFactsNotVerified, Limitation::OneCaseRowAssertionOnly,
        Limitation::OracleMeaningNotVerified, Limitation::FiniteRepetitionsOnly,
        Limitation::FirstProblemOnly,
    ]);
}

#[test]
fn reviewed_production_substitution_discriminates_at_the_intended_assertion() {
    let mut fixture = Fixture::new(2);
    fail_at_target(&mut fixture.baseline);
    let result = compare(&fixture.input());
    assert_eq!(result.outcome, Outcome::Discriminating);
    assert_eq!(result.support.unwrap().baseline_first_receipt, [90; 32]);
    assert_eq!(result.support.unwrap().candidate_first_receipt, [100; 32]);
}

#[test]
fn same_source_no_op_is_an_explicit_negative_control() {
    let mut fixture = Fixture::new(2);
    for item in &mut fixture.candidate {
        facts(item).snapshot = BASELINE;
        facts(item).build = [17; 32];
        event(item).observation = [30; 32];
    }
    let mut input = fixture.input();
    let substitution = input.substitution.as_mut().unwrap();
    substitution.candidate_snapshot = BASELINE;
    substitution.edits = &[];
    assert_eq!(compare(&input).outcome, Outcome::NoDiscrimination);
}

#[test]
fn no_op_fail_pass_is_unexplained_variability_not_discrimination() {
    let mut fixture = Fixture::new(2);
    fail_at_target(&mut fixture.baseline);
    for item in &mut fixture.candidate {
        facts(item).snapshot = BASELINE;
    }
    let mut input = fixture.input();
    let substitution = input.substitution.as_mut().unwrap();
    substitution.candidate_snapshot = BASELINE;
    substitution.edits = &[];
    unknown(input, Reason::UnstableObservations, None, None);
}

#[test]
fn identical_loaded_builds_cannot_supply_fail_pass_discrimination() {
    let mut fixture = Fixture::new(2);
    fail_at_target(&mut fixture.baseline);
    for item in &mut fixture.candidate {
        facts(item).build = [17; 32];
    }
    unknown(fixture.input(), Reason::UnstableObservations, None, None);
}

#[test]
fn exact_data_row_is_supported_but_mixed_or_ambiguous_rows_are_unknown() {
    let mut fixture = Fixture::new(2);
    let mut selected = fixture.target;
    selected.case.row = RowKey::Data([150; 32]);
    fixture.select(selected);
    fail_at_target(&mut fixture.baseline);
    assert_eq!(compare(&fixture.input()).outcome, Outcome::Discriminating);
    fixture.candidate[1].selection = Selection::Ambiguous;
    unknown(fixture.input(), Reason::CaseAmbiguous, Some(Arm::Candidate), Some(1));
    let mut other_row = selected.case;
    other_row.row = RowKey::Data([151; 32]);
    fixture.candidate[1].selection = Selection::Exact(other_row);
    unknown(fixture.input(), Reason::CaseMismatch, Some(Arm::Candidate), Some(1));
}

#[test]
fn unrelated_setup_exception_or_cleanup_failure_is_not_discrimination() {
    for kind in [FailureKind::Setup, FailureKind::Exception, FailureKind::Teardown,
        FailureKind::Build, FailureKind::Runner, FailureKind::Timeout, FailureKind::Cancelled] {
        let mut fixture = Fixture::new(2);
        fixture.baseline[0].case_outcome = CaseOutcome::OtherFailure(kind);
        unknown(fixture.input(), Reason::OtherCaseFailure(kind), Some(Arm::Baseline), Some(0));
    }
}

#[test]
fn terminal_failure_must_name_the_captured_assertion_event() {
    let mut fixture = Fixture::new(2);
    fail_at_target(&mut fixture.baseline);
    fixture.baseline[0].case_outcome = CaseOutcome::AssertionFailed([200; 32]);
    unknown(fixture.input(), Reason::UnrelatedAssertionFailure, Some(Arm::Baseline), Some(0));
}

#[test]
fn wrong_assertion_or_oracle_and_contradictory_outcomes_are_unknown() {
    let mut fixture = Fixture::new(2);
    event(&mut fixture.baseline[0]).target.assertion = [201; 32];
    unknown(fixture.input(), Reason::AssertionMismatch, Some(Arm::Baseline), Some(0));
    event(&mut fixture.baseline[0]).target = fixture.target;
    event(&mut fixture.baseline[0]).target.oracle = [202; 32];
    unknown(fixture.input(), Reason::AssertionMismatch, Some(Arm::Baseline), Some(0));
    event(&mut fixture.baseline[0]).target = fixture.target;
    event(&mut fixture.baseline[0]).result = AssertionResult::Failed;
    unknown(fixture.input(), Reason::ContradictoryOutcome, Some(Arm::Baseline), Some(0));
}

#[test]
fn candidate_failure_and_both_arms_failure_are_unknown() {
    let mut fixture = Fixture::new(2);
    fail_at_target(&mut fixture.candidate);
    unknown(fixture.input(), Reason::CandidateFailed, Some(Arm::Candidate), None);
    fail_at_target(&mut fixture.baseline);
    unknown(fixture.input(), Reason::BothArmsFailed, None, None);
}

#[test]
fn sdk_runner_and_other_actual_environment_mismatches_are_unknown() {
    let changes: [(EnvironmentField, fn(&mut Environment)); 7] = [
        (EnvironmentField::Sdk, |v| v.sdk = [200; 32]),
        (EnvironmentField::Runner, |v| v.runner = [200; 32]),
        (EnvironmentField::Adapter, |v| v.adapter = [200; 32]),
        (EnvironmentField::Target, |v| v.target = [200; 32]),
        (EnvironmentField::Dependencies, |v| v.dependencies = [200; 32]),
        (EnvironmentField::Settings, |v| v.settings = [200; 32]),
        (EnvironmentField::RuntimeEnvironment, |v| v.runtime_environment = [200; 32]),
    ];
    for (field, change) in changes {
        let mut fixture = Fixture::new(2);
        change(&mut facts(&mut fixture.candidate[0]).environment);
        unknown(fixture.input(), Reason::EnvironmentMismatch(field), Some(Arm::Candidate), Some(0));
    }
}

#[test]
fn changed_test_or_fixture_bytes_are_unknown_even_with_pass_pass() {
    let mut fixture = Fixture::new(2);
    facts(&mut fixture.candidate[0]).preserved.tests = [200; 32];
    unknown(fixture.input(), Reason::PreservedTestMismatch, Some(Arm::Candidate), Some(0));
    facts(&mut fixture.candidate[0]).preserved = PRESERVED;
    facts(&mut fixture.candidate[0]).preserved.fixture = [200; 32];
    unknown(fixture.input(), Reason::FixtureMismatch, Some(Arm::Candidate), Some(0));
}

#[test]
fn unstable_results_or_semantic_observations_are_unknown() {
    let mut fixture = Fixture::new(2);
    event(&mut fixture.baseline[1]).observation = [200; 32];
    unknown(fixture.input(), Reason::UnstableObservations, Some(Arm::Baseline), Some(1));
    event(&mut fixture.baseline[1]).observation = [30; 32];
    fail_at_target(&mut fixture.baseline[1..]);
    unknown(fixture.input(), Reason::UnstableObservations, Some(Arm::Baseline), Some(1));
}

#[test]
fn identical_deterministic_oracle_input_cannot_both_fail_and_pass() {
    let mut fixture = Fixture::new(2);
    fail_at_target(&mut fixture.baseline);
    for item in &mut fixture.baseline { event(item).observation = [31; 32]; }
    unknown(fixture.input(), Reason::ContradictoryOutcome, None, None);
}

#[test]
fn missing_provenance_or_capture_qualification_is_unknown() {
    let mut fixture = Fixture::new(2);
    let mut input = fixture.input();
    input.qualification = None;
    unknown(input, Reason::UnqualifiedCapture, None, None);
    fixture.baseline[1].provenance = None;
    unknown(fixture.input(), Reason::MissingProvenance, Some(Arm::Baseline), Some(1));
    fixture.baseline[1] = captured(Arm::Baseline, 1, fixture.target);
    facts(&mut fixture.candidate[0]).qualification = [200; 32];
    unknown(fixture.input(), Reason::QualificationMismatch, Some(Arm::Candidate), Some(0));
}

#[test]
fn provenance_must_bind_the_plan_snapshots_admission_and_unchanged_inputs() {
    let changes: [(Reason, fn(&mut Provenance)); 4] = [
        (Reason::PlanMismatch, |v| v.plan = [200; 32]),
        (Reason::SnapshotMismatch, |v| v.snapshot = [200; 32]),
        (Reason::SubstitutionMismatch, |v| v.substitution_review = [200; 32]),
        (Reason::OtherInputsMismatch, |v| v.unchanged_inputs = [200; 32]),
    ];
    for (reason, change) in changes {
        let mut fixture = Fixture::new(2);
        change(facts(&mut fixture.candidate[0]));
        unknown(fixture.input(), reason, Some(Arm::Candidate), Some(0));
    }
    let mut fixture = Fixture::new(2);
    facts(&mut fixture.baseline[1]).build = [200; 32];
    unknown(fixture.input(), Reason::BuildChanged, Some(Arm::Baseline), Some(1));
}

#[test]
fn missing_or_partial_attempt_sets_cannot_become_best_of_retries() {
    let mut fixture = Fixture::new(2);
    let mut input = fixture.input();
    input.baseline.complete = false;
    unknown(input, Reason::IncompleteArm, Some(Arm::Baseline), None);
    fixture.baseline.pop();
    unknown(fixture.input(), Reason::RepetitionSet, Some(Arm::Baseline), None);
    fixture.baseline.push(captured(Arm::Baseline, 0, fixture.target));
    unknown(fixture.input(), Reason::RepetitionSet, Some(Arm::Baseline), None);
    fixture.baseline[1].repetition = 2;
    unknown(fixture.input(), Reason::RepetitionSet, Some(Arm::Baseline), None);
}

#[test]
fn missing_skipped_or_incomplete_cases_do_not_supply_assertion_evidence() {
    for (outcome, reason) in [
        (CaseOutcome::Skipped, Reason::CaseSkipped),
        (CaseOutcome::NotRun, Reason::CaseNotRun),
        (CaseOutcome::Incomplete, Reason::CaseIncomplete),
    ] {
        let mut fixture = Fixture::new(2);
        fixture.candidate[0].case_outcome = outcome;
        unknown(fixture.input(), reason, Some(Arm::Candidate), Some(0));
    }
    for (selection, reason) in [
        (Selection::Missing, Reason::CaseMissing),
        (Selection::Unsupported, Reason::CaseUnsupported),
    ] {
        let mut fixture = Fixture::new(2);
        fixture.baseline[0].selection = selection;
        unknown(fixture.input(), reason, Some(Arm::Baseline), Some(0));
    }
}

#[test]
fn assertion_not_reached_multiple_occurrences_or_unsupported_capture_are_unknown() {
    for (capture, reason) in [
        (AssertionCapture::NotReached, Reason::AssertionNotReached),
        (AssertionCapture::Ambiguous, Reason::AssertionAmbiguous),
        (AssertionCapture::Unsupported, Reason::AssertionUnsupported),
    ] {
        let mut fixture = Fixture::new(2);
        fixture.candidate[0].assertion = capture;
        unknown(fixture.input(), reason, Some(Arm::Candidate), Some(0));
    }
}

#[test]
fn repeated_attempt_receipt_or_isolation_identity_is_rejected_across_arms() {
    for reason in [Reason::ReusedAttempt, Reason::ReusedReceipt, Reason::ReusedIsolation] {
        let mut fixture = Fixture::new(2);
        let prior = fixture.baseline[0].provenance.unwrap();
        let current = facts(&mut fixture.candidate[0]);
        match reason {
            Reason::ReusedAttempt => current.attempt = prior.attempt,
            Reason::ReusedReceipt => current.receipt = prior.receipt,
            Reason::ReusedIsolation => current.isolation = prior.isolation,
            _ => unreachable!(),
        }
        unknown(fixture.input(), reason, Some(Arm::Candidate), Some(0));
    }
}

#[test]
fn repetition_bounds_accept_the_maximum_and_reject_exhaustion() {
    let fixture = Fixture::new(MAX_REPETITIONS);
    assert_eq!(compare(&fixture.input()).outcome, Outcome::NoDiscrimination);
    for count in [0, MIN_REPETITIONS - 1, MAX_REPETITIONS + 1] {
        let fixture = Fixture::new(count);
        unknown(fixture.input(), Reason::InvalidRepetitionBudget, None, None);
    }
    let mut fixture = Fixture::new(2);
    fixture.baseline = (0..=MAX_REPETITIONS)
        .map(|i| captured(Arm::Baseline, i, fixture.target)).collect();
    let result = compare(&fixture.input());
    assert_eq!(result.baseline_attempts, usize::from(MAX_REPETITIONS) + 1);
    unknown(fixture.input(), Reason::TooManyAttempts, Some(Arm::Baseline), None);
}

#[test]
fn substitution_is_required_and_must_be_reviewed_and_consistent() {
    let fixture = Fixture::new(2);
    let mut input = fixture.input();
    input.substitution = None;
    unknown(input, Reason::MissingSubstitution, None, None);
    let mut input = fixture.input();
    input.substitution.as_mut().unwrap().review = None;
    unknown(input, Reason::UnreviewedSubstitution, None, None);
    let mut input = fixture.input();
    input.substitution.as_mut().unwrap().edits = &[];
    unknown(input, Reason::InvalidEditSet, None, None);
    let mut input = fixture.input();
    input.substitution.as_mut().unwrap().candidate_snapshot = BASELINE;
    unknown(input, Reason::InvalidEditSet, None, None);
}

#[test]
fn production_edit_paths_must_be_relative_canonical_and_strictly_ordered() {
    let fixture = Fixture::new(2);
    for path in ["", "/src/A.cs", "src/../A.cs", "src/./A.cs", "src//A.cs",
        "src\\A.cs", "src/A.cs/", "src/A.cs.", "C:/src/A.cs", "src/\nA.cs"] {
        let edits = [ProductionEdit { path, before: [40; 32], after: [41; 32] }];
        let mut input = fixture.input();
        input.substitution.as_mut().unwrap().edits = &edits;
        unknown(input, Reason::InvalidEditPath, None, None);
    }
    for paths in [["src/B.cs", "src/A.cs"], ["src/A.cs", "src/A.cs"]] {
        let edits = paths.map(|path| ProductionEdit { path, before: [40; 32], after: [41; 32] });
        let mut input = fixture.input();
        input.substitution.as_mut().unwrap().edits = &edits;
        unknown(input, Reason::InvalidEditSet, None, None);
    }
    let edits = [ProductionEdit { path: "src/A.cs", before: [40; 32], after: [40; 32] }];
    let mut input = fixture.input();
    input.substitution.as_mut().unwrap().edits = &edits;
    unknown(input, Reason::InvalidEditSet, None, None);
}

#[test]
fn path_byte_and_edit_count_limits_accept_maximum_and_reject_one_above() {
    let fixture = Fixture::new(2);
    let prefix = "segment/".repeat(40);
    let at_limit = format!("{}{}.cs", prefix, "a".repeat(MAX_PATH_BYTES - prefix.len() - 3));
    let above_limit = format!("{at_limit}x");
    for (path, expected) in [(at_limit.as_str(), Outcome::NoDiscrimination),
        (above_limit.as_str(), Outcome::Unknown(Problem {
            reason: Reason::InvalidEditPath, arm: None, repetition: None,
        }))] {
        let edits = [ProductionEdit { path, before: [40; 32], after: [41; 32] }];
        let mut input = fixture.input();
        input.substitution.as_mut().unwrap().edits = &edits;
        assert_eq!(compare(&input).outcome, expected);
    }
    for count in [MAX_SUBSTITUTIONS, MAX_SUBSTITUTIONS + 1] {
        let paths: Vec<String> = (0..count).map(|i| format!("src/File{i:02}.cs")).collect();
        let edits: Vec<ProductionEdit<'_>> = paths.iter().map(|path| ProductionEdit {
            path: path.as_str(), before: [40; 32], after: [41; 32],
        }).collect();
        let mut input = fixture.input();
        input.substitution.as_mut().unwrap().edits = &edits;
        if count == MAX_SUBSTITUTIONS {
            assert_eq!(compare(&input).outcome, Outcome::NoDiscrimination);
        } else {
            unknown(input, Reason::TooManyEdits, None, None);
        }
    }
}

#[test]
fn attempt_order_does_not_change_support_or_the_first_reported_problem() {
    let mut fixture = Fixture::new(3);
    fail_at_target(&mut fixture.baseline);
    let expected = compare(&fixture.input());
    fixture.baseline.reverse();
    fixture.candidate.rotate_left(1);
    assert_eq!(compare(&fixture.input()), expected);
    fixture.baseline.iter_mut().find(|item| item.repetition == 0).unwrap().selection = Selection::Ambiguous;
    fixture.baseline.iter_mut().find(|item| item.repetition == 1).unwrap().selection = Selection::Unsupported;
    let expected = compare(&fixture.input());
    assert_eq!(expected.outcome, Outcome::Unknown(Problem {
        reason: Reason::CaseAmbiguous, arm: Some(Arm::Baseline), repetition: Some(0),
    }));
    fixture.baseline.reverse();
    fixture.candidate.reverse();
    assert_eq!(compare(&fixture.input()), expected);
}

#[test]
fn baseline_provenance_precedes_missing_candidate_provenance_after_reordering() {
    let mut fixture = Fixture::new(2);
    facts(&mut fixture.baseline[1]).plan = [200; 32];
    fixture.candidate[0].provenance = None;
    let expected = compare(&fixture.input());
    assert_eq!(expected.outcome, Outcome::Unknown(Problem {
        reason: Reason::PlanMismatch, arm: Some(Arm::Baseline), repetition: Some(1),
    }));
    fixture.baseline.reverse();
    fixture.candidate.reverse();
    assert_eq!(compare(&fixture.input()), expected);
}
