//! SH-FOLLOW-05 source-only acceptance drafts. No build or test was run by the packet.
//! Small graph fixtures state the expected answer directly. The separately retained
//! expected seals stand in for owner-controlled capture records, not self-authentication.

use slophawk_test_impact::lost_routes::{Declaration, Limits, MissingFact, Outcome, Unknown};
use slophawk_test_impact::lost_routes::capture::{
    self, CapturedEvaluation, ConfiguredGroup, EvaluationKey, ExpectedCapture, OwnerRequest,
    Property, SourceFile, SourceObservation, Stamp, SupportPolicy, TargetContext, Unavailable,
    VerificationInput, graph_fingerprint, observation_fingerprint, verify_pair,
};
use slophawk_test_impact::{
    CompileMember, PathComparison, ProjectEvaluationIssue, ProjectGraph, ProjectTarget,
    SelectionInput, TestCause, TestSelectionTargetScope, select,
};
use std::collections::HashMap;

const SOURCE: &str = "src/Parser.cs";
// Published SHA-256 vector, independent of the implementation under test.
const ABC_SHA256: &str = "BA7816BF8F01CFEA414140DE5DAE2223B00361A396177A9CB410FF61F20015AD";

fn item(path: &str, link: Option<&str>) -> CompileMember {
    CompileMember { path: path.into(), link: link.map(str::to_owned) }
}

fn target(id: &str, test: bool, members: &[&str], references: &[&str]) -> ProjectTarget {
    ProjectTarget {
        id: id.into(), project_path: format!("{id}.csproj"), framework: "net10.0".into(),
        configuration: "Debug".into(), platform: "AnyCPU".into(),
        runtime_identifier: String::new(), sdk_version: "10.0.100".into(),
        is_test_project: test, compile: members.iter().map(|p| item(p, None)).collect(),
        references: references.iter().map(|r| (*r).into()).collect(),
        evaluation_fingerprint: "E".repeat(64),
    }
}

fn named<'a>(graph: &'a mut ProjectGraph, id: &str) -> &'a mut ProjectTarget {
    graph.targets.iter_mut().find(|t| t.id == id).unwrap()
}

fn stamp(side: &str) -> Stamp {
    Stamp {
        root: "fixture-root".into(), source_snapshot: format!("snapshot:{side}"),
        git_identity: (if side == "old" { "1" } else { "2" }).repeat(40),
    }
}

fn evaluation() -> EvaluationKey {
    EvaluationKey {
        sdk_version: "10.0.100".into(), provider_sha256: "A".repeat(64),
        toolchain_sha256: "B".repeat(64), environment_sha256: "C".repeat(64),
        external_inputs_sha256: "D".repeat(64),
        global_properties: vec![
            Property { name: "Configuration".into(), value: "Debug".into() },
            Property { name: "Platform".into(), value: "AnyCPU".into() },
        ],
    }
}

fn contexts(graph: &ProjectGraph) -> Vec<TargetContext> {
    graph.targets.iter().map(|t| TargetContext {
        project_path: t.project_path.clone(), framework: t.framework.clone(),
        configuration: t.configuration.clone(), platform: t.platform.clone(),
        runtime_identifier: t.runtime_identifier.clone(),
    }).collect()
}

fn expectation(graph: &ProjectGraph, sources: &SourceObservation<'_>,
    evaluation: &EvaluationKey) -> ExpectedCapture
{
    ExpectedCapture {
        stamp: sources.stamp.clone(), graph_sha256: graph_fingerprint(graph).unwrap(),
        observation_sha256: observation_fingerprint(sources).unwrap(),
        policy_sha256: "F".repeat(64),
        evaluation: evaluation.clone(), targets: contexts(graph),
    }
}

struct Fixture {
    previous: ProjectGraph,
    current: ProjectGraph,
    previous_sources: SourceObservation<'static>,
    current_sources: SourceObservation<'static>,
    previous_stamp: Stamp,
    current_stamp: Stamp,
    previous_evaluation: EvaluationKey,
    current_evaluation: EvaluationKey,
    previous_graph_sha256: String,
    current_graph_sha256: String,
    expected_previous: ExpectedCapture,
    expected_current: ExpectedCapture,
    policy: SupportPolicy,
    requests: Vec<OwnerRequest>,
    limits: Limits,
}

impl Fixture {
    fn chain() -> Self {
        let previous = ProjectGraph { targets: vec![
            target("lib", false, &[SOURCE], &[]),
            target("mid", false, &[], &["lib"]),
            target("tests", true, &[], &["mid"]),
        ], issues: vec![] };
        let current = previous.clone();
        let previous_stamp = stamp("old");
        let current_stamp = stamp("new");
        let previous_sources = SourceObservation { stamp: previous_stamp.clone(),
            files: vec![SourceFile { path: SOURCE.into(), bytes: b"abc" }] };
        let current_sources = SourceObservation { stamp: current_stamp.clone(),
            files: vec![SourceFile { path: SOURCE.into(), bytes: b"abc" }] };
        let previous_evaluation = evaluation();
        let current_evaluation = evaluation();
        let expected_previous = expectation(&previous, &previous_sources, &previous_evaluation);
        let expected_current = expectation(&current, &current_sources, &current_evaluation);
        Self {
            previous, current, previous_sources, current_sources, previous_stamp, current_stamp,
            previous_evaluation, current_evaluation,
            previous_graph_sha256: expected_previous.graph_sha256.clone(),
            current_graph_sha256: expected_current.graph_sha256.clone(),
            expected_previous, expected_current,
            policy: SupportPolicy {
                policy_sha256: "F".repeat(64), immutable_inputs: true, evaluation_only: true,
                paths: PathComparison::Ordinal, configured_group: ConfiguredGroup::AllEvaluatedTests,
            },
            requests: vec![OwnerRequest {
                key: "parser".into(), previous_target: "lib".into(),
                previous_item: item(SOURCE, None), current_item: item(SOURCE, None),
                rename: None, retirement: None,
            }],
            limits: Limits::default(),
        }
    }

    // Explicitly model a new owner-qualified capture; never call this in tamper cases.
    fn seal_current(&mut self) {
        self.expected_current = expectation(&self.current, &self.current_sources,
            &self.current_evaluation);
        self.current_graph_sha256 = self.expected_current.graph_sha256.clone();
    }

    fn seal_previous(&mut self) {
        self.expected_previous = expectation(&self.previous, &self.previous_sources,
            &self.previous_evaluation);
        self.previous_graph_sha256 = self.expected_previous.graph_sha256.clone();
    }

    fn input(&self) -> VerificationInput<'_> {
        VerificationInput {
            previous: CapturedEvaluation {
                version: capture::VERSION, graph: &self.previous,
                graph_sha256: self.previous_graph_sha256.clone(), before: self.previous_stamp.clone(),
                after: self.previous_stamp.clone(), evaluation: self.previous_evaluation.clone(),
                complete: true,
            },
            current: CapturedEvaluation {
                version: capture::VERSION, graph: &self.current,
                graph_sha256: self.current_graph_sha256.clone(), before: self.current_stamp.clone(),
                after: self.current_stamp.clone(), evaluation: self.current_evaluation.clone(),
                complete: true,
            },
            expected_previous: Some(&self.expected_previous),
            expected_current: Some(&self.expected_current), policy: Some(&self.policy),
            previous_sources: Some(&self.previous_sources), current_sources: Some(&self.current_sources),
            requests: &self.requests, limits: self.limits,
        }
    }
}

fn assert_unavailable(input: &VerificationInput<'_>, expected: Unavailable) {
    match verify_pair(input) {
        Err(actual) => assert_eq!(actual, expected),
        Ok(_) => panic!("expected unavailable evidence: {expected:?}"),
    }
}

fn assert_outcome(fixture: &Fixture, expected: Outcome) {
    let input = fixture.input();
    let pair = verify_pair(&input).unwrap();
    let report = pair.check(&fixture.current_stamp).unwrap();
    assert_eq!(report.problem, None);
    assert_eq!(report.results.len(), 1);
    assert_eq!(report.results[0].outcome, expected);
    assert_eq!(report.complete, !matches!(expected, Outcome::Unknown(_)));
}

#[test]
fn newly_sealed_reference_loss_retains_exact_graph_attribution() {
    let mut f = Fixture::chain();
    named(&mut f.current, "mid").references.clear();
    f.seal_current();
    let input = f.input();
    let pair = verify_pair(&input).unwrap();
    let report = pair.check(&f.current_stamp).unwrap();
    assert!(report.complete);
    assert_eq!(report.results[0].outcome, Outcome::LastConfiguredRouteLost);
    assert_eq!(report.results[0].previous_route.as_ref().unwrap().targets, ["lib", "mid", "tests"]);
    assert_eq!(report.results[0].first_missing, Some(MissingFact::Reference {
        previous_referencing: "mid".into(), previous_referenced: "lib".into(),
        current_referencing: "mid".into(), current_referenced: "lib".into(),
    }));
}

#[test]
fn conditional_compile_removal_uses_independent_surviving_source_bytes() {
    let mut f = Fixture::chain();
    named(&mut f.current, "lib").compile.clear();
    f.seal_current();
    let input = f.input();
    let pair = verify_pair(&input).unwrap();
    let source = pair.requests()[0].surviving_source.as_ref().unwrap();
    assert_eq!(source.path, SOURCE);
    assert_eq!(source.sha256, ABC_SHA256);
    assert_eq!(source.source_snapshot, f.current_stamp.source_snapshot);
    assert_outcome(&f, Outcome::CompileMembershipLost);
}

#[test]
fn selected_tests_never_replace_the_complete_configured_group() {
    for deferred in [false, true] {
        let mut f = Fixture::chain();
        named(&mut f.current, "tests").references.clear();
        if deferred {
            for index in 0..16 {
                f.current.targets.push(target(&format!("a{index:02}"), true, &[], &[]));
            }
        }
        f.current.targets.push(target("zz-rescue", true, &[], &["lib"]));
        f.seal_current();
        let selection = select(&SelectionInput {
            current: f.current.clone(), predecessors: HashMap::new(),
            source_digests: HashMap::from([(SOURCE.into(), ABC_SHA256.into())]),
            approved_unit_projects: f.current.targets.iter()
                .filter(|t| t.is_test_project && (deferred || t.id != "zz-rescue"))
                .map(|t| t.project_path.clone()).collect(),
            causes: vec![TestCause {
                revision: 1, path_key: SOURCE.into(), old_path: None, old_digest: None,
                new_path: Some(SOURCE.into()), new_digest: Some(ABC_SHA256.into()),
                predecessor: None, fallback_first_revision: Some(1),
            }],
            path_comparison: PathComparison::Ordinal,
        }).unwrap();
        assert!(!selection.targets.iter().any(|t| t.id == "zz-rescue"));
        let explanation = selection.projection.targets.iter()
            .find(|t| t.target_id == "zz-rescue").unwrap();
        assert_eq!(explanation.scope, if deferred {
            TestSelectionTargetScope::BudgetDeferred
        } else { TestSelectionTargetScope::Unapproved });
        let input = f.input();
        let pair = verify_pair(&input).unwrap();
        assert_eq!(pair.previous_configured_tests(), &["tests".to_owned()]);
        assert!(pair.current_configured_tests().iter().any(|id| id == "zz-rescue"));
        assert_outcome(&f, Outcome::Preserved);
    }
}

#[test]
fn graph_bytes_and_both_seals_are_required_even_when_labels_match() {
    for change in 0..3 {
        let mut f = Fixture::chain();
        named(&mut f.current, "mid").references.clear();
        match change {
            0 => (), // Both supplied labels still describe the old graph.
            1 => f.current_graph_sha256 = graph_fingerprint(&f.current).unwrap(),
            _ => f.expected_current.graph_sha256 = graph_fingerprint(&f.current).unwrap(),
        }
        assert_unavailable(&f.input(), Unavailable::GraphMismatch);
    }
}

#[test]
fn changed_observation_bytes_cannot_reuse_the_old_expected_seal() {
    let mut f = Fixture::chain();
    f.current_sources.files[0].bytes = b"xyz";
    assert_unavailable(&f.input(), Unavailable::ObservationMismatch);
}

#[test]
fn missing_inputs_are_explicit_instead_of_an_empty_success() {
    for missing in 0..5 {
        let f = Fixture::chain();
        let mut input = f.input();
        let expected = match missing {
            0 => { input.expected_previous = None; Unavailable::MissingExpectation },
            1 => { input.expected_current = None; Unavailable::MissingExpectation },
            2 => { input.policy = None; Unavailable::MissingPolicy },
            3 => { input.previous_sources = None; Unavailable::MissingObservation },
            _ => { input.current_sources = None; Unavailable::MissingObservation },
        };
        assert_unavailable(&input, expected);
    }
}

#[test]
fn missing_compile_membership_cannot_manufacture_a_source_observation() {
    let mut f = Fixture::chain();
    named(&mut f.current, "lib").compile.clear();
    f.current_sources.files.clear();
    f.seal_current();
    assert_unavailable(&f.input(), Unavailable::MissingSource);
}

#[test]
fn physical_and_logical_renames_require_fresh_explicit_declarations() {
    for logical in [false, true] {
        for declaration in 0..3 {
            let mut f = Fixture::chain();
            let changed = if logical { item(SOURCE, Some("Logical/Renamed.cs")) }
                else { item("src/Renamed.cs", None) };
            f.requests[0].current_item = changed.clone();
            named(&mut f.current, "lib").compile = vec![changed.clone()];
            f.current_sources.files[0].path = changed.path;
            if declaration != 0 {
                f.requests[0].rename = Some(Declaration {
                    source_snapshot: if declaration == 1 { f.current_stamp.source_snapshot.clone() }
                        else { f.previous_stamp.source_snapshot.clone() },
                    provenance: "owner-reviewed-rename".into(),
                });
            }
            f.seal_current();
            if declaration == 1 { assert_outcome(&f, Outcome::Preserved); }
            else { assert_unavailable(&f.input(), Unavailable::InvalidDeclaration); }
        }
    }
}

#[test]
fn retirement_is_an_owner_decision_and_can_omit_surviving_bytes() {
    for valid in [true, false] {
        let mut f = Fixture::chain();
        named(&mut f.current, "lib").compile.clear();
        f.current_sources.files.clear();
        f.requests[0].retirement = Some(Declaration {
            source_snapshot: if valid { f.current_stamp.source_snapshot.clone() }
                else { f.previous_stamp.source_snapshot.clone() },
            provenance: "owner-reviewed-retirement".into(),
        });
        f.seal_current();
        if valid { assert_outcome(&f, Outcome::Retired); }
        else { assert_unavailable(&f.input(), Unavailable::InvalidDeclaration); }
    }
}

#[test]
fn full_expected_target_universe_is_checked_independently_of_graph_labels() {
    let mut f = Fixture::chain();
    f.expected_current.targets.pop();
    assert_unavailable(&f.input(), Unavailable::UniverseMismatch);
}

#[test]
fn pre_and_post_capture_stamps_bind_source_git_and_root() {
    for previous in [false, true] {
        for changed in 0..4 {
            let f = Fixture::chain();
            let mut input = f.input();
            let capture = if previous { &mut input.previous } else { &mut input.current };
            match changed {
                0 => capture.before.source_snapshot = "another-observation".into(),
                1 => capture.after.source_snapshot = "another-observation".into(),
                2 => capture.after.git_identity = "another-Git-state".into(),
                _ => capture.after.root = "another-root".into(),
            }
            assert_unavailable(&input, Unavailable::StaleCapture);
        }
    }
}

#[test]
fn a_verified_pair_still_requires_the_current_use_time_stamp() {
    let f = Fixture::chain();
    let input = f.input();
    let pair = verify_pair(&input).unwrap();
    for changed in 0..3 {
        let mut now = f.current_stamp.clone();
        match changed {
            0 => now.source_snapshot = "newer-source".into(),
            1 => now.git_identity = "newer-Git-state".into(),
            _ => now.root = "different-root".into(),
        }
        assert_eq!(pair.check(&now).err(), Some(Unavailable::StaleCapture));
    }
}

#[test]
fn all_evaluation_dimensions_are_bound_to_the_expected_producer_record() {
    for field in 0..7 {
        let f = Fixture::chain();
        let mut input = f.input();
        let key = &mut input.current.evaluation;
        match field {
            0 => key.sdk_version = "10.0.200".into(),
            1 => key.provider_sha256 = "0".repeat(64),
            2 => key.toolchain_sha256 = "0".repeat(64),
            3 => key.environment_sha256 = "0".repeat(64),
            4 => key.external_inputs_sha256 = "0".repeat(64),
            5 => key.global_properties[0].value = "Release".into(),
            _ => key.global_properties.push(Property {
                name: "EnableOptionalReferences".into(), value: "true".into(),
            }),
        }
        assert_unavailable(&input, Unavailable::EvaluationMismatch);
    }
}

#[test]
fn changed_global_properties_are_not_comparable_even_with_new_current_seals() {
    let mut f = Fixture::chain();
    f.current_evaluation.global_properties.push(Property {
        name: "FeatureSwitch".into(), value: "enabled".into(),
    });
    f.seal_current();
    assert_unavailable(&f.input(), Unavailable::EvaluationMismatch);
}

#[test]
fn unknown_schema_partial_graph_and_unsupported_policy_stay_unavailable() {
    for case in 0..7 {
        let mut f = Fixture::chain();
        match case {
            3 => f.policy.immutable_inputs = false,
            4 => f.policy.evaluation_only = false,
            5 => f.policy.paths = PathComparison::OrdinalIgnoreAsciiCase,
            _ => (),
        }
        let mut input = f.input();
        let expected = match case {
            0 => { input.current.version = capture::VERSION + 1; Unavailable::UnsupportedSchema },
            1 => { input.previous.complete = false; Unavailable::PartialGraph },
            2 => { input.current.complete = false; Unavailable::PartialGraph },
            3 => Unavailable::ImmutableInputsRequired,
            4 => Unavailable::ExecutionItems,
            5 => Unavailable::UnsupportedPolicy,
            _ => { input.previous.version = 0; Unavailable::UnsupportedSchema },
        };
        assert_unavailable(&input, expected);
    }
}

#[test]
fn canonical_seals_ignore_collection_order_without_erasing_link_identity() {
    let mut f = Fixture::chain();
    named(&mut f.current, "lib").compile.push(item("src/Other.cs", Some("Logical/Other.cs")));
    named(&mut f.current, "tests").references.push("lib".into());
    f.current_sources.files.push(SourceFile { path: "src/Other.cs".into(), bytes: b"xyz" });
    let graph_hash = graph_fingerprint(&f.current).unwrap();
    let source_hash = observation_fingerprint(&f.current_sources).unwrap();
    f.current.targets.reverse();
    for target in &mut f.current.targets {
        target.compile.reverse();
        target.references.reverse();
    }
    f.current_sources.files.reverse();
    assert_eq!(graph_fingerprint(&f.current).unwrap(), graph_hash);
    assert_eq!(observation_fingerprint(&f.current_sources).unwrap(), source_hash);
    named(&mut f.current, "lib").compile.iter_mut()
        .find(|member| member.path == "src/Other.cs").unwrap().link = Some("Logical/New.cs".into());
    assert_ne!(graph_fingerprint(&f.current).unwrap(), graph_hash);
}

#[test]
fn core_edge_and_result_caps_remain_observable_after_verification() {
    for (budget, expected) in [
        (2, Outcome::Unknown(Unknown::EdgeBudget)),
        (3, Outcome::LastConfiguredRouteLost),
    ] {
        let mut f = Fixture::chain();
        named(&mut f.current, "tests").references.clear();
        f.seal_current();
        f.limits.edge_visits = budget;
        assert_outcome(&f, expected);
    }
    let mut f = Fixture::chain();
    f.limits.results = 0;
    let input = f.input();
    let pair = verify_pair(&input).unwrap();
    let report = pair.check(&f.current_stamp).unwrap();
    assert!(!report.complete);
    assert_eq!(report.omitted_results, 1);
    assert!(report.results.is_empty());
}

#[test]
fn core_request_ambiguity_is_not_hidden_by_capture_verification() {
    let mut f = Fixture::chain();
    f.requests.push(f.requests[0].clone());
    assert_unavailable(&f.input(), Unavailable::Core(Unknown::InvalidRequests));
}

#[test]
fn shared_physical_bytes_do_not_hide_loss_of_one_linked_logical_item() {
    let mut f = Fixture::chain();
    let first = item(SOURCE, Some("Logical/First.cs"));
    let second = item(SOURCE, Some("Logical/Second.cs"));
    named(&mut f.previous, "lib").compile = vec![first.clone(), second.clone()];
    named(&mut f.current, "lib").compile = vec![second];
    f.requests[0].previous_item = first.clone();
    f.requests[0].current_item = first;
    f.seal_previous();
    f.seal_current();
    assert_outcome(&f, Outcome::CompileMembershipLost);
}

#[test]
fn malformed_duplicates_and_noncanonical_paths_cannot_be_sealed() {
    for malformed in 0..5 {
        let mut f = Fixture::chain();
        match malformed {
            0 => f.current.targets.push(f.current.targets[0].clone()),
            1 => named(&mut f.current, "mid").references.push("lib".into()),
            2 => named(&mut f.current, "lib").compile.push(item(SOURCE, None)),
            3 => named(&mut f.current, "lib").compile[0].path = "../outside.cs".into(),
            _ => named(&mut f.current, "lib").compile[0].link = Some("Logical\\Bad.cs".into()),
        }
        assert_eq!(graph_fingerprint(&f.current), Err(Unavailable::InvalidEvidence));
    }
    let mut f = Fixture::chain();
    f.current_sources.files.push(f.current_sources.files[0].clone());
    assert_eq!(observation_fingerprint(&f.current_sources), Err(Unavailable::InvalidEvidence));
}

#[test]
fn graph_issues_cannot_certify_absence_and_expected_subset_is_rejected() {
    let mut f = Fixture::chain();
    f.current.issues.push(ProjectEvaluationIssue {
        path: "mid.csproj".into(), reason: "unresolved_reference_target".into(),
    });
    assert_unavailable(&f.input(), Unavailable::PartialGraph);
    let mut subset = Fixture::chain();
    subset.current.targets.retain(|t| t.id != "tests");
    subset.current_graph_sha256 = graph_fingerprint(&subset.current).unwrap();
    subset.expected_current.graph_sha256 = subset.current_graph_sha256.clone();
    // Even updated labels cannot substitute for the retained owner target universe.
    assert_unavailable(&subset.input(), Unavailable::UniverseMismatch);
}

#[test]
fn hard_caps_reject_oversized_graphs_source_files_and_observations() {
    let mut f = Fixture::chain();
    for index in f.current.targets.len()..=capture::MAX_TARGETS {
        f.current.targets.push(target(&format!("extra{index}"), false, &[], &[]));
    }
    assert_eq!(graph_fingerprint(&f.current), Err(Unavailable::InputLimit));
    let oversized = vec![0; capture::MAX_FILE_BYTES + 1];
    let observation = SourceObservation { stamp: stamp("new"), files: vec![
        SourceFile { path: SOURCE.into(), bytes: &oversized },
    ] };
    assert_eq!(observation_fingerprint(&observation), Err(Unavailable::InputLimit));
    let large = vec![0; capture::MAX_FILE_BYTES];
    let observation = SourceObservation { stamp: stamp("new"), files: (0..5).map(|i|
        SourceFile { path: format!("src/Large{i}.cs"), bytes: large.as_slice() }).collect() };
    assert_eq!(observation_fingerprint(&observation), Err(Unavailable::InputLimit));
}

#[test]
fn independent_observation_stamp_and_evaluated_sdk_must_match_the_capture() {
    let mut f = Fixture::chain();
    f.current_sources.stamp.git_identity = "stale-observation-Git-state".into();
    assert_unavailable(&f.input(), Unavailable::StaleCapture);
    let mut f = Fixture::chain();
    named(&mut f.current, "lib").sdk_version = "10.0.200".into();
    assert_unavailable(&f.input(), Unavailable::EvaluationMismatch);
}

#[test]
fn malformed_seals_properties_and_conflicting_declarations_are_rejected() {
    for malformed in 0..4 {
        let mut f = Fixture::chain();
        let expected = match malformed {
            0 => { f.current_graph_sha256 = "a".repeat(64); Unavailable::InvalidEvidence },
            1 => {
                f.current_evaluation.global_properties.push(Property {
                    name: "configuration".into(), value: "Debug".into(),
                });
                Unavailable::InvalidEvidence
            },
            2 => { f.policy.policy_sha256.clear(); Unavailable::InvalidEvidence },
            _ => {
                let declaration = Declaration {
                    source_snapshot: f.current_stamp.source_snapshot.clone(),
                    provenance: "conflicting-owner-plan".into(),
                };
                f.requests[0].rename = Some(declaration.clone());
                f.requests[0].retirement = Some(declaration);
                Unavailable::InvalidDeclaration
            },
        };
        assert_unavailable(&f.input(), expected);
    }
    let mut f = Fixture::chain();
    f.current_evaluation.global_properties.reverse();
    assert_outcome(&f, Outcome::Preserved);
}

#[test]
fn multiple_contexts_per_project_remain_outside_this_versioned_support_policy() {
    let mut f = Fixture::chain();
    let mut second = f.current.targets[0].clone();
    second.id = "lib-other-framework".into();
    second.framework = "net8.0".into();
    f.current.targets.push(second);
    assert_eq!(graph_fingerprint(&f.current), Err(Unavailable::UniverseMismatch));
}

#[test]
fn a_new_policy_cannot_reinterpret_previously_sealed_evidence() {
    let mut f = Fixture::chain();
    f.policy.policy_sha256 = "A".repeat(64);
    assert_unavailable(&f.input(), Unavailable::UnsupportedPolicy);
}

#[test]
fn a_declared_complete_new_universe_does_not_invent_a_target_successor() {
    for retired in [false, true] {
        let mut f = Fixture::chain();
        f.current.targets.retain(|target| target.id != "lib");
        named(&mut f.current, "mid").references.clear();
        if retired {
            f.current_sources.files.clear();
            f.requests[0].retirement = Some(Declaration {
                source_snapshot: f.current_stamp.source_snapshot.clone(),
                provenance: "owner-retired-source-context".into(),
            });
        }
        // The owner may attest a different complete roster. Its missing source
        // owner is still unknown unless explicitly retired, never a guessed loss.
        f.seal_current();
        assert_outcome(&f, if retired { Outcome::Retired }
            else { Outcome::Unknown(Unknown::TargetNotComparable) });
    }
}
