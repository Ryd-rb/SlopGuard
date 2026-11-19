//! Hand-authored data fixtures for receipt attribution. These are not Roslyn
//! exports and do not establish search quality or original-build behavior.
use super::*;
use std::sync::atomic::{AtomicU64, Ordering};

fn identity() -> Identity {
    Identity {
        protocol: PROTOCOL.into(), operation: "inspect".into(), job: "0".repeat(32),
        root: "/synthetic/source".into(), context: "/synthetic/source".into(),
        epoch: "epoch".into(), revision: "revision".into(),
        input_digest: "1".repeat(64), tool_digest: "2".repeat(64),
        policy_digest: "3".repeat(64), budget_ref: "budget".into(), grant_ref: "grant".into(),
    }
}

fn target(operation: &str) -> InspectedTarget {
    let signature = format!("Fixture.{operation}(int)");
    InspectedTarget {
        operation: operation.into(), signature: signature.clone(),
        bound: true, eligible: true, reason: None,
        rule_source: Some(format!("rules.md#{operation}")), parameters: vec!["int".into()],
        facts: concat!(
            "scalar-path-facts\t1\nlimits\t8\t4\nbound\t1\t1\t-\nparam\t0\tint\n",
            "block\t0\tEntry\tNone\nbranch\t0\t-\n",
            "edge\t0\tfallThrough\t1\tRegular\t0\t-\t-\n",
            "block\t1\tBlock\tNone\n",
            "node\t1\t0\tconstant\tint\t-\t7\t-\t0\t0\t0\t0\t-\t-\t00\n",
            "branch\t1\t0\nedge\t1\tfallThrough\t2\tReturn\t0\t-\t-\n",
            "block\t2\tExit\tNone\nbranch\t2\t-\nseed\t0\nend\n"
        ).into(),
        graph: Some(json!({"Status": "Bound", "Signature": signature,
            "Blocks": [{"Id": 0, "Kind": "Entry"}, {"Id": 1, "Kind": "Block"},
                       {"Id": 2, "Kind": "Exit"}]})),
        original: vec![Expectation { operation: operation.into(), args: vec![0], expected: json!(7) }],
        ordinary_keys: vec!["0".into()],
    }
}

fn inspection() -> InspectResult {
    InspectResult {
        compiler_sha256: "4".repeat(64), candidate_sha256: "5".repeat(64),
        runtime_version: "10.0.0".into(),
        compiler_inputs: json!({"Target": "net10.0", "Compiler": "same-compiler",
            "Sources": [{"Name": "Fixture.cs", "Sha256": "same-source"}],
            "References": [{"Name": "System.Runtime.dll", "Sha256": "same-reference"}],
            "Checked": false}),
        targets: vec![target("alpha"), target("beta")],
    }
}

fn plan(inspected: &InspectResult) -> PathPlan {
    PathPlan {
        identity: identity(),
        inputs: PathInputs { spec: "/synthetic/source/spec.json".into(),
            spec_sha256: "1".repeat(64), rules_sha256: "6".repeat(64),
            published_sha256: "7".repeat(64), compiler_sha256: inspected.compiler_sha256.clone(),
            runtime_version: inspected.runtime_version.clone() },
        scratch: "/synthetic/scratch".into(), build: "/synthetic/build/manifest.json".into(),
        build_sha256: "8".repeat(64), candidate_sha256: inspected.candidate_sha256.clone(),
        compiler_inputs: inspected.compiler_inputs.clone(), mode: CORRELATED_MODE.into(),
        planner_evidence: Some(planner_evidence(inspected, "9".repeat(64)).unwrap()),
        per_operation: 4, retest_limit: 1,
        methods: inspected.targets.iter().map(|target| MethodPlan {
            operation: target.operation.clone(), signature: target.signature.clone(),
            bound: target.bound, eligible: target.eligible, reason: target.reason.clone(),
            rule_source: target.rule_source.clone(), work: 1, unknown: Vec::new(),
            rejected_by_domain: 0, baseline_cases: 1, graph_proposals: 0,
        }).collect(),
        cases: inspected.targets.iter().map(|target| PathCase {
            expectation: target.original[0].clone(), origin: "ordinary".into(),
            predicted_goal: None, baseline_order: Some(0),
        }).collect(),
        blocker: None,
    }
}

fn artifacts(inspected: &InspectResult) -> (Value, Value) {
    (graph_artifact(inspected), json!({"ScalarFacts": scalar_facts(inspected)}))
}

#[test]
fn unchanged_export_is_bound_to_the_saved_facts_and_graphs() {
    let inspected = inspection();
    let plan = plan(&inspected);
    let (graphs, candidates) = artifacts(&inspected);
    assert!(same_inspection(&plan, &inspected).is_ok());
    assert!(validate_planner_artifacts(&plan, &graphs, &candidates).is_ok());
    assert_eq!(plan.planner_evidence.as_ref().unwrap().mode, CORRELATED_MODE);
}

#[test]
fn fact_drift_is_rejected_even_with_the_same_compiler_and_candidate() {
    let original = inspection();
    let plan = plan(&original);
    let mut changed = inspection();
    changed.targets[0].facts = changed.targets[0].facts.replace("\t-\t7\t-\t", "\t-\t8\t-\t");
    assert!(parse_wire(&changed.targets[0].facts).is_ok());
    assert_eq!(original.compiler_sha256, changed.compiler_sha256);
    assert_eq!(original.compiler_inputs, changed.compiler_inputs);
    assert_eq!(original.candidate_sha256, changed.candidate_sha256);
    assert!(same_inspection(&plan, &changed).unwrap_err().contains("evidence changed"));
}

#[test]
fn all_consumed_target_metadata_and_live_runtime_participate_in_the_binding() {
    let inspected = inspection();
    let plan = plan(&inspected);
    let original = serde_json::to_value(&inspected).unwrap();
    // These changes leave every raw fact string unchanged. A facts-only digest misses
    // them even though they affect eligibility, expectation or graph attribution.
    for (pointer, replacement) in [
        ("/Targets/0/Graph/Status", json!("Structural")),
        ("/Targets/0/Bound", json!(false)),
        ("/Targets/0/Eligible", json!(false)),
        ("/Targets/0/Reason", json!("new reason")),
        ("/Targets/0/RuleSource", json!("rules.md#another-clause")),
        ("/Targets/0/Parameters/0", json!("bool")),
        ("/Targets/0/Original/0/Expected", json!(8)),
        ("/Targets/0/OrdinaryKeys/0", json!("1")),
        ("/Targets/0/Signature", json!("Fixture.other(int)")),
        ("/Targets/0/Operation", json!("other")),
        ("/RuntimeVersion", json!("10.0.1")),
    ] {
        let mut changed = original.clone();
        *changed.pointer_mut(pointer).unwrap() = replacement;
        let changed: InspectResult = serde_json::from_value(changed).unwrap();
        assert_eq!(scalar_facts(&inspected).get("beta"), scalar_facts(&changed).get("beta"));
        assert!(same_inspection(&plan, &changed).is_err(), "unbound field: {pointer}");
    }
}

#[test]
fn target_order_and_missing_targets_change_the_inspection_identity() {
    let inspected = inspection();
    let plan = plan(&inspected);
    let mut reordered = inspection();
    reordered.targets.reverse();
    // Operation-keyed artifact objects are deliberately unordered; the full
    // inspection array still preserves the spec/worker target ordering.
    assert_eq!(scalar_facts(&inspected), scalar_facts(&reordered));
    assert!(same_inspection(&plan, &reordered).is_err());
    reordered.targets.pop();
    assert!(same_inspection(&plan, &reordered).is_err());
}

#[test]
fn saved_method_order_and_signatures_must_match_the_bound_inspection() {
    let inspected = inspection();
    let original = plan(&inspected);
    let mut reordered = original.clone();
    reordered.methods.reverse();
    assert!(same_inspection(&reordered, &inspected).is_err());
    let mut renamed = original.clone();
    renamed.methods[0].signature = "Fixture.unrelated(int)".into();
    assert!(same_inspection(&renamed, &inspected).is_err());
    let mut missing = original;
    missing.methods.pop();
    assert!(same_inspection(&missing, &inspected).is_err());
}

#[test]
fn saved_fact_and_graph_drift_are_rejected_before_live_inspection() {
    let inspected = inspection();
    let plan = plan(&inspected);
    let (graphs, mut candidates) = artifacts(&inspected);
    candidates["ScalarFacts"]["alpha"] = json!(inspected.targets[0].facts.replace("seed\t0", "seed\t1"));
    assert!(validate_planner_artifacts(&plan, &graphs, &candidates)
        .unwrap_err().contains("disagree with the planner binding"));

    let (mut graphs, candidates) = artifacts(&inspected);
    graphs["Graphs"]["alpha"]["Status"] = json!("different graph");
    assert!(validate_planner_artifacts(&plan, &graphs, &candidates)
        .unwrap_err().contains("disagree with the planner binding"));
}

#[test]
fn incomplete_malformed_and_structural_artifacts_never_satisfy_a_runnable_binding() {
    let inspected = inspection();
    let plan = plan(&inspected);
    let (graphs, candidates) = artifacts(&inspected);
    for changed in [json!({}), json!({"ScalarFacts": null}), json!({"ScalarFacts": []})] {
        assert!(validate_planner_artifacts(&plan, &graphs, &changed).is_err());
    }
    let mut missing = candidates.clone();
    missing["ScalarFacts"].as_object_mut().unwrap().remove("beta");
    assert!(validate_planner_artifacts(&plan, &graphs, &missing).is_err());
    let mut malformed = candidates.clone();
    malformed["ScalarFacts"]["alpha"] = json!("scalar-path-facts\t1\nend\n");
    assert!(validate_planner_artifacts(&plan, &graphs, &malformed).unwrap_err().contains("Incomplete"));
    let mut structural = graphs.clone();
    structural["BuildAttested"] = json!(false);
    assert!(validate_planner_artifacts(&plan, &structural, &candidates).is_err());
    let mut missing_graph = graphs;
    missing_graph["Graphs"].as_object_mut().unwrap().remove("beta");
    assert!(validate_planner_artifacts(&plan, &missing_graph, &candidates).is_err());
}

#[test]
fn duplicate_target_methods_cannot_hide_behind_operation_maps() {
    let inspected = inspection();
    let mut plan = plan(&inspected);
    let (graphs, candidates) = artifacts(&inspected);
    plan.methods[1] = plan.methods[0].clone();
    assert!(validate_planner_artifacts(&plan, &graphs, &candidates)
        .unwrap_err().contains("ambiguous target evidence"));
}

#[test]
fn canonical_objects_preserve_array_order_types_nulls_and_exact_fact_strings() {
    let left: Value = serde_json::from_str(r#"{"z":{"b":2,"a":1},"a":[1,2]}"#).unwrap();
    let right: Value = serde_json::from_str(r#"{"a":[1,2],"z":{"a":1,"b":2}}"#).unwrap();
    assert_eq!(evidence_hash(&left).unwrap(), evidence_hash(&right).unwrap());
    let mut reordered = right;
    reordered["a"] = json!([2, 1]);
    assert_ne!(evidence_hash(&left).unwrap(), evidence_hash(&reordered).unwrap());
    assert_ne!(evidence_hash(&json!({"value": 1})).unwrap(),
        evidence_hash(&json!({"value": "1"})).unwrap());
    assert_ne!(evidence_hash(&json!({"value": null})).unwrap(), evidence_hash(&json!({})).unwrap());
    assert_ne!(evidence_hash(&json!(i64::MIN)).unwrap(), evidence_hash(&json!(i64::MAX)).unwrap());

    let inspected = inspection();
    let plan = plan(&inspected);
    let mut crlf = inspection();
    crlf.targets[0].facts = crlf.targets[0].facts.replace('\n', "\r\n");
    assert!(parse_wire(&crlf.targets[0].facts).is_ok());
    assert!(same_inspection(&plan, &crlf).is_err());
}

#[test]
fn old_correlated_field_absence_is_readable_but_never_current() {
    let inspected = inspection();
    let mut encoded = serde_json::to_value(plan(&inspected)).unwrap();
    encoded.as_object_mut().unwrap().remove("PlannerEvidence");
    let old: PathPlan = serde_json::from_value(encoded).unwrap();
    assert!(old.planner_evidence.is_none());
    assert!(current_planner(&old).unwrap_err().contains("no sealed planner evidence"));
    assert!(same_inspection(&old, &inspected).is_err());
    assert!(!serde_json::to_value(&old).unwrap().as_object().unwrap().contains_key("PlannerEvidence"));

    for mode in ["graph-on", "graph-off"] {
        let mut legacy = old.clone();
        legacy.mode = mode.into();
        assert!(current_planner(&legacy).is_ok());
        assert!(same_inspection(&legacy, &inspected).is_ok());
    }
}

#[test]
fn unknown_binding_fields_are_malformed_and_unknown_revisions_are_not_current() {
    let inspected = inspection();
    let mut plan = plan(&inspected);
    let mut extra = serde_json::to_value(plan.planner_evidence.as_ref().unwrap()).unwrap();
    extra["UnrecognizedBinding"] = json!(true);
    assert!(serde_json::from_value::<PlannerEvidence>(extra).is_err());

    plan.planner_evidence.as_mut().unwrap().planner_revision = "unrecognized-revision".into();
    assert!(current_planner(&plan).unwrap_err().contains("Unsupported"));
    plan.planner_evidence.as_mut().unwrap().planner_revision = PLANNER_REVISION.into();
    plan.planner_evidence.as_mut().unwrap().format = "unrecognized-format".into();
    assert!(current_planner(&plan).unwrap_err().contains("Unsupported"));
}

#[test]
fn planner_artifact_changes_and_mode_inconsistent_bindings_are_rejected() {
    let inspected = inspection();
    let mut plan = plan(&inspected);
    let actual = planner_digest().unwrap();
    plan.planner_evidence.as_mut().unwrap().planner_sha256 = actual.clone();
    assert!(current_planner(&plan).is_ok());
    let mut changed = actual.into_bytes();
    changed[0] = if changed[0] == b'A' { b'B' } else { b'A' };
    plan.planner_evidence.as_mut().unwrap().planner_sha256 = String::from_utf8(changed).unwrap();
    assert!(current_planner(&plan).unwrap_err().contains("executable changed"));
    plan.planner_evidence.as_mut().unwrap().mode = "graph-off".into();
    assert!(same_inspection(&plan, &inspected).is_err());
}

#[test]
fn a_distinct_worker_remains_a_distinct_authority_with_equal_compiler_facts() {
    let first = identity();
    let mut second = first.clone();
    second.tool_digest = "A".repeat(64);
    assert!(!first.same_authority(&second));
    second.tool_digest = first.tool_digest.clone();
    second.operation = "replay".into();
    second.job = "B".repeat(32);
    assert!(first.same_authority(&second));
}

struct TempDirectory(PathBuf);

impl TempDirectory {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let nonce = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();
        let path = env::temp_dir().join(format!("slophawk-planner-evidence-{}-{nonce}-{}",
            std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
        fs::create_dir(&path).unwrap();
        Self(path)
    }
}

impl Drop for TempDirectory {
    fn drop(&mut self) { let _ = fs::remove_dir_all(&self.0); }
}

#[test]
fn planner_digest_is_byte_exact_and_rejects_empty_nonfiles_and_oversized_files() {
    let directory = TempDirectory::new();
    let file = directory.0.join("synthetic-native-bytes");
    fs::write(&file, b"abc").unwrap();
    assert_eq!(planner_digest_at(&file).unwrap(),
        "BA7816BF8F01CFEA414140DE5DAE2223B00361A396177A9CB410FF61F20015AD");
    fs::write(&file, b"abd").unwrap();
    assert_ne!(planner_digest_at(&file).unwrap(),
        "BA7816BF8F01CFEA414140DE5DAE2223B00361A396177A9CB410FF61F20015AD");
    fs::write(&file, b"").unwrap();
    assert!(planner_digest_at(&file).is_err());
    assert!(planner_digest_at(&directory.0).is_err());
    // Metadata rejection precedes any hashing; the length-only fixture does
    // not feed a 256 MiB payload to the digest loop.
    File::create(&file).unwrap().set_len(MAX_PLANNER + 1).unwrap();
    assert!(planner_digest_at(&file).unwrap_err().contains("at most 256 MiB"));
}

#[cfg(unix)]
#[test]
fn planner_digest_does_not_follow_a_supplied_symlink() {
    let directory = TempDirectory::new();
    let file = directory.0.join("target");
    let link = directory.0.join("link");
    fs::write(&file, b"abc").unwrap();
    std::os::unix::fs::symlink(&file, &link).unwrap();
    assert!(planner_digest_at(&link).is_err());
}

#[test]
fn evidence_size_cap_counts_serialized_bytes() {
    let oversized = json!({"Facts": "x".repeat(MAX_FRAME)});
    assert!(evidence_hash(&oversized).unwrap_err().contains("exceeds 16 MiB"));
}

#[test]
fn parsing_checks_the_artifact_bytes_against_the_verified_seal() {
    let directory = TempDirectory::new();
    let file = directory.0.join("plan.json");
    let before = br#"{"Mode":"graph-on"}"#;
    let seal = Seal {
        format: FORMAT.into(),
        kind: "plan".into(),
        files: vec![Entry {
            file: "plan.json".into(),
            length: before.len() as u64,
            sha256: hash(before),
        }],
        identity: identity(),
        mac: String::new(),
    };
    fs::write(&file, before).unwrap();
    let plan: Value = read_sealed_json(&directory.0, &seal, "plan.json").unwrap();
    assert_eq!(plan["Mode"], "graph-on");

    let replacement = br#"{"Mode":"tampered"}"#;
    assert_eq!(before.len(), replacement.len());
    fs::write(&file, replacement).unwrap();
    let error = read_sealed_json::<Value>(&directory.0, &seal, "plan.json").unwrap_err();
    assert!(error.contains("Sealed artifact changed while reading plan.json"));
}
