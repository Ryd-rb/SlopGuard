//! Draft graph regressions. These are model expectations, not runtime observations.
//! No compilation or test execution was performed while preparing this proposal.

use slophawk_verification::publication::{
    Budget, EvidenceId, ExclusionEvidence, Fact, FrontierReason, Invalidation, ModelCoverage,
    NodeId, PublicationIdentity, PublicationInputError, PublicationLimits, PublicationNode,
    PublicationRequest, PublicationState, UnsupportedFact, ValueId, analyze,
};

fn identity() -> PublicationIdentity {
    PublicationIdentity {
        revision: ValueId(1),
        epoch: ValueId(2),
        root: ValueId(3),
        digest: ValueId(4),
    }
}

fn node(id: u32, fact: Fact, successors: &[u32]) -> PublicationNode {
    PublicationNode {
        id: NodeId(id),
        fact,
        successors: successors.iter().copied().map(NodeId).collect(),
    }
}

fn request(nodes: Vec<PublicationNode>) -> PublicationRequest {
    PublicationRequest {
        identity: identity(),
        coverage: ModelCoverage { control_flow: true, identities: true, interference: true },
        entry: NodeId(0),
        nodes,
    }
}

fn assert_single(request: &PublicationRequest, expected: PublicationState, path: &[u32]) {
    let result = analyze(request, None).unwrap();
    assert!(result.complete);
    assert!(!result.budget_exhausted);
    assert!(result.frontier.is_empty());
    assert_eq!(result.identity, request.identity);
    assert_eq!(result.decisions.len(), 1);
    assert_eq!(result.decisions[0].state, expected);
    assert_eq!(result.decisions[0].witness, path.iter().copied().map(NodeId).collect::<Vec<_>>());
}

#[test]
fn same_identity_commit_time_validation_after_deferred_work() {
    let graph = request(vec![
        node(0, Fact::CaptureImmutable, &[1]),
        node(1, Fact::Invalidate(Invalidation::DeferredWork), &[2]),
        node(2, Fact::Validate(identity()), &[3]),
        node(3, Fact::Publish, &[]),
    ]);
    assert_single(&graph, PublicationState::GuardedInModel, &[]);
}

#[test]
fn every_identity_dimension_must_match() {
    let key = identity();
    for wrong in [
        PublicationIdentity { revision: ValueId(99), ..key },
        PublicationIdentity { epoch: ValueId(99), ..key },
        PublicationIdentity { root: ValueId(99), ..key },
        PublicationIdentity { digest: ValueId(99), ..key },
    ] {
        let graph = request(vec![
            node(0, Fact::Validate(wrong), &[1]),
            node(1, Fact::Publish, &[]),
        ]);
        assert_single(&graph, PublicationState::UnguardedInModel, &[0, 1]);
    }
}

#[test]
fn unrelated_validation_does_not_erase_an_existing_same_identity_guard() {
    let wrong = PublicationIdentity { root: ValueId(99), ..identity() };
    let graph = request(vec![
        node(0, Fact::Validate(identity()), &[1]),
        node(1, Fact::Validate(wrong), &[2]),
        node(2, Fact::Publish, &[]),
    ]);
    assert_single(&graph, PublicationState::GuardedInModel, &[]);
}

#[test]
fn branch_bypass_produces_an_entry_to_effect_witness() {
    let graph = request(vec![
        node(0, Fact::Noop, &[2, 1]),
        node(1, Fact::Validate(identity()), &[3]),
        node(2, Fact::Noop, &[3]),
        node(3, Fact::Publish, &[]),
    ]);
    assert_single(&graph, PublicationState::UnguardedInModel, &[0, 2, 3]);
}

#[test]
fn separate_validations_on_both_branches_satisfy_the_must_property() {
    let graph = request(vec![
        node(0, Fact::Noop, &[2, 1]),
        node(1, Fact::Validate(identity()), &[3]),
        node(2, Fact::Validate(identity()), &[3]),
        node(3, Fact::Publish, &[]),
    ]);
    // Neither validation node individually dominates node 3.
    assert_single(&graph, PublicationState::GuardedInModel, &[]);
}

#[test]
fn queue_handoff_requires_validation_after_the_handoff() {
    let graph = request(vec![
        node(0, Fact::Validate(identity()), &[1]),
        node(1, Fact::Invalidate(Invalidation::QueueHandoff), &[2]),
        node(2, Fact::Publish, &[]),
    ]);
    // Node 0 dominates publication but its authority does not survive the handoff.
    assert_single(&graph, PublicationState::UnguardedInModel, &[0, 1, 2]);
    let corrected = request(vec![
        node(0, Fact::Validate(identity()), &[1]),
        node(1, Fact::Invalidate(Invalidation::QueueHandoff), &[2]),
        node(2, Fact::Validate(identity()), &[3]),
        node(3, Fact::Publish, &[]),
    ]);
    assert_single(&corrected, PublicationState::GuardedInModel, &[]);
}

#[test]
fn immutable_capture_preserves_inputs_but_grants_no_publication_authority() {
    let only_capture = request(vec![
        node(0, Fact::CaptureImmutable, &[1]),
        node(1, Fact::Publish, &[]),
    ]);
    assert_single(&only_capture, PublicationState::UnguardedInModel, &[0, 1]);
    let after_handoff = request(vec![
        node(0, Fact::Validate(identity()), &[1]),
        node(1, Fact::Invalidate(Invalidation::QueueHandoff), &[2]),
        node(2, Fact::CaptureImmutable, &[3]),
        node(3, Fact::Publish, &[]),
    ]);
    assert_single(&after_handoff, PublicationState::UnguardedInModel, &[0, 1, 2, 3]);
    let already_guarded = request(vec![
        node(0, Fact::Validate(identity()), &[1]),
        node(1, Fact::CaptureImmutable, &[2]),
        node(2, Fact::Publish, &[]),
    ]);
    assert_single(&already_guarded, PublicationState::GuardedInModel, &[]);
}

#[test]
fn branch_switch_while_work_is_in_flight_cannot_validate_the_old_capture() {
    let switched = PublicationIdentity { digest: ValueId(99), ..identity() };
    let graph = request(vec![
        node(0, Fact::Validate(identity()), &[1]),
        node(1, Fact::Invalidate(Invalidation::DeferredWork), &[2]),
        node(2, Fact::Invalidate(Invalidation::IdentityMutation), &[3]),
        node(3, Fact::Validate(switched), &[4]),
        node(4, Fact::Publish, &[]),
    ]);
    assert_single(&graph, PublicationState::UnguardedInModel, &[0, 1, 2, 3, 4]);
}

#[test]
fn reachable_cycles_are_unknown_without_unrolling() {
    let graph = request(vec![
        node(0, Fact::Validate(identity()), &[1]),
        node(1, Fact::Noop, &[1, 2]),
        node(2, Fact::Publish, &[]),
    ]);
    let result = analyze(&graph, None).unwrap();
    assert!(!result.complete);
    assert!(!result.budget_exhausted);
    assert!(result.frontier.iter().any(|f| f.reason == FrontierReason::ReachableCycle));
    assert_eq!(result.decisions[0].state, PublicationState::Unknown);
    assert!(result.decisions[0].witness.is_empty());
}

#[test]
fn reachable_alias_and_opaque_frontiers_cannot_be_repaired_by_a_later_validation() {
    for reason in [UnsupportedFact::Alias, UnsupportedFact::OpaqueHelper, UnsupportedFact::UnresolvedIdentity] {
        let graph = request(vec![
            node(0, Fact::Unsupported(reason), &[1]),
            node(1, Fact::Validate(identity()), &[2]),
            node(2, Fact::Publish, &[]),
        ]);
        let result = analyze(&graph, None).unwrap();
        assert!(!result.complete);
        assert_eq!(result.frontier[0].node, Some(NodeId(0)));
        assert_eq!(result.frontier[0].reason, FrontierReason::Unsupported(reason));
        assert_eq!(result.decisions[0].state, PublicationState::Unknown);
    }
}

#[test]
fn incomplete_control_identity_or_interference_evidence_is_visible() {
    for (coverage, expected) in [
        (ModelCoverage { control_flow: false, identities: true, interference: true }, FrontierReason::IncompleteControlFlow),
        (ModelCoverage { control_flow: true, identities: false, interference: true }, FrontierReason::IncompleteIdentities),
        (ModelCoverage { control_flow: true, identities: true, interference: false }, FrontierReason::IncompleteInterference),
    ] {
        let mut graph = request(vec![
            node(0, Fact::Validate(identity()), &[1]),
            node(1, Fact::Publish, &[]),
        ]);
        graph.coverage = coverage;
        let result = analyze(&graph, None).unwrap();
        assert!(!result.complete);
        assert_eq!(result.frontier[0].reason, expected);
        assert_eq!(result.decisions[0].state, PublicationState::Unknown);
    }
}

#[test]
fn unreachable_predecessors_frontiers_and_cycles_cannot_poison_a_reachable_guard() {
    let graph = request(vec![
        node(0, Fact::Validate(identity()), &[2]),
        node(1, Fact::Invalidate(Invalidation::IdentityMutation), &[2]),
        node(2, Fact::Publish, &[]),
        node(3, Fact::Unsupported(UnsupportedFact::Alias), &[3]),
    ]);
    assert_single(&graph, PublicationState::GuardedInModel, &[]);
}

#[test]
fn shared_section_summary_requires_exact_identity_and_its_guard_does_not_escape() {
    let graph = request(vec![
        node(0, Fact::SharedCriticalSection {
            validated: identity(),
            exclusion: ExclusionEvidence::Established(EvidenceId(7)),
        }, &[1]),
        node(1, Fact::Publish, &[]),
    ]);
    let result = analyze(&graph, None).unwrap();
    assert!(result.complete);
    assert_eq!(result.decisions[0].state, PublicationState::GuardedInModel);
    assert_eq!(result.decisions[0].exclusion_evidence, Some(EvidenceId(7)));
    assert_eq!(result.decisions[1].state, PublicationState::UnguardedInModel);
    assert_eq!(result.decisions[1].witness, vec![NodeId(0), NodeId(1)]);

    let wrong = PublicationIdentity { root: ValueId(99), ..identity() };
    let wrong_section = request(vec![
        node(0, Fact::Validate(identity()), &[1]),
        node(1, Fact::SharedCriticalSection {
            validated: wrong,
            exclusion: ExclusionEvidence::Established(EvidenceId(8)),
        }, &[]),
    ]);
    // An earlier ordinary guard does not replace this helper's own validation.
    assert_single(&wrong_section, PublicationState::UnguardedInModel, &[0, 1]);
}

#[test]
fn a_lock_name_without_established_exclusion_is_unknown() {
    let graph = request(vec![node(0, Fact::SharedCriticalSection {
        validated: identity(),
        exclusion: ExclusionEvidence::Unknown,
    }, &[])]);
    let result = analyze(&graph, None).unwrap();
    assert!(!result.complete);
    assert_eq!(result.frontier[0].reason, FrontierReason::UnestablishedExclusion);
    assert_eq!(result.decisions[0].state, PublicationState::Unknown);
}

#[test]
fn decisions_and_witnesses_ignore_node_and_successor_input_order() {
    let graph = request(vec![
        node(0, Fact::Noop, &[2, 1]),
        node(1, Fact::Validate(identity()), &[3]),
        node(2, Fact::Noop, &[3]),
        node(3, Fact::Publish, &[4]),
        node(4, Fact::Publish, &[]),
    ]);
    let mut reordered = graph.clone();
    reordered.nodes.reverse();
    for node in &mut reordered.nodes {
        node.successors.reverse();
    }
    let expected = analyze(&graph, None).unwrap();
    assert_eq!(analyze(&reordered, None).unwrap(), expected);
    assert_eq!(expected.decisions[0].node, NodeId(3));
    assert_eq!(expected.decisions[1].node, NodeId(4));
    assert_eq!(expected.decisions[1].witness, vec![NodeId(0), NodeId(2), NodeId(3), NodeId(4)]);
}

#[test]
fn malformed_graphs_are_rejected_with_deterministic_errors() {
    let duplicate = request(vec![node(0, Fact::Noop, &[]), node(0, Fact::Publish, &[])]);
    assert_eq!(analyze(&duplicate, None), Err(PublicationInputError::DuplicateNode(NodeId(0))));
    let missing_entry = request(vec![node(1, Fact::Publish, &[])]);
    assert_eq!(analyze(&missing_entry, None), Err(PublicationInputError::MissingEntry(NodeId(0))));
    for successors in [&[9, 8][..], &[8, 9][..]] {
        let graph = request(vec![node(0, Fact::Noop, successors)]);
        assert_eq!(analyze(&graph, None), Err(PublicationInputError::MissingSuccessor {
            from: NodeId(0), to: NodeId(8),
        }));
    }
}

#[test]
fn every_limit_must_be_positive() {
    let graph = request(vec![node(0, Fact::Publish, &[])]);
    for limits in [
        PublicationLimits { max_nodes: 0, ..PublicationLimits::default() },
        PublicationLimits { max_edges: 0, ..PublicationLimits::default() },
        PublicationLimits { max_publications: 0, ..PublicationLimits::default() },
    ] {
        assert_eq!(analyze(&graph, Some(&limits)), Err(PublicationInputError::NonPositiveLimit));
    }
}

#[test]
fn every_budget_rejects_the_whole_input_without_a_prefix_result() {
    let graphs_and_limits = [
        (request(vec![node(0, Fact::Noop, &[1]), node(1, Fact::Publish, &[])]),
            PublicationLimits { max_nodes: 1, ..PublicationLimits::default() }, Budget::Nodes),
        // Duplicate edges still consume the raw input edge budget.
        (request(vec![node(0, Fact::Validate(identity()), &[1, 1, 1]), node(1, Fact::Publish, &[])]),
            PublicationLimits { max_edges: 2, ..PublicationLimits::default() }, Budget::Edges),
        (request(vec![node(0, Fact::Publish, &[1]), node(1, Fact::Publish, &[])]),
            PublicationLimits { max_publications: 1, ..PublicationLimits::default() }, Budget::Publications),
    ];
    for (graph, limits, budget) in graphs_and_limits {
        let result = analyze(&graph, Some(&limits)).unwrap();
        assert!(!result.complete);
        assert!(result.budget_exhausted);
        assert!(result.decisions.is_empty());
        assert_eq!(result.frontier[0].reason, FrontierReason::Budget(budget));
    }
}

#[test]
fn absence_of_a_reachable_publication_is_not_a_vacuous_clean_result() {
    let graph = request(vec![node(0, Fact::Noop, &[]), node(1, Fact::Publish, &[])]);
    let result = analyze(&graph, None).unwrap();
    assert!(!result.complete);
    assert!(!result.budget_exhausted);
    assert!(result.decisions.is_empty());
    assert_eq!(result.frontier[0].reason, FrontierReason::NoReachablePublication);
}
