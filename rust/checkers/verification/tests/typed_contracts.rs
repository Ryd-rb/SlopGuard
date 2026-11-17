use slophawk_verification::typed::{
    ProviderRunDisposition, ProviderVerificationEvidence, TypedVerificationChange,
    TypedVerificationRequest, VerificationChangeKind, evaluate_typed,
};
use slophawk_verification::*;

fn scope() -> VerificationScope {
    VerificationScope {
        context_id: "lib|net10.0|Debug".into(),
        target_id: "lib/net10.0".into(),
        symbol_id: "PublicApi(string)".into(),
        behavior_id: "wire@12".into(),
        source_digest: "s2".into(),
        build_id: "b2".into(),
    }
}

fn change(kind: VerificationChangeKind) -> TypedVerificationChange {
    TypedVerificationChange {
        kind,
        change: VerificationChange {
            id: "change".into(),
            scope: scope(),
            changed_range: "src/Wire.cs:12".into(),
            reasons: vec!["public input contract".into()],
        },
    }
}

fn search(kind: VerificationChangeKind) -> VerificationSearchFact {
    VerificationSearchFact {
        id: "search".into(),
        rule_id: kind.rule_id().into(),
        detector_id: "det".into(),
        scope: scope(),
        complete: true,
        searched_targets: vec![scope().target_id],
        excluded_targets: vec![],
        frontier: vec![],
        budget_exhausted: false,
    }
}

fn fact(
    kind: VerificationChangeKind,
    evidence_kind: VerificationEvidenceKind,
) -> VerificationEvidence {
    VerificationEvidence {
        id: "e".into(),
        rule_id: kind.rule_id().into(),
        detector_id: "det".into(),
        scope: scope(),
        kind: evidence_kind,
        binding: VerificationBinding::Exact,
        valid_receipt: true,
        exact_target_mapping: true,
        counter_mapping: VerificationCounterMapping::None,
        hit_count: None,
        run_outcome: VerificationRunOutcome::Passed,
        executed_behavior: true,
        relevant_assertion: true,
    }
}

fn request(kind: VerificationChangeKind) -> TypedVerificationRequest {
    TypedVerificationRequest {
        policy: VerificationPolicy {
            version: "policy-7".into(),
            default_mode: VerificationPolicyMode::Advisory,
            rules: vec![VerificationRuleSetting {
                rule_id: kind.rule_id().into(),
                mode: VerificationPolicyMode::Required,
                required_detectors: vec!["det".into()],
            }],
        },
        changes: vec![change(kind)],
        searches: vec![search(kind)],
        evidence: vec![],
    }
}

fn only(input: &TypedVerificationRequest) -> VerificationDecision {
    let output = evaluate_typed(input, None).unwrap();
    assert_eq!(output.decisions.len(), 1);
    output.decisions[0].clone()
}

#[test]
fn public_contract_and_serialization_require_attested_behavior_and_assertion() {
    for kind in [
        VerificationChangeKind::PublicContract,
        VerificationChangeKind::Serialization,
    ] {
        let mut input = request(kind);
        input
            .evidence
            .push(fact(kind, VerificationEvidenceKind::ContractExecution));
        let decision = only(&input);
        assert_eq!(decision.rule_id, kind.rule_id());
        assert_eq!(decision.category, VerificationCategory::ContractTests);
        assert_eq!(decision.rule_version, 1);
        assert_eq!(decision.policy_version, "policy-7");
        assert_eq!(decision.state, VerificationState::Observed);
        assert_eq!(decision.strength, VerificationStrength::MeasuredExecution);
        assert_eq!(decision.run_outcomes, [VerificationRunOutcome::Passed]);
        assert_eq!(decision.evidence_ids, ["e"]);
        assert!(decision.relevant_assertion_established);
        assert_eq!(
            decision.reasons,
            ["Verified matching behavior measurement: e."]
        );
        assert!(decision.frontier.is_empty());
        assert_eq!(
            decision.next_action,
            "Retain the verified receipt for target 'lib/net10.0' and re-evaluate after its inputs change."
        );
    }
}

#[test]
fn failed_but_completed_contract_receipt_does_not_claim_success() {
    let kind = VerificationChangeKind::Serialization;
    let mut input = request(kind);
    let mut e = fact(kind, VerificationEvidenceKind::ContractExecution);
    e.run_outcome = VerificationRunOutcome::Failed;
    input.evidence.push(e);
    let d = only(&input);
    assert_eq!(d.state, VerificationState::Observed);
    assert_eq!(d.run_outcomes, [VerificationRunOutcome::Failed]);
    assert!(d.relevant_assertion_established);
    assert_eq!(
        d.next_action,
        "Investigate the failed test result for target 'lib/net10.0', then rerun and import current measurement evidence."
    );
}

#[test]
fn contract_requires_every_exact_attestation_field_and_completed_outcome() {
    let kind = VerificationChangeKind::PublicContract;
    for variant in 0..9 {
        let mut input = request(kind);
        let mut evidence = fact(kind, VerificationEvidenceKind::ContractExecution);
        match variant {
            0 => evidence.binding = VerificationBinding::Possible,
            1 => evidence.valid_receipt = false,
            2 => evidence.exact_target_mapping = false,
            3 => evidence.executed_behavior = false,
            4 => evidence.relevant_assertion = false,
            5 => evidence.run_outcome = VerificationRunOutcome::Skipped,
            6 => evidence.run_outcome = VerificationRunOutcome::Incomplete,
            7 => evidence.run_outcome = VerificationRunOutcome::NotRun,
            _ => evidence.binding = VerificationBinding::NameCandidate,
        }
        input.evidence.push(evidence);
        let d = only(&input);
        assert_eq!(
            d.state,
            if variant == 8 {
                VerificationState::Missing
            } else {
                VerificationState::Unknown
            },
            "variant {variant}"
        );
        assert!(!d.relevant_assertion_established);
        if variant != 8 {
            assert!(
                d.frontier
                    .iter()
                    .any(|s| s.contains("attested behavior/assertion"))
            );
        }
    }
}

#[test]
fn a_test_run_without_a_behavior_assertion_cannot_satisfy_a_contract() {
    let kind = VerificationChangeKind::Serialization;
    let mut input = request(kind);
    input
        .evidence
        .push(fact(kind, VerificationEvidenceKind::TestRun));
    let d = only(&input);
    assert_eq!(d.state, VerificationState::Unknown);
    assert_eq!(d.run_outcomes, [VerificationRunOutcome::Passed]);
    assert!(!d.relevant_assertion_established);
    assert_eq!(
        d.frontier,
        ["evidence 'e' does not measure the required behavior"]
    );
}

#[test]
fn complete_negative_search_is_missing_and_other_scopes_cannot_fill_it() {
    let kind = VerificationChangeKind::PublicContract;
    let mut input = request(kind);
    let mut e = fact(kind, VerificationEvidenceKind::ContractExecution);
    e.scope.target_id = "another-target".into();
    input.evidence.push(e);
    let d = only(&input);
    assert_eq!(d.state, VerificationState::Missing);
    assert_eq!(d.strength, VerificationStrength::None);
    assert_eq!(d.consulted_detectors, ["det"]);
    assert!(d.evidence_ids.is_empty());
    assert_eq!(
        d.next_action,
        "Add a ContractTests check for 'PublicApi(string)/wire@12' in target 'lib/net10.0' and repeat the detector search."
    );
}

#[test]
fn invalid_negative_search_boundaries_are_unknown_not_missing() {
    let kind = VerificationChangeKind::Serialization;
    for variant in 0..5 {
        let mut input = request(kind);
        match variant {
            0 => input.searches[0].complete = false,
            1 => input.searches[0].budget_exhausted = true,
            2 => input.searches[0].excluded_targets.push(scope().target_id),
            3 => input.searches[0].searched_targets.clear(),
            _ => input.searches[0].frontier.push("unresolved context".into()),
        }
        let d = only(&input);
        assert_eq!(d.state, VerificationState::Unknown);
        assert!(!d.frontier.is_empty());
    }
}

#[test]
fn missing_policy_detector_receipt_blocks_negative_inference() {
    let kind = VerificationChangeKind::Serialization;
    let mut input = request(kind);
    input.policy.rules[0]
        .required_detectors
        .push("second".into());
    let d = only(&input);
    assert_eq!(d.state, VerificationState::Unknown);
    assert_eq!(
        d.frontier,
        ["required detector 'second' has no current search receipt"]
    );
}

#[test]
fn stale_contract_measurement_is_stale_and_current_negative_search_supersedes_old_search() {
    let kind = VerificationChangeKind::PublicContract;
    let mut input = request(kind);
    let mut e = fact(kind, VerificationEvidenceKind::ContractExecution);
    e.scope.source_digest = "old".into();
    input.evidence.push(e);
    assert_eq!(only(&input).state, VerificationState::Stale);
    input.evidence.clear();
    let mut historical = search(kind);
    historical.id = "previous".into();
    historical.scope.build_id = "old".into();
    input.searches.push(historical);
    let d = only(&input);
    assert_eq!(d.state, VerificationState::Missing);
    assert_eq!(
        d.reasons,
        [
            "Historical search 'previous' was superseded by a current search.",
            "Every required detector completed the current applicable scope without linked evidence."
        ]
    );
}

#[test]
fn static_path_is_configured_and_preserves_exactness_strength_and_run_outcomes() {
    let kind = VerificationChangeKind::PublicContract;
    let mut input = request(kind);
    input
        .evidence
        .push(fact(kind, VerificationEvidenceKind::StaticPath));
    let d = only(&input);
    assert_eq!(d.state, VerificationState::Configured);
    assert_eq!(d.strength, VerificationStrength::ExactStaticPath);
    assert_eq!(d.run_outcomes, [VerificationRunOutcome::Passed]);
    assert!(!d.relevant_assertion_established);
    assert_eq!(
        d.next_action,
        "Run the configured ContractTests check for target 'lib/net10.0' and import a current, exactly mapped result."
    );
}

#[test]
fn wrong_rule_evidence_is_ignored_even_with_exact_matching_scope() {
    let kind = VerificationChangeKind::PublicContract;
    let mut input = request(kind);
    input.evidence.push(fact(
        VerificationChangeKind::Serialization,
        VerificationEvidenceKind::ContractExecution,
    ));
    assert_eq!(only(&input).state, VerificationState::Missing);
}

#[test]
fn branch_counter_is_not_a_serialization_assertion() {
    let kind = VerificationChangeKind::Serialization;
    let mut input = request(kind);
    let mut e = fact(kind, VerificationEvidenceKind::BranchCounter);
    e.counter_mapping = VerificationCounterMapping::ExactBranch;
    e.hit_count = Some(23);
    input.evidence.push(e);
    let d = only(&input);
    assert_eq!(d.state, VerificationState::Unknown);
    assert!(!d.relevant_assertion_established);
}

#[test]
fn disabled_contract_does_not_consult_scoped_evidence() {
    let kind = VerificationChangeKind::Serialization;
    let mut input = request(kind);
    input.policy.rules[0].mode = VerificationPolicyMode::Disabled;
    input
        .evidence
        .push(fact(kind, VerificationEvidenceKind::ContractExecution));
    let d = only(&input);
    assert_eq!(d.state, VerificationState::Unknown);
    assert_eq!(d.frontier, ["policy.disabled"]);
    assert!(d.evidence_ids.is_empty());
}

#[test]
fn mixed_change_sort_and_scope_budget_are_global_not_per_rule() {
    let mut input = request(VerificationChangeKind::PublicContract);
    let mut old = change(VerificationChangeKind::EditedBranch);
    old.change.id = "a".into();
    input.changes.push(old);
    let mut different = change(VerificationChangeKind::Serialization);
    different.change.id = "z".into();
    input.changes.push(different);
    let batch = evaluate_typed(
        &input,
        Some(&VerificationLimits {
            max_evidence: 1,
            ..VerificationLimits::default()
        }),
    )
    .unwrap();
    assert_eq!(
        batch
            .decisions
            .iter()
            .map(|d| d.change_id.as_str())
            .collect::<Vec<_>>(),
        ["a", "change", "z"]
    );
    assert!(!batch.budget_exhausted);
    input.evidence.push(fact(
        VerificationChangeKind::PublicContract,
        VerificationEvidenceKind::ContractExecution,
    ));
    input.evidence.push(fact(
        VerificationChangeKind::Serialization,
        VerificationEvidenceKind::ContractExecution,
    ));
    let limited = evaluate_typed(
        &input,
        Some(&VerificationLimits {
            max_evidence: 1,
            ..VerificationLimits::default()
        }),
    )
    .unwrap();
    assert!(limited.budget_exhausted);
    assert_eq!(limited.decisions.len(), 3);
    assert!(
        limited
            .decisions
            .iter()
            .all(|d| d.state == VerificationState::Unknown)
    );
}

#[test]
fn nested_list_budget_rejects_mixed_batch_without_partial_decisions() {
    let mut input = request(VerificationChangeKind::Serialization);
    input.changes[0].change.reasons.push("second".into());
    let result = evaluate_typed(
        &input,
        Some(&VerificationLimits {
            max_list_items: 1,
            ..VerificationLimits::default()
        }),
    )
    .unwrap();
    assert!(result.budget_exhausted);
    assert!(result.decisions.is_empty());
    assert!(
        result
            .frontier
            .iter()
            .any(|s| s.contains("nested scope-list budget"))
    );
}

#[test]
fn rejected_provider_dispositions_never_map_to_completed_attested_runs() {
    assert_eq!(
        ProviderRunDisposition::Passed.decision_outcome(),
        VerificationRunOutcome::Passed
    );
    assert_eq!(
        ProviderRunDisposition::Failed.decision_outcome(),
        VerificationRunOutcome::Failed
    );
    assert!(ProviderRunDisposition::Failed.is_completed());
    for disposition in [
        ProviderRunDisposition::Skipped,
        ProviderRunDisposition::ZeroDiscovered,
        ProviderRunDisposition::Unadmitted,
        ProviderRunDisposition::Incomplete,
    ] {
        assert!(!disposition.is_completed());
        assert!(!matches!(
            disposition.decision_outcome(),
            VerificationRunOutcome::Passed | VerificationRunOutcome::Failed
        ));
    }
    assert_ne!(
        ProviderRunDisposition::ZeroDiscovered,
        ProviderRunDisposition::Incomplete
    );
    assert_ne!(
        ProviderRunDisposition::Unadmitted,
        ProviderRunDisposition::Skipped
    );
}

#[test]
fn typed_edited_branch_is_complete_decision_parity_with_legacy_entry_point() {
    let kind = VerificationChangeKind::EditedBranch;
    let mut input = request(kind);
    let mut e = fact(kind, VerificationEvidenceKind::BranchCounter);
    e.counter_mapping = VerificationCounterMapping::ExactBranch;
    e.hit_count = Some(0);
    input.evidence.push(e);
    let legacy = VerificationRequest {
        policy: input.policy.clone(),
        changes: input.changes.iter().map(|x| x.change.clone()).collect(),
        searches: input.searches.clone(),
        evidence: input.evidence.clone(),
    };
    assert_eq!(
        evaluate_typed(&input, None).unwrap(),
        evaluate(&legacy, None).unwrap()
    );
}

#[test]
fn provider_evidence_retains_zero_discovery_and_unadmitted_provenance() {
    let kind = VerificationChangeKind::PublicContract;
    let mut input = request(kind);
    for disposition in [
        ProviderRunDisposition::ZeroDiscovered,
        ProviderRunDisposition::Unadmitted,
        ProviderRunDisposition::Skipped,
        ProviderRunDisposition::Incomplete,
    ] {
        let provider = ProviderVerificationEvidence {
            fact: fact(kind, VerificationEvidenceKind::ContractExecution),
            disposition,
        };
        input.evidence = vec![provider.for_evaluation()];
        let d = only(&input);
        assert_eq!(d.state, VerificationState::Unknown);
        assert!(!d.relevant_assertion_established);
    }
    let accepted = ProviderVerificationEvidence {
        fact: fact(kind, VerificationEvidenceKind::ContractExecution),
        disposition: ProviderRunDisposition::Failed,
    };
    input.evidence = vec![accepted.for_evaluation()];
    assert_eq!(only(&input).run_outcomes, [VerificationRunOutcome::Failed]);
}
