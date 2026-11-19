//! Draft adapter fixtures; SH-FOLLOW-10 deliberately did not run them.
//!
//! Source strings are synthetic anchors, not resolver output or executed Rust.
//! The fixed expectations below model a separate owner channel. The helper that
//! reapproves a receipt is ONLY a test device for reaching later validation gates;
//! production approval must never be derived from the input being admitted.
//! These tests qualify translation, binding and limits, not the truth of semantic
//! assertions, the external producer, package bytes or actual cancellation.

use sha2::{Digest, Sha256};
use slophawk_rust_source_provider::cancellation::attested::{
    ADAPTER_ID, Approval, Configuration, Error, MAX_ARMS, MacroResolution,
    MethodResolution, RECEIPT_SCHEMA, ReadExactReceipt, TOKIO_ARCHIVE_SHA256,
    Unavailable, adapt, receipt_sha256,
};
use slophawk_rust_source_provider::cancellation::{
    ArmFact, Attribution, Callee, Cancellation, ContextIdentity, Coverage,
    Exclusion, InputError, Introduction, IoOperation, MAX_OPERATIONS, Options,
    Outcome, Ownership, PartialEffect, Repetition, SelectMacro, SourceUnit,
    TokioPackage, Unsupported, assess,
};
use slophawk_rust_source_provider::{MAX_SOURCE_BYTES, SourceEdition, Span};

const PRODUCER: &str = "synthetic-owner-approved/1";
const DOCUMENT: &str = "src/synthetic.rs";
const QUALIFICATION: [u8; 32] = [3; 32];
const LOADED_PACKAGE: [u8; 32] = [4; 32];
const MACRO_CLOSURE: [u8; 32] = [5; 32];
const SELECT_DEFINITION: [u8; 32] = [6; 32];
const READ_EXACT_DEFINITION: [u8; 32] = [7; 32];
const SELECT: &str = "tokio::select! { value = socket.read_exact(&mut bytes) => {}, _ = tick => {} }";
const FUTURE: &str = "socket.read_exact(&mut bytes)";
const SOURCE: &str = "// café\r\ntokio::select! { value = socket.read_exact(&mut bytes) => {}, _ = tick => {} }\r\n";

fn context() -> ContextIdentity {
    ContextIdentity {
        edition: SourceEdition::E2024,
        project_sha256: [1; 32],
        comparison_sha256: [2; 32],
    }
}

fn unit(source: &str) -> SourceUnit<'_> {
    SourceUnit { logical_document: DOCUMENT, source, context: context() }
}

fn span(source: &str, anchor: &str) -> Span {
    let start = source.find(anchor).unwrap() as u32;
    Span { start, end: start + anchor.len() as u32 }
}

fn nomination<'a>(source: &'a str, select: &str, future: &str) -> ReadExactReceipt<'a> {
    ReadExactReceipt {
        schema_version: RECEIPT_SCHEMA,
        producer: PRODUCER,
        qualification_sha256: QUALIFICATION,
        logical_document: DOCUMENT,
        source_sha256: Sha256::digest(source.as_bytes()).into(),
        context: context(),
        tokio_archive_sha256: TOKIO_ARCHIVE_SHA256,
        loaded_package_sha256: LOADED_PACKAGE,
        macro_closure_sha256: MACRO_CLOSURE,
        semantic_evidence_sha256: [8; 32],
        introduction_evidence_sha256: [9; 32],
        select_span: span(source, select),
        future_span: span(source, future),
        select: MacroResolution::Resolved(SELECT_DEFINITION),
        method: MethodResolution::Resolved(READ_EXACT_DEFINITION),
        configuration: Configuration::Active,
        attribution: Attribution::Direct,
        introduction: Introduction::Introduced,
        ownership: Ownership::OwnedBySelect,
        cancellation: Cancellation::MayDropPending,
        repetition: Repetition::RecreatedInLoop,
    }
}

fn original() -> ReadExactReceipt<'static> {
    nomination(SOURCE, SELECT, FUTURE)
}

fn enabled(max_records: usize) -> Options {
    Options { enabled: true, max_records }
}

fn synthetic_reapproval_for_test<'a>(receipt: ReadExactReceipt<'a>) -> Approval<'a> {
    // Fixed semantic expectations are not copied from the incoming receipt.
    // Only this fixture's model owner reapproves the changed claims' digest.
    // Calling receipt_sha256 alone never supplies real owner authorization.
    Approval {
        producer: PRODUCER,
        qualification_sha256: QUALIFICATION,
        receipt_sha256: receipt_sha256(receipt).unwrap(),
        loaded_package_sha256: LOADED_PACKAGE,
        macro_closure_sha256: MACRO_CLOSURE,
        select_definition: SELECT_DEFINITION,
        read_exact_definition: READ_EXACT_DEFINITION,
    }
}

fn translated<'a>(source: &'a str, receipt: ReadExactReceipt<'a>) -> (ArmFact, Outcome) {
    let approval = synthetic_reapproval_for_test(receipt);
    let adapted = adapt(unit(source), Some(approval), Some(receipt), enabled(1))
        .unwrap().unwrap();
    assert_eq!(adapted.unavailable(), None);
    assert_eq!(adapted.approved_receipt_sha256(), Some(approval.receipt_sha256));
    let facts = adapted.typed_facts();
    assert_eq!(facts.coverage, Coverage::Complete);
    assert_eq!(facts.operations.len(), 1);
    let report = assess(unit(source), facts, enabled(1)).unwrap().unwrap();
    (report.records[0].fact, report.records[0].outcome)
}

#[test]
fn approved_direct_read_exact_translates_into_the_unchanged_consumer() {
    let receipt = original();
    let approval = synthetic_reapproval_for_test(receipt);
    let adapted = adapt(unit(SOURCE), Some(approval), Some(receipt), enabled(1))
        .unwrap().unwrap();
    let facts = adapted.typed_facts();
    assert_eq!(facts.logical_document, DOCUMENT);
    assert_eq!(facts.source_sha256, receipt.source_sha256);
    assert_eq!(facts.context, context());
    assert_eq!(facts.producer, PRODUCER);
    assert_eq!(facts.coverage, Coverage::Complete);
    let report = assess(unit(SOURCE), facts, enabled(1)).unwrap().unwrap();
    assert_eq!((report.examined, report.advisories, report.unsupported), (1, 1, 0));
    assert!(report.complete_for_supplied_facts);
    assert_eq!(report.omitted_records, 0);
    assert_eq!(report.records[0].outcome, Outcome::Advisory(PartialEffect::ReadProgress));
    assert_eq!(report.records[0].fact.select_macro,
        SelectMacro::Tokio(TokioPackage::Canonical1_54_1));
    assert_eq!(report.records[0].fact.callee, Callee::TokioIo {
        package: TokioPackage::Canonical1_54_1, operation: IoOperation::ReadExact,
    });
    let actual = report.records[0].fact.future_span;
    assert_eq!(&SOURCE[actual.start as usize..actual.end as usize], FUTURE);
}

#[test]
fn renamed_and_reexported_macro_spellings_use_resolved_definition_ids() {
    for selected in [
        "renamed_tokio::select! { value = socket.read_exact(&mut bytes) => {}, _ = tick => {} }",
        "choose! { value = socket.read_exact(&mut bytes) => {}, _ = tick => {} }",
    ] {
        // A qualified external producer, not this string, established that the
        // renamed/re-exported macro and trait item have the approved identities.
        let receipt = nomination(selected, selected, FUTURE);
        let (fact, outcome) = translated(selected, receipt);
        assert_eq!(fact.select_macro, SelectMacro::Tokio(TokioPackage::Canonical1_54_1));
        assert_eq!(outcome, Outcome::Advisory(PartialEffect::ReadProgress));
    }
}

#[test]
fn same_spelling_does_not_override_resolved_local_macro_or_custom_trait() {
    let mut local_macro = original();
    local_macro.select = MacroResolution::Other;
    let (fact, outcome) = translated(SOURCE, local_macro);
    assert_eq!(fact.select_macro, SelectMacro::Other);
    assert_eq!(outcome, Outcome::NoAdvisory(Exclusion::DifferentMacro));

    let mut custom_async_read_ext = original();
    custom_async_read_ext.method = MethodResolution::Other;
    let (fact, outcome) = translated(SOURCE, custom_async_read_ext);
    assert_eq!(fact.callee, Callee::Other);
    assert_eq!(outcome, Outcome::NoAdvisory(Exclusion::DifferentCallee));
}

#[test]
fn other_tokio_api_is_unsupported_instead_of_a_custom_callee_exclusion() {
    let mut receipt = original();
    receipt.method = MethodResolution::OtherTokioApi;
    let (fact, outcome) = translated(SOURCE, receipt);
    assert_eq!(fact.callee, Callee::TokioIo {
        package: TokioPackage::Canonical1_54_1, operation: IoOperation::Other,
    });
    assert_eq!(outcome, Outcome::Unsupported(Unsupported::ApiSummary));
}

#[test]
fn owned_pin_and_reborrowed_same_future_keep_distinct_owner_fates() {
    for (future, ownership, expected) in [
        ("owned_pin", Ownership::OwnedBySelect, Outcome::Advisory(PartialEffect::ReadProgress)),
        ("same_future.as_mut()", Ownership::RetainedOutsideSelect,
            Outcome::NoAdvisory(Exclusion::RetainedFuture)),
    ] {
        let source = format!("tokio::select! {{ _ = {future} => {{}}, _ = tick => {{}} }}");
        let mut receipt = nomination(&source, &source, future);
        // Synthetic approved evidence establishes a moved Pin<Box<ReadExact>>
        // versus a transparent Pin<&mut ReadExact> reborrow whose owner survives.
        // The adapter must not infer either fact from the future's spelling.
        receipt.ownership = ownership;
        let (fact, outcome) = translated(&source, receipt);
        assert_eq!(fact.ownership, ownership);
        assert_eq!(outcome, expected);
    }
}

#[test]
fn active_unknown_and_inactive_configuration_have_distinct_dispositions() {
    let mut unknown = original();
    unknown.configuration = Configuration::Unknown;
    let (fact, outcome) = translated(SOURCE, unknown);
    assert_eq!(fact.attribution, Attribution::UnknownConfiguration);
    assert_eq!(outcome, Outcome::Unsupported(Unsupported::Configuration));

    let mut inactive = original();
    inactive.configuration = Configuration::Inactive;
    let approval = synthetic_reapproval_for_test(inactive);
    let adapted = adapt(unit(SOURCE), Some(approval), Some(inactive), enabled(1))
        .unwrap().unwrap();
    assert_eq!(adapted.unavailable(), Some(Unavailable::InactiveConfiguration));
    assert_eq!(adapted.approved_receipt_sha256(), Some(approval.receipt_sha256));
    let facts = adapted.typed_facts();
    assert_eq!(facts.coverage, Coverage::Unavailable);
    assert!(facts.operations.is_empty());
    let report = assess(unit(SOURCE), facts, enabled(1)).unwrap().unwrap();
    assert_eq!(report.advisories, 0);
    assert!(!report.complete_for_supplied_facts);
    inactive.semantic_evidence_sha256 = [0; 32];
    assert_eq!(adapt(unit(SOURCE), Some(synthetic_reapproval_for_test(inactive)),
        Some(inactive), enabled(1)).unwrap_err(), Error::MissingSemanticEvidence);
}

#[test]
fn unresolved_identity_dispatch_and_source_provenance_do_not_become_canonical() {
    let mut unknown_macro = original();
    unknown_macro.select = MacroResolution::Unresolved;
    assert_eq!(translated(SOURCE, unknown_macro).1,
        Outcome::Unsupported(Unsupported::MacroIdentity));
    for (resolution, expected) in [
        (MethodResolution::Unresolved, Unsupported::CalleeIdentity),
        (MethodResolution::AmbiguousTrait, Unsupported::TraitDispatch),
    ] {
        let mut receipt = original();
        receipt.method = resolution;
        assert_eq!(translated(SOURCE, receipt).1, Outcome::Unsupported(expected));
    }
    for (attribution, expected) in [
        (Attribution::HelperOrComposite, Unsupported::HelperOrComposite),
        (Attribution::ExternalExpansion, Unsupported::ExternalExpansion),
        (Attribution::UnknownSourceMapping, Unsupported::SourceMapping),
    ] {
        let mut receipt = original();
        receipt.attribution = attribution;
        assert_eq!(translated(SOURCE, receipt).1, Outcome::Unsupported(expected));
    }
}

#[test]
fn wrong_or_zero_definition_ids_fail_even_after_synthetic_receipt_reapproval() {
    for id in [[91; 32], [0; 32]] {
        let mut wrong_macro = original();
        wrong_macro.select = MacroResolution::Resolved(id);
        let mut wrong_method = original();
        wrong_method.method = MethodResolution::Resolved(id);
        for receipt in [wrong_macro, wrong_method] {
            assert_eq!(adapt(unit(SOURCE), Some(synthetic_reapproval_for_test(receipt)),
                Some(receipt), enabled(1)).unwrap_err(), Error::DefinitionMismatch);
        }
    }
}

#[test]
fn canonical_archive_does_not_authorize_forked_loaded_bytes_or_macro_closure() {
    let mut fork = original();
    fork.loaded_package_sha256 = [42; 32];
    assert_eq!(fork.tokio_archive_sha256, TOKIO_ARCHIVE_SHA256);
    assert_eq!(adapt(unit(SOURCE), Some(synthetic_reapproval_for_test(fork)),
        Some(fork), enabled(1)).unwrap_err(), Error::LoadedPackageMismatch);

    for digest in [[0; 32], [42; 32]] {
        let mut wrong_helpers = original();
        wrong_helpers.macro_closure_sha256 = digest;
        assert_eq!(adapt(unit(SOURCE), Some(synthetic_reapproval_for_test(wrong_helpers)),
            Some(wrong_helpers), enabled(1)).unwrap_err(), Error::MacroClosureMismatch);
    }
    let mut unreviewed = original();
    unreviewed.tokio_archive_sha256[0] ^= 1;
    assert_eq!(adapt(unit(SOURCE), Some(synthetic_reapproval_for_test(unreviewed)),
        Some(unreviewed), enabled(1)).unwrap_err(), Error::UnreviewedArchive);
}

#[test]
fn semantic_and_introduction_evidence_remain_separate_required_inputs() {
    let mut missing_semantics = original();
    missing_semantics.semantic_evidence_sha256 = [0; 32];
    assert_eq!(adapt(unit(SOURCE), Some(synthetic_reapproval_for_test(missing_semantics)),
        Some(missing_semantics), enabled(1)).unwrap_err(), Error::MissingSemanticEvidence);
    for introduction in [Introduction::Introduced, Introduction::Preexisting] {
        let mut receipt = original();
        receipt.introduction = introduction;
        receipt.introduction_evidence_sha256 = [0; 32];
        assert_eq!(adapt(unit(SOURCE), Some(synthetic_reapproval_for_test(receipt)),
            Some(receipt), enabled(1)).unwrap_err(), Error::MissingIntroductionEvidence);
    }
    let mut uncertain = original();
    uncertain.introduction = Introduction::Unknown;
    uncertain.introduction_evidence_sha256 = [0; 32];
    assert_eq!(translated(SOURCE, uncertain).1, Outcome::Unsupported(Unsupported::Introduction));
    let mut preexisting = original();
    preexisting.introduction = Introduction::Preexisting;
    assert_eq!(translated(SOURCE, preexisting).1, Outcome::NoAdvisory(Exclusion::Preexisting));
}

#[test]
fn changing_the_current_source_document_context_or_edition_invalidates_approval() {
    let receipt = original();
    let approval = synthetic_reapproval_for_test(receipt);
    let normalized = SOURCE.replace("\r\n", "\n");
    assert_eq!(adapt(unit(&normalized), Some(approval), Some(receipt), enabled(1)).unwrap_err(),
        Error::Input(InputError::IdentityMismatch));
    let mut units = [unit(SOURCE); 4];
    units[0].logical_document = "src/different-module.rs";
    units[1].context.project_sha256[0] ^= 1;
    units[2].context.comparison_sha256[0] ^= 1;
    units[3].context.edition = SourceEdition::E2021;
    for stale in units {
        assert_eq!(adapt(stale, Some(approval), Some(receipt), enabled(1)).unwrap_err(),
            Error::Input(InputError::IdentityMismatch));
    }
}

#[test]
fn absent_authority_or_receipt_yields_unavailable_empty_facts() {
    let receipt = original();
    let approval = synthetic_reapproval_for_test(receipt);
    for (owner, input, reason) in [
        (None, Some(receipt), Unavailable::MissingApproval),
        (Some(approval), None, Unavailable::MissingReceipt),
        (None, None, Unavailable::MissingApproval),
    ] {
        let adapted = adapt(unit(SOURCE), owner, input, enabled(1)).unwrap().unwrap();
        assert_eq!(adapted.unavailable(), Some(reason));
        assert_eq!(adapted.approved_receipt_sha256(), None);
        let facts = adapted.typed_facts();
        assert_eq!(facts.producer, ADAPTER_ID);
        assert_eq!(facts.coverage, Coverage::Unavailable);
        assert!(facts.operations.is_empty());
        let report = assess(unit(SOURCE), facts, enabled(1)).unwrap().unwrap();
        assert_eq!((report.examined, report.advisories), (0, 0));
        assert!(!report.complete_for_supplied_facts);
    }
}

#[test]
fn opt_out_bypasses_even_invalid_inputs_without_granting_authority() {
    assert!(!Options::default().enabled);
    let mut malformed_unit = unit(SOURCE);
    malformed_unit.logical_document = "";
    malformed_unit.context.project_sha256 = [0; 32];
    let mut malformed_receipt = original();
    malformed_receipt.schema_version = RECEIPT_SCHEMA + 1;
    let mut invalid_approval = synthetic_reapproval_for_test(original());
    invalid_approval.receipt_sha256 = [0; 32];
    assert!(adapt(malformed_unit, Some(invalid_approval), Some(malformed_receipt),
        Options { enabled: false, max_records: 0 }).unwrap().is_none());
    assert!(adapt(unit(SOURCE), None, None, Options::default()).unwrap().is_none());
}

#[test]
fn schema_and_identity_errors_are_rejected_before_admission() {
    let approval = synthetic_reapproval_for_test(original());
    let mut future_schema = original();
    future_schema.schema_version = RECEIPT_SCHEMA + 1;
    assert_eq!(receipt_sha256(future_schema).unwrap_err(), Error::UnsupportedSchema);
    assert_eq!(adapt(unit(SOURCE), Some(approval), Some(future_schema), enabled(1)).unwrap_err(),
        Error::UnsupportedSchema);
    for producer in ["", "producer\ncontrol"] {
        let mut receipt = original();
        receipt.producer = producer;
        assert_eq!(adapt(unit(SOURCE), Some(approval), Some(receipt), enabled(1)).unwrap_err(),
            Error::Input(InputError::InvalidIdentity));
    }
}

#[test]
fn incomplete_approval_cannot_authorize_a_receipt() {
    type Change = fn(&mut Approval<'static>);
    let changes: &[Change] = &[
        |a| a.producer = "",
        |a| a.qualification_sha256 = [0; 32],
        |a| a.receipt_sha256 = [0; 32],
        |a| a.loaded_package_sha256 = [0; 32],
        |a| a.macro_closure_sha256 = [0; 32],
        |a| a.select_definition = [0; 32],
        |a| a.read_exact_definition = [0; 32],
        |a| a.select_definition = a.read_exact_definition,
    ];
    for change in changes {
        let mut approval = synthetic_reapproval_for_test(original());
        change(&mut approval);
        assert_eq!(adapt(unit(SOURCE), Some(approval), Some(original()), enabled(1)).unwrap_err(),
            Error::InvalidApproval);
    }
}

#[test]
fn every_receipt_field_is_bound_to_the_exact_prior_approval() {
    type Change = fn(&mut ReadExactReceipt<'static>);
    let identity_mismatch = Error::Input(InputError::IdentityMismatch);
    let changes: &[(&str, Change, Error)] = &[
        ("schema", |r| r.schema_version += 1, Error::UnsupportedSchema),
        ("producer", |r| r.producer = "different-producer/1", Error::ProducerMismatch),
        ("qualification", |r| r.qualification_sha256[0] ^= 1, Error::QualificationMismatch),
        ("document", |r| r.logical_document = "src/other.rs", identity_mismatch),
        ("source", |r| r.source_sha256[0] ^= 1, identity_mismatch),
        ("project", |r| r.context.project_sha256[0] ^= 1, identity_mismatch),
        ("comparison", |r| r.context.comparison_sha256[0] ^= 1, identity_mismatch),
        ("edition", |r| r.context.edition = SourceEdition::E2021, identity_mismatch),
        ("archive", |r| r.tokio_archive_sha256[0] ^= 1, Error::ReceiptMismatch),
        ("loaded bytes", |r| r.loaded_package_sha256[0] ^= 1, Error::ReceiptMismatch),
        ("macro closure", |r| r.macro_closure_sha256[0] ^= 1, Error::ReceiptMismatch),
        ("semantic evidence", |r| r.semantic_evidence_sha256[0] ^= 1, Error::ReceiptMismatch),
        ("introduction evidence", |r| r.introduction_evidence_sha256[0] ^= 1, Error::ReceiptMismatch),
        ("select start", |r| r.select_span.start += 1, Error::ReceiptMismatch),
        ("select end", |r| r.select_span.end -= 1, Error::ReceiptMismatch),
        ("future start", |r| r.future_span.start += 1, Error::ReceiptMismatch),
        ("future end", |r| r.future_span.end -= 1, Error::ReceiptMismatch),
        ("macro resolution", |r| r.select = MacroResolution::Other, Error::ReceiptMismatch),
        ("macro identity", |r| r.select = MacroResolution::Resolved([42; 32]), Error::ReceiptMismatch),
        ("method resolution", |r| r.method = MethodResolution::Other, Error::ReceiptMismatch),
        ("method identity", |r| r.method = MethodResolution::Resolved([42; 32]), Error::ReceiptMismatch),
        ("configuration", |r| r.configuration = Configuration::Inactive, Error::ReceiptMismatch),
        ("attribution", |r| r.attribution = Attribution::HelperOrComposite, Error::ReceiptMismatch),
        ("introduction", |r| r.introduction = Introduction::Preexisting, Error::ReceiptMismatch),
        ("ownership", |r| r.ownership = Ownership::RetainedOutsideSelect, Error::ReceiptMismatch),
        ("cancellation", |r| r.cancellation = Cancellation::NeverPolled, Error::ReceiptMismatch),
        ("repetition", |r| r.repetition = Repetition::OneShot, Error::ReceiptMismatch),
    ];
    let approved = original();
    let original_digest = receipt_sha256(approved).unwrap();
    let approval = synthetic_reapproval_for_test(approved);
    for (label, change, expected) in changes {
        let mut changed = approved;
        change(&mut changed);
        if changed.schema_version == RECEIPT_SCHEMA {
            assert_ne!(receipt_sha256(changed).unwrap(), original_digest, "{label}");
        }
        // Deliberately reuse the previous approval: mutating a trusted claim
        // must not silently inherit authority or produce even one advisory.
        assert_eq!(adapt(unit(SOURCE), Some(approval), Some(changed), enabled(1)).unwrap_err(),
            *expected, "{label}");
    }
}

#[test]
fn every_resolution_variant_has_a_distinct_canonical_digest() {
    let mut seen = Vec::new();
    for resolution in [
        MacroResolution::Resolved(SELECT_DEFINITION), MacroResolution::Other,
        MacroResolution::Unresolved,
    ] {
        let mut receipt = original();
        receipt.select = resolution;
        let digest = receipt_sha256(receipt).unwrap();
        assert!(!seen.contains(&digest));
        seen.push(digest);
    }
    seen.clear();
    for resolution in [
        MethodResolution::Resolved(READ_EXACT_DEFINITION), MethodResolution::Other,
        MethodResolution::AmbiguousTrait, MethodResolution::Unresolved,
        MethodResolution::OtherTokioApi,
    ] {
        let mut receipt = original();
        receipt.method = resolution;
        let digest = receipt_sha256(receipt).unwrap();
        assert!(!seen.contains(&digest));
        seen.push(digest);
    }
}

#[test]
fn receipt_identity_limits_count_utf8_bytes_before_hashing() {
    // These are hashing bounds, not evidence of a real producer or document.
    let producer_at_limit = "é".repeat(64);
    let document_at_limit = "d".repeat(512);
    let mut receipt = original();
    receipt.producer = &producer_at_limit;
    receipt.logical_document = &document_at_limit;
    assert!(receipt_sha256(receipt).is_ok());

    let producer_above_limit = format!("{producer_at_limit}x");
    receipt.producer = &producer_above_limit;
    assert_eq!(receipt_sha256(receipt).unwrap_err(), Error::Input(InputError::InvalidIdentity));
    receipt.producer = PRODUCER;
    let document_above_limit = format!("{document_at_limit}x");
    receipt.logical_document = &document_above_limit;
    assert_eq!(receipt_sha256(receipt).unwrap_err(), Error::Input(InputError::InvalidIdentity));
    receipt.logical_document = "src/control\u{7f}.rs";
    assert_eq!(receipt_sha256(receipt).unwrap_err(), Error::Input(InputError::InvalidIdentity));
}

#[test]
fn invalid_utf8_boundaries_and_noncontained_spans_reject_an_approved_nomination() {
    let good = original();
    let mut invalid = [good; 6];
    invalid[0].future_span.end = SOURCE.len() as u32 + 1;
    invalid[1].future_span.end = good.future_span.start;
    invalid[2].future_span.start = good.future_span.end + 1;
    invalid[3].select_span.end = good.future_span.start;
    invalid[4].select_span.start = SOURCE.find('é').unwrap() as u32 + 1;
    invalid[5].select_span.start = 0;
    invalid[5].future_span = Span {
        start: SOURCE.find('é').unwrap() as u32 + 1,
        end: SOURCE.find('é').unwrap() as u32 + 2,
    };
    for receipt in invalid {
        assert_eq!(adapt(unit(SOURCE), Some(synthetic_reapproval_for_test(receipt)),
            Some(receipt), enabled(1)).unwrap_err(), Error::Input(InputError::InvalidSpan));
    }
}

#[test]
fn repetition_owner_and_poll_controls_are_preserved_without_executing_futures() {
    for repetition in [Repetition::OneShot, Repetition::RecreatedInLoop, Repetition::Unknown] {
        let source = if repetition == Repetition::RecreatedInLoop {
            format!("loop {{ {SELECT} }}")
        } else {
            SELECT.to_owned()
        };
        let mut receipt = nomination(&source, SELECT, FUTURE);
        receipt.repetition = repetition;
        let (fact, outcome) = translated(&source, receipt);
        assert_eq!(fact.repetition, repetition);
        assert_eq!(outcome, Outcome::Advisory(PartialEffect::ReadProgress));
    }
    for (cancellation, expected) in [
        (Cancellation::NeverPolled, Outcome::NoAdvisory(Exclusion::NeverPolled)),
        (Cancellation::NoPendingDrop, Outcome::NoAdvisory(Exclusion::NoPendingDrop)),
        (Cancellation::Unknown, Outcome::Unsupported(Unsupported::Cancellation)),
    ] {
        let mut receipt = original();
        receipt.cancellation = cancellation;
        assert_eq!(translated(SOURCE, receipt).1, expected);
    }
    let mut unknown_owner = original();
    unknown_owner.ownership = Ownership::Unknown;
    assert_eq!(translated(SOURCE, unknown_owner).1, Outcome::Unsupported(Unsupported::Ownership));
}

#[test]
fn source_and_record_caps_apply_before_missing_evidence_without_expanding_scope() {
    assert_eq!(MAX_ARMS, 1);
    for limit in [1, MAX_OPERATIONS] {
        let receipt = original();
        let adapted = adapt(unit(SOURCE), Some(synthetic_reapproval_for_test(receipt)),
            Some(receipt), enabled(limit)).unwrap().unwrap();
        assert_eq!(adapted.typed_facts().operations.len(), MAX_ARMS);
    }
    for limit in [0, MAX_OPERATIONS + 1] {
        assert_eq!(adapt(unit(SOURCE), None, None, enabled(limit)).unwrap_err(),
            Error::Input(InputError::InvalidRecordLimit));
    }
    let at_limit = "x".repeat(MAX_SOURCE_BYTES);
    let unavailable = adapt(unit(&at_limit), None, None, enabled(1)).unwrap().unwrap();
    assert_eq!(unavailable.unavailable(), Some(Unavailable::MissingApproval));
    assert!(unavailable.typed_facts().operations.is_empty());
    let above_limit = "x".repeat(MAX_SOURCE_BYTES + 1);
    assert_eq!(adapt(unit(&above_limit), None, None, enabled(1)).unwrap_err(),
        Error::Input(InputError::SourceTooLarge));
    let mut missing_context = unit(SOURCE);
    missing_context.context.comparison_sha256 = [0; 32];
    assert_eq!(adapt(missing_context, None, None, enabled(1)).unwrap_err(),
        Error::Input(InputError::MissingContextDigest));
}
