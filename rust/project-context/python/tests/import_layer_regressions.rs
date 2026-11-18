//! Draft regression cases for owner-side execution. Not run by SG-CHECK-R06.
//! These fixtures supply typed facts; they do not parse Python or prove bindings.
//! Hashes are well-formed fixture identities, not attestations of source bytes.

use slophawk_python_project_context::import_layers::*;

const A: &str = "pkg.domain.a";
const B: &str = "pkg.domain.b";
const C: &str = "pkg.domain.c";
const UI: &str = "pkg.ui.a";
const UI_B: &str = "pkg.ui.b";
const HELPER_A: &str = "pkg.helper_a";
const HELPER_B: &str = "pkg.helper_b";

fn snapshot(revision: &str, edges: &[(&str, &str)]) -> ImportSnapshot {
    let source_hash = (if revision == "before" { "a" } else { "b" }).repeat(64);
    let modules = [
        ("pkg", true), ("pkg.domain", true), ("pkg.ui", true),
        (A, false), (B, false), (C, false), (UI, false), (UI_B, false),
        (HELPER_A, false), (HELPER_B, false), ("pkg.helper_c", false),
    ].into_iter().map(|(name, is_package)| Module {
        name: name.into(),
        path: format!("src/{}{}", name.replace('.', "/"),
            if is_package { "/__init__.py" } else { ".py" }),
        source_sha256: source_hash.clone(),
        byte_len: 100,
        is_package,
    }).collect();
    ImportSnapshot {
        identity: SnapshotIdentity {
            revision: revision.into(), source_sha256: source_hash.clone(),
            context_sha256: "c".repeat(64),
        },
        complete: true,
        roots: vec![AdmittedRoot { package: "pkg".into(), source_root: "src".into() }],
        modules,
        imports: edges.iter().map(|(from, to)| DirectImport {
            importer: (*from).into(), source_sha256: source_hash.clone(),
            target: ImportTarget::Internal((*to).into()), use_kind: ImportUse::Runtime,
            span: Span { start: 1, end: 9 },
        }).collect(),
        unsupported: Vec::new(),
    }
}

fn policy() -> LayerPolicy {
    LayerPolicy { layers: vec![vec!["pkg.ui".into()], vec!["pkg.domain".into()]] }
}

fn expected(before: &ImportSnapshot, after: &ImportSnapshot) -> SnapshotPair {
    SnapshotPair { before: before.identity.clone(), after: after.identity.clone() }
}

fn run(before: &ImportSnapshot, after: &ImportSnapshot, limits: Limits) -> Comparison {
    compare(before, after, &expected(before, after), &policy(), limits)
}

fn complete_empty(report: Comparison) {
    assert_eq!(report.status, ComparisonStatus::Complete);
    assert!(report.findings.is_empty());
    assert!(report.frontiers.is_empty());
}

fn has_reason(report: &Comparison, reason: Reason) {
    assert_eq!(report.status, ComparisonStatus::Unknown);
    assert!(report.frontiers.iter().any(|frontier| frontier.reason == reason));
}

fn finding<'a>(report: &'a Comparison, kind: FindingKind, from: &str, to: &str) -> &'a Finding {
    report.findings.iter().find(|f| f.kind == kind && f.source == from && f.target == to)
        .expect("independently expected relationship")
}

#[test]
fn same_layer_addition_can_introduce_a_transitive_forbidden_relation() {
    let before = snapshot("before", &[(B, UI)]);
    let after = snapshot("after", &[(B, UI), (A, B)]);
    let report = run(&before, &after, Limits::default());
    assert_eq!(report.status, ComparisonStatus::Complete);
    assert_eq!(report.findings.len(), 1);
    let f = finding(&report, FindingKind::ForbiddenPath, A, UI);
    assert_eq!(f.witness.iter().map(|e| (e.from.as_str(), e.to.as_str(), e.added))
        .collect::<Vec<_>>(), vec![(A, B, true), (B, UI, false)]);
    assert_eq!(f.witness[0].path, "src/pkg/domain/a.py");
    assert_eq!(f.witness[0].source_sha256, "b".repeat(64));
    assert_eq!(f.witness[0].span, Span { start: 1, end: 9 });
}

#[test]
fn an_unassigned_helper_remains_traversable() {
    let before = snapshot("before", &[(HELPER_A, UI)]);
    let after = snapshot("after", &[(HELPER_A, UI), (A, HELPER_A)]);
    let report = run(&before, &after, Limits::default());
    assert_eq!(report.status, ComparisonStatus::Complete);
    let f = finding(&report, FindingKind::ForbiddenPath, A, UI);
    assert_eq!(f.witness.len(), 2);
    assert_eq!(f.witness[0].to, HELPER_A);
    assert!(f.witness[0].added);
    assert!(!f.witness[1].added);
}

#[test]
fn alternative_shorter_and_replacement_routes_do_not_reintroduce_old_debt() {
    let before = snapshot("before", &[(A, HELPER_A), (HELPER_A, UI)]);
    for edges in [
        vec![(A, HELPER_A), (HELPER_A, UI), (A, HELPER_B), (HELPER_B, UI)],
        vec![(A, HELPER_A), (HELPER_A, UI), (A, UI)],
        vec![(A, HELPER_B), (HELPER_B, UI)],
    ] {
        complete_empty(run(&before, &snapshot("after", &edges), Limits::default()));
    }
}

#[test]
fn downward_routes_and_removing_old_debt_are_negative_controls() {
    let before = snapshot("before", &[(UI, A)]);
    let after = snapshot("after", &[(UI, A), (UI, HELPER_A), (HELPER_A, A)]);
    complete_empty(run(&before, &after, Limits::default()));
    complete_empty(run(&snapshot("before", &[(A, UI)]),
        &snapshot("after", &[]), Limits::default()));
}

#[test]
fn pre_existing_cycle_chord_and_self_loop_are_suppressed() {
    let before = snapshot("before", &[(A, B), (B, C), (C, A)]);
    let after = snapshot("after", &[(A, B), (B, C), (C, A), (A, C), (B, B)]);
    complete_empty(run(&before, &after, Limits::default()));
}

#[test]
fn replacing_an_old_cycle_entirely_with_a_self_loop_is_not_new_debt() {
    let before = snapshot("before", &[(A, B), (B, A)]);
    let after = snapshot("after", &[(A, A)]);
    complete_empty(run(&before, &after, Limits::default()));
}

#[test]
fn expanding_an_old_cycle_reports_the_new_connections_with_closed_witnesses() {
    let before = snapshot("before", &[(A, B), (B, A)]);
    let after = snapshot("after", &[(A, B), (B, A), (A, C), (C, A)]);
    let report = run(&before, &after, Limits::default());
    assert_eq!(report.status, ComparisonStatus::Complete);
    assert_eq!(report.findings.len(), 2);
    for (from, to) in [(A, C), (C, A)] {
        let f = finding(&report, FindingKind::Cycle, from, to);
        assert_eq!(f.witness[0].from, from);
        assert_eq!(f.witness[0].to, to);
        assert!(f.witness[0].added);
        assert_eq!(f.witness.last().unwrap().to, from);
        assert!(f.witness.windows(2).all(|w| w[0].to == w[1].from));
    }
}

#[test]
fn first_self_loop_is_a_one_edge_cycle() {
    let report = run(&snapshot("before", &[]), &snapshot("after", &[(A, A)]), Limits::default());
    assert_eq!(report.status, ComparisonStatus::Complete);
    assert_eq!(report.findings.len(), 1);
    let f = finding(&report, FindingKind::Cycle, A, A);
    assert_eq!(f.witness.len(), 1);
    assert!(f.witness[0].added);
}

#[test]
fn newly_cyclic_helpers_are_reported_without_layer_membership() {
    let report = run(&snapshot("before", &[]),
        &snapshot("after", &[(HELPER_A, HELPER_B), (HELPER_B, HELPER_A)]), Limits::default());
    assert_eq!(report.status, ComparisonStatus::Complete);
    assert_eq!(report.findings.len(), 2);
    for (from, to) in [(HELPER_A, HELPER_B), (HELPER_B, HELPER_A)] {
        let f = finding(&report, FindingKind::Cycle, from, to);
        assert_eq!(f.witness.len(), 2);
        assert!(f.witness.iter().all(|edge| edge.added));
    }
}

#[test]
fn source_revisions_moved_spans_and_duplicate_occurrences_are_not_new_edges() {
    let before = snapshot("before", &[(A, UI)]);
    let mut after = snapshot("after", &[(A, UI)]);
    after.imports[0].span = Span { start: 40, end: 48 };
    let mut duplicate = after.imports[0].clone();
    duplicate.span = Span { start: 60, end: 68 };
    after.imports.push(duplicate);
    complete_empty(run(&before, &after, Limits::default()));
}

#[test]
fn equal_length_paths_and_occurrences_have_canonical_witnesses() {
    let before = snapshot("before", &[]);
    let mut after = snapshot("after", &[
        (A, HELPER_B), (HELPER_B, UI), (A, HELPER_A), (HELPER_A, UI),
    ]);
    let mut duplicate = after.imports[2].clone();
    duplicate.span = Span { start: 30, end: 38 };
    after.imports.push(duplicate);
    let report = run(&before, &after, Limits::default());
    let f = finding(&report, FindingKind::ForbiddenPath, A, UI);
    assert_eq!(f.witness.iter().map(|e| e.to.as_str()).collect::<Vec<_>>(), vec![HELPER_A, UI]);
    assert_eq!(f.witness[0].span, Span { start: 1, end: 9 });
    assert!(f.witness.iter().any(|edge| edge.added));
    let mut shuffled_before = before.clone();
    shuffled_before.modules.reverse();
    after.imports.reverse();
    after.modules.reverse();
    assert_eq!(report, run(&shuffled_before, &after, Limits::default()));
    // Repeated pure calls have the same result; no filesystem cache is exercised.
    assert_eq!(report, run(&before, &after, Limits::default()));
}

#[test]
fn package_initializer_identity_and_unique_src_layout_retain_source_evidence() {
    // An extractor must normalize absolute and relative spellings to these facts.
    let report = run(&snapshot("before", &[]),
        &snapshot("after", &[("pkg.domain", "pkg.ui")]), Limits::default());
    assert_eq!(report.status, ComparisonStatus::Complete);
    let f = finding(&report, FindingKind::ForbiddenPath, "pkg.domain", "pkg.ui");
    assert_eq!(f.witness[0].path, "src/pkg/domain/__init__.py");
    assert_eq!(f.witness[0].to, "pkg.ui");
}

#[test]
fn proven_type_only_is_omitted_optional_is_included_and_external_is_outside_scope() {
    let before = snapshot("before", &[]);
    let mut after = snapshot("after", &[(A, UI)]);
    after.imports[0].use_kind = ImportUse::ProvenTypeOnly;
    let report = run(&before, &after, Limits::default());
    assert_eq!((report.usage.excluded_type_only, report.usage.external_imports,
        report.usage.optional_imports), (1, 0, 0));
    complete_empty(report);
    after.imports[0].use_kind = ImportUse::Optional;
    let report = run(&before, &after, Limits::default());
    assert_eq!(report.status, ComparisonStatus::Complete);
    assert_eq!((report.usage.excluded_type_only, report.usage.external_imports,
        report.usage.optional_imports), (0, 0, 1));
    finding(&report, FindingKind::ForbiddenPath, A, UI);
    after.imports[0].target = ImportTarget::External;
    let report = run(&before, &after, Limits::default());
    // Target and use counts are independent: this external import is optional.
    assert_eq!((report.usage.excluded_type_only, report.usage.external_imports,
        report.usage.optional_imports), (0, 1, 1));
    complete_empty(report);
}

#[test]
fn runtime_import_replacing_a_proven_type_only_import_is_a_new_graph_edge() {
    let mut before = snapshot("before", &[(A, UI)]);
    before.imports[0].use_kind = ImportUse::ProvenTypeOnly;
    let report = run(&before, &snapshot("after", &[(A, UI)]), Limits::default());
    assert_eq!(report.status, ComparisonStatus::Complete);
    assert!(finding(&report, FindingKind::ForbiddenPath, A, UI).witness[0].added);
}

#[test]
fn partial_baseline_or_candidate_cannot_support_a_new_finding() {
    for side in [SnapshotSide::Before, SnapshotSide::After] {
        let mut before = snapshot("before", &[]);
        let mut after = snapshot("after", &[(A, UI)]);
        match side {
            SnapshotSide::Before => before.complete = false,
            SnapshotSide::After => after.complete = false,
            SnapshotSide::Both => unreachable!(),
        }
        let report = run(&before, &after, Limits::default());
        has_reason(&report, Reason::IncompleteSnapshot);
        assert!(report.findings.is_empty());
        assert!(report.frontiers.iter().any(|f| f.side == side));
    }
}

#[test]
fn unsupported_facts_are_visible_instead_of_proven_absences() {
    for kind in [UnsupportedKind::DynamicImport, UnsupportedKind::AmbiguousReexport,
        UnsupportedKind::NamespacePackage, UnsupportedKind::ShadowedRoot,
        UnsupportedKind::PathAlias, UnsupportedKind::UnsupportedSyntax,
        UnsupportedKind::UnsupportedName, UnsupportedKind::UnresolvedTarget] {
        let mut before = snapshot("before", &[]);
        before.unsupported.push(UnsupportedFact { module: Some(A.into()),
            span: Some(Span { start: 1, end: 9 }), kind });
        let report = run(&before, &snapshot("after", &[(A, UI)]), Limits::default());
        has_reason(&report, Reason::Unsupported(kind));
        assert!(report.findings.is_empty());
        assert_eq!(report.frontiers[0].side, SnapshotSide::Before);
        assert_eq!(report.frontiers[0].module.as_deref(), Some(A));
    }
}

#[test]
fn valid_side_unsupported_evidence_survives_an_invalid_peer() {
    let mut before = snapshot("before", &[]);
    before.unsupported.push(UnsupportedFact { module: Some(A.into()),
        span: Some(Span { start: 1, end: 9 }), kind: UnsupportedKind::DynamicImport });
    let mut after = snapshot("after", &[(A, UI)]);
    after.imports[0].span = Span { start: 99, end: 101 };
    let report = run(&before, &after, Limits::default());
    has_reason(&report, Reason::Unsupported(UnsupportedKind::DynamicImport));
    has_reason(&report, Reason::InvalidInput);
    assert!(report.frontiers.iter().any(|f| f.side == SnapshotSide::Before
        && f.reason == Reason::Unsupported(UnsupportedKind::DynamicImport)
        && f.module.as_deref() == Some(A) && f.span == Some(Span { start: 1, end: 9 })));
    assert!(report.frontiers.iter().any(|f| f.side == SnapshotSide::After
        && f.reason == Reason::InvalidInput));
    assert!(report.findings.is_empty());
}

#[test]
fn unresolved_from_import_and_guard_facts_are_unknown() {
    let before = snapshot("before", &[]);
    let mut after = snapshot("after", &[(A, UI)]);
    after.imports[0].target = ImportTarget::Unknown(UnsupportedKind::AmbiguousReexport);
    let report = run(&before, &after, Limits::default());
    has_reason(&report, Reason::Unsupported(UnsupportedKind::AmbiguousReexport));
    assert!(report.findings.is_empty());
    after.imports[0].target = ImportTarget::Internal(UI.into());
    after.imports[0].use_kind = ImportUse::UnresolvedGuard;
    let report = run(&before, &after, Limits::default());
    has_reason(&report, Reason::Unsupported(UnsupportedKind::UnresolvedGuard));
    assert!(report.findings.is_empty());
}

#[test]
fn independently_expected_identity_rejects_stale_facts_and_context_skew() {
    let before = snapshot("before", &[]);
    let mut after = snapshot("after", &[(A, UI)]);
    let pinned = expected(&before, &after);
    after.identity.revision = "stale-generation".into();
    let report = compare(&before, &after, &pinned, &policy(), Limits::default());
    has_reason(&report, Reason::StaleSnapshot);
    assert!(report.findings.is_empty());
    after.identity.context_sha256 = "d".repeat(64);
    let report = run(&before, &after, Limits::default());
    has_reason(&report, Reason::IncompatibleContext);
    assert!(report.findings.is_empty());
}

#[test]
fn changed_root_mapping_is_not_attributed_to_import_edits() {
    let before = snapshot("before", &[]);
    let mut after = snapshot("after", &[(A, UI)]);
    after.roots[0].source_root = "lib".into();
    for module in &mut after.modules { module.path = module.path.replacen("src/", "lib/", 1); }
    let report = run(&before, &after, Limits::default());
    has_reason(&report, Reason::RootMappingChanged);
    assert!(report.findings.is_empty());
}

#[test]
fn duplicate_roots_case_aliases_missing_parents_and_bad_spans_are_invalid() {
    let before = snapshot("before", &[]);
    for case in 0..5 {
        let mut after = snapshot("after", &[(A, UI)]);
        match case {
            0 => after.roots.push(AdmittedRoot { package: "pkg".into(), source_root: "vendor".into() }),
            1 => after.modules.push(Module { name: "pkg.domain.A".into(),
                path: "src/pkg/domain/A.py".into(), source_sha256: "b".repeat(64),
                byte_len: 100, is_package: false }),
            2 => after.modules.retain(|m| m.name != "pkg.domain"),
            3 => after.imports[0].span = Span { start: 99, end: 101 },
            4 => after.imports[0].source_sha256 = "e".repeat(64),
            _ => unreachable!(),
        }
        let report = run(&before, &after, Limits::default());
        has_reason(&report, Reason::InvalidInput);
        assert!(report.findings.is_empty());
    }
}

#[test]
fn oversized_and_malformed_names_stop_before_graph_analysis() {
    let before = snapshot("before", &[]);
    let mut after = snapshot("after", &[(A, UI)]);
    after.modules.iter_mut().find(|module| module.name == A).unwrap().name = "a.".repeat(200);
    let report = run(&before, &after, Limits::default());
    has_reason(&report, Reason::InputLimit);
    assert_eq!(report.frontiers.len(), 1);
    assert!(report.findings.is_empty());
    assert_eq!(report.usage.work, 0);

    let mut malformed = snapshot("after", &[(A, UI)]);
    malformed.modules.iter_mut().find(|module| module.name == A).unwrap().name = "pkg..a".into();
    let report = run(&before, &malformed, Limits::default());
    has_reason(&report, Reason::InvalidInput);
    has_reason(&report, Reason::Unsupported(UnsupportedKind::UnsupportedName));
    assert!(report.findings.is_empty());
    assert_eq!(report.usage.work, 0);
}

#[test]
fn case_only_module_rename_across_individually_valid_snapshots_is_unknown() {
    let mut before = snapshot("before", &[]);
    // Rename the existing fixture entry; do not leave both spellings in one snapshot.
    let renamed = before.modules.iter_mut().find(|module| module.name == A).unwrap();
    renamed.name = "pkg.domain.A".into();
    renamed.path = "src/pkg/domain/A.py".into();
    let after = snapshot("after", &[(A, UI)]);
    complete_empty(run(&before, &before, Limits::default()));
    complete_empty(run(&after, &after, Limits::default()));
    let report = run(&before, &after, Limits::default());
    has_reason(&report, Reason::Unsupported(UnsupportedKind::PathAlias));
    assert!(report.frontiers.iter().any(|f| f.side == SnapshotSide::Both
        && f.reason == Reason::Unsupported(UnsupportedKind::PathAlias)));
    assert!(report.findings.is_empty());
}

#[test]
fn overlapping_layer_prefixes_are_rejected() {
    let before = snapshot("before", &[]);
    let after = snapshot("after", &[(A, UI)]);
    let overlapping = LayerPolicy { layers: vec![vec!["pkg".into()], vec!["pkg.domain".into()]] };
    let report = compare(&before, &after, &expected(&before, &after), &overlapping, Limits::default());
    has_reason(&report, Reason::InvalidPolicy);
    assert!(report.findings.is_empty());
}

#[test]
fn module_import_and_byte_limits_count_the_combined_input() {
    let before = snapshot("before", &[(A, UI)]);
    let after = snapshot("after", &[(A, UI)]);
    for limits in [
        Limits { max_modules: before.modules.len() + after.modules.len() - 1, ..Limits::default() },
        Limits { max_imports: 1, ..Limits::default() },
        Limits { max_input_bytes: 1, ..Limits::default() },
    ] {
        let report = run(&before, &after, limits);
        has_reason(&report, Reason::InputLimit);
        assert!(report.findings.is_empty());
    }
}

#[test]
fn work_exhaustion_is_unknown_with_the_effective_budget_visible() {
    let report = run(&snapshot("before", &[]), &snapshot("after", &[(A, UI)]),
        Limits { max_work: 1, ..Limits::default() });
    has_reason(&report, Reason::Limit(LimitKind::Work));
    assert!(report.limits_accepted);
    assert_eq!(report.limits.max_work, 1);
    assert!(report.usage.work <= report.limits.max_work);
    assert!(report.findings.is_empty());
}

#[test]
fn finding_cap_retains_the_first_proved_result_and_marks_unknown() {
    let report = run(&snapshot("before", &[]), &snapshot("after", &[(A, UI), (B, UI_B)]),
        Limits { max_findings: 1, ..Limits::default() });
    has_reason(&report, Reason::Limit(LimitKind::Findings));
    assert_eq!(report.findings.len(), 1);
    finding(&report, FindingKind::ForbiddenPath, A, UI);
}

#[test]
fn witness_cap_retains_prior_proof_without_emitting_a_truncated_path() {
    let after = snapshot("after", &[(A, UI), (B, HELPER_A), (HELPER_A, UI_B)]);
    let report = run(&snapshot("before", &[]), &after,
        Limits { max_witness_edges: 1, ..Limits::default() });
    has_reason(&report, Reason::Limit(LimitKind::WitnessEdges));
    assert_eq!(report.findings.len(), 1);
    let f = finding(&report, FindingKind::ForbiddenPath, A, UI);
    assert_eq!(f.witness.len(), 1);
    assert_eq!(f.witness[0].to, UI);
}

#[test]
fn frontier_cap_reports_omissions_in_canonical_order() {
    let before = snapshot("before", &[]);
    let mut after = snapshot("after", &[]);
    for kind in [UnsupportedKind::NamespacePackage, UnsupportedKind::DynamicImport] {
        after.unsupported.push(UnsupportedFact { module: None, span: None, kind });
    }
    let report = run(&before, &after, Limits { max_frontiers: 1, ..Limits::default() });
    has_reason(&report, Reason::Unsupported(UnsupportedKind::DynamicImport));
    assert_eq!(report.frontiers.len(), 1);
    assert_eq!(report.omitted_frontiers, 1);
    after.unsupported.reverse();
    assert_eq!(report, run(&before, &after, Limits { max_frontiers: 1, ..Limits::default() }));
}

#[test]
fn zero_or_above_hard_limits_are_not_silently_clamped() {
    for limits in [
        Limits { max_work: 0, ..Limits::default() },
        Limits { max_findings: 0, ..Limits::default() },
        Limits { max_witness_edges: 129, ..Limits::default() },
        Limits { max_work: 20_000_001, ..Limits::default() },
    ] {
        let report = run(&snapshot("before", &[]), &snapshot("after", &[(A, UI)]), limits);
        has_reason(&report, Reason::InvalidLimits);
        assert!(!report.limits_accepted);
        assert!(report.findings.is_empty());
    }
}
