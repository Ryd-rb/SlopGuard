use slophawk_test_impact::*;
use std::collections::HashMap;

fn target(id: &str, test: bool, members: &[&str], refs: &[&str]) -> ProjectTarget {
    ProjectTarget {
        id: id.into(),
        project_path: id.split('|').next().unwrap().into(),
        framework: id.split('|').nth(1).unwrap_or("net10.0").into(),
        configuration: "Debug".into(),
        platform: "AnyCPU".into(),
        runtime_identifier: String::new(),
        sdk_version: "10.0.404".into(),
        is_test_project: test,
        compile: members
            .iter()
            .map(|&path| CompileMember {
                path: path.into(),
                link: path.starts_with("Shared/").then(|| "Linked.cs".into()),
            })
            .collect(),
        references: refs.iter().map(|&x| x.into()).collect(),
        evaluation_fingerprint: format!("eval:{id}"),
    }
}
fn graph(targets: Vec<ProjectTarget>) -> ProjectGraph {
    ProjectGraph {
        targets,
        issues: vec![],
    }
}
fn cause(old: Option<&str>, new: Option<&str>) -> TestCause {
    TestCause {
        revision: 1,
        path_key: old.or(new).unwrap_or("none").into(),
        old_path: old.map(str::to_owned),
        old_digest: old.map(|_| "old-digest".into()),
        new_path: new.map(str::to_owned),
        new_digest: new.map(|_| "new-digest".into()),
        predecessor: Some("graph:0".into()),
        fallback_first_revision: None,
    }
}
fn input(current: ProjectGraph, causes: Vec<TestCause>, approved: &[&str]) -> SelectionInput {
    SelectionInput {
        current,
        predecessors: HashMap::new(),
        source_digests: HashMap::new(),
        approved_unit_projects: approved.iter().map(|&x| x.into()).collect(),
        causes,
        path_comparison: PathComparison::Ordinal,
    }
}
fn explanation<'a>(result: &'a TestSelection, id: &str) -> &'a TestTargetExplanation {
    result
        .projection
        .targets
        .iter()
        .find(|t| t.target_id == id)
        .expect("target explanation")
}
#[test]
fn linked_source_uses_distinct_target_contexts_and_transitive_path() {
    let current = graph(vec![
        target("Lib/Lib.csproj|net8.0", false, &["Shared/Linked.cs"], &[]),
        target("Lib/Lib.csproj|net10.0", false, &["Shared/Linked.cs"], &[]),
        target(
            "Mid/Mid.csproj|net8.0",
            false,
            &[],
            &["Lib/Lib.csproj|net8.0"],
        ),
        target(
            "Tests/Tests.csproj|net8.0",
            true,
            &[],
            &["Mid/Mid.csproj|net8.0"],
        ),
        target(
            "Tests/Tests.csproj|net10.0",
            true,
            &["Shared/Linked.cs"],
            &["Lib/Lib.csproj|net10.0"],
        ),
    ]);
    let result = select(&input(
        current,
        vec![cause(None, Some("Shared/Linked.cs"))],
        &["Tests/Tests.csproj"],
    ))
    .unwrap();
    assert!(result.complete);
    assert_eq!(
        result
            .targets
            .iter()
            .map(|t| t.id.as_str())
            .collect::<Vec<_>>(),
        ["Tests/Tests.csproj|net10.0", "Tests/Tests.csproj|net8.0"]
    );
    let net8 = explanation(&result, "Tests/Tests.csproj|net8.0");
    assert_eq!(
        net8.reasons[0].kind,
        TestSelectionReasonKind::CurrentDependency
    );
    assert_eq!(
        net8.reasons[0].target_reference_chain,
        [
            "Lib/Lib.csproj|net8.0",
            "Mid/Mid.csproj|net8.0",
            "Tests/Tests.csproj|net8.0"
        ]
    );
    let direct = explanation(&result, "Tests/Tests.csproj|net10.0");
    assert_eq!(
        direct.reasons[0].kind,
        TestSelectionReasonKind::CurrentMembership
    );
    assert_eq!(
        direct.reasons[0].target_reference_chain,
        ["Tests/Tests.csproj|net10.0"]
    );
}

#[test]
fn rename_combines_old_and_new_graph_chains_without_duplicate_cause_binding() {
    let test = target(
        "Tests/Test.csproj|net10.0",
        true,
        &[],
        &["Lib/Lib.csproj|net10.0"],
    );
    let current = graph(vec![
        target("Lib/Lib.csproj|net10.0", false, &["New.cs"], &[]),
        test.clone(),
    ]);
    let previous = graph(vec![
        target("Lib/Lib.csproj|net10.0", false, &["Old.cs"], &[]),
        test,
    ]);
    let mut input = input(
        current,
        vec![cause(Some("Old.cs"), Some("New.cs"))],
        &["Tests/Test.csproj"],
    );
    input.predecessors.insert("graph:0".into(), previous);
    let result = select(&input).unwrap();
    assert!(result.complete);
    assert_eq!(result.targets[0].cause_ids.len(), 1);
    let reasons = &explanation(&result, "Tests/Test.csproj|net10.0").reasons;
    assert_eq!(reasons.len(), 2);
    assert!(
        reasons
            .iter()
            .any(|r| r.kind == TestSelectionReasonKind::PredecessorDependency
                && r.source_path.as_deref() == Some("Old.cs"))
    );
    assert!(
        reasons
            .iter()
            .any(|r| r.kind == TestSelectionReasonKind::CurrentDependency
                && r.source_path.as_deref() == Some("New.cs"))
    );
}

#[test]
fn deletion_uses_old_membership_and_missing_predecessor_falls_back() {
    let test = target(
        "Tests/Test.csproj|net10.0",
        true,
        &[],
        &["Lib/Lib.csproj|net10.0"],
    );
    let current = graph(vec![
        target("Lib/Lib.csproj|net10.0", false, &[], &[]),
        test.clone(),
    ]);
    let previous = graph(vec![
        target("Lib/Lib.csproj|net10.0", false, &["Deleted.cs"], &[]),
        test,
    ]);
    let mut i = input(
        current,
        vec![cause(Some("Deleted.cs"), None)],
        &["Tests/Test.csproj"],
    );
    i.predecessors.insert("graph:0".into(), previous);
    let found = select(&i).unwrap();
    assert_eq!(
        explanation(&found, "Tests/Test.csproj|net10.0").reasons[0].kind,
        TestSelectionReasonKind::PredecessorDependency
    );
    i.predecessors.clear();
    let missing = select(&i).unwrap();
    assert!(missing.complete); // Mirrors legacy C# admission; fallback/frontier remain visible.
    assert!(missing.projection.conservative_fallback);
    assert_eq!(
        explanation(&missing, "Tests/Test.csproj|net10.0").reasons[0].kind,
        TestSelectionReasonKind::MissingPredecessorFallback
    );
    assert!(
        missing
            .projection
            .unresolved_frontiers
            .iter()
            .any(|f| f.reason == "missing_predecessor")
    );
}

#[test]
fn first_revision_fallback_never_invents_reference_path() {
    let current = graph(vec![
        target("Tests/A.csproj|net10.0", true, &[], &[]),
        target("Tests/B.csproj|net10.0", true, &[], &[]),
    ]);
    let mut c = cause(None, None);
    c.fallback_first_revision = Some(1);
    let result = select(&input(
        current,
        vec![c],
        &["Tests/A.csproj", "Tests/B.csproj"],
    ))
    .unwrap();
    assert!(result.complete);
    assert_eq!(result.targets.len(), 2);
    for t in &result.projection.targets {
        assert_eq!(
            t.reasons[0].kind,
            TestSelectionReasonKind::FirstRevisionFallback
        );
        assert!(t.reasons[0].target_reference_chain.is_empty());
    }
}

#[test]
fn cycles_terminate_and_unapproved_and_unresolved_scopes_are_visible() {
    let mut current = graph(vec![
        target(
            "Lib/Lib.csproj|net10.0",
            false,
            &["Changed.cs"],
            &["Other/Other.csproj|net10.0"],
        ),
        target(
            "Other/Other.csproj|net10.0",
            false,
            &[],
            &["Lib/Lib.csproj|net10.0"],
        ),
        target(
            "Tests/A.csproj|net10.0",
            true,
            &[],
            &["Other/Other.csproj|net10.0"],
        ),
        target(
            "Tests/B.csproj|net10.0",
            true,
            &[],
            &["Lib/Lib.csproj|net10.0"],
        ),
    ]);
    current.issues.push(ProjectEvaluationIssue {
        path: "Other/Other.csproj".into(),
        reason: "unresolved_reference_context".into(),
    });
    let result = select(&input(
        current,
        vec![cause(None, Some("Changed.cs"))],
        &["Tests/A.csproj"],
    ))
    .unwrap();
    assert!(!result.complete);
    assert_eq!(result.targets.len(), 1);
    assert_eq!(
        explanation(&result, "Tests/B.csproj|net10.0").scope,
        TestSelectionTargetScope::Unapproved
    );
    assert_eq!(result.projection.unapproved_affected_targets, 1);
    assert_eq!(
        result.projection.unresolved_frontiers[0].reason,
        "unresolved_reference_context"
    );
}

#[test]
fn selection_is_limited_to_first_sixteen_and_explanations_have_exact_overflow_counts() {
    let root = target("Lib/Lib.csproj|net10.0", false, &["Changed.cs"], &[]);
    let mut targets = vec![root];
    let mut approved = Vec::new();
    for i in 0..40 {
        let t = target(
            &format!("Tests/T{i:02}.csproj|net10.0"),
            true,
            &[],
            &["Lib/Lib.csproj|net10.0"],
        );
        approved.push(t.project_path.clone());
        targets.push(t);
    }
    let result = select(&input(
        graph(targets),
        vec![cause(None, Some("Changed.cs"))],
        &approved.iter().map(String::as_str).collect::<Vec<_>>(),
    ))
    .unwrap();
    assert!(!result.complete);
    assert_eq!(result.targets.len(), 16);
    assert_eq!(result.projection.approved_affected_targets, 40);
    assert_eq!(result.projection.budget_deferred_targets, 24);
    assert_eq!(
        result
            .projection
            .targets
            .iter()
            .filter(|t| t.scope == TestSelectionTargetScope::BudgetDeferred)
            .count(),
        16
    );
    assert_eq!(result.projection.omitted_target_explanations, 8);
    assert_eq!(result.targets[0].id, "Tests/T00.csproj|net10.0");
    assert_eq!(result.targets[15].id, "Tests/T15.csproj|net10.0");
}

#[test]
fn deep_chain_and_eight_causes_retain_last_eight_targets_and_four_sorted_reasons() {
    let mut targets = vec![target("P00|net10.0", false, &["Changed.cs"], &[])];
    for i in 1..12 {
        let prev = format!("P{:02}|net10.0", i - 1);
        targets.push(target(&format!("P{i:02}|net10.0"), false, &[], &[&prev]));
    }
    targets.push(target(
        "Tests/Test.csproj|net10.0",
        true,
        &[],
        &["P11|net10.0"],
    ));
    let causes: Vec<_> = (1..=8)
        .map(|revision| TestCause {
            revision,
            ..cause(None, Some("Changed.cs"))
        })
        .collect();
    let result = select(&input(
        graph(targets),
        causes.clone(),
        &["Tests/Test.csproj"],
    ))
    .unwrap();
    let t = explanation(&result, "Tests/Test.csproj|net10.0");
    assert_eq!(t.reasons.len(), 4);
    assert_eq!(t.omitted_reasons, 4);
    assert!(
        t.reasons
            .iter()
            .all(|r| r.chain_truncated && r.target_reference_chain.len() == 8)
    );
    assert!(
        t.reasons
            .iter()
            .all(|r| r.target_reference_chain.last() == Some(&t.target_id))
    );
    let mut ids: Vec<_> = causes.iter().map(TestCause::id).collect();
    ids.sort(); // Uppercase ASCII hex matches Ordinal.
    assert_eq!(
        t.reasons
            .iter()
            .map(|r| r.cause_id.as_str())
            .collect::<Vec<_>>(),
        ids.iter().take(4).map(String::as_str).collect::<Vec<_>>()
    );
}

#[test]
fn absent_predecessor_test_target_is_a_frontier_not_a_current_selection() {
    let current = graph(vec![target("Tests/Still.csproj|net10.0", true, &[], &[])]);
    let old = graph(vec![target(
        "Tests/Gone.csproj|net10.0",
        true,
        &["Gone.cs"],
        &[],
    )]);
    let mut i = input(
        current,
        vec![cause(Some("Gone.cs"), None)],
        &["Tests/Still.csproj"],
    );
    i.predecessors.insert("graph:0".into(), old);
    let result = select(&i).unwrap();
    assert!(result.complete);
    assert!(result.targets.is_empty());
    assert!(
        result
            .projection
            .unresolved_frontiers
            .iter()
            .any(|f| f.predecessor.as_deref() == Some("graph:0")
                && f.path == "Gone.cs"
                && f.reason == "predecessor_test_target_absent_current")
    );
}

#[test]
fn frontiers_are_bounded_sorted_and_count_dropped() {
    let mut current = graph(vec![]);
    current.issues = (0..24)
        .rev()
        .map(|i| ProjectEvaluationIssue {
            path: format!("Project/{i:02}.csproj"),
            reason: "unresolved_reference_context".into(),
        })
        .collect();
    let result = select(&input(current, vec![cause(None, Some("A.cs"))], &[])).unwrap();
    assert!(!result.complete);
    assert!(result.targets.is_empty());
    assert_eq!(result.projection.unresolved_frontiers.len(), 16);
    assert_eq!(result.projection.omitted_frontiers, 8);
    assert_eq!(
        result.projection.unresolved_frontiers.first().unwrap().path,
        "Project/00.csproj"
    );
    assert_eq!(
        result.projection.unresolved_frontiers.last().unwrap().path,
        "Project/15.csproj"
    );
}

#[test]
fn repeated_discarded_frontiers_count_events_but_retained_duplicates_do_not() {
    let mut current = graph(vec![]);
    current.issues = (0..17)
        .map(|index| ProjectEvaluationIssue {
            path: format!("p{index:02}"),
            reason: "unresolved".into(),
        })
        .collect();
    current.issues.extend([
        current.issues[16].clone(),
        current.issues[16].clone(),
        current.issues[0].clone(),
    ]);
    let result = select(&input(current, vec![], &[])).unwrap();
    assert!(!result.complete);
    assert_eq!(result.projection.unresolved_frontiers.len(), 16);
    assert_eq!(result.projection.omitted_frontiers, 3);
    assert_eq!(result.projection.unresolved_frontiers[0].path, "p00");
    assert_eq!(result.projection.unresolved_frontiers[15].path, "p15");
}

#[test]
fn complete_empty_and_unchanged_fingerprints_retain_provenance_and_cause() {
    let current = graph(vec![
        target("Lib|net10.0", false, &["Owned.cs"], &[]),
        target("Tests/Tests.csproj|net10.0", true, &[], &["Lib|net10.0"]),
    ]);
    let i = input(
        current.clone(),
        vec![cause(None, Some("Unowned.cs"))],
        &["Tests/Tests.csproj"],
    );
    let empty = select(&i).unwrap();
    assert!(empty.complete);
    assert!(empty.targets.is_empty());
    let i = input(
        current,
        vec![cause(None, Some("Owned.cs"))],
        &["Tests/Tests.csproj"],
    );
    let one = select(&i).unwrap();
    let two = select(&i).unwrap();
    assert!(one.complete);
    assert_eq!(one, two);
    assert_eq!(one.targets.len(), 1);
    assert_eq!(one.targets[0].cause_ids, vec![i.causes[0].id()]);
    assert_eq!(one.targets[0].input_fingerprint.len(), 64);
    // Exact C# anonymous-record JSON + SHA256 fixture (see integration evidence).
    let mut with_digest = i;
    with_digest
        .source_digests
        .insert("Owned.cs".into(), "sha1".into());
    let fresh = select(&with_digest).unwrap();
    assert_eq!(
        fresh.targets[0].input_fingerprint,
        "74D88E41C222E4E6EAB644465F8BDC10A25367B537645D2B23B61D4D2AB75A7D"
    );
}

#[test]
fn missing_approved_test_project_means_partial_even_if_a_target_is_selected() {
    let current = graph(vec![target(
        "Tests/Present.csproj|net10.0",
        true,
        &["Changed.cs"],
        &[],
    )]);
    let result = select(&input(
        current,
        vec![cause(None, Some("Changed.cs"))],
        &["Tests/Present.csproj", "Tests/Missing.csproj"],
    ))
    .unwrap();
    assert!(!result.complete);
    assert_eq!(result.targets.len(), 1);
    assert_eq!(result.projection.unresolved_frontiers.len(), 1);
    assert_eq!(
        result.projection.unresolved_frontiers[0].reason,
        "approved_test_project_absent_current"
    );
}

#[test]
fn invalid_selected_reference_is_not_silent_empty_selection() {
    let current = graph(vec![target(
        "Tests/Tests.csproj|net10.0",
        true,
        &["Owned.cs"],
        &["Unknown|net10.0"],
    )]);
    let error = select(&input(
        current,
        vec![cause(None, Some("Owned.cs"))],
        &["Tests/Tests.csproj"],
    ))
    .unwrap_err();
    assert!(matches!(error, SelectionError::MissingReference(_)));
}

#[test]
fn windows_ascii_casefold_is_explicit_and_unicode_unsupported() {
    let current = graph(vec![target(
        "Tests/Test.csproj|net10.0",
        true,
        &["SRC/File.cs"],
        &[],
    )]);
    let mut i = input(
        current,
        vec![cause(None, Some("src/file.cs"))],
        &["tests/test.csproj"],
    );
    i.path_comparison = PathComparison::OrdinalIgnoreAsciiCase;
    assert_eq!(select(&i).unwrap().targets.len(), 1);
    i.current.targets[0].compile[0].path = "Søurce/File.cs".into();
    assert!(matches!(
        select(&i),
        Err(SelectionError::NonAsciiCaseInsensitivePath(_))
    ));
}

#[test]
fn conditional_concrete_reference_context_does_not_spill_across_frameworks() {
    let current = graph(vec![
        target("Lib/Lib.csproj|net8.0", false, &["Edited.cs"], &[]),
        target("Lib/Lib.csproj|net10.0", false, &["Other.cs"], &[]),
        target(
            "Tests/Tests.csproj|net8.0",
            true,
            &[],
            &["Lib/Lib.csproj|net8.0"],
        ),
        target(
            "Tests/Tests.csproj|net10.0",
            true,
            &[],
            &["Lib/Lib.csproj|net10.0"],
        ),
    ]);
    let result = select(&input(
        current,
        vec![cause(None, Some("Edited.cs"))],
        &["Tests/Tests.csproj"],
    ))
    .unwrap();
    assert!(result.complete);
    assert_eq!(
        result
            .targets
            .iter()
            .map(|t| t.id.as_str())
            .collect::<Vec<_>>(),
        ["Tests/Tests.csproj|net8.0"]
    );
    assert!(
        result
            .projection
            .targets
            .iter()
            .all(|t| t.target_id.ends_with("net8.0"))
    );
}

#[test]
fn provider_marked_unresolved_reference_keeps_partial_scope_and_frontier() {
    let mut current = graph(vec![
        target("Lib/Lib.csproj|net10.0", false, &["Changed.cs"], &[]),
        target(
            "Tests/Tests.csproj|net10.0",
            true,
            &[],
            &["Lib/Lib.csproj|net10.0"],
        ),
    ]);
    current.issues.push(ProjectEvaluationIssue {
        path: "Tests/Tests.csproj".into(),
        reason: "unresolved_reference_context".into(),
    });
    let result = select(&input(
        current,
        vec![cause(None, Some("Changed.cs"))],
        &["Tests/Tests.csproj"],
    ))
    .unwrap();
    assert!(!result.complete);
    assert_eq!(result.targets.len(), 1);
    assert_eq!(
        result.projection.unresolved_frontiers[0].reason,
        "unresolved_reference_context"
    );
}

#[test]
fn cause_identifier_hash_matches_csharp_sha256_format() {
    assert_eq!(
        cause(None, Some("Owned.cs")).id(),
        "862CE84BF46F402BD047797C8FF6622D9EF0E9982FD942419235C5DEC6DF85A5"
    );
}
