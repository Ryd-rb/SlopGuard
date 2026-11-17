use slophawk_python_project_context::{
    Admission, Artifact, CaseOutcome, Change, Collected, ContextError, ContextStatus, Gap,
    Inventory, Observation, Process, PytestCase, ReceiptBinding, ReceiptDisposition, Run,
    SelectionContext, SelectionStatus, SnapshotFile, WorkspaceScope, analyze, classify_receipt,
    node_ids_digest, select_tests,
};

fn source(path: &str, text: &str) -> SnapshotFile {
    SnapshotFile {
        path: path.into(),
        bytes: text.as_bytes().to_vec(),
    }
}

fn complete_context() -> slophawk_python_project_context::ProjectContext {
    analyze(
        WorkspaceScope {
            workspace_id: "workspace-1".into(),
            revision: "epoch-7-revision-9".into(),
            project_roots: vec!["apps/beta".into(), "apps/alpha".into()],
            inventory: Inventory::Complete,
        },
        vec![
            source(
                "apps/alpha/pyproject.toml",
                "[project]\nname = 'alpha'\n[tool.pytest.ini_options]\ntestpaths = ['tests']\n",
            ),
            source("apps/alpha/src/alpha/__init__.py", ""),
            source("apps/alpha/src/alpha/main.py", "from .utils import work\n"),
            source("apps/alpha/src/alpha/utils.py", "def work(): return 3\n"),
            source(
                "apps/alpha/tests/test_main.py",
                "def test_main(): assert True\n",
            ),
            source("apps/beta/pyproject.toml", "[project]\nname = 'beta'\n"),
            source("apps/beta/beta/__init__.py", ""),
            source(
                "apps/beta/tests/test_cmd.py",
                "def test_cmd(): assert True\n",
            ),
        ],
    )
    .expect("static context")
}

fn selected() -> SelectionContext {
    let context = complete_context();
    select_tests(
        &context,
        &"0".repeat(64),
        &[Change {
            before: Some("apps/alpha/src/alpha/main.py".into()),
            after: Some("apps/alpha/src/alpha/main.py".into()),
        }],
    )
    .unwrap()
}

fn binding(selection: &SelectionContext, ids: &[String]) -> ReceiptBinding {
    ReceiptBinding {
        attempt_id: "attempt-17".into(),
        project_root: "apps/alpha".into(),
        scope_sha256: selection.identity.scope_sha256.clone(),
        source_sha256: selection.identity.source_sha256.clone(),
        configuration_sha256: selection.identity.configuration_sha256.clone(),
        effective_pytest_config_sha256: "f".repeat(64),
        selection_sha256: selection.selection_sha256.clone(),
        interpreter_sha256: "a".repeat(64),
        environment_sha256: "b".repeat(64),
        pytest_sha256: "c".repeat(64),
        plugins_sha256: "d".repeat(64),
        collected_ids_sha256: node_ids_digest(ids),
    }
}

fn artifact(when: u64) -> Artifact {
    Artifact {
        sha256: "e".repeat(64),
        byte_count: 510,
        modified_ms: when,
        output_excerpt: String::new(),
    }
}

fn admission(b: &ReceiptBinding) -> Admission {
    Admission {
        binding: b.clone(),
        started_ms: 100,
        finished_ms: 200,
        owned_directory: true,
    }
}

fn evidence(
    b: &ReceiptBinding,
    ids: &[String],
    statuses: &[CaseOutcome],
    exit: i32,
) -> Observation {
    Observation {
        binding: b.clone(),
        collection: Some(Collected {
            process: if ids.is_empty() {
                Process::Exited(5)
            } else {
                Process::Exited(0)
            },
            finished: true,
            full_project_scope: true,
            filtered: false,
            collection_errors: 0,
            node_ids: ids.to_vec(),
            artifact: Some(artifact(130)),
        }),
        run: if ids.is_empty() {
            None
        } else {
            Some(Run {
                process: Process::Exited(exit),
                finished: true,
                cases: ids
                    .iter()
                    .zip(statuses.iter())
                    .map(|(id, status)| PytestCase {
                        node_id: id.clone(),
                        outcome: *status,
                        failure_excerpt: None,
                    })
                    .collect(),
                artifact: Some(artifact(170)),
            })
        },
    }
}

#[test]
fn package_roots_and_distinct_pyprojects_are_scoped() {
    let context = complete_context();
    assert_eq!(context.status, ContextStatus::StaticHintsOnly);
    assert_eq!(context.projects.len(), 2);
    assert_eq!(context.projects[0].root, "apps/alpha");
    assert_eq!(context.projects[0].project_name.as_deref(), Some("alpha"));
    assert_eq!(context.projects[0].pytest_testpaths, vec!["tests"]);
    assert_eq!(context.projects[0].package_roots[0].import_name, "alpha");
    assert_eq!(
        context.projects[0].package_roots[0].source_root,
        "apps/alpha/src"
    );
    assert_eq!(
        context.projects[0].static_test_hints,
        vec!["apps/alpha/tests/test_main.py"]
    );
    assert_eq!(
        context.projects[1].package_roots[0].source_root,
        "apps/beta"
    );
    let selection = selected();
    assert_eq!(selection.status, SelectionStatus::NeedsAdmittedCollection);
    assert_eq!(selection.impacted.len(), 1);
    assert!(selection.impacted[0].collect_entire_root);
    assert_eq!(selection.impacted[0].root, "apps/alpha");
}

#[test]
fn changes_spanning_projects_and_deletes_do_not_drop_before_paths() {
    let context = complete_context();
    let selection = select_tests(
        &context,
        &"0".repeat(64),
        &[
            Change {
                before: Some("apps/alpha/tests/test_old.py".into()),
                after: Some("apps/beta/tests/test_new.py".into()),
            },
            Change {
                before: Some("apps/alpha/src/alpha/removed.py".into()),
                after: None,
            },
        ],
    )
    .unwrap();
    assert_eq!(selection.impacted.len(), 2);
    assert!(
        selection.impacted[0]
            .changed_paths
            .contains(&"apps/alpha/src/alpha/removed.py".to_string())
    );
    assert!(
        selection.impacted[1]
            .changed_paths
            .contains(&"apps/beta/tests/test_new.py".to_string())
    );
    assert_ne!(selection.selection_sha256, selected().selection_sha256);
}

#[test]
fn offscope_and_partial_inventory_never_yield_an_empty_complete_selection() {
    let context = complete_context();
    let selection = select_tests(
        &context,
        &"0".repeat(64),
        &[Change {
            before: Some("not_admitted/test_x.py".into()),
            after: None,
        }],
    )
    .unwrap();
    assert_eq!(selection.status, SelectionStatus::UnresolvedScope);
    assert_eq!(selection.unowned_changes, vec!["not_admitted/test_x.py"]);
    let ctx = analyze(
        WorkspaceScope {
            workspace_id: "w".into(),
            revision: "r".into(),
            project_roots: vec![".".into()],
            inventory: Inventory::Partial,
        },
        vec![source("lib.py", "__import__(name)\n")],
    )
    .unwrap();
    assert!(ctx.projects[0].static_test_hints.is_empty());
    assert!(ctx.projects[0].gaps.contains(&Gap::PartialInventory));
    assert!(ctx.projects[0].gaps.contains(&Gap::NoStaticTestHints));
    assert!(ctx.projects[0].gaps.contains(&Gap::DynamicImport));
    let partial = select_tests(
        &ctx,
        &"0".repeat(64),
        &[Change {
            before: None,
            after: Some("lib.py".into()),
        }],
    )
    .unwrap();
    assert_eq!(partial.status, SelectionStatus::UnresolvedScope);
}

#[test]
fn content_and_config_changes_invalidate_fingerprints() {
    let base = complete_context();
    let changed = analyze(
        WorkspaceScope {
            workspace_id: "workspace-1".into(),
            revision: "epoch-7-revision-9".into(),
            project_roots: vec!["apps/alpha".into(), "apps/beta".into()],
            inventory: Inventory::Complete,
        },
        vec![
            source("apps/alpha/pyproject.toml", "[project]\nname='different'\n"),
            source("apps/beta/pyproject.toml", "[project]\nname='beta'\n"),
        ],
    )
    .unwrap();
    assert_ne!(base.identity.source_sha256, changed.identity.source_sha256);
    assert_ne!(
        base.identity.configuration_sha256,
        changed.identity.configuration_sha256
    );
    assert_eq!(base.identity.scope_sha256, changed.identity.scope_sha256);
    assert!(
        analyze(
            WorkspaceScope {
                workspace_id: "w".into(),
                revision: "r".into(),
                project_roots: vec![".".into()],
                inventory: Inventory::Complete,
            },
            vec![source("../escape.py", "")]
        )
        .is_err()
    );
    assert_eq!(
        select_tests(
            &base,
            "xyz",
            &[Change {
                before: None,
                after: Some("apps/alpha/foo.py".into())
            }]
        ),
        Err(ContextError::InvalidIdentity("baseline_source_sha256"))
    );
}

#[test]
fn source_only_does_not_import_and_marks_collection_hooks() {
    let c = analyze(
        WorkspaceScope {
            workspace_id: "w".into(),
            revision: "r".into(),
            project_roots: vec![".".into()],
            inventory: Inventory::Complete,
        },
        vec![
            source(
                "pyproject.toml",
                "[tool.pytest.ini_options]\npython_files = ['spec_*.py']\n",
            ),
            source(
                "tests/conftest.py",
                "def pytest_collect_file(file_path, parent): raise RuntimeError('not executed')\n",
            ),
            source(
                "src/pkg/mod.py",
                "import importlib\nraise RuntimeError('not executed')\n",
            ),
        ],
    )
    .unwrap();
    let project = &c.projects[0];
    assert!(project.gaps.contains(&Gap::CustomCollection));
    assert!(project.gaps.contains(&Gap::DynamicImport));
    assert!(project.gaps.contains(&Gap::UnsupportedTestDiscovery));
    assert!(project.gaps.contains(&Gap::NamespacePackage));
    assert!(project.static_test_hints.is_empty());
}

#[test]
fn zero_requires_finished_admitted_collection() {
    let selection = selected();
    let ids: Vec<String> = vec![];
    let b = binding(&selection, &ids);
    let admitted = admission(&b);
    let complete = evidence(&b, &ids, &[], 5);
    assert_eq!(
        classify_receipt(&selection, &admitted, &complete).disposition,
        ReceiptDisposition::ZeroDiscovered
    );
    let mut partial = complete.clone();
    partial.collection.as_mut().unwrap().finished = false;
    assert_eq!(
        classify_receipt(&selection, &admitted, &partial).disposition,
        ReceiptDisposition::CollectionFailure
    );
    partial.collection.as_mut().unwrap().finished = true;
    partial.collection.as_mut().unwrap().process = Process::Exited(3);
    assert_eq!(
        classify_receipt(&selection, &admitted, &partial).disposition,
        ReceiptDisposition::CollectionFailure
    );
}

#[test]
fn missing_runner_all_skipped_failure_and_pass_are_distinct() {
    let selection = selected();
    let ids = vec![
        "tests/test_main.py::test_one".into(),
        "tests/test_main.py::test_two".into(),
    ];
    let b = binding(&selection, &ids);
    let admitted = admission(&b);
    let mut missing = evidence(&b, &ids, &[CaseOutcome::Passed, CaseOutcome::Skipped], 0);
    missing.collection.as_mut().unwrap().process = Process::MissingRunner;
    assert_eq!(
        classify_receipt(&selection, &admitted, &missing).disposition,
        ReceiptDisposition::MissingRunner
    );
    let passed = evidence(&b, &ids, &[CaseOutcome::Passed, CaseOutcome::Skipped], 0);
    assert_eq!(
        classify_receipt(&selection, &admitted, &passed).disposition,
        ReceiptDisposition::Passed
    );
    let all_skipped = evidence(&b, &ids, &[CaseOutcome::Skipped, CaseOutcome::XFailed], 0);
    assert_eq!(
        classify_receipt(&selection, &admitted, &all_skipped).disposition,
        ReceiptDisposition::AllSkipped
    );
    let failed = evidence(&b, &ids, &[CaseOutcome::Failed, CaseOutcome::Passed], 1);
    let report = classify_receipt(&selection, &admitted, &failed);
    assert_eq!(report.disposition, ReceiptDisposition::Failed);
    assert_eq!((report.passed, report.failed), (1, 1));
}

#[test]
fn stale_environment_ids_artifact_and_malformed_facts_are_rejected() {
    let selection = selected();
    let ids = vec![
        "tests/test_main.py::test_one".into(),
        "tests/test_main.py::test_two".into(),
    ];
    let b = binding(&selection, &ids);
    let admitted = admission(&b);
    let good = evidence(&b, &ids, &[CaseOutcome::Passed, CaseOutcome::Passed], 0);
    let mut wrong_env = good.clone();
    wrong_env.binding.environment_sha256 = "f".repeat(64);
    assert_eq!(
        classify_receipt(&selection, &admitted, &wrong_env).disposition,
        ReceiptDisposition::Stale
    );
    let mut wrong_config = good.clone();
    wrong_config.binding.effective_pytest_config_sha256 = "1".repeat(64);
    assert_eq!(
        classify_receipt(&selection, &admitted, &wrong_config).disposition,
        ReceiptDisposition::Stale
    );
    let mut wrong_source = good.clone();
    wrong_source.binding.source_sha256 = "2".repeat(64);
    assert_eq!(
        classify_receipt(&selection, &admitted, &wrong_source).disposition,
        ReceiptDisposition::Stale
    );
    let mut stale = good.clone();
    stale
        .run
        .as_mut()
        .unwrap()
        .artifact
        .as_mut()
        .unwrap()
        .modified_ms = 99;
    assert_eq!(
        classify_receipt(&selection, &admitted, &stale).disposition,
        ReceiptDisposition::Stale
    );
    let mut duplicate = good.clone();
    duplicate.collection.as_mut().unwrap().node_ids[1] = ids[0].clone();
    assert_eq!(
        classify_receipt(&selection, &admitted, &duplicate).disposition,
        ReceiptDisposition::InvalidFacts
    );
    let mut mismatched = good.clone();
    mismatched.collection.as_mut().unwrap().node_ids.reverse();
    assert_eq!(
        classify_receipt(&selection, &admitted, &mismatched).disposition,
        ReceiptDisposition::Stale
    );
    let mut oversized = good.clone();
    oversized
        .run
        .as_mut()
        .unwrap()
        .artifact
        .as_mut()
        .unwrap()
        .output_excerpt = "x".repeat(4_097);
    assert_eq!(
        classify_receipt(&selection, &admitted, &oversized).disposition,
        ReceiptDisposition::InvalidFacts
    );
    let mut uncollected = good.clone();
    uncollected.run.as_mut().unwrap().cases[0].node_id = "tests/unknown.py::test_ghost".into();
    assert_eq!(
        classify_receipt(&selection, &admitted, &uncollected).disposition,
        ReceiptDisposition::InvalidFacts
    );
    let mut missing_case = good;
    missing_case.run.as_mut().unwrap().cases.pop();
    assert_eq!(
        classify_receipt(&selection, &admitted, &missing_case).disposition,
        ReceiptDisposition::Incomplete
    );
}

#[test]
fn nonzero_exits_do_not_become_passes() {
    let selection = selected();
    let ids = vec!["tests/test_main.py::test_one".into()];
    let b = binding(&selection, &ids);
    let admitted = admission(&b);
    assert_eq!(
        classify_receipt(
            &selection,
            &admitted,
            &evidence(&b, &ids, &[CaseOutcome::Passed], 3)
        )
        .disposition,
        ReceiptDisposition::RunnerFailure
    );
    assert_eq!(
        classify_receipt(
            &selection,
            &admitted,
            &evidence(&b, &ids, &[CaseOutcome::Failed], 0)
        )
        .disposition,
        ReceiptDisposition::InvalidFacts
    );
    let mut incomplete = evidence(&b, &ids, &[CaseOutcome::Passed], 0);
    incomplete.run.as_mut().unwrap().process = Process::TimedOut;
    assert_eq!(
        classify_receipt(&selection, &admitted, &incomplete).disposition,
        ReceiptDisposition::TimedOut
    );
}

#[test]
fn explicitly_filtered_or_partial_collection_cannot_claim_zero_tests() {
    let selection = selected();
    let ids: Vec<String> = vec![];
    let b = binding(&selection, &ids);
    let admitted = admission(&b);
    let mut filtered = evidence(&b, &ids, &[], 5);
    filtered.collection.as_mut().unwrap().filtered = true;
    assert_eq!(
        classify_receipt(&selection, &admitted, &filtered).disposition,
        ReceiptDisposition::UnresolvedSelection
    );
    filtered.collection.as_mut().unwrap().filtered = false;
    filtered.collection.as_mut().unwrap().full_project_scope = false;
    assert_eq!(
        classify_receipt(&selection, &admitted, &filtered).disposition,
        ReceiptDisposition::UnresolvedSelection
    );
}

#[test]
fn synthetic_pytest_9_0_2_observed_outcomes_have_failure_disposition() {
    // Reproduced once in an external admitted sandbox with
    // PYTEST_DISABLE_PLUGIN_AUTOLOAD=1, pytest 9.0.2, Python 3.13.5.
    // JUnit XML reports 3 tests, 0 errors, 1 failure, 1 skipped, exit 1.
    // The identity/profile hashes below are typed fixture placeholders, not
    // a claim that this local test imports or verifies the raw XML receipt.
    let selection = selected();
    let ids = vec![
        "tests/test_synthetic.py::test_pass".into(),
        "tests/test_synthetic.py::test_skip".into(),
        "tests/test_synthetic.py::test_fail".into(),
    ];
    let b = binding(&selection, &ids);
    let admitted = admission(&b);
    let report = classify_receipt(
        &selection,
        &admitted,
        &evidence(
            &b,
            &ids,
            &[
                CaseOutcome::Passed,
                CaseOutcome::Skipped,
                CaseOutcome::Failed,
            ],
            1,
        ),
    );
    assert_eq!(report.disposition, ReceiptDisposition::Failed);
    assert_eq!(
        (
            report.collected,
            report.passed,
            report.failed,
            report.skipped
        ),
        (3, 1, 1, 1)
    );
}
