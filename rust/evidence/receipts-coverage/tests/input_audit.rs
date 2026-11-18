//! Draft SG-CHECK-R04 regression cases. Not executed by this packet.
//! Settings constants were obtained by hashing fixed, independently transcribed
//! SG-13 wire bytes with sha256sum, never by running the candidate implementation.
use slophawk_receipts_coverage::{identity as build, input_audit as audit};
use audit::{AuditReason as Reason, AuditState as State};

const A: &str = "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA";
const B: &str = "BBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBB";
// Text(domain), Int32 count; each member is Text(path), Text(digest), Int64 length.
const EMPTY: &str = "E1BC1C8EFDCFB9396A1D916763F56D1D85CE8CD2D64C5F82484ECF3D0A40E748";
const ONE: &str = "C8F77D84B60D6767001D813AE2D7CD7DDA7BA2008CEA0C9F9EF179C402B71278";
const TWO: &str = "E9F807EDBE51FCCF8FB2D22BEC130089D24F354068D3B1C29F66EB86D3CAE67F";
const UNICODE: &str = "A01A60D402B7923D96970CD1A27F7FDCC662B7F93F5276A7D3C614FCD79C752F";
const FILES: [audit::SettingsFile<'static>; 2] = [
    audit::SettingsFile { path: "config/a.json", sha256: A, length: 4 },
    audit::SettingsFile { path: "config/b.json", sha256: B, length: 4 },
];

fn inputs(settings: &str) -> build::BuildInputs {
    build::BuildInputs {
        target: build::ProjectTarget {
            id: Some("tests/Unit.csproj|net10.0".into()), project_path: Some("tests/Unit.csproj".into()),
            framework: Some("net10.0".into()), configuration: Some("Debug".into()),
            platform: Some("AnyCPU".into()), runtime_identifier: Some("".into()),
            sdk_version: Some("10.0.100".into()), is_test_project: true,
            evaluation_fingerprint: Some(A.into()), compile: Some(vec![]), references: Some(vec![]),
        },
        project_graph_complete: true, path_mapping_identity: Some("mapped-root/identity".into()),
        path_mapping_trusted: true, collection_mode: build::CollectionMode::ImmutableSnapshot,
        settings_sha256: Some(settings.into()), runner_identity: Some("vstest-1".into()),
        adapter_identity: Some("adapter-1".into()), ordered_compile_inputs: Some(vec![]),
        reference_assemblies: Some(vec![]), runtime_assets: Some(vec![]),
        declared_outputs: Some(vec![build::DeclaredOutput {
            path: "out/Unit.dll".into(), sha256: Some(A.into()),
            observation: build::OutputObservation::ByteVerified,
        }]),
    }
}

fn context(stamp: &build::Stamp) -> audit::BuildInputContext<'_> {
    audit::BuildInputContext {
        root_identity: "root-1", epoch: "epoch-1", revision: 5,
        operation_id: "build-42", capture_id: "decision-capture-1",
        provider: "csharp-additional-files/vNext-fixture", target_id: "tests/Unit.csproj|net10.0",
        mapping_identity: "mapped-root/identity", collection_mode: build::CollectionMode::ImmutableSnapshot,
        coherence: audit::CaptureCoherence::ProviderVerifiedStable, build_sha256: &stamp.sha256,
    }
}

fn observation<'a>(context: audit::BuildInputContext<'a>) -> audit::AdditionalFileObservation<'a> {
    audit::AdditionalFileObservation {
        context, scope: audit::ObservationScope::EvaluatedAdditionalFile,
        semantics: audit::InputSemantics::UnorderedFileBytes,
        path_identity: audit::PathIdentity::ProviderVerifiedUnlinked,
        path: Some("config/a.json"), sha256: Some(A), length: Some(4),
    }
}

fn witness<'a>(context: audit::BuildInputContext<'a>, files: &'a [audit::SettingsFile<'a>])
    -> audit::SettingsWitness<'a> {
    audit::SettingsWitness { context, path_identity: audit::PathIdentity::ProviderVerifiedUnlinked, files }
}

#[test]
fn exact_member_is_covered_and_retains_its_binding_and_limits() {
    let input = inputs(ONE); let stamp = build::capture(&input); let ctx = context(&stamp);
    let found = audit::audit_additional_file(&input, &stamp, &ctx, &observation(ctx),
        Some(&witness(ctx, &FILES[..1])));
    assert_eq!((State::Covered, Reason::CoveredBySettings), (found.state, found.reason));
    assert_eq!(Some(A), found.represented_sha256.as_deref());
    assert_eq!(Some(4), found.represented_length);
    assert_eq!(Some(ctx.capture_id), found.binding.as_ref().map(|value| value.capture_id.as_str()));
    assert_eq!(Some(stamp.sha256.as_str()), found.binding.as_ref().map(|value| value.build_sha256.as_str()));
    assert!(found.limits.contains(&audit::AuditLimit::OtherIdentityRepresentationsUnchecked));
    assert!(found.limits.contains(&audit::AuditLimit::ConsumptionAndBehaviorUnproven));
}

#[test]
fn empty_is_a_checked_gap_but_absent_witness_is_unknown() {
    let input = inputs(EMPTY); let stamp = build::capture(&input); let ctx = context(&stamp);
    let obs = observation(ctx);
    let gap = audit::audit_additional_file(&input, &stamp, &ctx, &obs, Some(&witness(ctx, &[])));
    assert_eq!((State::CandidateGap, Reason::SettingsPathAbsent), (gap.state, gap.reason));
    assert_eq!(Some(A), gap.observed_sha256.as_deref());
    assert_eq!(None, gap.represented_sha256);
    let unknown = audit::audit_additional_file(&input, &stamp, &ctx, &obs, None);
    assert_eq!((State::Unknown, Reason::WitnessUnavailable), (unknown.state, unknown.reason));
    assert!(unknown.binding.is_none());
}

#[test]
fn omitted_old_and_new_digests_are_not_confused_with_each_other_or_a_reuse_error() {
    let input = inputs(EMPTY); let before = build::capture(&input); let after = build::capture(&input);
    assert_eq!(before, after); assert!(build::compare(&before, &after).reusable);
    let old = context(&before);
    let new = audit::BuildInputContext { revision: 6, operation_id: "build-43",
        capture_id: "decision-capture-2", ..context(&after) };
    for (ctx, digest) in [(old, A), (new, B)] {
        let obs = audit::AdditionalFileObservation { sha256: Some(digest), ..observation(ctx) };
        let found = audit::audit_additional_file(&input, &before, &ctx, &obs, Some(&witness(ctx, &[])));
        assert_eq!(Reason::SettingsPathAbsent, found.reason);
        assert_eq!(Some(digest), found.observed_sha256.as_deref());
        assert_eq!(None, found.represented_sha256);
        assert!(found.limits.contains(&audit::AuditLimit::ConsumptionAndBehaviorUnproven));
        let binding = found.binding.as_ref().unwrap();
        assert_eq!(ctx.revision, binding.revision);
        assert_eq!(ctx.operation_id, binding.operation_id);
        assert_eq!(ctx.capture_id, binding.capture_id);
    }
}

#[test]
fn differing_member_digest_and_length_remain_distinct_candidates() {
    let input = inputs(ONE); let stamp = build::capture(&input); let ctx = context(&stamp);
    for (digest, length, reason) in [(B, 4, Reason::SettingsDigestDiffers),
        (A, 5, Reason::SettingsLengthDiffers)] {
        let obs = audit::AdditionalFileObservation { sha256: Some(digest), length: Some(length), ..observation(ctx) };
        let found = audit::audit_additional_file(&input, &stamp, &ctx, &obs, Some(&witness(ctx, &FILES[..1])));
        assert_eq!((State::CandidateGap, reason), (found.state, found.reason));
        assert_eq!(Some(A), found.represented_sha256.as_deref());
        assert_eq!(Some(digest), found.observed_sha256.as_deref());
    }
}

#[test]
fn new_aggregate_coverage_changes_v1_without_reinterpreting_the_old_arm() {
    let old_input = inputs(EMPTY); let new_input = inputs(ONE);
    let old_stamp = build::capture(&old_input); let new_stamp = build::capture(&new_input);
    assert!(!build::compare(&old_stamp, &new_stamp).reusable);
    let old_ctx = context(&old_stamp); let new_ctx = context(&new_stamp);
    assert_eq!(Reason::SettingsPathAbsent, audit::audit_additional_file(&old_input, &old_stamp,
        &old_ctx, &observation(old_ctx), Some(&witness(old_ctx, &[]))).reason);
    assert_eq!(Reason::CoveredBySettings, audit::audit_additional_file(&new_input, &new_stamp,
        &new_ctx, &observation(new_ctx), Some(&witness(new_ctx, &FILES[..1]))).reason);
}

#[test]
fn irrelevant_witness_member_and_enumeration_order_do_not_create_a_gap() {
    let input = inputs(TWO); let stamp = build::capture(&input); let ctx = context(&stamp);
    let reversed = [FILES[1], FILES[0]];
    let first = audit::audit_additional_file(&input, &stamp, &ctx, &observation(ctx), Some(&witness(ctx, &FILES)));
    let second = audit::audit_additional_file(&input, &stamp, &ctx, &observation(ctx), Some(&witness(ctx, &reversed)));
    assert_eq!(State::Covered, first.state); assert_eq!(first, second);
    // A new file outside the provider's AdditionalFiles class is not an observation
    // passed to this single-observation API. It cannot invent a second finding.
}

#[test]
fn ordinal_utf16_witness_order_matches_managed_capture() {
    let input = inputs(UNICODE); let stamp = build::capture(&input); let ctx = context(&stamp);
    // Deliberately UTF-8-sorted, the reverse of .NET ordinal order for these names.
    let files = [audit::SettingsFile { path: "\u{E000}.txt", sha256: B, length: 4 },
        audit::SettingsFile { path: "\u{10000}.txt", sha256: A, length: 4 }];
    let obs = audit::AdditionalFileObservation { path: Some("\u{10000}.txt"), ..observation(ctx) };
    assert_eq!(Reason::CoveredBySettings, audit::audit_additional_file(&input, &stamp,
        &ctx, &obs, Some(&witness(ctx, &files))).reason);
}

#[test]
fn missing_observation_fields_are_unknown_not_omissions() {
    let input = inputs(ONE); let stamp = build::capture(&input); let ctx = context(&stamp);
    let good = observation(ctx);
    for obs in [audit::AdditionalFileObservation { path: None, ..good },
        audit::AdditionalFileObservation { sha256: None, ..good },
        audit::AdditionalFileObservation { length: None, ..good }] {
        let found = audit::audit_additional_file(&input, &stamp, &ctx, &obs, Some(&witness(ctx, &FILES[..1])));
        assert_eq!((State::Unknown, Reason::ObservationIncomplete), (found.state, found.reason));
    }
    let invalid = audit::AdditionalFileObservation { sha256: Some("not-a-digest"), ..good };
    assert_eq!(Reason::ObservationInvalid, audit::audit_additional_file(&input, &stamp,
        &ctx, &invalid, Some(&witness(ctx, &FILES[..1]))).reason);
}

#[test]
fn dynamic_generator_requests_and_order_or_metadata_claims_are_not_certified() {
    let input = inputs(ONE); let stamp = build::capture(&input); let ctx = context(&stamp);
    let dynamic = audit::AdditionalFileObservation {
        scope: audit::ObservationScope::GeneratorWithUnresolvedReads, ..observation(ctx) };
    assert_eq!(Reason::DynamicReadsUnresolved, audit::audit_additional_file(&input, &stamp,
        &ctx, &dynamic, Some(&witness(ctx, &FILES[..1]))).reason);
    for (semantics, state, reason) in [
        (audit::InputSemantics::OrderedOrMetadataSensitive, State::Unsupported, Reason::OrderOrMetadataNotRepresented),
        (audit::InputSemantics::Unknown, State::Unknown, Reason::InputSemanticsUnknown)] {
        let obs = audit::AdditionalFileObservation { semantics, ..observation(ctx) };
        let found = audit::audit_additional_file(&input, &stamp, &ctx, &obs, Some(&witness(ctx, &FILES[..1])));
        assert_eq!((state, reason), (found.state, found.reason));
    }
}

#[test]
fn provider_reported_path_ambiguity_and_noncanonical_spelling_are_rejected() {
    let input = inputs(ONE); let stamp = build::capture(&input); let ctx = context(&stamp);
    let uncertain = audit::AdditionalFileObservation {
        path_identity: audit::PathIdentity::AmbiguousOrLinked, ..observation(ctx) };
    assert_eq!(Reason::PathIdentityUnresolved, audit::audit_additional_file(&input, &stamp,
        &ctx, &uncertain, Some(&witness(ctx, &FILES[..1]))).reason);
    let obs = audit::AdditionalFileObservation { path: Some("config\\a.json"), ..observation(ctx) };
    assert_eq!(Reason::ObservationInvalid, audit::audit_additional_file(&input, &stamp,
        &ctx, &obs, Some(&witness(ctx, &FILES[..1]))).reason);
    let mut ambiguous = witness(ctx, &FILES[..1]); ambiguous.path_identity = audit::PathIdentity::Unknown;
    assert_eq!(Reason::PathIdentityUnresolved, audit::audit_additional_file(&input, &stamp,
        &ctx, &observation(ctx), Some(&ambiguous)).reason);
}

#[test]
fn stale_observation_and_witness_contexts_cannot_be_rebound_by_equal_hashes() {
    let input = inputs(ONE); let stamp = build::capture(&input); let ctx = context(&stamp);
    for stale in [audit::BuildInputContext { revision: 4, ..ctx },
        audit::BuildInputContext { root_identity: "another-root", ..ctx },
        audit::BuildInputContext { epoch: "old-epoch", ..ctx },
        audit::BuildInputContext { operation_id: "another-build", ..ctx },
        audit::BuildInputContext { capture_id: "old-capture", ..ctx },
        audit::BuildInputContext { provider: "other-provider", ..ctx },
        audit::BuildInputContext { target_id: "other.csproj|net10.0", ..ctx },
        audit::BuildInputContext { mapping_identity: "other-map", ..ctx }] {
        assert_eq!(Reason::ContextMismatch, audit::audit_additional_file(&input, &stamp,
            &ctx, &observation(stale), Some(&witness(ctx, &FILES[..1]))).reason);
        assert_eq!(Reason::ContextMismatch, audit::audit_additional_file(&input, &stamp,
            &ctx, &observation(ctx), Some(&witness(stale, &FILES[..1]))).reason);
    }
}

#[test]
fn unknown_mode_and_temporal_coherence_are_visible() {
    let mut input = inputs(ONE); input.collection_mode = build::CollectionMode::Unknown;
    let stamp = build::capture(&input);
    let ctx = audit::BuildInputContext { collection_mode: build::CollectionMode::Unknown, ..context(&stamp) };
    assert_eq!(Reason::UnknownCollectionMode, audit::audit_additional_file(&input, &stamp,
        &ctx, &observation(ctx), Some(&witness(ctx, &FILES[..1]))).reason);
    let mut input = inputs(ONE); input.collection_mode = build::CollectionMode::LiveCheckout;
    let stamp = build::capture(&input);
    let ctx = audit::BuildInputContext { collection_mode: build::CollectionMode::LiveCheckout,
        coherence: audit::CaptureCoherence::Unknown, ..context(&stamp) };
    assert_eq!(Reason::CaptureCoherenceUnresolved, audit::audit_additional_file(&input, &stamp,
        &ctx, &observation(ctx), Some(&witness(ctx, &FILES[..1]))).reason);
    let coherent = audit::BuildInputContext { coherence: audit::CaptureCoherence::ProviderVerifiedStable, ..ctx };
    assert_eq!(Reason::CoveredBySettings, audit::audit_additional_file(&input, &stamp,
        &coherent, &observation(coherent), Some(&witness(coherent, &FILES[..1]))).reason);
}

#[test]
fn tampered_stamp_and_unbound_identity_are_unknown() {
    let input = inputs(ONE); let good = build::capture(&input); let mut stamp = good.clone();
    stamp.sections[2].sha256 = B.into(); let ctx = context(&stamp);
    assert_eq!(Reason::IdentityMismatch, audit::audit_additional_file(&input, &stamp,
        &ctx, &observation(ctx), Some(&witness(ctx, &FILES[..1]))).reason);
    let mut changed = input.clone(); changed.runner_identity = Some("other-runner".into());
    let ctx = context(&good);
    assert_eq!(Reason::IdentityMismatch, audit::audit_additional_file(&changed, &good,
        &ctx, &observation(ctx), Some(&witness(ctx, &FILES[..1]))).reason);
    let mut incomplete = input; incomplete.path_mapping_trusted = false;
    assert_eq!(Reason::IdentityIncomplete, audit::audit_additional_file(&incomplete, &good,
        &ctx, &observation(ctx), Some(&witness(ctx, &FILES[..1]))).reason);
}

#[test]
fn tampered_incomplete_duplicate_or_invalid_aggregate_never_proves_membership() {
    let input = inputs(TWO); let stamp = build::capture(&input); let ctx = context(&stamp);
    let tampered = [FILES[0], audit::SettingsFile { sha256: A, ..FILES[1] }];
    for files in [&FILES[..1], &tampered[..]] {
        assert_eq!(Reason::WitnessDigestMismatch, audit::audit_additional_file(&input, &stamp,
            &ctx, &observation(ctx), Some(&witness(ctx, files))).reason);
    }
    let duplicate = [FILES[0], FILES[0]];
    let negative_length = [audit::SettingsFile { length: -1, ..FILES[0] }];
    let traversal = [audit::SettingsFile { path: "../config/a.json", ..FILES[0] }];
    for files in [&duplicate[..], &negative_length[..], &traversal[..]] {
        assert_eq!(Reason::WitnessInvalid, audit::audit_additional_file(&input, &stamp,
            &ctx, &observation(ctx), Some(&witness(ctx, files))).reason);
    }
}

#[test]
fn digest_case_is_normalized_but_logical_path_spelling_is_exact() {
    let input = inputs(ONE); let stamp = build::capture(&input); let ctx = context(&stamp);
    let lower = A.to_ascii_lowercase(); let files = [audit::SettingsFile { sha256: &lower, ..FILES[0] }];
    let obs = audit::AdditionalFileObservation { sha256: Some(&lower), ..observation(ctx) };
    assert_eq!(State::Covered, audit::audit_additional_file(&input, &stamp,
        &ctx, &obs, Some(&witness(ctx, &files))).state);
    // Models independently verified distinct case-sensitive logical paths.
    // A Windows alias instead requires a provider ambiguity verdict, above.
    let distinct = audit::AdditionalFileObservation { path: Some("config/A.json"), ..observation(ctx) };
    assert_eq!(Reason::SettingsPathAbsent, audit::audit_additional_file(&input, &stamp,
        &ctx, &distinct, Some(&witness(ctx, &files))).reason);
}

#[test]
fn record_field_and_total_text_exhaustion_precede_capture_or_witness_processing() {
    let input = inputs(ONE); let stamp = build::capture(&input); let ctx = context(&stamp);
    let large = vec![FILES[0]; audit::MAX_RECORDS + 1];
    assert_eq!(Reason::BudgetExceeded, audit::audit_additional_file(&input, &stamp,
        &ctx, &observation(ctx), Some(&witness(ctx, &large))).reason);
    let text = "x".repeat(audit::MAX_FIELD_BYTES + 1);
    let obs = audit::AdditionalFileObservation { path: Some(&text), ..observation(ctx) };
    assert_eq!(Reason::BudgetExceeded, audit::audit_additional_file(&input, &stamp,
        &ctx, &obs, Some(&witness(ctx, &FILES[..1]))).reason);
    let mut oversized = input.clone(); oversized.target.compile = Some(vec![
        build::Member { path: "A.cs".into(), link: None }; audit::MAX_RECORDS + 1]);
    assert_eq!(Reason::BudgetExceeded, audit::audit_additional_file(&oversized, &stamp,
        &ctx, &observation(ctx), Some(&witness(ctx, &FILES[..1]))).reason);
    let full = "x".repeat(audit::MAX_FIELD_BYTES);
    let entries = vec![audit::SettingsFile { path: &full, sha256: A, length: 4 };
        audit::MAX_TOTAL_TEXT_BYTES / audit::MAX_FIELD_BYTES + 1];
    assert_eq!(Reason::BudgetExceeded, audit::audit_additional_file(&input, &stamp,
        &ctx, &observation(ctx), Some(&witness(ctx, &entries))).reason);
}

#[test]
fn a_settings_gap_does_not_deny_representation_in_another_section() {
    let mut input = inputs(EMPTY);
    input.runtime_assets = Some(vec![build::IdentityFile { path: "config/a.json".into(), sha256: Some(A.into()) }]);
    let stamp = build::capture(&input); let ctx = context(&stamp);
    let found = audit::audit_additional_file(&input, &stamp, &ctx,
        &observation(ctx), Some(&witness(ctx, &[])));
    assert_eq!(State::CandidateGap, found.state);
    assert!(found.limits.contains(&audit::AuditLimit::OtherIdentityRepresentationsUnchecked));
    assert!(build::compare(&stamp, &stamp).reusable);
    assert_eq!(stamp, build::capture(&input));
}
