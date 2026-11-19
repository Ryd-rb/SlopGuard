//! SH-FOLLOW-08 owner-run regression source. Not executed during patch authoring.
//! SG-13 aggregate constants are the independent fixed vectors already present
//! in tests/input_audit.rs; the sidecar adapter must preserve those exact bytes.
use serde_json::{Value, json};
use slophawk_receipts_coverage::{additional_file_evidence as wire, identity as build,
    input_audit as audit};
use wire::{CaptureAudit, EvidenceReason as Reason};

const A: &str = "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA";
const B: &str = "BBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBB";
const EMPTY: &str = "E1BC1C8EFDCFB9396A1D916763F56D1D85CE8CD2D64C5F82484ECF3D0A40E748";
const ONE: &str = "C8F77D84B60D6767001D813AE2D7CD7DDA7BA2008CEA0C9F9EF179C402B71278";
const TWO: &str = "E9F807EDBE51FCCF8FB2D22BEC130089D24F354068D3B1C29F66EB86D3CAE67F";
const UNICODE: &str = "A01A60D402B7923D96970CD1A27F7FDCC662B7F93F5276A7D3C614FCD79C752F";

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
        operation_id: "build-42", capture_id: "post-build", provider: wire::PROVIDER,
        target_id: "tests/Unit.csproj|net10.0", mapping_identity: "mapped-root/identity",
        collection_mode: build::CollectionMode::ImmutableSnapshot,
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

fn envelope(input: &build::BuildInputs, stamp: &build::Stamp, files: Value) -> Value {
    let ctx = context(stamp);
    json!({
        "schema": wire::SCHEMA, "provider": wire::PROVIDER,
        "binding": {
            "root_identity": ctx.root_identity, "epoch": ctx.epoch, "revision": ctx.revision,
            "operation_id": ctx.operation_id, "capture_id": ctx.capture_id,
            "target_id": ctx.target_id, "mapping_identity": ctx.mapping_identity,
            "collection_mode": input.collection_mode, "build_sha256": ctx.build_sha256,
        },
        "identity_inputs": input, "stamp": stamp, "settings_files": files,
        "observation_status": "evaluated_additional_files_unavailable",
    })
}

fn one() -> (build::BuildInputs, build::Stamp, Value) {
    let input = inputs(ONE);
    let stamp = build::capture(&input);
    let value = envelope(&input, &stamp, json!([{"path":"config/a.json","sha256":A,"length":4}]));
    (input, stamp, value)
}

fn inspect(value: &Value, stamp: &build::Stamp) -> CaptureAudit {
    let bytes = serde_json::to_vec(value).unwrap();
    let ctx = context(stamp);
    wire::audit_capture(Some(&bytes), &ctx, Some(&observation(ctx)),
        audit::PathIdentity::ProviderVerifiedUnlinked)
}

fn diagnostic(result: CaptureAudit) -> audit::Audit {
    match result {
        CaptureAudit::Advisory(value) => value,
        other => panic!("expected the existing audit diagnostic, got {other:?}"),
    }
}

#[test]
fn exact_transported_settings_and_independent_observation_are_covered_without_v1_changes() {
    let (input, stamp, value) = one();
    let found = diagnostic(inspect(&value, &stamp));
    assert_eq!((audit::AuditState::Covered, audit::AuditReason::CoveredBySettings),
        (found.state, found.reason));
    assert_eq!(Some(A), found.represented_sha256.as_deref());
    assert_eq!(Some("post-build"), found.binding.as_ref().map(|value| value.capture_id.as_str()));
    assert!(found.limits.contains(&audit::AuditLimit::ProviderAuthenticationExternal));
    assert_eq!(stamp, build::capture(&input));
    assert!(build::compare(&stamp, &stamp).reusable);
    // Evidence remains outside the v1 schema and cannot be smuggled into it.
    let mut old_wire = serde_json::to_value(&input).unwrap();
    old_wire["additional_file_evidence"] = value;
    assert!(serde_json::from_value::<build::BuildInputs>(old_wire).is_err());
}

#[test]
fn normal_managed_route_and_old_route_remain_explicit_evidence_gaps() {
    let (_, stamp, value) = one();
    let ctx = context(&stamp);
    let bytes = serde_json::to_vec(&value).unwrap();
    assert_eq!(CaptureAudit::Unavailable(Reason::EvaluatedAdditionalFilesUnavailable),
        wire::audit_capture(Some(&bytes), &ctx, None, audit::PathIdentity::Unknown));
    assert_eq!(CaptureAudit::Unavailable(Reason::EvidenceUnavailable),
        wire::audit_capture(None, &ctx, None, audit::PathIdentity::Unknown));
    assert!(build::compare(&stamp, &stamp).reusable);
}

#[test]
fn empty_witness_is_distinct_from_missing_witness() {
    let input = inputs(EMPTY); let stamp = build::capture(&input);
    let mut value = envelope(&input, &stamp, json!([]));
    let found = diagnostic(inspect(&value, &stamp));
    assert_eq!((audit::AuditState::CandidateGap, audit::AuditReason::SettingsPathAbsent),
        (found.state, found.reason));
    value["settings_files"] = Value::Null;
    assert_eq!(CaptureAudit::Unavailable(Reason::SettingsUnavailable), inspect(&value, &stamp));
    let _ = value.as_object_mut().unwrap().remove("settings_files");
    assert_eq!(CaptureAudit::Unavailable(Reason::SettingsUnavailable), inspect(&value, &stamp));
}

#[test]
fn changed_bytes_and_length_under_same_name_keep_legacy_candidate_reasons() {
    let (_, stamp, value) = one(); let bytes = serde_json::to_vec(&value).unwrap();
    let ctx = context(&stamp);
    for (digest, length, reason) in [(B, 4, audit::AuditReason::SettingsDigestDiffers),
        (A, 5, audit::AuditReason::SettingsLengthDiffers)] {
        let obs = audit::AdditionalFileObservation { sha256: Some(digest), length: Some(length),
            ..observation(ctx) };
        let found = diagnostic(wire::audit_capture(Some(&bytes), &ctx, Some(&obs),
            audit::PathIdentity::ProviderVerifiedUnlinked));
        assert_eq!((audit::AuditState::CandidateGap, reason), (found.state, found.reason));
    }
}

#[test]
fn settings_tampering_is_rejected_even_without_an_observation() {
    let input = inputs(TWO); let stamp = build::capture(&input); let ctx = context(&stamp);
    let files = json!([{"path":"config/a.json","sha256":A,"length":4},
        {"path":"config/b.json","sha256":B,"length":4}]);
    let original = envelope(&input, &stamp, files);
    let mut missing = original.clone();
    let _ = missing["settings_files"].as_array_mut().unwrap().pop();
    let mut tampered = original.clone(); tampered["settings_files"][1]["sha256"] = json!(A);
    for value in [missing, tampered] {
        let bytes = serde_json::to_vec(&value).unwrap();
        assert_eq!(CaptureAudit::Rejected(Reason::SettingsDigestMismatch),
            wire::audit_capture(Some(&bytes), &ctx, None, audit::PathIdentity::Unknown));
    }
    let mut reversed = original.clone(); reversed["settings_files"].as_array_mut().unwrap().reverse();
    assert_eq!(inspect(&original, &stamp), inspect(&reversed, &stamp));
}

#[test]
fn duplicate_noncanonical_and_negative_settings_records_are_rejected() {
    let (_, stamp, original) = one();
    let mut duplicate = original.clone();
    duplicate["settings_files"].as_array_mut().unwrap().push(original["settings_files"][0].clone());
    let mut backslash = original.clone(); backslash["settings_files"][0]["path"] = json!("config\\a.json");
    let mut traversal = original.clone(); traversal["settings_files"][0]["path"] = json!("../config/a.json");
    let mut negative = original.clone(); negative["settings_files"][0]["length"] = json!(-1);
    let mut digest = original.clone(); digest["settings_files"][0]["sha256"] = json!("broken");
    for value in [duplicate, backslash, traversal, negative, digest] {
        assert_eq!(CaptureAudit::Rejected(Reason::SettingsInvalid), inspect(&value, &stamp));
    }
}

#[test]
fn old_capture_wrong_target_changed_closure_and_tampered_stamp_do_not_rebind() {
    let (_, stamp, original) = one();
    for (name, value) in [("root_identity", json!("another-root")), ("epoch", json!("old-epoch")),
        ("revision", json!(4)), ("operation_id", json!("another-build")),
        ("capture_id", json!("pre-test")), ("target_id", json!("other.csproj|net10.0")),
        ("mapping_identity", json!("other-map")), ("build_sha256", json!(B))] {
        let mut changed = original.clone(); changed["binding"][name] = value;
        assert_eq!(CaptureAudit::Rejected(Reason::ContextMismatch), inspect(&changed, &stamp));
    }
    let mut changed = original.clone();
    changed["identity_inputs"]["runtime_assets"] = json!([{"path":"config/a.json","sha256":A}]);
    assert_eq!(CaptureAudit::Rejected(Reason::IdentityMismatch), inspect(&changed, &stamp));
    let mut tampered = original.clone(); tampered["stamp"]["sections"][2]["sha256"] = json!(B);
    assert_eq!(CaptureAudit::Rejected(Reason::IdentityMismatch), inspect(&tampered, &stamp));
    let mut partial = original; partial["identity_inputs"]["project_graph_complete"] = json!(false);
    assert_eq!(CaptureAudit::Unavailable(Reason::IdentityIncomplete), inspect(&partial, &stamp));
}

#[test]
fn independent_observation_must_match_current_capture_and_not_claim_dynamic_scope() {
    let (_, stamp, value) = one(); let bytes = serde_json::to_vec(&value).unwrap();
    let ctx = context(&stamp);
    let old = audit::AdditionalFileObservation {
        context: audit::BuildInputContext { capture_id: "previous-build", ..ctx }, ..observation(ctx) };
    assert_eq!(CaptureAudit::Rejected(Reason::ContextMismatch),
        wire::audit_capture(Some(&bytes), &ctx, Some(&old), audit::PathIdentity::ProviderVerifiedUnlinked));
    let dynamic = audit::AdditionalFileObservation {
        scope: audit::ObservationScope::GeneratorWithUnresolvedReads, ..observation(ctx) };
    assert_eq!(audit::AuditReason::DynamicReadsUnresolved,
        diagnostic(wire::audit_capture(Some(&bytes), &ctx, Some(&dynamic),
            audit::PathIdentity::ProviderVerifiedUnlinked)).reason);
    let linked = audit::AdditionalFileObservation {
        path_identity: audit::PathIdentity::AmbiguousOrLinked, ..observation(ctx) };
    assert_eq!(audit::AuditReason::PathIdentityUnresolved,
        diagnostic(wire::audit_capture(Some(&bytes), &ctx, Some(&linked),
            audit::PathIdentity::ProviderVerifiedUnlinked)).reason);
}

#[test]
fn external_semantics_and_incomplete_observations_are_not_promoted_by_transport() {
    let (_, stamp, value) = one(); let bytes = serde_json::to_vec(&value).unwrap();
    let ctx = context(&stamp);
    for (semantics, state, reason) in [
        (audit::InputSemantics::OrderedOrMetadataSensitive, audit::AuditState::Unsupported,
            audit::AuditReason::OrderOrMetadataNotRepresented),
        (audit::InputSemantics::Unknown, audit::AuditState::Unknown,
            audit::AuditReason::InputSemanticsUnknown),
    ] {
        let obs = audit::AdditionalFileObservation { semantics, ..observation(ctx) };
        let found = diagnostic(wire::audit_capture(Some(&bytes), &ctx, Some(&obs),
            audit::PathIdentity::ProviderVerifiedUnlinked));
        assert_eq!((state, reason), (found.state, found.reason));
        assert!(found.binding.is_none());
    }
    let complete = observation(ctx);
    for obs in [audit::AdditionalFileObservation { path: None, ..complete },
        audit::AdditionalFileObservation { sha256: None, ..complete },
        audit::AdditionalFileObservation { length: None, ..complete }] {
        let found = diagnostic(wire::audit_capture(Some(&bytes), &ctx, Some(&obs),
            audit::PathIdentity::ProviderVerifiedUnlinked));
        assert_eq!((audit::AuditState::Unknown, audit::AuditReason::ObservationIncomplete),
            (found.state, found.reason));
        assert!(found.binding.is_none());
    }
}

#[test]
fn live_double_reads_and_transport_cannot_supply_coherence_or_mapping_authority() {
    let mut input = inputs(ONE); input.collection_mode = build::CollectionMode::LiveCheckout;
    let stamp = build::capture(&input);
    let value = envelope(&input, &stamp, json!([{"path":"config/a.json","sha256":A,"length":4}]));
    let bytes = serde_json::to_vec(&value).unwrap();
    let ctx = audit::BuildInputContext { collection_mode: build::CollectionMode::LiveCheckout,
        coherence: audit::CaptureCoherence::Unknown, ..context(&stamp) };
    assert_eq!(CaptureAudit::Unavailable(Reason::CaptureCoherenceUnresolved),
        wire::audit_capture(Some(&bytes), &ctx, Some(&observation(ctx)),
            audit::PathIdentity::ProviderVerifiedUnlinked));
    let admitted = audit::BuildInputContext { coherence: audit::CaptureCoherence::ProviderVerifiedStable, ..ctx };
    assert_eq!(CaptureAudit::Unavailable(Reason::PathIdentityUnresolved),
        wire::audit_capture(Some(&bytes), &admitted, Some(&observation(admitted)),
            audit::PathIdentity::Unknown));
    let mut untrusted = value;
    untrusted["binding"]["coherence"] = json!("provider_verified_stable");
    assert_eq!(CaptureAudit::Rejected(Reason::InvalidJson), inspect(&untrusted, &stamp));
}

#[test]
fn schema_provider_null_duplicate_keys_and_unknown_status_fail_closed() {
    let (_, stamp, original) = one(); let ctx = context(&stamp);
    let mut schema = original.clone(); schema["schema"] = json!("slophawk.additional-files-evidence.v2");
    assert_eq!(CaptureAudit::Rejected(Reason::UnsupportedSchema), inspect(&schema, &stamp));
    let mut provider = original.clone(); provider["provider"] = json!("unqualified-provider");
    assert_eq!(CaptureAudit::Rejected(Reason::UnsupportedProvider), inspect(&provider, &stamp));
    let mut status = original.clone(); status["observation_status"] = json!("attributed");
    let mut missing = original.clone();
    let _ = missing.as_object_mut().unwrap().remove("observation_status");
    let mut extra = original.clone(); extra["admitted"] = json!(true);
    let mut null_input = original.clone(); null_input["identity_inputs"] = Value::Null;
    let mut null_file = original.clone(); null_file["settings_files"] = json!([null]);
    for value in [Value::Null, status, missing, extra, null_input, null_file] {
        assert_eq!(CaptureAudit::Rejected(Reason::InvalidJson), inspect(&value, &stamp));
    }
    let serialized = serde_json::to_string(&original).unwrap();
    let duplicate = format!("{{\"schema\":\"{}\",{}", wire::SCHEMA, &serialized[1..]);
    assert_eq!(CaptureAudit::Rejected(Reason::InvalidJson),
        wire::audit_capture(Some(duplicate.as_bytes()), &ctx, None, audit::PathIdentity::Unknown));
}

#[test]
fn byte_field_and_combined_record_caps_precede_identity_recomputation() {
    let (_, stamp, original) = one(); let ctx = context(&stamp);
    let oversized = vec![b' '; wire::MAX_EVIDENCE_BYTES + 1];
    assert_eq!(CaptureAudit::Rejected(Reason::BudgetExceeded),
        wire::audit_capture(Some(&oversized), &ctx, None, audit::PathIdentity::Unknown));
    let mut field = original.clone();
    field["settings_files"][0]["path"] = json!("x".repeat(audit::MAX_FIELD_BYTES + 1));
    assert_eq!(CaptureAudit::Rejected(Reason::BudgetExceeded), inspect(&field, &stamp));
    // Small JSON entries stay below the wire cap, but combined records exceed 4096.
    let mut records = original;
    records["identity_inputs"]["target"]["references"] = json!(vec!["a"; audit::MAX_RECORDS]);
    assert!(serde_json::to_vec(&records).unwrap().len() < wire::MAX_EVIDENCE_BYTES);
    assert_eq!(CaptureAudit::Rejected(Reason::BudgetExceeded), inspect(&records, &stamp));
}

#[test]
fn utf16_ordinal_settings_wire_and_digest_case_match_managed_capture() {
    let input = inputs(UNICODE); let stamp = build::capture(&input); let ctx = context(&stamp);
    let value = envelope(&input, &stamp, json!([
        {"path":"\u{E000}.txt","sha256":B.to_ascii_lowercase(),"length":4},
        {"path":"\u{10000}.txt","sha256":A.to_ascii_lowercase(),"length":4},
    ]));
    let bytes = serde_json::to_vec(&value).unwrap();
    let obs = audit::AdditionalFileObservation { path: Some("\u{10000}.txt"), ..observation(ctx) };
    assert_eq!(audit::AuditReason::CoveredBySettings,
        diagnostic(wire::audit_capture(Some(&bytes), &ctx, Some(&obs),
            audit::PathIdentity::ProviderVerifiedUnlinked)).reason);
}
