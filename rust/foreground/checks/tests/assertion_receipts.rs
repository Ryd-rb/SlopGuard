//! Source-only cases for SH-FOLLOW-07; not executed by the proposal packet.
//! Supervisor/qualification facts and repeated-byte identities are synthetic.
//! Hashing these JSON bytes tests binding, not real producer qualification,
//! execution, loaded-artifact attestation, oracle correctness or authentication.

use serde_json::{Value, json};
use sha2::{Digest as _, Sha256};
use slophawk_foreground_checks::assertion_receipts::{
    self as receipts, HeaderField, Qualification, Reason as ReceiptReason, Supervisor, Validation,
};
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

fn owner(expected: i64, row: RowKey) -> Qualification {
    Qualification {
        record: QUALIFICATION, adapter: [22; 32], preserved: PRESERVED, expected,
        target: Target {
            case: CaseKey { test: [2; 32], row },
            assertion: [3; 32], oracle: receipts::oracle_id(expected),
        },
    }
}

fn supervisor(owner: &Qualification, arm: Arm, repetition: u8) -> Supervisor {
    let offset = if arm == Arm::Baseline { 0 } else { 10 };
    Supervisor {
        arm, repetition, selection: Selection::Exact(owner.target.case), capture_complete: true,
        provenance: Provenance {
            plan: PLAN, attempt: [70 + offset + repetition; 32], receipt: [0; 32],
            isolation: [110 + offset + repetition; 32],
            snapshot: if arm == Arm::Baseline { BASELINE } else { CANDIDATE },
            build: if arm == Arm::Baseline { [17; 32] } else { [18; 32] },
            substitution_review: REVIEW, unchanged_inputs: UNCHANGED,
            qualification: owner.record, preserved: owner.preserved,
            environment: Environment {
                sdk: [20; 32], runner: [21; 32], adapter: owner.adapter,
                target: [23; 32], dependencies: [24; 32], settings: [25; 32],
                runtime_environment: [26; 32],
            },
        },
    }
}

fn case_json(case: CaseKey) -> Value {
    let (row, row_key) = match case.row {
        RowKey::Single => ("single", [0; 32]),
        RowKey::Data(key) => ("data", key),
    };
    json!({ "test": case.test, "row": row, "row_key": row_key })
}

fn document(owner: &Qualification, supervisor: &Supervisor, actual: i64) -> Value {
    let p = supervisor.provenance;
    let event = receipts::event_id(p.attempt, owner.target);
    let passed = actual == owner.expected;
    json!({
        "format": receipts::FORMAT,
        "header": {
            "plan": p.plan, "attempt": p.attempt,
            "arm": if supervisor.arm == Arm::Baseline { "baseline" } else { "candidate" },
            "repetition": supervisor.repetition, "isolation": p.isolation,
            "snapshot": p.snapshot, "build": p.build,
            "substitution_review": p.substitution_review, "unchanged_inputs": p.unchanged_inputs,
            "qualification": p.qualification, "adapter": p.environment.adapter,
            "tests": p.preserved.tests, "fixture": p.preserved.fixture,
        },
        "case": case_json(owner.target.case), "complete": true,
        "setup": "completed", "teardown": "completed",
        "assertions": [{
            "attempt": p.attempt, "case": case_json(owner.target.case),
            "assertion": owner.target.assertion, "oracle": owner.target.oracle, "event": event,
            "actual": actual, "result": if passed { "passed" } else { "failed" },
        }],
        "terminal": {
            "kind": if passed { "passed" } else { "assertion_failed" },
            "event": if passed { None } else { Some(event) },
        },
    })
}

struct Capture { bytes: Vec<u8>, supervisor: Supervisor }

impl Capture {
    // This helper deliberately models separately supplied synthetic supervisor
    // authority. Production callers must never derive authority from the JSON.
    fn from_bytes(mut supervisor: Supervisor, bytes: Vec<u8>) -> Self {
        supervisor.provenance.receipt = Sha256::digest(&bytes).into();
        Self { bytes, supervisor }
    }

    fn from_document(supervisor: Supervisor, document: &Value) -> Self {
        Self::from_bytes(supervisor, serde_json::to_vec(document).unwrap())
    }

    fn valid(owner: &Qualification, arm: Arm, repetition: u8, actual: i64) -> Self {
        let supervisor = supervisor(owner, arm, repetition);
        Self::from_document(supervisor, &document(owner, &supervisor, actual))
    }

    fn validate(&self, owner: &Qualification) -> Validation {
        receipts::validate(Some(&self.bytes), Some(owner), Some(&self.supervisor))
    }

    fn complete(&self, owner: &Qualification) -> Attempt {
        match self.validate(owner) {
            Validation::Complete(attempt) => attempt,
            other => panic!("synthetic complete capture rejected: {other:?}"),
        }
    }
}

fn rejected(owner: &Qualification, supervisor: Supervisor, value: &Value, reason: ReceiptReason) {
    assert_eq!(Capture::from_document(supervisor, value).validate(owner), Validation::Unknown(reason));
}

struct Pair {
    owner: Qualification,
    repetitions: u8,
    baseline: Vec<Attempt>,
    candidate: Vec<Attempt>,
}

impl Pair {
    fn new(owner: Qualification, baseline: &[i64], candidate: &[i64]) -> Self {
        let captures = |arm, values: &[i64]| values.iter().enumerate().map(|(i, actual)| {
            Capture::valid(&owner, arm, i as u8, *actual).complete(&owner)
        }).collect();
        Self {
            repetitions: baseline.len() as u8,
            baseline: captures(Arm::Baseline, baseline),
            candidate: captures(Arm::Candidate, candidate), owner,
        }
    }

    fn input(&self) -> PairEvidence<'_> {
        PairEvidence {
            plan: PLAN, target: self.owner.target, preserved: PRESERVED,
            qualification: Some(QUALIFICATION), repetitions: self.repetitions,
            substitution: Some(Substitution {
                review: Some(REVIEW), baseline_snapshot: BASELINE,
                candidate_snapshot: CANDIDATE, unchanged_inputs: UNCHANGED, edits: &EDITS,
            }),
            baseline: ArmEvidence { attempts: &self.baseline, complete: true },
            candidate: ArmEvidence { attempts: &self.candidate, complete: true },
        }
    }
}

fn pair_unknown(input: PairEvidence<'_>, reason: Reason, arm: Option<Arm>, repetition: Option<u8>) {
    let result = compare(&input);
    assert_eq!(result.outcome, Outcome::Unknown(Problem { reason, arm, repetition }));
    assert_eq!(result.support, None);
}

#[test]
fn complete_receipts_feed_the_existing_pair_comparator_without_inventing_a_pair_verdict() {
    let owner = owner(7, RowKey::Single);
    let pass_pass = Pair::new(owner, &[7, 7], &[7, 7]);
    assert_eq!(compare(&pass_pass.input()).outcome, Outcome::NoDiscrimination);
    let discriminating = Pair::new(owner, &[6, 6], &[7, 7]);
    let result = compare(&discriminating.input());
    assert_eq!(result.outcome, Outcome::Discriminating);
    let support = result.support.unwrap();
    assert_eq!(support.baseline_first_receipt, discriminating.baseline[0].provenance.unwrap().receipt);
    assert_eq!(support.candidate_first_receipt, discriminating.candidate[0].provenance.unwrap().receipt);
    assert_ne!(support.baseline_observation, support.candidate_observation);
    assert!(result.limitations.contains(&Limitation::SuppliedFactsNotVerified));
}

#[test]
fn data_row_identity_must_match_in_selection_case_and_event() {
    let owner = owner(7, RowKey::Data([150; 32]));
    let pair = Pair::new(owner, &[6, 6], &[7, 7]);
    assert_eq!(compare(&pair.input()).outcome, Outcome::Discriminating);
    let supervisor = supervisor(&owner, Arm::Baseline, 0);
    let mut value = document(&owner, &supervisor, 6);
    value["case"]["row_key"] = json!(([151; 32]));
    rejected(&owner, supervisor, &value, ReceiptReason::CaseMismatch);
    value = document(&owner, &supervisor, 6);
    value["assertions"][0]["case"]["row_key"] = json!(([151; 32]));
    rejected(&owner, supervisor, &value, ReceiptReason::EventCaseMismatch);
    let mut other_selection = supervisor;
    other_selection.selection = Selection::Exact(CaseKey { test: owner.target.case.test, row: RowKey::Data([151; 32]) });
    rejected(&owner, other_selection, &document(&owner, &supervisor, 6), ReceiptReason::CaseMismatch);
}

#[test]
fn display_names_and_ordinal_rows_are_not_identity_fallbacks() {
    let owner = owner(7, RowKey::Single);
    let supervisor = supervisor(&owner, Arm::Baseline, 0);
    for (field, value) in [("display_name", json!("Test(row 1)")), ("row_ordinal", json!(1))] {
        let mut receipt = document(&owner, &supervisor, 7);
        receipt["case"][field] = value;
        rejected(&owner, supervisor, &receipt, ReceiptReason::MalformedReceipt);
    }
    let mut value = document(&owner, &supervisor, 7);
    value["case"]["row_key"] = json!(([1; 32]));
    rejected(&owner, supervisor, &value, ReceiptReason::MalformedReceipt);
}

#[test]
fn body_assertion_does_not_hide_setup_or_teardown_failure() {
    let owner = owner(7, RowKey::Single);
    let supervisor = supervisor(&owner, Arm::Baseline, 0);
    for (field, phase, reason) in [
        ("setup", "failed", ReceiptReason::OtherCaseFailure(FailureKind::Setup)),
        ("teardown", "failed", ReceiptReason::OtherCaseFailure(FailureKind::Teardown)),
        ("setup", "not_run", ReceiptReason::SetupIncomplete),
        ("teardown", "incomplete", ReceiptReason::TeardownIncomplete),
    ] {
        let mut value = document(&owner, &supervisor, 6);
        value[field] = json!(phase);
        rejected(&owner, supervisor, &value, reason);
    }
}

#[test]
fn unrelated_terminal_outcomes_never_become_assertion_failures() {
    let owner = owner(7, RowKey::Single);
    let supervisor = supervisor(&owner, Arm::Baseline, 0);
    for (kind, reason) in [
        ("setup_failed", ReceiptReason::OtherCaseFailure(FailureKind::Setup)),
        ("teardown_failed", ReceiptReason::OtherCaseFailure(FailureKind::Teardown)),
        ("exception", ReceiptReason::OtherCaseFailure(FailureKind::Exception)),
        ("build_failed", ReceiptReason::OtherCaseFailure(FailureKind::Build)),
        ("runner_failed", ReceiptReason::OtherCaseFailure(FailureKind::Runner)),
        ("timeout", ReceiptReason::OtherCaseFailure(FailureKind::Timeout)),
        ("cancelled", ReceiptReason::OtherCaseFailure(FailureKind::Cancelled)),
        ("skipped", ReceiptReason::CaseSkipped),
        ("not_run", ReceiptReason::CaseNotRun),
        ("incomplete", ReceiptReason::CaseIncomplete),
    ] {
        let mut value = document(&owner, &supervisor, 6);
        value["terminal"]["kind"] = json!(kind);
        value["terminal"]["event"] = Value::Null;
        rejected(&owner, supervisor, &value, reason);
    }
}

#[test]
fn omitted_and_duplicate_assertions_do_not_supply_exactly_one_capture() {
    let owner = owner(7, RowKey::Single);
    let supervisor = supervisor(&owner, Arm::Baseline, 0);
    let original = document(&owner, &supervisor, 6);
    for (count, reason) in [
        (0, ReceiptReason::AssertionNotReached),
        (2, ReceiptReason::AssertionAmbiguous),
        (receipts::MAX_ASSERTIONS, ReceiptReason::AssertionAmbiguous),
        (receipts::MAX_ASSERTIONS + 1, ReceiptReason::TooManyAssertions),
    ] {
        let mut value = original.clone();
        value["assertions"] = Value::Array(vec![original["assertions"][0].clone(); count]);
        assert!(serde_json::to_vec(&value).unwrap().len() <= receipts::MAX_RECEIPT_BYTES);
        rejected(&owner, supervisor, &value, reason);
    }
}

#[test]
fn terminal_failure_must_name_the_single_bound_event() {
    let owner = owner(7, RowKey::Single);
    let supervisor = supervisor(&owner, Arm::Baseline, 0);
    for cause in [Value::Null, json!(([200; 32]))] {
        let mut value = document(&owner, &supervisor, 6);
        value["terminal"]["event"] = cause;
        rejected(&owner, supervisor, &value, ReceiptReason::UnrelatedAssertionFailure);
    }
    let mut value = document(&owner, &supervisor, 6);
    value["terminal"].as_object_mut().unwrap().remove("event");
    rejected(&owner, supervisor, &value, ReceiptReason::UnrelatedAssertionFailure);
}

#[test]
fn oracle_and_terminal_verdicts_are_checked_against_the_observed_integer() {
    let owner = owner(7, RowKey::Single);
    let supervisor = supervisor(&owner, Arm::Baseline, 0);
    let mut value = document(&owner, &supervisor, 6);
    value["assertions"][0]["result"] = json!("passed");
    rejected(&owner, supervisor, &value, ReceiptReason::ContradictoryOutcome);
    value = document(&owner, &supervisor, 7);
    value["terminal"]["kind"] = json!("assertion_failed");
    value["terminal"]["event"] = value["assertions"][0]["event"].clone();
    rejected(&owner, supervisor, &value, ReceiptReason::ContradictoryOutcome);
    value = document(&owner, &supervisor, 7);
    value["terminal"]["event"] = value["assertions"][0]["event"].clone();
    rejected(&owner, supervisor, &value, ReceiptReason::ContradictoryOutcome);
}

#[test]
fn cross_attempt_receipt_relabeling_cannot_leave_an_old_event_or_cause() {
    let owner = owner(7, RowKey::Single);
    let first = supervisor(&owner, Arm::Baseline, 0);
    let second = supervisor(&owner, Arm::Baseline, 1);
    let mut value = document(&owner, &first, 6);
    rejected(&owner, second, &value, ReceiptReason::HeaderMismatch(HeaderField::Attempt));
    value["header"] = document(&owner, &second, 6)["header"].clone();
    rejected(&owner, second, &value, ReceiptReason::EventAttemptMismatch);
    value["assertions"][0]["attempt"] = json!(second.provenance.attempt);
    rejected(&owner, second, &value, ReceiptReason::EventIdentityMismatch);
    value["assertions"][0]["event"] = json!(receipts::event_id(second.provenance.attempt, owner.target));
    rejected(&owner, second, &value, ReceiptReason::UnrelatedAssertionFailure);
}

#[test]
fn changing_all_receipt_claims_does_not_change_the_independent_byte_binding() {
    let owner = owner(7, RowKey::Single);
    let captured = Capture::valid(&owner, Arm::Baseline, 0, 6);
    let mut claimed_supervisor = captured.supervisor;
    claimed_supervisor.provenance.attempt = [200; 32];
    let rewritten = serde_json::to_vec(&document(&owner, &claimed_supervisor, 6)).unwrap();
    assert_eq!(
        receipts::validate(Some(&rewritten), Some(&owner), Some(&claimed_supervisor)),
        Validation::Unknown(ReceiptReason::ReceiptDigestMismatch),
    );
    // A caller that also fabricates a replacement supervisor digest is outside
    // this validator's authentication boundary, not a new trusted producer.
}

#[test]
fn independent_qualification_and_source_closure_cannot_be_overridden_by_json() {
    let owner = owner(7, RowKey::Single);
    let changes: [(ReceiptReason, fn(&mut Supervisor)); 4] = [
        (ReceiptReason::QualificationMismatch, |s| s.provenance.qualification = [200; 32]),
        (ReceiptReason::AdapterMismatch, |s| s.provenance.environment.adapter = [200; 32]),
        (ReceiptReason::PreservedTestMismatch, |s| s.provenance.preserved.tests = [200; 32]),
        (ReceiptReason::FixtureMismatch, |s| s.provenance.preserved.fixture = [200; 32]),
    ];
    for (reason, change) in changes {
        let mut supervisor = supervisor(&owner, Arm::Baseline, 0);
        change(&mut supervisor);
        rejected(&owner, supervisor, &document(&owner, &supervisor, 6), reason);
    }
    let capture = Capture::valid(&owner, Arm::Baseline, 0, 6);
    let mut wrong_oracle = owner;
    wrong_oracle.expected = 8;
    assert_eq!(capture.validate(&wrong_oracle), Validation::Unknown(ReceiptReason::OracleMismatch));
}

#[test]
fn stale_manifest_header_claims_are_checked_even_when_the_bytes_are_captured() {
    let owner = owner(7, RowKey::Single);
    let supervisor = supervisor(&owner, Arm::Baseline, 0);
    for (field, expected) in [
        ("plan", HeaderField::Plan), ("snapshot", HeaderField::Snapshot),
        ("build", HeaderField::Build), ("substitution_review", HeaderField::SubstitutionReview),
        ("unchanged_inputs", HeaderField::UnchangedInputs), ("adapter", HeaderField::Adapter),
        ("tests", HeaderField::Tests), ("fixture", HeaderField::Fixture),
    ] {
        let mut value = document(&owner, &supervisor, 6);
        value["header"][field] = json!(([200; 32]));
        rejected(&owner, supervisor, &value, ReceiptReason::HeaderMismatch(expected));
    }
    for (field, value, expected) in [
        ("arm", json!("candidate"), HeaderField::Arm),
        ("repetition", json!(1), HeaderField::Repetition),
    ] {
        let mut receipt = document(&owner, &supervisor, 6);
        receipt["header"][field] = value;
        rejected(&owner, supervisor, &receipt, ReceiptReason::HeaderMismatch(expected));
    }
}

#[test]
fn exact_event_identity_includes_the_assertion_oracle_and_target() {
    let owner = owner(7, RowKey::Single);
    let supervisor = supervisor(&owner, Arm::Baseline, 0);
    for field in ["assertion", "oracle"] {
        let mut value = document(&owner, &supervisor, 6);
        value["assertions"][0][field] = json!(([200; 32]));
        rejected(&owner, supervisor, &value, ReceiptReason::AssertionMismatch);
    }
    let mut value = document(&owner, &supervisor, 6);
    value["assertions"][0]["event"] = json!(([200; 32]));
    rejected(&owner, supervisor, &value, ReceiptReason::EventIdentityMismatch);
    let mut other_target = owner.target;
    other_target.case.row = RowKey::Data([0; 32]);
    assert_ne!(receipts::event_id(supervisor.provenance.attempt, owner.target),
        receipts::event_id(supervisor.provenance.attempt, other_target));
}

#[test]
fn missing_authority_or_incomplete_capture_never_produces_a_partial_attempt() {
    let owner = owner(7, RowKey::Single);
    let capture = Capture::valid(&owner, Arm::Baseline, 0, 6);
    assert_eq!(receipts::validate(Some(&capture.bytes), None, Some(&capture.supervisor)),
        Validation::Unknown(ReceiptReason::UnqualifiedCapture));
    assert_eq!(receipts::validate(Some(&capture.bytes), Some(&owner), None),
        Validation::Unknown(ReceiptReason::MissingProvenance));
    assert_eq!(receipts::validate(None, Some(&owner), Some(&capture.supervisor)),
        Validation::Unknown(ReceiptReason::MissingReceipt));
    let mut incomplete = capture.supervisor;
    incomplete.capture_complete = false;
    assert_eq!(receipts::validate(Some(&capture.bytes), Some(&owner), Some(&incomplete)),
        Validation::Unknown(ReceiptReason::IncompleteCapture));
    let mut value = document(&owner, &capture.supervisor, 6);
    value["complete"] = json!(false);
    rejected(&owner, capture.supervisor, &value, ReceiptReason::IncompleteCapture);
    incomplete = capture.supervisor;
    incomplete.repetition = MAX_REPETITIONS;
    rejected(&owner, incomplete, &document(&owner, &incomplete, 6), ReceiptReason::InvalidRepetition);
}

#[test]
fn unavailable_case_selection_cannot_be_repaired_by_a_receipt_label() {
    let owner = owner(7, RowKey::Single);
    let initial = supervisor(&owner, Arm::Baseline, 0);
    for (selection, reason) in [
        (Selection::Missing, ReceiptReason::CaseMissing),
        (Selection::Ambiguous, ReceiptReason::CaseAmbiguous),
        (Selection::Unsupported, ReceiptReason::CaseUnsupported),
    ] {
        let mut supervisor = initial;
        supervisor.selection = selection;
        rejected(&owner, supervisor, &document(&owner, &initial, 6), reason);
    }
}

#[test]
fn malformed_unknown_or_missing_json_fields_are_not_silently_defaulted() {
    let owner = owner(7, RowKey::Single);
    let supervisor = supervisor(&owner, Arm::Baseline, 0);
    let original = document(&owner, &supervisor, 7);
    let mut missing = original.clone();
    missing.as_object_mut().unwrap().remove("complete");
    rejected(&owner, supervisor, &missing, ReceiptReason::MalformedReceipt);
    let mut unknown = original.clone();
    unknown["trusted"] = json!(true);
    rejected(&owner, supervisor, &unknown, ReceiptReason::MalformedReceipt);
    let mut bad_identity = original.clone();
    bad_identity["assertions"][0]["assertion"] = json!(([3; 31]));
    rejected(&owner, supervisor, &bad_identity, ReceiptReason::MalformedReceipt);
    let mut bad_phase = original.clone();
    bad_phase["teardown"] = json!("success");
    rejected(&owner, supervisor, &bad_phase, ReceiptReason::MalformedReceipt);
    let mut future = original;
    future["format"] = json!("slophawk-owned-i64-equality-v2");
    rejected(&owner, supervisor, &future, ReceiptReason::UnsupportedFormat);
}

#[test]
fn duplicate_json_keys_invalid_utf8_and_trailing_documents_are_rejected() {
    let owner = owner(7, RowKey::Single);
    let supervisor = supervisor(&owner, Arm::Baseline, 0);
    let json = serde_json::to_string(&document(&owner, &supervisor, 7)).unwrap();
    let duplicate_root = format!("{{\"format\":\"{}\",{}", receipts::FORMAT, &json[1..]);
    let duplicate_event = json.replace("\"actual\":7", "\"actual\":7,\"actual\":7");
    assert_ne!(duplicate_event, json);
    for bytes in [
        duplicate_root.into_bytes(), duplicate_event.into_bytes(), format!("{json} {{}}").into_bytes(),
        vec![b'{', 0xff, b'}'], Vec::new(), b"[]".to_vec(),
        br#"{"Format":"slophawk-check-v1","Run":"legacy","Result":{}}"#.to_vec(),
        b"<TestRun/>".to_vec(),
    ] {
        assert_eq!(Capture::from_bytes(supervisor, bytes).validate(&owner),
            Validation::Unknown(ReceiptReason::MalformedReceipt));
    }
}

#[test]
fn passed_terminal_allows_only_the_documented_absent_or_null_event() {
    let owner = owner(7, RowKey::Single);
    let supervisor = supervisor(&owner, Arm::Baseline, 0);
    let mut value = document(&owner, &supervisor, 7);
    value["terminal"].as_object_mut().unwrap().remove("event");
    assert!(matches!(Capture::from_document(supervisor, &value).validate(&owner), Validation::Complete(_)));
}

#[test]
fn receipt_byte_cap_is_checked_before_digest_and_decoding() {
    let owner = owner(7, RowKey::Single);
    let capture = Capture::valid(&owner, Arm::Baseline, 0, 7);
    let mut bytes = capture.bytes.clone();
    assert!(bytes.len() < receipts::MAX_RECEIPT_BYTES);
    bytes.resize(receipts::MAX_RECEIPT_BYTES, b' ');
    let at_limit = Capture::from_bytes(capture.supervisor, bytes);
    assert!(matches!(at_limit.validate(&owner), Validation::Complete(_)));
    let mut over = at_limit.bytes.clone();
    over.push(b' ');
    assert_eq!(receipts::validate(Some(&over), Some(&owner), Some(&at_limit.supervisor)),
        Validation::Unknown(ReceiptReason::OversizedReceipt));
    let mut tampered = capture.bytes.clone();
    tampered.push(b' ');
    assert_eq!(receipts::validate(Some(&tampered), Some(&owner), Some(&capture.supervisor)),
        Validation::Unknown(ReceiptReason::ReceiptDigestMismatch));
}

#[test]
fn signed_i64_extremes_are_compared_without_subtraction_or_floating_point() {
    for (expected, actual, result) in [
        (i64::MIN, i64::MIN, AssertionResult::Passed),
        (i64::MAX, i64::MAX, AssertionResult::Passed),
        (i64::MIN, i64::MAX, AssertionResult::Failed),
        (i64::MAX, i64::MIN, AssertionResult::Failed),
    ] {
        let owner = owner(expected, RowKey::Single);
        let attempt = Capture::valid(&owner, Arm::Baseline, 0, actual).complete(&owner);
        match attempt.assertion {
            AssertionCapture::ExactlyOne(event) => assert_eq!(event.result, result),
            other => panic!("integer observation was not captured: {other:?}"),
        }
    }
    assert_ne!(receipts::observation_id(i64::MIN, i64::MAX), receipts::observation_id(i64::MAX, i64::MIN));
}

#[test]
fn nonintegers_and_out_of_range_values_cannot_supply_i64_observations() {
    let owner = owner(7, RowKey::Single);
    let supervisor = supervisor(&owner, Arm::Baseline, 0);
    for actual in [json!(7.5), json!(7.0), json!("7"), json!(true), Value::Null, json!(u64::MAX)] {
        let mut value = document(&owner, &supervisor, 7);
        value["assertions"][0]["actual"] = actual;
        rejected(&owner, supervisor, &value, ReceiptReason::MalformedReceipt);
    }
}

#[test]
fn pair_comparison_retains_reuse_staleness_and_instability_guards() {
    let owner = owner(7, RowKey::Single);
    for (reuse_attempt, expected) in [(true, Reason::ReusedAttempt), (false, Reason::ReusedIsolation)] {
        let mut pair = Pair::new(owner, &[6, 6], &[7, 7]);
        let prior = pair.baseline[0].provenance.unwrap();
        let mut supervisor = supervisor(&owner, Arm::Candidate, 0);
        if reuse_attempt { supervisor.provenance.attempt = prior.attempt; }
        else { supervisor.provenance.isolation = prior.isolation; }
        pair.candidate[0] = Capture::from_document(supervisor, &document(&owner, &supervisor, 7)).complete(&owner);
        pair_unknown(pair.input(), expected, Some(Arm::Candidate), Some(0));
    }
    let pair = Pair::new(owner, &[6, 6], &[7, 7]);
    let mut stale = pair.input();
    stale.substitution.as_mut().unwrap().review = Some([200; 32]);
    pair_unknown(stale, Reason::SubstitutionMismatch, Some(Arm::Baseline), Some(0));
    let mut stale = pair.input();
    stale.substitution.as_mut().unwrap().candidate_snapshot = [200; 32];
    pair_unknown(stale, Reason::SnapshotMismatch, Some(Arm::Candidate), Some(0));
    for baseline in [[6, 8], [6, 7]] {
        let unstable = Pair::new(owner, &baseline, &[7, 7]);
        pair_unknown(unstable.input(), Reason::UnstableObservations, Some(Arm::Baseline), Some(1));
    }
}

#[test]
fn pair_direction_and_complete_attempt_inventory_remain_required() {
    let owner = owner(7, RowKey::Single);
    let candidate_fails = Pair::new(owner, &[7, 7], &[6, 6]);
    pair_unknown(candidate_fails.input(), Reason::CandidateFailed, Some(Arm::Candidate), None);
    let both_fail = Pair::new(owner, &[6, 6], &[6, 6]);
    pair_unknown(both_fail.input(), Reason::BothArmsFailed, None, None);
    let mut omitted = Pair::new(owner, &[6, 6], &[7, 7]);
    omitted.baseline.pop();
    pair_unknown(omitted.input(), Reason::RepetitionSet, Some(Arm::Baseline), None);
    let complete = Pair::new(owner, &[6, 6], &[7, 7]);
    let mut partial = complete.input();
    partial.candidate.complete = false;
    pair_unknown(partial, Reason::IncompleteArm, Some(Arm::Candidate), None);
}

#[test]
fn receipt_attempt_order_does_not_change_pair_support_or_first_problem() {
    let owner = owner(7, RowKey::Data([150; 32]));
    let mut pair = Pair::new(owner, &[6, 6, 6], &[7, 7, 7]);
    let original = compare(&pair.input());
    pair.baseline.reverse();
    pair.candidate.rotate_left(1);
    assert_eq!(compare(&pair.input()), original);
    let mut unstable = Pair::new(owner, &[6, 8, 6], &[7, 7, 7]);
    let original = compare(&unstable.input());
    unstable.baseline.rotate_left(2);
    unstable.candidate.reverse();
    assert_eq!(compare(&unstable.input()), original);
}

#[test]
fn json_member_order_changes_byte_identity_but_not_the_bound_observation() {
    let owner = owner(7, RowKey::Single);
    let original = Capture::valid(&owner, Arm::Baseline, 0, 6);
    let mut value: Value = serde_json::from_slice(&original.bytes).unwrap();
    value.as_object_mut().unwrap().remove("format");
    let remaining = serde_json::to_string(&value).unwrap();
    let reordered = format!("{},\"format\":\"{}\"}}", &remaining[..remaining.len() - 1], receipts::FORMAT);
    let reordered = Capture::from_bytes(original.supervisor, reordered.into_bytes());
    assert_ne!(original.supervisor.provenance.receipt, reordered.supervisor.provenance.receipt);
    let first = original.complete(&owner);
    let second = reordered.complete(&owner);
    assert_eq!(first.assertion, second.assertion);
    assert_eq!(first.case_outcome, second.case_outcome);
    assert_eq!(first.selection, second.selection);
}
