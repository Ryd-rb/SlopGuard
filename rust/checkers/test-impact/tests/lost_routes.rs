//! Source-only regression drafts for SG-CHECK-R01. Deliberately unrun by packet.
//! Expected results come from the small graph descriptions, not another selector.

use slophawk_test_impact::lost_routes::{
    self, Capture, Declaration, Identity, Input, Limits, MissingFact, Outcome, Report, Request,
    SurvivingSource, Unknown,
};
use slophawk_test_impact::{
    CompileMember, PathComparison, ProjectEvaluationIssue, ProjectGraph, ProjectTarget,
    SelectionInput, TestCause, select,
};
use std::collections::HashMap;

const SOURCE: &str = "src/Parser.cs";
const OTHER: &str = "src/Other.cs";

fn item(path: &str, link: Option<&str>) -> CompileMember {
    CompileMember { path: path.into(), link: link.map(str::to_owned) }
}

fn target(id: &str, test: bool, members: &[&str], references: &[&str]) -> ProjectTarget {
    ProjectTarget {
        id: id.into(),
        project_path: format!("{id}.csproj"),
        framework: "net10.0".into(),
        configuration: "Debug".into(),
        platform: "AnyCPU".into(),
        runtime_identifier: String::new(),
        sdk_version: "10.0.100".into(),
        is_test_project: test,
        compile: members.iter().map(|path| item(path, None)).collect(),
        references: references.iter().map(|id| (*id).into()).collect(),
        evaluation_fingerprint: format!("evaluation:{id}"),
    }
}

fn graph(targets: Vec<ProjectTarget>) -> ProjectGraph {
    ProjectGraph { targets, issues: Vec::new() }
}

fn named<'a>(graph: &'a mut ProjectGraph, id: &str) -> &'a mut ProjectTarget {
    graph.targets.iter_mut().find(|target| target.id == id).unwrap()
}

fn identity(version: &str) -> Identity {
    Identity {
        root: "physical-root:1".into(),
        source_snapshot: format!("source:{version}"),
        graph_snapshot: format!("graph:{version}"),
        evaluation_scope: "dotnet10-evaluation-v1".into(),
        configured_scope: "supported-unit-group-v1".into(),
        paths: PathComparison::Ordinal,
    }
}

fn survival(path: &str) -> SurvivingSource {
    SurvivingSource {
        source_snapshot: "source:new".into(),
        path: path.into(),
        sha256: "A".repeat(64),
    }
}

fn declaration() -> Declaration {
    Declaration {
        source_snapshot: "source:new".into(),
        provenance: "policy-record:reviewed-change".into(),
    }
}

fn request(key: &str, owner: &str, member: CompileMember) -> Request {
    Request {
        key: key.into(),
        previous_target: owner.into(),
        surviving_source: Some(survival(&member.path)),
        previous_item: member.clone(),
        current_item: member,
        rename: None,
        retirement: None,
    }
}

#[derive(Clone)]
struct Fixture {
    previous: ProjectGraph,
    current: ProjectGraph,
    previous_identity: Identity,
    current_identity: Identity,
    expected_previous: Identity,
    expected_current: Identity,
    complete: [bool; 2],
    evaluation_sufficient: [bool; 2],
    previous_configured: Vec<String>,
    current_configured: Vec<String>,
    requests: Vec<Request>,
    limits: Limits,
}

impl Fixture {
    fn new(previous: ProjectGraph, current: ProjectGraph) -> Self {
        let previous_identity = identity("old");
        let current_identity = identity("new");
        Self {
            previous,
            current,
            expected_previous: previous_identity.clone(),
            expected_current: current_identity.clone(),
            previous_identity,
            current_identity,
            complete: [true, true],
            evaluation_sufficient: [true, true],
            previous_configured: vec!["tests".into()],
            current_configured: vec!["tests".into()],
            requests: vec![request("parser", "lib", item(SOURCE, None))],
            limits: Limits::default(),
        }
    }

    fn input(&self) -> Input<'_> {
        Input {
            previous: Capture {
                identity: &self.previous_identity,
                graph: &self.previous,
                complete: self.complete[0],
                evaluation_sufficient: self.evaluation_sufficient[0],
            },
            current: Capture {
                identity: &self.current_identity,
                graph: &self.current,
                complete: self.complete[1],
                evaluation_sufficient: self.evaluation_sufficient[1],
            },
            expected_previous: &self.expected_previous,
            expected_current: &self.expected_current,
            previous_configured_tests: &self.previous_configured,
            current_configured_tests: &self.current_configured,
            requests: &self.requests,
            limits: self.limits,
        }
    }

    fn report(&self) -> Report<'_> {
        lost_routes::check(&self.input())
    }
}

fn chain() -> Fixture {
    let previous = graph(vec![
        target("lib", false, &[SOURCE], &[]),
        target("mid", false, &[], &["lib"]),
        target("tests", true, &[], &["mid"]),
    ]);
    Fixture::new(previous.clone(), previous)
}

fn diamond() -> Fixture {
    let previous = graph(vec![
        target("lib", false, &[SOURCE], &[]),
        target("a", false, &[], &["lib"]),
        target("b", false, &[], &["lib"]),
        target("tests", true, &[], &["a", "b"]),
    ]);
    Fixture::new(previous.clone(), previous)
}

fn assert_problem(fixture: &Fixture, expected: Unknown) {
    let report = fixture.report();
    assert_eq!(report.problem, Some(expected));
    assert!(!report.complete);
    assert!(report.results.is_empty());
    assert_eq!(report.omitted_results, fixture.requests.len());
}

fn assert_outcome(fixture: &Fixture, expected: Outcome) {
    let report = fixture.report();
    assert_eq!(report.problem, None);
    assert_eq!(report.results.len(), 1);
    assert_eq!(report.results[0].outcome, expected);
    assert_eq!(report.complete, !matches!(expected, Outcome::Unknown(_)));
    assert_eq!(report.omitted_results, 0);
}

#[test]
fn conditional_removal_is_specific_to_framework_and_logical_link() {
    let mut old8 = target("lib8", false, &[], &[]);
    old8.project_path = "Lib.csproj".into();
    old8.framework = "net8.0".into();
    old8.compile = vec![item(SOURCE, Some("First.cs")), item(SOURCE, Some("Second.cs"))];
    let mut old10 = old8.clone();
    old10.id = "lib10".into();
    old10.framework = "net10.0".into();
    let previous = graph(vec![
        old8, old10,
        target("tests8", true, &[], &["lib8"]),
        target("tests10", true, &[], &["lib10"]),
    ]);
    let mut f = Fixture::new(previous.clone(), previous);
    named(&mut f.current, "lib8").compile.remove(0);
    f.previous_configured = vec!["tests8".into(), "tests10".into()];
    f.current_configured = f.previous_configured.clone();
    f.requests = vec![
        request("a-net8-first", "lib8", item(SOURCE, Some("First.cs"))),
        request("b-net8-second", "lib8", item(SOURCE, Some("Second.cs"))),
        request("c-net10-first", "lib10", item(SOURCE, Some("First.cs"))),
    ];
    let report = f.report();
    assert!(report.complete);
    assert_eq!(report.results.iter().map(|r| r.outcome).collect::<Vec<_>>(), [
        Outcome::CompileMembershipLost, Outcome::Preserved, Outcome::Preserved,
    ]);
    assert_eq!(report.results[0].first_missing, Some(MissingFact::CompileItem {
        previous_target: "lib8".into(), current_target: "lib8".into(),
    }));
}

#[test]
fn transitive_loss_identifies_first_missing_reference_from_source_outward() {
    let mut f = chain();
    named(&mut f.current, "mid").references.clear();
    let report = f.report();
    assert!(report.complete);
    let found = &report.results[0];
    assert_eq!(found.outcome, Outcome::LastConfiguredRouteLost);
    assert_eq!(found.previous_route.as_ref().unwrap().targets, ["lib", "mid", "tests"]);
    assert!(found.current_route.is_none());
    assert_eq!(found.first_missing, Some(MissingFact::Reference {
        previous_referencing: "mid".into(), previous_referenced: "lib".into(),
        current_referencing: "mid".into(), current_referenced: "lib".into(),
    }));
}

#[test]
fn alternate_diamond_route_prevents_a_false_last_route_finding() {
    let mut f = diamond();
    named(&mut f.current, "a").references.clear();
    let report = f.report();
    assert!(report.complete);
    let found = &report.results[0];
    assert_eq!(found.outcome, Outcome::Preserved);
    assert_eq!(found.previous_route.as_ref().unwrap().targets, ["lib", "a", "tests"]);
    assert_eq!(found.current_route.as_ref().unwrap().targets, ["lib", "b", "tests"]);
    assert!(found.first_missing.is_none());
}

#[test]
fn newly_configured_current_endpoint_can_rescue_a_broken_old_route() {
    let mut f = chain();
    named(&mut f.current, "tests").references.clear();
    f.current.targets.push(target("new-tests", true, &[], &["lib"]));
    f.current_configured.push("new-tests".into());
    let report = f.report();
    assert!(report.complete);
    assert_eq!(report.results[0].outcome, Outcome::Preserved);
    assert_eq!(report.results[0].current_route.as_ref().unwrap().targets, ["lib", "new-tests"]);
    assert!(report.results[0].first_missing.is_none());
}

#[test]
fn missing_target_test_marker_and_configured_membership_are_distinct() {
    for kind in 0..3 {
        let mut f = chain();
        f.current_configured.clear();
        let expected = match kind {
            0 => {
                f.current.targets.retain(|target| target.id != "tests");
                MissingFact::Target { previous_target: "tests".into() }
            }
            1 => {
                named(&mut f.current, "tests").is_test_project = false;
                MissingFact::TestClassification {
                    previous_target: "tests".into(), current_target: "tests".into(),
                }
            }
            _ => MissingFact::ConfiguredMembership {
                previous_target: "tests".into(), current_target: "tests".into(),
            },
        };
        let report = f.report();
        assert!(report.complete);
        assert_eq!(report.results[0].outcome, Outcome::LastConfiguredRouteLost);
        assert_eq!(report.results[0].first_missing, Some(expected));
    }
}

#[test]
fn provider_id_changes_preserve_equal_full_contexts() {
    let mut f = chain();
    for target in &mut f.current.targets {
        target.id.push_str("-new");
        for reference in &mut target.references { reference.push_str("-new"); }
    }
    f.current_configured = vec!["tests-new".into()];
    let report = f.report();
    assert!(report.complete);
    assert_eq!(report.results[0].outcome, Outcome::Preserved);
    assert_eq!(report.results[0].current_route.as_ref().unwrap().targets,
        ["lib-new", "mid-new", "tests-new"]);
}

#[test]
fn explicit_rename_requires_the_successor_membership_and_route() {
    for keep_successor in [true, false] {
        let mut f = chain();
        let old = item("src/Old.cs", Some("OldLogical.cs"));
        let new = item("src/New.cs", Some("NewLogical.cs"));
        named(&mut f.previous, "lib").compile = vec![old.clone()];
        named(&mut f.current, "lib").compile = if keep_successor { vec![new.clone()] } else { vec![] };
        f.requests[0].previous_item = old;
        f.requests[0].current_item = new;
        f.requests[0].surviving_source = Some(survival("src/New.cs"));
        f.requests[0].rename = Some(declaration());
        assert_outcome(&f, if keep_successor { Outcome::Preserved } else { Outcome::CompileMembershipLost });
    }
}

#[test]
fn an_unproven_path_or_logical_rename_is_unknown() {
    for (change_link, stale_declaration) in [(false, false), (true, false), (false, true)] {
        let mut f = chain();
        if change_link {
            f.requests[0].current_item.link = Some("RenamedLogical.cs".into());
        } else {
            f.requests[0].current_item.path = "src/Renamed.cs".into();
            f.requests[0].surviving_source = Some(survival("src/Renamed.cs"));
        }
        named(&mut f.current, "lib").compile = vec![f.requests[0].current_item.clone()];
        if stale_declaration {
            let mut stale = declaration();
            stale.source_snapshot = "source:old".into();
            f.requests[0].rename = Some(stale);
        }
        assert_outcome(&f, Outcome::Unknown(Unknown::RenameUnproven));
    }
}

#[test]
fn duplicate_keys_and_many_to_one_or_one_to_many_mappings_are_unknown() {
    for kind in 0..3 {
        let mut f = chain();
        named(&mut f.previous, "lib").compile.push(item(OTHER, None));
        let mut second = f.requests[0].clone();
        match kind {
            0 => second.previous_item = item(OTHER, None), // Duplicate request key.
            1 => {
                second.key = "second".into();
                second.current_item = item(OTHER, None); // Same old item, two destinations.
            }
            _ => {
                second.key = "second".into();
                second.previous_item = item(OTHER, None); // Two old items, same destination.
            }
        }
        f.requests.push(second);
        assert_problem(&f, Unknown::InvalidRequests);
    }
}

#[test]
fn declared_retirement_is_attributed_and_stale_retirement_is_unknown() {
    for stale in [false, true] {
        let mut f = chain();
        named(&mut f.current, "mid").references.clear();
        f.current.targets.retain(|target| target.id != "lib");
        f.requests[0].surviving_source = None;
        let mut retirement = declaration();
        if stale { retirement.source_snapshot = "source:old".into(); }
        f.requests[0].retirement = Some(retirement);
        assert_outcome(&f, if stale {
            Outcome::Unknown(Unknown::RetirementUnproven)
        } else { Outcome::Retired });
        let report = f.report();
        assert!(report.results[0].first_missing.is_none());
        assert_eq!(report.results[0].request.retirement, f.requests[0].retirement);
    }
}

#[test]
fn capture_binding_must_match_each_expected_identity() {
    for side in 0..3 {
        let mut f = chain();
        match side {
            0 => f.expected_previous.graph_snapshot = "wrong-previous-graph".into(),
            1 => f.expected_current.source_snapshot = "wrong-current-source".into(),
            _ => {
                f.previous_identity.root.clear();
                f.expected_previous = f.previous_identity.clone();
            }
        }
        assert_problem(&f, Unknown::CaptureIdentity);
    }
}

#[test]
fn roots_evaluation_scopes_policy_scopes_and_path_modes_must_match() {
    for field in 0..4 {
        let mut f = chain();
        match field {
            0 => f.current_identity.root = "other-root".into(),
            1 => f.current_identity.evaluation_scope = "other-global-properties".into(),
            2 => f.current_identity.configured_scope = "other-support-policy".into(),
            _ => f.current_identity.paths = PathComparison::OrdinalIgnoreAsciiCase,
        }
        f.expected_current = f.current_identity.clone();
        assert_problem(&f, Unknown::ScopeMismatch);
    }
}

#[test]
fn rebound_ids_and_sdk_changes_are_not_losses() {
    for field in 0..6 {
        let mut f = chain();
        let current = named(&mut f.current, "lib");
        match field {
            0 => current.project_path = "Other.csproj".into(),
            1 => current.framework = "net8.0".into(),
            2 => current.configuration = "Release".into(),
            3 => current.platform = "x64".into(),
            4 => current.runtime_identifier = "linux-x64".into(),
            _ => current.sdk_version = "10.0.200".into(),
        }
        assert_problem(&f, if field == 5 { Unknown::ToolchainChanged } else { Unknown::ContextChanged });
    }
}

#[test]
fn absent_or_replaced_owner_context_is_unknown_without_a_retirement() {
    for kind in 0..3 {
        let mut f = chain();
        match kind {
            0 => {
                named(&mut f.current, "mid").references.clear();
                f.current.targets.retain(|target| target.id != "lib");
            }
            1 => f.requests[0].previous_target = "unrecorded-owner".into(),
            _ => {
                let replacement = named(&mut f.current, "lib");
                replacement.id = "replacement".into();
                replacement.project_path = "Replacement.csproj".into();
                named(&mut f.current, "mid").references = vec!["replacement".into()];
            }
        }
        assert_outcome(&f, Outcome::Unknown(Unknown::TargetNotComparable));
    }
}

#[test]
fn prior_logical_membership_and_independent_source_survival_are_required() {
    for kind in 0..6 {
        let mut f = chain();
        match kind {
            0 => f.requests[0].previous_item.link = Some("NotRecorded.cs".into()),
            1 => f.requests[0].surviving_source = None,
            2 => f.requests[0].surviving_source.as_mut().unwrap().source_snapshot = "source:old".into(),
            3 => f.requests[0].surviving_source.as_mut().unwrap().path = OTHER.into(),
            4 => f.requests[0].surviving_source.as_mut().unwrap().sha256 = "a".repeat(64),
            _ => f.requests[0].surviving_source.as_mut().unwrap().sha256 = "A".repeat(63),
        }
        assert_outcome(&f, Outcome::Unknown(if kind == 0 {
            Unknown::PriorItemAbsent
        } else { Unknown::SurvivalUnproven }));
    }
}

#[test]
fn partial_captures_and_execution_created_items_never_prove_absence() {
    for side in 0..2 {
        for kind in 0..3 {
            let mut f = chain();
            named(&mut f.current, "lib").compile.clear();
            match kind {
                0 => f.complete[side] = false,
                1 => {
                    let graph = if side == 0 { &mut f.previous } else { &mut f.current };
                    graph.issues.push(ProjectEvaluationIssue {
                        path: "Lib.csproj".into(), reason: "unresolved_reference_target".into(),
                    });
                }
                _ => f.evaluation_sufficient[side] = false,
            }
            assert_problem(&f, if kind == 2 { Unknown::ExecutionItems } else { Unknown::PartialGraph });
        }
    }
}

#[test]
fn malformed_graphs_and_unqualified_configured_groups_fail_closed() {
    for side in 0..2 {
        for kind in 0..4 {
            let mut f = chain();
            let graph = if side == 0 { &mut f.previous } else { &mut f.current };
            match kind {
                0 => graph.targets.push(graph.targets[0].clone()),
                1 => named(graph, "mid").references.push("unresolved".into()),
                2 => {
                    let mut duplicate_context = graph.targets[0].clone();
                    duplicate_context.id = "other-id-same-context".into();
                    graph.targets.push(duplicate_context);
                }
                _ => named(graph, "lib").framework.clear(),
            }
            assert_problem(&f, if kind == 2 { Unknown::AmbiguousContext } else { Unknown::InvalidGraph });
        }
    }
    for side in 0..2 {
        let mut f = chain();
        if side == 0 { f.previous_configured = vec!["absent".into()]; }
        else { f.current_configured = vec!["lib".into()]; }
        assert_problem(&f, Unknown::InvalidConfiguredGroup);
    }
}

#[test]
fn ascii_windows_paths_match_but_unicode_case_is_explicitly_unsupported() {
    let mut windows = chain();
    windows.previous_identity.paths = PathComparison::OrdinalIgnoreAsciiCase;
    windows.current_identity.paths = PathComparison::OrdinalIgnoreAsciiCase;
    windows.expected_previous = windows.previous_identity.clone();
    windows.expected_current = windows.current_identity.clone();
    named(&mut windows.current, "lib").project_path = "LIB.CSPROJ".into();
    named(&mut windows.current, "lib").compile[0].path = "SRC/PARSER.CS".into();
    windows.requests[0].surviving_source = Some(survival("SRC/PARSER.CS"));
    assert_outcome(&windows, Outcome::Preserved);
    let mut unicode = windows.clone();
    named(&mut unicode.previous, "lib").compile[0].path = "src/Ä.cs".into();
    assert_problem(&unicode, Unknown::UnsupportedPathCase);

    let mut linux = chain();
    named(&mut linux.current, "lib").compile[0].path = "SRC/PARSER.CS".into();
    assert_outcome(&linux, Outcome::CompileMembershipLost);
}

#[test]
fn edge_budget_is_shared_and_exact_exhaustion_is_not_silently_complete() {
    // Old lib <- mid <- tests consumes two edges; current lib <- mid consumes one.
    for (budget, expected) in [
        (2, Outcome::Unknown(Unknown::EdgeBudget)),
        (3, Outcome::LastConfiguredRouteLost),
    ] {
        let mut f = chain();
        named(&mut f.current, "tests").references.clear();
        f.limits.edge_visits = budget;
        assert_outcome(&f, expected);
        assert_eq!(f.report().edge_visits, budget);
    }
    // Both retained two-edge witnesses require exactly four visits in total.
    for (budget, expected) in [
        (3, Outcome::Unknown(Unknown::EdgeBudget)), (4, Outcome::Preserved),
    ] {
        let mut f = chain();
        f.limits.edge_visits = budget;
        assert_outcome(&f, expected);
        assert_eq!(f.report().edge_visits, budget);
    }
}

#[test]
fn zero_edge_roots_and_a_missing_prior_route_have_distinct_results() {
    let previous = graph(vec![target("lib", true, &[SOURCE], &[])]);
    let mut root = Fixture::new(previous.clone(), previous);
    root.previous_configured = vec!["lib".into()];
    root.current_configured = root.previous_configured.clone();
    root.limits.edge_visits = 0;
    assert_outcome(&root, Outcome::Preserved);
    assert_eq!(root.report().edge_visits, 0);
    assert_eq!(root.report().results[0].current_route.as_ref().unwrap().targets, ["lib"]);

    let mut unrouted = root.clone();
    unrouted.previous_configured.clear();
    unrouted.current_configured.clear();
    assert_outcome(&unrouted, Outcome::NoPriorConfiguredRoute);
    assert!(unrouted.report().results[0].first_missing.is_none());
    assert_eq!(unrouted.report().edge_visits, 0);
}

#[test]
fn cycles_terminate_and_duplicate_edges_are_charged() {
    let previous = graph(vec![
        target("lib", false, &[SOURCE], &[]),
        target("a", false, &[], &["lib", "lib", "b"]),
        target("b", false, &[], &["a"]),
        target("tests", true, &[], &["b"]),
    ]);
    // Each search visits two lib->a entries, a->b, b->a, then b->tests.
    for (budget, expected) in [(9, Outcome::Unknown(Unknown::EdgeBudget)), (10, Outcome::Preserved)] {
        let mut f = Fixture::new(previous.clone(), previous.clone());
        f.limits.edge_visits = budget;
        assert_outcome(&f, expected);
        assert_eq!(f.report().edge_visits, budget);
    }
}

#[test]
fn finite_input_quotas_refuse_before_returning_item_decisions() {
    for limits in [
        Limits { targets: 5, ..Limits::default() },
        Limits { members: 1, ..Limits::default() },
        Limits { references: 3, ..Limits::default() },
        Limits { requests: 0, ..Limits::default() },
        Limits { input_bytes: 0, ..Limits::default() },
        Limits { string_bytes: 4, ..Limits::default() },
    ] {
        let mut f = chain();
        f.limits = limits;
        assert_problem(&f, Unknown::InputLimit);
    }
}

#[test]
fn repeated_long_ids_are_bounded_before_graph_index_allocation() {
    let mut f = chain();
    let long_id = "L".repeat(2048);
    for graph in [&mut f.previous, &mut f.current] {
        let root = named(graph, "lib");
        root.id = long_id.clone();
        root.compile.extend((0..2200).map(|i| item(&format!("src/Extra{i}.cs"), None)));
        named(graph, "mid").references = vec![long_id.clone()];
    }
    f.requests[0].previous_target = long_id;
    // Short paths keep raw input small, but 2 * 2201 * 2048 ID-copy bytes
    // already exceed 8 MiB before indexing. Every individual string fits.
    assert_problem(&f, Unknown::InputLimit);
}

#[test]
fn result_and_witness_caps_are_visible_without_losing_exact_attribution() {
    let mut capped = chain();
    named(&mut capped.previous, "lib").compile.push(item(OTHER, None));
    named(&mut capped.current, "lib").compile.push(item(OTHER, None));
    capped.requests = vec![request("z", "lib", item(OTHER, None)), request("a", "lib", item(SOURCE, None))];
    capped.limits.results = 1;
    capped.limits.witness_nodes = 0;
    let report = capped.report();
    assert!(!report.complete);
    assert_eq!(report.omitted_results, 1);
    assert_eq!(report.results[0].request.key, "a");
    assert_eq!(report.results[0].outcome, Outcome::Preserved);
    let witness = report.results[0].previous_route.as_ref().unwrap();
    assert!(witness.targets.is_empty());
    assert_eq!(witness.omitted_nodes, 3);

    let previous = graph(vec![
        target("lib", false, &[SOURCE], &[]),
        target("a", false, &[], &["lib"]), target("b", false, &[], &["a"]),
        target("c", false, &[], &["b"]), target("tests", true, &[], &["c"]),
    ]);
    let mut truncated = Fixture::new(previous.clone(), previous);
    named(&mut truncated.current, "tests").references.clear();
    truncated.limits.witness_nodes = 2;
    let report = truncated.report();
    assert!(report.complete);
    assert_eq!(report.results[0].outcome, Outcome::LastConfiguredRouteLost);
    let witness = report.results[0].previous_route.as_ref().unwrap();
    assert_eq!(witness.targets, ["lib", "a"]);
    assert_eq!(witness.omitted_nodes, 3);
    assert_eq!(report.results[0].first_missing, Some(MissingFact::Reference {
        previous_referencing: "tests".into(), previous_referenced: "c".into(),
        current_referencing: "tests".into(), current_referenced: "c".into(),
    }));

    let mut no_output = chain();
    no_output.limits.results = 0;
    let report = no_output.report();
    assert!(!report.complete);
    assert!(report.results.is_empty());
    assert_eq!(report.omitted_results, 1);
    assert_eq!(report.edge_visits, 0);
}

#[test]
fn graph_request_and_group_permutations_have_identical_reports() {
    let mut original = diamond();
    named(&mut original.current, "a").references.clear();
    named(&mut original.previous, "lib").compile.push(item(OTHER, Some("Logical.cs")));
    named(&mut original.current, "lib").compile.push(item(OTHER, Some("Logical.cs")));
    // .NET ordinal order compares the surrogate pair before U+E000.
    original.requests = vec![
        request("\u{E000}", "lib", item(SOURCE, None)),
        request("\u{10000}", "lib", item(OTHER, Some("Logical.cs"))),
    ];
    original.previous.targets.push(target("idle-tests", true, &[], &[]));
    original.current.targets.push(target("idle-tests", true, &[], &[]));
    original.previous_configured.push("idle-tests".into());
    original.current_configured.push("idle-tests".into());
    let mut permuted = original.clone();
    for graph in [&mut permuted.previous, &mut permuted.current] {
        graph.targets.reverse();
        for target in &mut graph.targets {
            target.compile.reverse();
            target.references.reverse();
        }
    }
    permuted.requests.reverse();
    permuted.previous_configured.reverse();
    permuted.current_configured.reverse();
    let expected = original.report();
    let actual = permuted.report();
    assert!(expected.complete);
    assert_eq!(expected, actual);
    assert_eq!(actual.results[0].request.key, "\u{10000}");
}

#[test]
fn configured_unapproved_and_budget_deferred_tests_do_not_change_selection() {
    for survivor in [None, Some(16), Some(17)] {
        let mut targets = vec![target("lib", false, &[SOURCE], &[])];
        let ids: Vec<_> = (0..18).map(|i| format!("tests{i:02}")).collect();
        for id in &ids { targets.push(target(id, true, &[], &["lib"])); }
        let previous = graph(targets);
        let mut f = Fixture::new(previous.clone(), previous);
        let surviving_id = survivor.map(|index| format!("tests{index:02}"));
        for target in &mut f.current.targets {
            if target.is_test_project && surviving_id.as_deref() != Some(target.id.as_str()) {
                target.references.clear();
            }
        }
        f.previous_configured = ids.clone();
        f.current_configured = ids;
        let selection = SelectionInput {
            current: f.current.clone(),
            predecessors: HashMap::from([("before".into(), f.previous.clone())]),
            source_digests: HashMap::from([(SOURCE.into(), "A".repeat(64))]),
            approved_unit_projects: (0..17).map(|i| format!("tests{i:02}.csproj")).collect(),
            causes: vec![TestCause {
                revision: 2, path_key: SOURCE.into(),
                old_path: Some(SOURCE.into()), old_digest: Some("B".repeat(64)),
                new_path: Some(SOURCE.into()), new_digest: Some("A".repeat(64)),
                predecessor: Some("before".into()), fallback_first_revision: None,
            }],
            path_comparison: PathComparison::Ordinal,
        };
        let before = select(&selection).unwrap();
        assert_eq!(before.targets.len(), 16);
        assert_eq!(before.projection.budget_deferred_targets, 1);
        assert_eq!(before.projection.unapproved_affected_targets, 1);
        assert!(!before.complete);
        let report = f.report();
        assert!(report.complete);
        assert_eq!(report.results[0].outcome, if survivor.is_some() {
            Outcome::Preserved
        } else { Outcome::LastConfiguredRouteLost });
        if let Some(id) = surviving_id {
            assert_eq!(report.results[0].current_route.as_ref().unwrap().targets, ["lib", id.as_str()]);
        }
        assert_eq!(select(&selection).unwrap(), before);
    }
}
