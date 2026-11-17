//! Typed transcriptions of the pinned `VerificationRulesTests` edited-branch
//! fixtures, plus whole-contract ordering, boundary and .NET ordinal cases.
use slophawk_verification::*;

fn scope() -> VerificationScope {
    VerificationScope {
        context_id: "src/app|net10.0|Debug".into(),
        target_id: "app/net10.0".into(),
        symbol_id: "Parse(string)".into(),
        behavior_id: "if-true@42".into(),
        source_digest: "source-v2".into(),
        build_id: "build-v2".into(),
    }
}

fn change(id: &str) -> VerificationChange {
    VerificationChange {
        id: id.into(),
        scope: scope(),
        changed_range: "src/Parse.cs:42:5-42:25".into(),
        reasons: vec!["edited validation branch".into()],
    }
}

fn setting(detector_ids: &[&str]) -> VerificationRuleSetting {
    VerificationRuleSetting {
        rule_id: EDITED_BRANCH_RULE_ID.into(),
        mode: VerificationPolicyMode::Required,
        required_detectors: detector_ids.iter().map(|s| (*s).into()).collect(),
    }
}

fn request(
    changes: Vec<VerificationChange>,
    searches: Vec<VerificationSearchFact>,
    evidence: Vec<VerificationEvidence>,
) -> VerificationRequest {
    VerificationRequest {
        policy: VerificationPolicy {
            version: "repo-policy-v7".into(),
            default_mode: VerificationPolicyMode::Advisory,
            rules: vec![setting(&["adapter"])],
        },
        changes,
        searches,
        evidence,
    }
}

fn search(id: &str) -> VerificationSearchFact {
    VerificationSearchFact {
        id: id.into(),
        rule_id: EDITED_BRANCH_RULE_ID.into(),
        detector_id: "adapter".into(),
        scope: scope(),
        complete: true,
        searched_targets: vec![scope().target_id],
        excluded_targets: vec![],
        frontier: vec![],
        budget_exhausted: false,
    }
}

fn fact(kind: VerificationEvidenceKind, id: &str) -> VerificationEvidence {
    VerificationEvidence {
        id: id.into(),
        rule_id: EDITED_BRANCH_RULE_ID.into(),
        detector_id: "adapter".into(),
        scope: scope(),
        kind,
        binding: VerificationBinding::Exact,
        valid_receipt: false,
        exact_target_mapping: false,
        counter_mapping: VerificationCounterMapping::None,
        hit_count: None,
        run_outcome: VerificationRunOutcome::NotRun,
        executed_behavior: false,
        relevant_assertion: false,
    }
}

fn counter(id: &str, hits: Option<i32>) -> VerificationEvidence {
    VerificationEvidence {
        valid_receipt: true,
        exact_target_mapping: true,
        counter_mapping: VerificationCounterMapping::ExactBranch,
        hit_count: hits,
        run_outcome: VerificationRunOutcome::Passed,
        ..fact(VerificationEvidenceKind::BranchCounter, id)
    }
}

fn batch(request: &VerificationRequest) -> VerificationBatch {
    evaluate(request, None).expect("valid default limits")
}

fn only(request: &VerificationRequest) -> VerificationDecision {
    let result = batch(request);
    assert_eq!(result.decisions.len(), 1);
    result.decisions.into_iter().next().unwrap()
}

#[test]
fn supports_only_edited_branch() {
    assert_eq!(SUPPORTED_RULE_IDS, &["sg.verification.edited-branch"]);
    assert_eq!(RULE_VERSION, 1);
}

#[test]
fn exact_positive_matches_pinned_full_decision_contract() {
    let d = only(&request(
        vec![change("change")],
        vec![search("search")],
        vec![counter("counter", Some(3))],
    ));
    assert_eq!(d.change_id, "change");
    assert_eq!(d.rule_id, EDITED_BRANCH_RULE_ID);
    assert_eq!(d.rule_version, 1);
    assert_eq!(d.category, VerificationCategory::CoverageTests);
    assert_eq!(d.policy_version, "repo-policy-v7");
    assert_eq!(d.applied_policy, VerificationPolicyMode::Required);
    assert_eq!(d.scope, scope());
    assert_eq!(d.changed_range, "src/Parse.cs:42:5-42:25");
    assert_eq!(d.trigger_reasons, ["edited validation branch"]);
    assert_eq!(d.state, VerificationState::Observed);
    assert_eq!(d.strength, VerificationStrength::MeasuredExecution);
    assert_eq!(d.consulted_detectors, ["adapter"]);
    assert_eq!(d.searched_targets, ["app/net10.0"]);
    assert!(d.excluded_targets.is_empty());
    assert_eq!(d.evidence_ids, ["counter"]);
    assert_eq!(d.run_outcomes, [VerificationRunOutcome::Passed]);
    assert!(!d.relevant_assertion_established);
    assert_eq!(
        d.reasons,
        ["Verified matching behavior measurement: counter."]
    );
    assert!(d.frontier.is_empty());
    assert_eq!(
        d.next_action,
        "Retain the verified receipt for target 'app/net10.0' and re-evaluate after its inputs change."
    );
}

#[test]
fn exact_zero_proves_uncovered_but_not_missing() {
    let d = only(&request(
        vec![change("change")],
        vec![search("search")],
        vec![counter("zero", Some(0))],
    ));
    assert_eq!(d.state, VerificationState::Uncovered);
    assert_eq!(d.strength, VerificationStrength::VerifiedMeasurement);
    assert_eq!(
        d.reasons,
        ["Exact mapped branch counters recorded zero hits: zero."]
    );
    assert_eq!(
        d.next_action,
        "Add a test executing branch 'if-true@42' in target 'app/net10.0', then collect a new exact branch counter."
    );
}

#[test]
fn line_only_partial_ambiguous_and_absent_mapping_never_measure_branch_arm() {
    for mapping in [
        VerificationCounterMapping::LineOnly,
        VerificationCounterMapping::PartialBranch,
        VerificationCounterMapping::Ambiguous,
        VerificationCounterMapping::None,
    ] {
        for hits in [Some(3), Some(0)] {
            let evidence = VerificationEvidence {
                counter_mapping: mapping,
                ..counter("mapped", hits)
            };
            let d = only(&request(
                vec![change("change")],
                vec![search("search")],
                vec![evidence],
            ));
            assert_eq!(d.state, VerificationState::Unknown);
            assert_eq!(d.strength, VerificationStrength::None);
            assert_eq!(
                d.frontier,
                ["branch counter 'mapped' lacks an exact verified, completed measurement"]
            );
        }
    }
}

#[test]
fn counters_require_complete_current_valid_attestations() {
    let invalid = vec![
        VerificationEvidence {
            valid_receipt: false,
            ..counter("invalid", Some(3))
        },
        VerificationEvidence {
            exact_target_mapping: false,
            ..counter("invalid", Some(3))
        },
        VerificationEvidence {
            binding: VerificationBinding::Possible,
            ..counter("invalid", Some(3))
        },
        VerificationEvidence {
            run_outcome: VerificationRunOutcome::Incomplete,
            ..counter("invalid", Some(3))
        },
        VerificationEvidence {
            run_outcome: VerificationRunOutcome::Skipped,
            ..counter("invalid", Some(3))
        },
        VerificationEvidence {
            run_outcome: VerificationRunOutcome::NotRun,
            ..counter("invalid", Some(3))
        },
        counter("invalid", Some(-1)),
        counter("invalid", None),
    ];
    for e in invalid {
        let d = only(&request(
            vec![change("change")],
            vec![search("search")],
            vec![e],
        ));
        assert_eq!(d.state, VerificationState::Unknown);
        assert!(!d.frontier.is_empty());
    }
}

#[test]
fn completed_failed_run_preserves_failure_even_when_coverage_was_observed() {
    let failed = VerificationEvidence {
        run_outcome: VerificationRunOutcome::Failed,
        ..counter("failed", Some(2))
    };
    let d = only(&request(
        vec![change("change")],
        vec![search("search")],
        vec![failed],
    ));
    assert_eq!(d.state, VerificationState::Observed);
    assert_eq!(d.strength, VerificationStrength::MeasuredExecution);
    assert_eq!(d.run_outcomes, [VerificationRunOutcome::Failed]);
    assert!(!d.relevant_assertion_established);
    assert_eq!(
        d.next_action,
        "Investigate the failed test result for target 'app/net10.0', then rerun and import current measurement evidence."
    );
}

#[test]
fn a_failed_test_outcome_is_visible_even_when_another_receipt_observed_coverage() {
    let other_failed = VerificationEvidence {
        run_outcome: VerificationRunOutcome::Failed,
        valid_receipt: true,
        ..fact(VerificationEvidenceKind::TestRun, "test")
    };
    let d = only(&request(
        vec![change("change")],
        vec![search("search")],
        vec![counter("counter", Some(1)), other_failed],
    ));
    assert_eq!(d.state, VerificationState::Observed);
    assert_eq!(
        d.run_outcomes,
        [
            VerificationRunOutcome::Passed,
            VerificationRunOutcome::Failed
        ]
    );
    assert!(d.next_action.contains("failed test"));
    assert_eq!(
        d.frontier,
        ["evidence 'test' does not measure the required behavior"]
    );
}

#[test]
fn static_paths_and_test_runs_establish_configuration_not_behavior_execution() {
    let path = fact(VerificationEvidenceKind::StaticPath, "path");
    let test = VerificationEvidence {
        run_outcome: VerificationRunOutcome::Passed,
        valid_receipt: true,
        ..fact(VerificationEvidenceKind::TestRun, "run")
    };
    let d = only(&request(
        vec![change("change")],
        vec![search("search")],
        vec![path, test],
    ));
    assert_eq!(d.state, VerificationState::Configured);
    assert_eq!(d.strength, VerificationStrength::ExactStaticPath);
    assert_eq!(
        d.reasons,
        ["Scoped configuration/static path exists: path; execution is not established."]
    );
    assert_eq!(d.run_outcomes, [VerificationRunOutcome::Passed]);
    assert_eq!(
        d.frontier,
        ["evidence 'run' does not measure the required behavior"]
    );
}

#[test]
fn declarations_and_possible_paths_have_separate_strengths() {
    let declared = only(&request(
        vec![change("change")],
        vec![search("search")],
        vec![fact(VerificationEvidenceKind::Configuration, "config")],
    ));
    assert_eq!(declared.state, VerificationState::Configured);
    assert_eq!(declared.strength, VerificationStrength::Declared);
    let possible = VerificationEvidence {
        binding: VerificationBinding::Possible,
        ..fact(VerificationEvidenceKind::StaticPath, "candidate")
    };
    let d = only(&request(
        vec![change("change")],
        vec![search("search")],
        vec![possible],
    ));
    assert_eq!(d.state, VerificationState::Configured);
    assert_eq!(d.strength, VerificationStrength::PossibleStaticPath);
    assert_eq!(
        d.frontier,
        ["static path 'candidate' is only a possible binding"]
    );
}

#[test]
fn name_candidates_and_other_behaviors_targets_and_rules_do_not_match() {
    let mut wrong_behavior = counter("wrong-behavior", Some(5));
    wrong_behavior.scope.behavior_id = "if-false@42".into();
    let mut wrong_target = counter("wrong-target", Some(7));
    wrong_target.scope.target_id = "other/net10.0".into();
    let name_only = VerificationEvidence {
        binding: VerificationBinding::NameCandidate,
        ..counter("name-only", Some(100))
    };
    let mut other_rule = counter("other-rule", Some(8));
    other_rule.rule_id = "sg.verification.public-contract".into();
    let d = only(&request(
        vec![change("change")],
        vec![search("search")],
        vec![wrong_behavior, wrong_target, name_only, other_rule],
    ));
    assert_eq!(d.state, VerificationState::Missing);
    assert!(d.evidence_ids.is_empty());
    assert_eq!(d.consulted_detectors, ["adapter"]);
}

#[test]
fn stale_source_or_build_evidence_is_not_current() {
    for change_revision in ["source", "build"] {
        let mut old = counter("old-counter", Some(19));
        if change_revision == "source" {
            old.scope.source_digest = "old-source".into();
        } else {
            old.scope.build_id = "old-build".into();
        }
        let d = only(&request(
            vec![change("change")],
            vec![search("search")],
            vec![old],
        ));
        assert_eq!(d.state, VerificationState::Stale);
        assert_eq!(d.strength, VerificationStrength::None);
        assert_eq!(d.evidence_ids, ["old-counter"]);
        assert!(d.frontier.iter().any(|s| s.contains("obsolete")));
        assert!(d.next_action.contains("source-v2"));
        assert!(d.next_action.contains("build-v2"));
    }
}

#[test]
fn historical_negative_search_is_completely_superseded_by_current_search() {
    let mut old = search("old");
    old.scope.source_digest = "previous".into();
    let d = only(&request(
        vec![change("change")],
        vec![old, search("current")],
        vec![],
    ));
    assert_eq!(d.state, VerificationState::Missing);
    assert_eq!(
        d.reasons,
        [
            "Historical search 'old' was superseded by a current search.",
            "Every required detector completed the current applicable scope without linked evidence.",
        ]
    );
    assert!(d.frontier.is_empty());
    assert_eq!(d.consulted_detectors, ["adapter"]);
}

#[test]
fn missing_replacement_of_stale_search_is_stale_not_missing() {
    let mut old = search("old");
    old.scope.build_id = "old".into();
    let d = only(&request(vec![change("change")], vec![old], vec![]));
    assert_eq!(d.state, VerificationState::Stale);
    assert_eq!(
        d.frontier,
        [
            "required detector 'adapter' has no current search receipt",
            "search 'old' has an obsolete source or build identity",
        ]
    );
}

#[test]
fn incomplete_current_replacement_overrides_stale_but_stays_unknown() {
    let mut old = search("old");
    old.scope.source_digest = "previous".into();
    let mut current = search("current");
    current.complete = false;
    let d = only(&request(vec![change("change")], vec![old, current], vec![]));
    assert_eq!(d.state, VerificationState::Unknown);
    assert_eq!(
        d.reasons,
        [
            "Historical search 'old' was superseded by a current search.",
            "No confirmed behavior measurement; the evidence search or mappings are unresolved.",
        ]
    );
    assert_eq!(
        d.frontier,
        ["detector 'adapter' search 'current' is incomplete or budget-exhausted"]
    );
}

#[test]
fn missing_requires_complete_negative_search_for_all_applicable_detectors() {
    let mut req = request(vec![change("change")], vec![search("a")], vec![]);
    req.policy.rules[0].required_detectors = vec!["adapter".into(), "second-adapter".into()];
    let d = only(&req);
    assert_eq!(d.state, VerificationState::Unknown);
    assert_eq!(
        d.frontier,
        ["required detector 'second-adapter' has no current search receipt"]
    );
    let mut second = search("b");
    second.detector_id = "second-adapter".into();
    req.searches.push(second);
    let d = only(&req);
    assert_eq!(d.state, VerificationState::Missing);
    assert_eq!(d.consulted_detectors, ["adapter", "second-adapter"]);
}

#[test]
fn no_detector_boundary_is_unknown_even_if_a_search_is_complete() {
    let mut req = request(vec![change("change")], vec![search("search")], vec![]);
    req.policy.rules[0].required_detectors.clear();
    let d = only(&req);
    assert_eq!(d.state, VerificationState::Unknown);
    assert_eq!(
        d.frontier,
        ["policy has no exhaustive detector boundary; missing cannot be inferred"]
    );
}

#[test]
fn excluded_target_or_incomplete_frontier_disallows_missing() {
    let mut excluded = search("search");
    excluded.searched_targets = vec!["other/net10.0".into()];
    excluded.excluded_targets = vec![scope().target_id];
    let d = only(&request(vec![change("change")], vec![excluded], vec![]));
    assert_eq!(d.state, VerificationState::Unknown);
    assert_eq!(d.searched_targets, ["other/net10.0"]);
    assert_eq!(d.excluded_targets, ["app/net10.0"]);
    assert_eq!(
        d.frontier,
        ["detector 'adapter' did not include target 'app/net10.0'"]
    );
    let mut incomplete = search("search");
    incomplete.complete = false;
    incomplete.frontier = vec!["dynamic caller unresolved".into()];
    let d = only(&request(vec![change("change")], vec![incomplete], vec![]));
    assert_eq!(d.state, VerificationState::Unknown);
    assert_eq!(
        d.frontier,
        [
            "detector 'adapter' search 'search' is incomplete or budget-exhausted",
            "detector 'adapter': dynamic caller unresolved"
        ]
    );
}

#[test]
fn a_duplicate_incomplete_search_receipt_prevents_missing() {
    let mut duplicate = search("z");
    duplicate.complete = false;
    let d = only(&request(
        vec![change("change")],
        vec![search("a"), duplicate],
        vec![],
    ));
    assert_eq!(d.state, VerificationState::Unknown);
    assert_eq!(
        d.frontier,
        ["detector 'adapter' search 'z' is incomplete or budget-exhausted"]
    );
}

#[test]
fn duplicate_required_detector_ids_deduplicate_without_changing_search_boundary() {
    let mut req = request(vec![change("change")], vec![search("a")], vec![]);
    req.policy.rules[0].required_detectors = vec!["adapter".into(), "adapter".into()];
    let d = only(&req);
    assert_eq!(d.state, VerificationState::Missing);
    assert!(d.frontier.is_empty());
}

#[test]
fn detector_budget_exhaustion_visible_in_batch_and_decision() {
    let mut exhausted = search("search");
    exhausted.budget_exhausted = true;
    let b = batch(&request(vec![change("change")], vec![exhausted], vec![]));
    assert!(b.budget_exhausted);
    assert!(b.frontier.is_empty());
    assert_eq!(b.decisions[0].state, VerificationState::Unknown);
    assert_eq!(
        b.decisions[0].frontier,
        ["detector 'adapter' search 'search' is incomplete or budget-exhausted"]
    );
}

#[test]
fn budget_exhaustion_even_from_non_applicable_search_flags_whole_batch() {
    let mut irrelevant = search("irrelevant");
    irrelevant.rule_id = "other-rule".into();
    irrelevant.budget_exhausted = true;
    let b = batch(&request(
        vec![change("change")],
        vec![search("search"), irrelevant],
        vec![],
    ));
    assert!(b.budget_exhausted);
    assert_eq!(b.decisions[0].state, VerificationState::Missing);
}

#[test]
fn exceeded_evidence_and_search_fact_caps_preserve_each_change_as_unknown() {
    let req = request(
        vec![change("z"), change("a")],
        vec![search("search")],
        vec![counter("a", Some(3)), counter("b", Some(0))],
    );
    let b = evaluate(
        &req,
        Some(&VerificationLimits {
            max_evidence: 1,
            ..VerificationLimits::default()
        }),
    )
    .unwrap();
    assert!(b.budget_exhausted);
    assert_eq!(b.frontier, ["evidence-fact budget exceeded: 2 > 1"]);
    assert_eq!(b.decisions.len(), 2);
    assert_eq!(b.decisions[0].change_id, "a");
    assert_eq!(b.decisions[1].change_id, "z");
    for d in &b.decisions {
        assert_eq!(d.state, VerificationState::Unknown);
        assert_eq!(d.strength, VerificationStrength::None);
        assert_eq!(d.reasons, ["The complete fact set was not evaluated."]);
        assert_eq!(d.frontier, b.frontier);
        assert!(d.consulted_detectors.is_empty());
        assert!(d.evidence_ids.is_empty());
    }
    let req = request(
        vec![change("change")],
        vec![search("a"), search("b")],
        vec![],
    );
    let b = evaluate(
        &req,
        Some(&VerificationLimits {
            max_searches: 1,
            ..VerificationLimits::default()
        }),
    )
    .unwrap();
    assert_eq!(b.frontier, ["search-fact budget exceeded: 2 > 1"]);
    assert_eq!(b.decisions[0].state, VerificationState::Unknown);
}

#[test]
fn change_and_policy_counts_reject_entire_request_without_sampling() {
    let req = request(vec![change("b"), change("a")], vec![], vec![]);
    let b = evaluate(
        &req,
        Some(&VerificationLimits {
            max_changes: 1,
            ..VerificationLimits::default()
        }),
    )
    .unwrap();
    assert!(b.budget_exhausted);
    assert!(b.decisions.is_empty());
    assert_eq!(b.frontier, ["change budget exceeded: 2 > 1"]);
    let mut req = request(vec![change("change")], vec![], vec![]);
    req.policy.rules.push(setting(&["adapter"]));
    let b = evaluate(
        &req,
        Some(&VerificationLimits {
            max_policy_rules: 1,
            ..VerificationLimits::default()
        }),
    )
    .unwrap();
    assert!(b.decisions.is_empty());
    assert_eq!(b.frontier, ["policy-rule budget exceeded: 2 > 1"]);
}

#[test]
fn oversized_nested_lists_reject_whole_request_even_with_valid_facts() {
    let limits = VerificationLimits {
        max_list_items: 1,
        ..VerificationLimits::default()
    };
    let mut req = request(vec![change("change")], vec![search("search")], vec![]);
    req.searches[0].searched_targets.push("extra".into());
    let b = evaluate(&req, Some(&limits)).unwrap();
    assert_eq!(
        b.frontier,
        ["nested scope-list budget exceeded: maximum 1 items per list"]
    );
    assert!(b.budget_exhausted);
    assert!(b.decisions.is_empty());
    req.searches[0].searched_targets.pop();
    req.changes[0].reasons.push("another".into());
    let b = evaluate(&req, Some(&limits)).unwrap();
    assert!(b.decisions.is_empty());
    req.changes[0].reasons.pop();
    req.policy.rules[0]
        .required_detectors
        .push("another".into());
    let b = evaluate(&req, Some(&limits)).unwrap();
    assert!(b.decisions.is_empty());
}

#[test]
fn nested_search_budget_is_not_inspected_when_search_count_already_exceeded() {
    let mut req = request(
        vec![change("change")],
        vec![search("first"), search("second")],
        vec![],
    );
    req.searches[0].frontier = vec!["one".into(), "two".into()];
    let limits = VerificationLimits {
        max_searches: 1,
        max_list_items: 1,
        ..VerificationLimits::default()
    };
    let b = evaluate(&req, Some(&limits)).unwrap();
    assert_eq!(b.frontier, ["search-fact budget exceeded: 2 > 1"]);
    assert_eq!(b.decisions[0].state, VerificationState::Unknown);
}

#[test]
fn zero_input_limit_is_rejected_like_csharp_out_of_range() {
    let req = request(vec![], vec![], vec![]);
    let result = evaluate(
        &req,
        Some(&VerificationLimits {
            max_changes: 0,
            ..VerificationLimits::default()
        }),
    );
    assert_eq!(result, Err(VerificationInputError::NonPositiveLimit));
}

#[test]
fn disabled_policy_makes_no_claim_and_does_not_consult_evidence() {
    let mut req = request(
        vec![change("change")],
        vec![search("search")],
        vec![counter("counter", Some(5))],
    );
    req.policy.rules[0].mode = VerificationPolicyMode::Disabled;
    let d = only(&req);
    assert_eq!(d.state, VerificationState::Unknown);
    assert_eq!(d.applied_policy, VerificationPolicyMode::Disabled);
    assert_eq!(d.strength, VerificationStrength::None);
    assert_eq!(d.frontier, ["policy.disabled"]);
    assert_eq!(
        d.reasons,
        ["Rule disabled by explicit policy; no evidence claim was evaluated."]
    );
    assert!(d.consulted_detectors.is_empty());
    assert!(d.evidence_ids.is_empty());
    assert!(d.run_outcomes.is_empty());
}

#[test]
fn default_policy_applies_without_matching_rule_and_first_setting_wins() {
    let mut req = request(vec![change("change")], vec![search("search")], vec![]);
    req.policy.rules.clear();
    let d = only(&req);
    assert_eq!(d.applied_policy, VerificationPolicyMode::Advisory);
    assert_eq!(d.state, VerificationState::Unknown);
    req.policy.rules = vec![setting(&["adapter"]), setting(&["other"])];
    let d = only(&req);
    assert_eq!(d.applied_policy, VerificationPolicyMode::Required);
    assert_eq!(d.state, VerificationState::Missing);
    req.policy.rules[0].mode = VerificationPolicyMode::Disabled;
    assert_eq!(only(&req).state, VerificationState::Unknown);
}

#[test]
fn reorder_input_retains_complete_batch_and_sorted_provenance() {
    let mut a = counter("z", Some(2));
    a.detector_id = "beta".into();
    let b = counter("a", Some(0));
    let mut first = search("z");
    first.detector_id = "beta".into();
    let second = search("a");
    let mut req = request(
        vec![change("z"), change("a")],
        vec![first, second],
        vec![a, b],
    );
    req.policy.rules[0].required_detectors.push("beta".into());
    let original = batch(&req);
    req.changes.reverse();
    req.searches.reverse();
    req.evidence.reverse();
    let reordered = batch(&req);
    assert_eq!(original, reordered);
    assert_eq!(
        original
            .decisions
            .iter()
            .map(|d| d.change_id.as_str())
            .collect::<Vec<_>>(),
        ["a", "z"]
    );
    for d in &original.decisions {
        assert_eq!(d.consulted_detectors, ["adapter", "beta"]);
        assert_eq!(d.evidence_ids, ["a", "z"]);
        assert_eq!(d.reasons, ["Verified matching behavior measurement: z."]);
    }
}

#[test]
fn dotnet_utf16_ordinal_is_not_rust_utf8_codepoint_order() {
    // UTF-16: U+1F600 starts with D83D, before E000. UTF-8/Rust str::cmp
    // orders U+E000 before U+1F600. This catches a silent parity regression.
    let grin = "😀";
    let bmp = "\u{E000}";
    assert!(grin > bmp); // Proves ordinary Rust string ordering differs.
    let mut grin_change = change(grin);
    grin_change.scope.context_id = grin.into();
    grin_change.reasons = vec![bmp.into(), grin.into()];
    let mut bmp_change = change(bmp);
    bmp_change.scope.context_id = bmp.into();
    let b = batch(&request(vec![bmp_change, grin_change], vec![], vec![]));
    assert_eq!(
        b.decisions
            .iter()
            .map(|d| d.change_id.as_str())
            .collect::<Vec<_>>(),
        [grin, bmp]
    );
    assert_eq!(b.decisions[0].trigger_reasons, [grin, bmp]);
    // Also detect the comparator in nested evidence IDs and detectors.
    let mut a = counter(grin, Some(1));
    a.detector_id = grin.into();
    let mut z = counter(bmp, Some(1));
    z.detector_id = bmp.into();
    let mut q = request(vec![change("change")], vec![search("search")], vec![z, a]);
    q.evidence.reverse();
    let d = only(&q);
    assert_eq!(d.evidence_ids, [grin, bmp]);
    assert_eq!(d.consulted_detectors, ["adapter", grin, bmp]);
    assert_eq!(
        d.reasons,
        [format!(
            "Verified matching behavior measurement: {grin}, {bmp}."
        )]
    );
}

#[test]
fn malformed_branch_evidence_does_not_obscure_current_exact_hit() {
    let bad = VerificationEvidence {
        counter_mapping: VerificationCounterMapping::LineOnly,
        ..counter("bad", Some(10))
    };
    let d = only(&request(
        vec![change("change")],
        vec![search("search")],
        vec![bad, counter("good", Some(1))],
    ));
    assert_eq!(d.state, VerificationState::Observed);
    assert_eq!(d.evidence_ids, ["bad", "good"]);
    assert_eq!(d.reasons, ["Verified matching behavior measurement: good."]);
    assert_eq!(
        d.frontier,
        ["branch counter 'bad' lacks an exact verified, completed measurement"]
    );
}
