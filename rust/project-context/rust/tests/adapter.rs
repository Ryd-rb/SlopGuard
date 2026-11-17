use serde_json::{Value, json};
use slophawk_rust_project_context::{
    Binding, CargoGraph, CargoTestJob, Change, Coverage, Disposition, Error, Features,
    ImpactReasonKind, MetadataInput, SnapshotFile, WorkerCapture, WorkerExit,
    classify_test_capture, interpret_metadata, metadata_argv, select_tests, sha256_hex, test_argv,
};

const ROOT: &str = "/work";
const ALPHA: &str = "path+file:///work/alpha#0.1.0";
const BETA: &str = "path+file:///work/beta#0.1.0";
const FAKE_DIGEST: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const GIT: &str = "0123456789012345678901234567890123456789";

fn binding(revision: u64) -> Binding {
    Binding {
        workspace_root: ROOT.into(),
        workspace_revision: revision,
        epoch: "workspace_epoch_1".into(),
        git_commit: GIT.into(),
        target_triple: "x86_64-unknown-linux-gnu".into(),
        rustc_version: "rustc 1.99.1".into(),
        rustc_vv_sha256: FAKE_DIGEST.into(),
        cargo_executable_sha256: FAKE_DIGEST.into(),
        environment_sha256: FAKE_DIGEST.into(),
        features: Features {
            selected: vec!["alpha/with-beta".into()],
            ..Features::default()
        },
    }
}

fn snapshot(path: &str) -> SnapshotFile {
    let bytes = format!("# bounded synthetic fixture at {path}\n").into_bytes();
    SnapshotFile {
        path: path.into(),
        expected_sha256: sha256_hex(&bytes),
        bytes,
    }
}

fn fake_package(name: &str, path: &str, src: &str, optional_dep: bool) -> Value {
    json!({
        "id": if name == "alpha" { ALPHA } else { BETA },
        "name": name,
        "version": "0.1.0",
        "edition": "2024",
        "manifest_path": path,
        "targets": [{"name": name,"kind":["lib"],"src_path":src}],
        "dependencies": if optional_dep { vec![json!({
            "name": "beta", "rename": null, "req": "*", "kind": null,
            "target": "cfg(target_os = \"linux\")", "optional": true
        })] } else { vec![] }
    })
}

fn metadata_value(beta_path: &str, active: bool) -> Value {
    json!({
        "version":1,
        "workspace_root": ROOT,
        "workspace_members": [ALPHA, BETA],
        "packages": [
            fake_package("alpha", "/work/alpha/Cargo.toml", "/work/alpha/src/lib.rs", true),
            fake_package("beta", "/work/beta/Cargo.toml", beta_path, false)
        ],
        "resolve": {"root":null,"nodes":[
            {"id":ALPHA, "features": if active {vec!["with-beta"]} else {vec![]},
             "deps":if active {vec![json!({
                 "name":"beta","pkg":BETA,
                 "dep_kinds":[{"kind":null,"target":"cfg(target_os = \"linux\")"}]
             })]} else {vec![]}},
            {"id":BETA,"features":[],"deps":[]}
        ]}
    })
}

fn metadata_input(version: u64, value: Value) -> MetadataInput {
    let b = binding(version);
    let path = format!("{ROOT}/Cargo.toml");
    let command = metadata_argv(&b, &path).unwrap();
    MetadataInput {
        binding: b.clone(),
        manifest_path: path,
        manifests: vec![
            snapshot("/work/Cargo.toml"),
            snapshot("/work/alpha/Cargo.toml"),
            snapshot("/work/beta/Cargo.toml"),
        ],
        lock: snapshot("/work/Cargo.lock"),
        capture: WorkerCapture {
            binding: b,
            argv: command,
            admitted: true,
            attempt_id: None,
            fence_sequence: None,
            graph_sha256: None,
            exit: WorkerExit::Exited(0),
            truncated: false,
            stdout: serde_json::to_vec(&value).unwrap(),
            stderr: vec![],
        },
    }
}

fn graph(revision: u64) -> CargoGraph {
    interpret_metadata(metadata_input(
        revision,
        metadata_value("/work/beta/src/lib.rs", true),
    ))
    .unwrap()
}

fn test_capture(
    graph: &CargoGraph,
    output: &str,
    exit: WorkerExit,
) -> (CargoTestJob, WorkerCapture) {
    let job = CargoTestJob {
        context_sha256: graph.context_sha256.clone(),
        package_id: ALPHA.into(),
        attempt_id: 71,
        fence_sequence: 3,
    };
    let command = test_argv(&graph.binding, &graph.root_manifest, "alpha@0.1.0").unwrap();
    (
        job,
        WorkerCapture {
            binding: graph.binding.clone(),
            argv: command,
            admitted: true,
            attempt_id: Some(71),
            fence_sequence: Some(3),
            graph_sha256: Some(graph.context_sha256.clone()),
            exit,
            truncated: false,
            stdout: output.as_bytes().to_vec(),
            stderr: vec![],
        },
    )
}

const PASS: &str = concat!(
    "{\"reason\":\"build-finished\",\"success\":true}\n",
    "running 2 tests\n",
    "test one ... ok\ntest two ... ok\n",
    "test result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s\n",
    "running 0 tests\n",
    "test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s\n",
);

#[test]
fn manifest_lock_target_variant_and_optional_edge_are_bound() {
    let g = graph(1);
    assert_eq!(g.workspace_members.len(), 2);
    assert_eq!(g.coverage, Coverage::EvaluatedCargoGraph);
    assert_eq!(g.manifest_digests.len(), 3);
    assert_eq!(g.lock_sha256.len(), 64);
    assert_eq!(g.context_sha256.len(), 64);
    assert_eq!(g.resolved_edges.len(), 1);
    assert_eq!(g.resolved_edges[0].optional, Some(true));
    assert_eq!(
        g.resolved_edges[0].kinds[0].target_expression.as_deref(),
        Some("cfg(target_os = \"linux\")")
    );
    assert_eq!(g.packages[ALPHA].activated_features, vec!["with-beta"]);
    assert!(g.packages[ALPHA].declared_dependencies[0].optional);
    let no_optional = interpret_metadata(metadata_input(
        1,
        metadata_value("/work/beta/src/lib.rs", false),
    ))
    .unwrap();
    assert!(no_optional.resolved_edges.is_empty());
    assert_ne!(g.context_sha256, no_optional.context_sha256);
}

#[test]
fn selection_walks_current_and_predecessor_memberships() {
    let old = graph(1);
    let new = graph(2);
    let selection = select_tests(
        Some(&old),
        &new,
        &[Change {
            path: "/work/beta/src/lib.rs".into(),
            previous_path: None,
        }],
        8,
    )
    .unwrap();
    assert!(selection.exact);
    assert_eq!(selection.selected.len(), 2);
    assert!(selection.selected.iter().any(|t| {
        t.package_id == ALPHA
            && t.reasons
                .iter()
                .any(|r| r.kind == ImpactReasonKind::CurrentDependency)
    }));
    assert!(selection.selected.iter().any(|t| {
        t.package_id == BETA
            && t.reasons
                .iter()
                .any(|r| r.kind == ImpactReasonKind::PredecessorMembership)
    }));
    let renamed = interpret_metadata(metadata_input(
        2,
        metadata_value("/work/beta/src/renamed.rs", true),
    ))
    .unwrap();
    let changed = select_tests(
        Some(&old),
        &renamed,
        &[Change {
            path: "/work/beta/src/renamed.rs".into(),
            previous_path: Some("/work/beta/src/lib.rs".into()),
        }],
        8,
    )
    .unwrap();
    assert!(changed.exact);
    assert_eq!(changed.selected.len(), 2);
}

#[test]
fn unknown_module_removed_package_and_variant_changes_defer_exactness() {
    let old = graph(1);
    let new = graph(2);
    let result = select_tests(
        Some(&old),
        &new,
        &[Change {
            path: "/work/alpha/src/unknown.rs".into(),
            previous_path: None,
        }],
        1,
    )
    .unwrap();
    assert!(result.conservative_fallback);
    assert_eq!(result.selected.len(), 1);
    assert_eq!(result.deferred_package_ids.len(), 1);
    assert!(!result.exact);
    assert!(result.unresolved.iter().any(|s| s.contains("membership")));
    let mut version = new.clone();
    version.binding.target_triple = "aarch64-unknown-linux-gnu".into();
    let changed = select_tests(
        Some(&old),
        &version,
        &[Change {
            path: "/work/beta/src/lib.rs".into(),
            previous_path: None,
        }],
        8,
    )
    .unwrap();
    assert!(changed.conservative_fallback);
    assert!(
        changed
            .unresolved
            .contains(&"predecessor_variant_mismatch".into())
    );
    let first_revision = select_tests(
        None,
        &old,
        &[Change {
            path: "/work/beta/src/lib.rs".into(),
            previous_path: None,
        }],
        8,
    )
    .unwrap();
    assert!(first_revision.conservative_fallback);
}

#[test]
fn unknown_non_rust_inputs_conservatively_select_every_workspace_package() {
    let old = graph(1);
    let new = graph(2);
    for path in [
        "/work/alpha/data.json",
        "/work/beta/payload.txt",
        "/work/schema",
    ] {
        let selection = select_tests(
            Some(&old),
            &new,
            &[Change {
                path: path.into(),
                previous_path: None,
            }],
            8,
        )
        .unwrap();
        assert!(!selection.exact, "{path}");
        assert!(selection.conservative_fallback, "{path}");
        assert_eq!(selection.selected.len(), 2, "{path}");
        assert!(
            selection
                .unresolved
                .iter()
                .any(|reason| reason == "source_membership_not_enumerated_by_cargo")
        );
    }
}

#[test]
fn matching_rename_endpoint_does_not_hide_unknown_membership_in_other_revision() {
    let old = graph(1);
    let new = graph(2);
    for (path, previous_path, frontier) in [
        (
            "/work/alpha/src/lib.rs",
            "/work/beta/src/moved.rs",
            "prior_source_membership_not_enumerated",
        ),
        (
            "/work/alpha/src/moved.rs",
            "/work/beta/src/lib.rs",
            "source_membership_not_enumerated_by_cargo",
        ),
    ] {
        let selection = select_tests(
            Some(&old),
            &new,
            &[Change {
                path: path.into(),
                previous_path: Some(previous_path.into()),
            }],
            8,
        )
        .unwrap();
        assert!(!selection.exact);
        assert!(selection.conservative_fallback);
        assert_eq!(selection.selected.len(), 2);
        assert!(selection.unresolved.iter().any(|reason| reason == frontier));
    }
}

#[test]
fn missing_dependencies_and_unsupported_flags_are_blocked() {
    let mut blocked = metadata_input(1, metadata_value("/work/beta/src/lib.rs", true));
    blocked.capture.exit = WorkerExit::Exited(101);
    assert!(matches!(
        interpret_metadata(blocked),
        Err(Error::Blocked(_))
    ));

    let mut wrong_flag = metadata_input(1, metadata_value("/work/beta/src/lib.rs", true));
    wrong_flag.capture.argv.push("--no-deps".into());
    assert!(matches!(
        interpret_metadata(wrong_flag),
        Err(Error::Unsupported(_))
    ));

    let mut missing = metadata_value("/work/beta/src/lib.rs", true);
    missing["resolve"]["nodes"][0]["deps"][0]["pkg"] = json!("unknown");
    assert!(matches!(
        interpret_metadata(metadata_input(1, missing)),
        Err(Error::Malformed(_))
    ));
}

#[test]
fn malformed_stale_truncated_and_missing_snapshot_are_not_complete() {
    let mut stale = metadata_input(1, metadata_value("/work/beta/src/lib.rs", true));
    stale.manifests[0].bytes.push(0);
    assert!(matches!(interpret_metadata(stale), Err(Error::Stale(_))));

    let mut stale_capture = metadata_input(1, metadata_value("/work/beta/src/lib.rs", true));
    stale_capture.capture.binding.workspace_revision += 1;
    assert!(matches!(
        interpret_metadata(stale_capture),
        Err(Error::Stale(_))
    ));

    let mut wrong_json = metadata_input(1, metadata_value("/work/beta/src/lib.rs", true));
    wrong_json.capture.stdout = b"{".to_vec();
    assert!(matches!(
        interpret_metadata(wrong_json),
        Err(Error::Malformed(_))
    ));

    let mut truncated = metadata_input(1, metadata_value("/work/beta/src/lib.rs", true));
    truncated.capture.truncated = true;
    assert!(matches!(
        interpret_metadata(truncated),
        Err(Error::Blocked(_))
    ));

    let mut absent = metadata_input(1, metadata_value("/work/beta/src/lib.rs", true));
    absent.manifests.pop();
    let partial = interpret_metadata(absent).unwrap();
    assert_eq!(partial.coverage, Coverage::PartialCargoGraph);
    assert!(
        partial
            .unresolved
            .contains(&"member_manifest_digest_missing".into())
    );
}

#[test]
fn test_receipts_pass_fail_skip_zero_and_filtered() {
    let g = graph(4);
    let (job, cap) = test_capture(&g, PASS, WorkerExit::Exited(0));
    let receipt = classify_test_capture(&g, &job, &cap).unwrap();
    assert_eq!(receipt.disposition, Disposition::Passed);
    assert_eq!(receipt.counters.unwrap().harnesses, 2);
    assert!(receipt.is_current(&g));
    assert!(receipt.package_scope_only);

    let fail = "{\"reason\":\"build-finished\",\"success\":true}\nrunning 1 test\ntest a ... FAILED\n\
        test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s\n";
    let (job, cap) = test_capture(&g, fail, WorkerExit::Exited(101));
    let receipt = classify_test_capture(&g, &job, &cap).unwrap();
    assert_eq!(receipt.disposition, Disposition::Failed);
    assert_eq!(receipt.counters.unwrap().failed, 1);

    let skipped = "{\"reason\":\"build-finished\",\"success\":true}\nrunning 2 tests\n\
        test result: ok. 0 passed; 0 failed; 2 ignored; 0 measured; 0 filtered out; finished in 0.00s\n";
    let (job, cap) = test_capture(&g, skipped, WorkerExit::Exited(0));
    assert_eq!(
        classify_test_capture(&g, &job, &cap).unwrap().disposition,
        Disposition::AllSkipped
    );

    let zero = "{\"reason\":\"build-finished\",\"success\":true}\nrunning 0 tests\n\
        test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s\n";
    let (job, cap) = test_capture(&g, zero, WorkerExit::Exited(0));
    assert_eq!(
        classify_test_capture(&g, &job, &cap).unwrap().disposition,
        Disposition::ZeroDiscovered
    );

    let filtered = "{\"reason\":\"build-finished\",\"success\":true}\nrunning 0 tests\n\
        test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 4 filtered out; finished in 0.00s\n";
    let (job, cap) = test_capture(&g, filtered, WorkerExit::Exited(0));
    assert_eq!(
        classify_test_capture(&g, &job, &cap).unwrap().disposition,
        Disposition::FilteredOut
    );
}

#[test]
fn test_receipts_distinguish_build_runner_and_malformed_output() {
    let g = graph(9);
    let (job, cap) = test_capture(
        &g,
        "{\"reason\":\"build-finished\",\"success\":false}\n",
        WorkerExit::Exited(101),
    );
    let receipt = classify_test_capture(&g, &job, &cap).unwrap();
    assert_eq!(receipt.disposition, Disposition::BuildFailure);
    assert!(receipt.counters.is_none());

    let (job, cap) = test_capture(&g, "compilation failed\n", WorkerExit::Exited(101));
    assert_eq!(
        classify_test_capture(&g, &job, &cap).unwrap().disposition,
        Disposition::RunnerFailure
    );

    let (job, cap) = test_capture(
        &g,
        "{\"reason\":\"build-finished\",\"success\":true}\nrunning 1 test\n",
        WorkerExit::Exited(0),
    );
    assert_eq!(
        classify_test_capture(&g, &job, &cap).unwrap().disposition,
        Disposition::Malformed
    );

    let (job, cap) = test_capture(
        &g,
        "{\"reason\":\"build-finished\",\"success\":true}\nrunning 1 test\ntest result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s\n",
        WorkerExit::Exited(0),
    );
    assert_eq!(
        classify_test_capture(&g, &job, &cap).unwrap().disposition,
        Disposition::Malformed
    );

    let (job, mut cap) = test_capture(&g, PASS, WorkerExit::Exited(0));
    cap.truncated = true;
    let receipt = classify_test_capture(&g, &job, &cap).unwrap();
    assert_eq!(receipt.disposition, Disposition::Incomplete);
    assert!(receipt.counters.is_none());

    let (job, cap) = test_capture(&g, PASS, WorkerExit::TimedOut);
    assert_eq!(
        classify_test_capture(&g, &job, &cap).unwrap().disposition,
        Disposition::TimedOut
    );
}

#[test]
fn attempts_and_revision_fences_reject_replayed_test_output() {
    let g = graph(5);
    let (job, mut cap) = test_capture(&g, PASS, WorkerExit::Exited(0));
    cap.attempt_id = Some(job.attempt_id + 1);
    assert!(matches!(
        classify_test_capture(&g, &job, &cap),
        Err(Error::Stale(_))
    ));
    cap.attempt_id = Some(job.attempt_id);
    cap.fence_sequence = Some(job.fence_sequence + 1);
    assert!(matches!(
        classify_test_capture(&g, &job, &cap),
        Err(Error::Stale(_))
    ));
    cap.fence_sequence = Some(job.fence_sequence);
    cap.graph_sha256 = Some(FAKE_DIGEST.into());
    assert!(matches!(
        classify_test_capture(&g, &job, &cap),
        Err(Error::Stale(_))
    ));
    cap.graph_sha256 = Some(g.context_sha256.clone());
    cap.argv.push("--ignored".into());
    assert!(matches!(
        classify_test_capture(&g, &job, &cap),
        Err(Error::Unsupported(_))
    ));
}

#[test]
fn missing_build_receipt_and_oversized_capture_cannot_pass() {
    let g = graph(12);
    let summary_without_build = "running 1 test\n\
        test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s\n";
    let (job, cap) = test_capture(&g, summary_without_build, WorkerExit::Exited(0));
    let receipt = classify_test_capture(&g, &job, &cap).unwrap();
    assert_eq!(receipt.disposition, Disposition::RunnerFailure);
    assert!(receipt.counters.is_none());

    let (job, mut cap) = test_capture(&g, PASS, WorkerExit::Exited(0));
    cap.stdout
        .resize(slophawk_rust_project_context::MAX_LOG_BYTES + 1, b'X');
    let receipt = classify_test_capture(&g, &job, &cap).unwrap();
    assert_eq!(receipt.disposition, Disposition::Incomplete);
    assert!(receipt.counters.is_none());

    let mut oversized_metadata = metadata_input(12, metadata_value("/work/beta/src/lib.rs", true));
    oversized_metadata
        .capture
        .stdout
        .resize(slophawk_rust_project_context::MAX_METADATA_BYTES + 1, b'X');
    assert!(matches!(
        interpret_metadata(oversized_metadata),
        Err(Error::Limit(_))
    ));

    let mut unsupported = binding(12);
    unsupported.features.all = true;
    assert!(matches!(unsupported.validate(), Err(Error::Unsupported(_))));
    unsupported.features.all = false;
    unsupported.features.selected.push("alpha/with-beta".into());
    assert!(matches!(unsupported.validate(), Err(Error::Unsupported(_))));
}

#[test]
fn removed_workspace_member_and_partial_predecessor_are_conservative() {
    let old = graph(21);
    let mut new = graph(22);
    new.workspace_members.remove(BETA);
    let changed = select_tests(
        Some(&old),
        &new,
        &[Change {
            path: "/work/beta/src/lib.rs".into(),
            previous_path: None,
        }],
        8,
    )
    .unwrap();
    assert!(changed.conservative_fallback);
    assert!(!changed.exact);
    assert!(
        changed
            .unresolved
            .iter()
            .any(|r| r == "removed_or_reidentified_workspace_member")
    );
    assert_eq!(changed.selected.len(), 1);
    assert_eq!(changed.selected[0].package_id, ALPHA);

    let mut prior = old;
    prior.coverage = Coverage::PartialCargoGraph;
    let changed = select_tests(
        Some(&prior),
        &graph(22),
        &[Change {
            path: "/work/alpha/src/lib.rs".into(),
            previous_path: None,
        }],
        8,
    )
    .unwrap();
    assert!(changed.conservative_fallback);
    assert!(
        changed
            .unresolved
            .contains(&"predecessor_cargo_graph_partial".into())
    );
}
