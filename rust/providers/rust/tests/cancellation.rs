//! Draft typed-consumer fixtures, deliberately unrun by SG-CHECK-R07.
//!
//! SOURCE is synthetic anchor text, not executable Rust or resolver evidence.
//! Facts are independently supplied assertions. These tests qualify classification
//! and bounded input handling only, never extraction or actual cancellation.

use sha2::{Digest, Sha256};
use slophawk_rust_source_provider::cancellation::*;
use slophawk_rust_source_provider::{MAX_SOURCE_BYTES, SourceEdition, Span};

const SOURCE: &str = "// café\r\nselect! { first; second; third; fourth; }\r\n";

fn context() -> ContextIdentity {
    ContextIdentity {
        edition: SourceEdition::E2024,
        project_sha256: [1; 32],
        comparison_sha256: [2; 32],
    }
}

fn unit(source: &str) -> SourceUnit<'_> {
    SourceUnit { logical_document: "src/synthetic.rs", source, context: context() }
}

fn evidence<'a>(source: &str, operations: &'a [ArmFact]) -> TypedFacts<'a> {
    TypedFacts {
        logical_document: "src/synthetic.rs",
        source_sha256: Sha256::digest(source.as_bytes()).into(),
        context: context(),
        producer: "synthetic-fixture/1",
        coverage: Coverage::Complete,
        operations,
    }
}

fn arm(anchor: &str, operation: IoOperation) -> ArmFact {
    let start = SOURCE.find(anchor).unwrap() as u32;
    ArmFact {
        select_span: Span { start: SOURCE.find("select!").unwrap() as u32, end: SOURCE.len() as u32 },
        future_span: Span { start, end: start + anchor.len() as u32 },
        select_macro: SelectMacro::Tokio(TokioPackage::Canonical1_54_1),
        callee: Callee::TokioIo { package: TokioPackage::Canonical1_54_1, operation },
        attribution: Attribution::Direct,
        introduction: Introduction::Introduced,
        ownership: Ownership::OwnedBySelect,
        cancellation: Cancellation::MayDropPending,
        repetition: Repetition::RecreatedInLoop,
    }
}

fn enabled(max_records: usize) -> Options {
    Options { enabled: true, max_records }
}

fn run(operations: &[ArmFact]) -> Report<'_> {
    assess(unit(SOURCE), evidence(SOURCE, operations), enabled(MAX_OPERATIONS))
        .unwrap().unwrap()
}

fn outcome(fact: ArmFact) -> Outcome {
    run(&[fact]).records[0].outcome
}

#[test]
fn default_is_opt_out_and_disabled_does_not_inspect_inputs() {
    assert!(!Options::default().enabled);
    let mut source = unit(SOURCE);
    source.logical_document = "";
    let mut facts = evidence(SOURCE, &[]);
    facts.source_sha256 = [0; 32];
    assert!(assess(source, facts, Options::default()).unwrap().is_none());
    assert_eq!(assess(source, facts, enabled(1)).unwrap_err(), InputError::InvalidIdentity);
}

#[test]
fn documented_partial_io_is_advisory_in_loops_and_one_shot_selections() {
    for (api, effect) in [
        (IoOperation::ReadExact, PartialEffect::ReadProgress),
        (IoOperation::WriteAll, PartialEffect::WriteProgress),
    ] {
        for repetition in [Repetition::RecreatedInLoop, Repetition::OneShot, Repetition::Unknown] {
            let mut fact = arm("first", api);
            fact.repetition = repetition;
            let supplied = [fact];
            let report = run(&supplied);
            assert_eq!(report.records, [Assessment { fact, outcome: Outcome::Advisory(effect) }]);
            assert_eq!(report.advisories, 1);
            assert!(report.complete_for_supplied_facts);
            assert_eq!(report.rule_id, RULE_ID);
            assert_eq!(report.summary_version, SUMMARY_VERSION);
            assert_eq!(report.producer, "synthetic-fixture/1");
            assert_eq!(report.context, context());
        }
    }
}

#[test]
fn documented_single_read_and_write_are_safe_wait_controls() {
    for api in [IoOperation::Read, IoOperation::Write] {
        assert_eq!(outcome(arm("first", api)), Outcome::NoAdvisory(Exclusion::DocumentedSafeWait));
    }
}

#[test]
fn retained_pinned_future_differs_from_an_owned_pinned_future() {
    // A producer proved the underlying owner survives a reborrow in the first
    // case. Merely seeing Pin or &mut in source would not establish that fact.
    let mut retained = arm("first", IoOperation::WriteAll);
    retained.ownership = Ownership::RetainedOutsideSelect;
    assert_eq!(outcome(retained), Outcome::NoAdvisory(Exclusion::RetainedFuture));
    let owned_pin = arm("first", IoOperation::WriteAll);
    assert_eq!(outcome(owned_pin), Outcome::Advisory(PartialEffect::WriteProgress));
    // A &mut JoinHandle is a different selected callee, not its task's I/O.
    retained.callee = Callee::Other;
    assert_eq!(outcome(retained), Outcome::NoAdvisory(Exclusion::DifferentCallee));
}

#[test]
fn disabled_and_proven_unpolled_operations_have_no_after_poll_advisory() {
    let mut disabled = arm("first", IoOperation::ReadExact);
    disabled.cancellation = Cancellation::NeverPolled;
    let mut unpolled_by_order = arm("second", IoOperation::WriteAll);
    unpolled_by_order.cancellation = Cancellation::NeverPolled;
    for fact in [disabled, unpolled_by_order] {
        assert_eq!(outcome(fact), Outcome::NoAdvisory(Exclusion::NeverPolled));
    }
    let mut no_losing_path = arm("first", IoOperation::WriteAll);
    no_losing_path.cancellation = Cancellation::NoPendingDrop;
    assert_eq!(outcome(no_losing_path), Outcome::NoAdvisory(Exclusion::NoPendingDrop));
    // read_to_string can change its argument during construction; an unpolled
    // operation outside the two reviewed summaries must remain unsupported.
    disabled.callee = Callee::TokioIo { package: TokioPackage::Canonical1_54_1, operation: IoOperation::Other };
    assert_eq!(outcome(disabled), Outcome::Unsupported(Unsupported::ApiSummary));
}

#[test]
fn names_do_not_override_macro_callee_or_trait_identity() {
    let mut local_macro = arm("first", IoOperation::ReadExact);
    local_macro.select_macro = SelectMacro::Other;
    assert_eq!(outcome(local_macro), Outcome::NoAdvisory(Exclusion::DifferentMacro));
    let mut shadowed_receiver = arm("first", IoOperation::ReadExact);
    shadowed_receiver.callee = Callee::Other;
    assert_eq!(outcome(shadowed_receiver), Outcome::NoAdvisory(Exclusion::DifferentCallee));
    shadowed_receiver.callee = Callee::AmbiguousTrait;
    assert_eq!(outcome(shadowed_receiver), Outcome::Unsupported(Unsupported::TraitDispatch));
    local_macro.select_macro = SelectMacro::Unresolved;
    assert_eq!(outcome(local_macro), Outcome::Unsupported(Unsupported::MacroIdentity));
    shadowed_receiver.callee = Callee::Unresolved;
    assert_eq!(outcome(shadowed_receiver), Outcome::Unsupported(Unsupported::CalleeIdentity));
}

#[test]
fn helpers_expansions_cfg_and_unmapped_source_remain_visible_unknowns() {
    for (attribution, reason) in [
        (Attribution::HelperOrComposite, Unsupported::HelperOrComposite),
        (Attribution::ExternalExpansion, Unsupported::ExternalExpansion),
        (Attribution::UnknownConfiguration, Unsupported::Configuration),
        (Attribution::UnknownSourceMapping, Unsupported::SourceMapping),
    ] {
        let mut fact = arm("first", IoOperation::WriteAll);
        fact.attribution = attribution;
        assert_eq!(outcome(fact), Outcome::Unsupported(reason));
    }
}

#[test]
fn versions_forks_and_unmodeled_apis_are_not_extrapolated() {
    for package in [TokioPackage::Unknown, TokioPackage::Unreviewed] {
        let mut fact = arm("first", IoOperation::WriteAll);
        fact.select_macro = SelectMacro::Tokio(package);
        assert_eq!(outcome(fact), Outcome::Unsupported(Unsupported::MacroPackage));
        fact.select_macro = SelectMacro::Tokio(TokioPackage::Canonical1_54_1);
        fact.callee = Callee::TokioIo { package, operation: IoOperation::WriteAll };
        assert_eq!(outcome(fact), Outcome::Unsupported(Unsupported::CalleePackage));
    }
    // A name such as write_all_buf is not matched by a write_all prefix.
    assert_eq!(outcome(arm("first", IoOperation::Other)), Outcome::Unsupported(Unsupported::ApiSummary));
}

#[test]
fn introduction_requires_a_bound_comparison_not_an_edited_enclosing_function() {
    let mut fact = arm("first", IoOperation::ReadExact);
    fact.introduction = Introduction::Preexisting;
    assert_eq!(outcome(fact), Outcome::NoAdvisory(Exclusion::Preexisting));
    fact.introduction = Introduction::Unknown;
    assert_eq!(outcome(fact), Outcome::Unsupported(Unsupported::Introduction));
}

#[test]
fn unknown_ownership_and_polling_possibility_do_not_become_warnings() {
    let mut fact = arm("first", IoOperation::WriteAll);
    fact.ownership = Ownership::Unknown;
    assert_eq!(outcome(fact), Outcome::Unsupported(Unsupported::Ownership));
    fact.ownership = Ownership::OwnedBySelect;
    fact.cancellation = Cancellation::Unknown;
    assert_eq!(outcome(fact), Outcome::Unsupported(Unsupported::Cancellation));
}

#[test]
fn incomplete_or_unavailable_fact_coverage_is_never_reported_as_complete() {
    let operations = [arm("first", IoOperation::ReadExact)];
    let mut facts = evidence(SOURCE, &operations);
    facts.coverage = Coverage::Partial;
    let report = assess(unit(SOURCE), facts, enabled(1)).unwrap().unwrap();
    assert_eq!(report.advisories, 1);
    assert_eq!(report.input_coverage, Coverage::Partial);
    assert!(!report.complete_for_supplied_facts);
    facts.coverage = Coverage::Unavailable;
    assert_eq!(assess(unit(SOURCE), facts, enabled(1)).unwrap_err(), InputError::InconsistentCoverage);
    facts.operations = &[];
    let unavailable = assess(unit(SOURCE), facts, enabled(1)).unwrap().unwrap();
    assert!(unavailable.records.is_empty());
    assert!(!unavailable.complete_for_supplied_facts);
    assert_eq!(unavailable.input_coverage, Coverage::Unavailable);
    assert!(run(&[]).complete_for_supplied_facts);
}

#[test]
fn stale_document_bytes_context_comparison_and_edition_are_rejected_atomically() {
    let operations = [arm("first", IoOperation::WriteAll)];
    let original = evidence(SOURCE, &operations);
    let mut variants = [original; 5];
    variants[0].source_sha256[0] ^= 1;
    variants[1].context.project_sha256[0] ^= 1;
    variants[2].context.comparison_sha256[0] ^= 1;
    variants[3].context.edition = SourceEdition::E2021;
    variants[4].logical_document = "src/different-module.rs";
    for facts in variants {
        assert_eq!(assess(unit(SOURCE), facts, enabled(1)).unwrap_err(), InputError::IdentityMismatch);
    }
    let normalized = SOURCE.replace("\r\n", "\n");
    assert_eq!(assess(unit(&normalized), original, enabled(1)).unwrap_err(), InputError::IdentityMismatch);
}

#[test]
fn original_unicode_crlf_spans_survive_and_malformed_spans_reject_the_batch() {
    let good = arm("second", IoOperation::ReadExact);
    let operations = [good];
    let report = run(&operations);
    let span = report.records[0].fact.future_span;
    assert_eq!(&SOURCE[span.start as usize..span.end as usize], "second");
    assert_eq!(report.source_sha256, evidence(SOURCE, &operations).source_sha256);
    let inside_e_acute = SOURCE.find('é').unwrap() as u32 + 1;
    let mut invalid = [good; 5];
    invalid[0].future_span.end = SOURCE.len() as u32 + 1;
    invalid[1].future_span.end = invalid[1].future_span.start;
    invalid[2].future_span.start = invalid[2].future_span.end + 1;
    invalid[3].select_span.end = good.future_span.start;
    invalid[4].select_span.start = 0;
    invalid[4].future_span = Span { start: inside_e_acute, end: inside_e_acute + 1 };
    for bad in invalid {
        let batch = [arm("first", IoOperation::WriteAll), bad];
        assert_eq!(assess(unit(SOURCE), evidence(SOURCE, &batch), enabled(2)).unwrap_err(), InputError::InvalidSpan);
    }
}

#[test]
fn enumeration_order_does_not_change_records_or_the_bounded_prefix() {
    let first = arm("first", IoOperation::Write);
    let second = arm("second", IoOperation::ReadExact);
    let third = arm("third", IoOperation::WriteAll);
    let a = [third, first, second];
    let b = [second, third, first];
    let left = assess(unit(SOURCE), evidence(SOURCE, &a), enabled(2)).unwrap().unwrap();
    let right = assess(unit(SOURCE), evidence(SOURCE, &b), enabled(2)).unwrap().unwrap();
    assert_eq!(left, right);
    assert_eq!(left.records.iter().map(|r| r.fact).collect::<Vec<_>>(), [first, second]);
}

#[test]
fn duplicate_future_anchors_are_rejected_even_with_different_select_ranges() {
    let first = arm("first", IoOperation::WriteAll);
    let mut duplicate = first;
    duplicate.select_span.start = 0;
    duplicate.callee = Callee::Other;
    let operations = [first, duplicate];
    assert_eq!(assess(unit(SOURCE), evidence(SOURCE, &operations), enabled(2)).unwrap_err(), InputError::DuplicateFutureSpan);
}

#[test]
fn omitted_unsupported_records_and_advisories_still_contribute_exact_totals() {
    let mut unknown = arm("second", IoOperation::ReadExact);
    unknown.callee = Callee::AmbiguousTrait;
    let operations = [arm("third", IoOperation::WriteAll), unknown, arm("first", IoOperation::Read)];
    let report = assess(unit(SOURCE), evidence(SOURCE, &operations), enabled(1)).unwrap().unwrap();
    assert_eq!((report.examined, report.advisories, report.unsupported, report.omitted_records), (3, 1, 1, 2));
    assert_eq!(report.records.len(), 1);
    assert_eq!(report.records[0].outcome, Outcome::NoAdvisory(Exclusion::DocumentedSafeWait));
    assert!(!report.complete_for_supplied_facts);
}

#[test]
fn exact_operation_limit_is_supported_and_oversize_inputs_are_rejected() {
    let source = "x".repeat(MAX_OPERATIONS);
    let operations = (0..MAX_OPERATIONS).map(|index| ArmFact {
        select_span: Span { start: 0, end: MAX_OPERATIONS as u32 },
        future_span: Span { start: index as u32, end: index as u32 + 1 },
        ..arm("first", IoOperation::WriteAll)
    }).collect::<Vec<_>>();
    let report = assess(unit(&source), evidence(&source, &operations), enabled(1)).unwrap().unwrap();
    assert_eq!((report.examined, report.advisories, report.omitted_records), (MAX_OPERATIONS, MAX_OPERATIONS, MAX_OPERATIONS - 1));
    let too_many = vec![arm("first", IoOperation::WriteAll); MAX_OPERATIONS + 1];
    assert_eq!(assess(unit(SOURCE), evidence(SOURCE, &too_many), enabled(1)).unwrap_err(), InputError::TooManyOperations);
    for limit in [0, MAX_OPERATIONS + 1] {
        assert_eq!(assess(unit(SOURCE), evidence(SOURCE, &[]), enabled(limit)).unwrap_err(), InputError::InvalidRecordLimit);
    }
    let too_large = "x".repeat(MAX_SOURCE_BYTES + 1);
    assert_eq!(assess(unit(&too_large), evidence(&too_large, &[]), enabled(1)).unwrap_err(), InputError::SourceTooLarge);
}

#[test]
fn absent_context_digests_and_invalid_producer_identity_are_rejected() {
    let mut source = unit(SOURCE);
    source.context.project_sha256 = [0; 32];
    assert_eq!(assess(source, evidence(SOURCE, &[]), enabled(1)).unwrap_err(), InputError::MissingContextDigest);
    source = unit(SOURCE);
    source.context.comparison_sha256 = [0; 32];
    assert_eq!(assess(source, evidence(SOURCE, &[]), enabled(1)).unwrap_err(), InputError::MissingContextDigest);
    let mut facts = evidence(SOURCE, &[]);
    facts.producer = "producer\ncontrol";
    assert_eq!(assess(unit(SOURCE), facts, enabled(1)).unwrap_err(), InputError::InvalidIdentity);
}
