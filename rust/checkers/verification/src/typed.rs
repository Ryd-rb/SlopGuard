//! V1 verification decisions for typed edited branches, public contracts, and serialization.
//!
//! This is a pure, typed decision surface: provider-side admission, receipt validation,
//! coverage provenance, source indexing and serialization happen outside this crate.
//! The legacy `crate::evaluate` entry point remains unchanged for edited-branch clients.

use crate::{
    EDITED_BRANCH_RULE_ID, PUBLIC_CONTRACT_RULE_ID, RULE_VERSION, SERIALIZATION_RULE_ID,
    VerificationBatch, VerificationBinding, VerificationCategory, VerificationChange,
    VerificationCounterMapping, VerificationDecision, VerificationEvidence,
    VerificationEvidenceKind, VerificationInputError, VerificationLimits, VerificationPolicy,
    VerificationPolicyMode, VerificationRuleSetting, VerificationRunOutcome, VerificationScope,
    VerificationSearchFact, VerificationState, VerificationStrength,
};
use std::cmp::Ordering;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VerificationChangeKind {
    EditedBranch,
    PublicContract,
    Serialization,
}

impl VerificationChangeKind {
    pub const fn rule_id(self) -> &'static str {
        match self {
            Self::EditedBranch => EDITED_BRANCH_RULE_ID,
            Self::PublicContract => PUBLIC_CONTRACT_RULE_ID,
            Self::Serialization => SERIALIZATION_RULE_ID,
        }
    }

    pub const fn category(self) -> VerificationCategory {
        match self {
            Self::EditedBranch => VerificationCategory::CoverageTests,
            Self::PublicContract | Self::Serialization => VerificationCategory::ContractTests,
        }
    }
}

/// Does not change the pre-existing `VerificationChange` wire shape for edited-branch callers.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TypedVerificationChange {
    pub kind: VerificationChangeKind,
    pub change: VerificationChange,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TypedVerificationRequest {
    pub policy: VerificationPolicy,
    pub changes: Vec<TypedVerificationChange>,
    pub searches: Vec<VerificationSearchFact>,
    pub evidence: Vec<VerificationEvidence>,
}

/// Typed classification at a runner/provider boundary. The C# v1 evaluator records
/// `VerificationRunOutcome`, which has fewer states; this richer disposition must
/// not be collapsed into a claimed completed receipt or an observed assertion.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProviderRunDisposition {
    Passed,
    Failed,
    Skipped,
    ZeroDiscovered,
    Unadmitted,
    Incomplete,
}

impl ProviderRunDisposition {
    pub const fn decision_outcome(self) -> VerificationRunOutcome {
        match self {
            Self::Passed => VerificationRunOutcome::Passed,
            Self::Failed => VerificationRunOutcome::Failed,
            Self::Skipped => VerificationRunOutcome::Skipped,
            Self::ZeroDiscovered | Self::Incomplete => VerificationRunOutcome::Incomplete,
            Self::Unadmitted => VerificationRunOutcome::NotRun,
        }
    }

    pub const fn is_completed(self) -> bool {
        matches!(self, Self::Passed | Self::Failed)
    }
}

/// Keep the precise runner disposition alongside the generic v1 evidence fact.
/// Conversion deliberately never turns zero discovery or unadmitted work into a
/// passed/failed receipt; callers retain this original record for reports.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProviderVerificationEvidence {
    pub fact: VerificationEvidence,
    pub disposition: ProviderRunDisposition,
}

impl ProviderVerificationEvidence {
    pub fn for_evaluation(&self) -> VerificationEvidence {
        let mut fact = self.fact.clone();
        fact.run_outcome = self.disposition.decision_outcome();
        fact
    }
}

fn ordinal(left: &str, right: &str) -> Ordering {
    left.encode_utf16().cmp(right.encode_utf16())
}

fn sorted_distinct(items: impl IntoIterator<Item = String>) -> Vec<String> {
    let mut values: Vec<String> = items.into_iter().collect();
    values.sort_by(|left, right| ordinal(left, right));
    values.dedup();
    values
}

fn same_behavior(left: &VerificationScope, right: &VerificationScope) -> bool {
    left.context_id == right.context_id
        && left.target_id == right.target_id
        && left.symbol_id == right.symbol_id
        && left.behavior_id == right.behavior_id
}

fn same_revision(left: &VerificationScope, right: &VerificationScope) -> bool {
    left.source_digest == right.source_digest && left.build_id == right.build_id
}

fn is_completed(outcome: VerificationRunOutcome) -> bool {
    matches!(
        outcome,
        VerificationRunOutcome::Passed | VerificationRunOutcome::Failed
    )
}

#[allow(clippy::too_many_arguments)]
fn decision(
    typed: &TypedVerificationChange,
    policy_version: &str,
    mode: VerificationPolicyMode,
    state: VerificationState,
    strength: VerificationStrength,
    consulted: Vec<String>,
    searched: Vec<String>,
    excluded: Vec<String>,
    evidence_ids: Vec<String>,
    run_outcomes: Vec<VerificationRunOutcome>,
    assertion: bool,
    reasons: Vec<String>,
    frontier: Vec<String>,
    action: String,
) -> VerificationDecision {
    let change = &typed.change;
    VerificationDecision {
        change_id: change.id.clone(),
        rule_id: typed.kind.rule_id().into(),
        rule_version: RULE_VERSION,
        category: typed.kind.category(),
        policy_version: policy_version.into(),
        applied_policy: mode,
        scope: change.scope.clone(),
        changed_range: change.changed_range.clone(),
        trigger_reasons: sorted_distinct(change.reasons.iter().cloned()),
        state,
        strength,
        consulted_detectors: consulted,
        searched_targets: searched,
        excluded_targets: excluded,
        evidence_ids,
        run_outcomes,
        relevant_assertion_established: assertion,
        reasons,
        frontier,
        next_action: action,
    }
}

/// Apply all three C# v1 rules to a mixed batch, in the same contextual ordinal order.
///
/// Over-limit changes/policy or nested lists reject the whole batch. An oversized
/// search or evidence set returns an `Unknown` decision for every in-budget change,
/// not an input-order sample. In particular no negative search without a complete
/// required detector boundary can establish `Missing`.
pub fn evaluate_typed(
    request: &TypedVerificationRequest,
    limits: Option<&VerificationLimits>,
) -> Result<VerificationBatch, VerificationInputError> {
    let defaults = VerificationLimits::default();
    let limits = limits.unwrap_or(&defaults);
    if [
        limits.max_changes,
        limits.max_searches,
        limits.max_evidence,
        limits.max_policy_rules,
        limits.max_list_items,
    ]
    .contains(&0)
    {
        return Err(VerificationInputError::NonPositiveLimit);
    }

    let mut limit_frontier = Vec::new();
    if request.changes.len() > limits.max_changes {
        limit_frontier.push(format!(
            "change budget exceeded: {} > {}",
            request.changes.len(),
            limits.max_changes
        ));
    }
    if request.searches.len() > limits.max_searches {
        limit_frontier.push(format!(
            "search-fact budget exceeded: {} > {}",
            request.searches.len(),
            limits.max_searches
        ));
    }
    if request.evidence.len() > limits.max_evidence {
        limit_frontier.push(format!(
            "evidence-fact budget exceeded: {} > {}",
            request.evidence.len(),
            limits.max_evidence
        ));
    }
    if request.policy.rules.len() > limits.max_policy_rules {
        limit_frontier.push(format!(
            "policy-rule budget exceeded: {} > {}",
            request.policy.rules.len(),
            limits.max_policy_rules
        ));
    }

    if request.changes.len() <= limits.max_changes
        && request.policy.rules.len() <= limits.max_policy_rules
        && (request
            .changes
            .iter()
            .any(|item| item.change.reasons.len() > limits.max_list_items)
            || request
                .policy
                .rules
                .iter()
                .any(|item| item.required_detectors.len() > limits.max_list_items)
            || (request.searches.len() <= limits.max_searches
                && request.searches.iter().any(|item| {
                    item.searched_targets.len() > limits.max_list_items
                        || item.excluded_targets.len() > limits.max_list_items
                        || item.frontier.len() > limits.max_list_items
                })))
    {
        limit_frontier.push(format!(
            "nested scope-list budget exceeded: maximum {} items per list",
            limits.max_list_items
        ));
        return Ok(VerificationBatch {
            decisions: vec![],
            budget_exhausted: true,
            frontier: limit_frontier,
        });
    }
    if request.changes.len() > limits.max_changes
        || request.policy.rules.len() > limits.max_policy_rules
    {
        return Ok(VerificationBatch {
            decisions: vec![],
            budget_exhausted: true,
            frontier: limit_frontier,
        });
    }

    let mut changes: Vec<&TypedVerificationChange> = request.changes.iter().collect();
    changes.sort_by(|left, right| {
        let a = &left.change;
        let b = &right.change;
        ordinal(&a.scope.context_id, &b.scope.context_id)
            .then_with(|| ordinal(&a.scope.target_id, &b.scope.target_id))
            .then_with(|| ordinal(&a.scope.symbol_id, &b.scope.symbol_id))
            .then_with(|| ordinal(&a.scope.behavior_id, &b.scope.behavior_id))
            .then_with(|| ordinal(&a.id, &b.id))
    });
    let mut decisions = Vec::with_capacity(changes.len());
    for typed in changes {
        let rule_id = typed.kind.rule_id();
        let setting = request.policy.rules.iter().find(|r| r.rule_id == rule_id);
        let mode = setting.map_or(request.policy.default_mode, |r| r.mode);
        decisions.push(if limit_frontier.is_empty() {
            evaluate_rule(typed, &request.policy.version, setting, mode, request)
        } else {
            decision(
                typed,
                &request.policy.version,
                mode,
                VerificationState::Unknown,
                VerificationStrength::None,
                vec![],
                vec![],
                vec![],
                vec![],
                vec![],
                false,
                vec!["The complete fact set was not evaluated.".into()],
                limit_frontier.clone(),
                format!(
                    "Reduce or partition the fact set for target '{}' and rerun the rule.",
                    typed.change.scope.target_id
                ),
            )
        });
    }
    Ok(VerificationBatch {
        decisions,
        budget_exhausted: !limit_frontier.is_empty()
            || request.searches.iter().any(|s| s.budget_exhausted),
        frontier: limit_frontier,
    })
}

fn ids(items: &[&VerificationEvidence]) -> String {
    items
        .iter()
        .map(|item| item.id.as_str())
        .collect::<Vec<_>>()
        .join(", ")
}

fn evaluate_rule(
    typed: &TypedVerificationChange,
    policy_version: &str,
    setting: Option<&VerificationRuleSetting>,
    mode: VerificationPolicyMode,
    request: &TypedVerificationRequest,
) -> VerificationDecision {
    let rule_id = typed.kind.rule_id();
    let category = match typed.kind {
        VerificationChangeKind::EditedBranch => typed.kind.category().as_str(),
        VerificationChangeKind::PublicContract | VerificationChangeKind::Serialization => {
            typed.kind.category().display_name()
        }
    };
    let scope = &typed.change.scope;
    if mode == VerificationPolicyMode::Disabled {
        return decision(
            typed,
            policy_version,
            mode,
            VerificationState::Unknown,
            VerificationStrength::None,
            vec![],
            vec![],
            vec![],
            vec![],
            vec![],
            false,
            vec!["Rule disabled by explicit policy; no evidence claim was evaluated.".into()],
            vec!["policy.disabled".into()],
            format!(
                "Enable rule '{rule_id}' for target '{}' to request verification.",
                scope.target_id
            ),
        );
    }

    let mut scoped_searches: Vec<&VerificationSearchFact> = request
        .searches
        .iter()
        .filter(|fact| fact.rule_id == rule_id && same_behavior(&fact.scope, scope))
        .collect();
    scoped_searches.sort_by(|a, b| {
        ordinal(&a.detector_id, &b.detector_id).then_with(|| ordinal(&a.id, &b.id))
    });
    let mut scoped_evidence: Vec<&VerificationEvidence> = request
        .evidence
        .iter()
        .filter(|fact| fact.rule_id == rule_id && same_behavior(&fact.scope, scope))
        .collect();
    scoped_evidence.sort_by(|a, b| {
        ordinal(&a.detector_id, &b.detector_id).then_with(|| ordinal(&a.id, &b.id))
    });

    let mut frontier = Vec::new();
    let mut reasons = Vec::new();
    let expected = sorted_distinct(
        setting
            .into_iter()
            .flat_map(|s| s.required_detectors.iter().cloned()),
    );
    let fresh_searches: Vec<&VerificationSearchFact> = scoped_searches
        .iter()
        .copied()
        .filter(|fact| same_revision(&fact.scope, scope))
        .collect();
    let mut unreplaced_stale_search = false;
    for search in &scoped_searches {
        if !same_revision(&search.scope, scope) {
            if fresh_searches
                .iter()
                .any(|s| s.detector_id == search.detector_id)
            {
                reasons.push(format!(
                    "Historical search '{}' was superseded by a current search.",
                    search.id
                ));
            } else {
                unreplaced_stale_search = true;
                frontier.push(format!(
                    "search '{}' has an obsolete source or build identity",
                    search.id
                ));
            }
        } else {
            if !search.complete || search.budget_exhausted {
                frontier.push(format!(
                    "detector '{}' search '{}' is incomplete or budget-exhausted",
                    search.detector_id, search.id
                ));
            }
            if !search.searched_targets.contains(&scope.target_id)
                || search.excluded_targets.contains(&scope.target_id)
            {
                frontier.push(format!(
                    "detector '{}' did not include target '{}'",
                    search.detector_id, scope.target_id
                ));
            }
            for item in &search.frontier {
                frontier.push(format!("detector '{}': {item}", search.detector_id));
            }
        }
    }
    if expected.is_empty() {
        frontier
            .push("policy has no exhaustive detector boundary; missing cannot be inferred".into());
    }
    for detector in &expected {
        if !fresh_searches.iter().any(|s| &s.detector_id == detector) {
            frontier.push(format!(
                "required detector '{detector}' has no current search receipt"
            ));
        }
    }

    let matching: Vec<&VerificationEvidence> = scoped_evidence
        .iter()
        .copied()
        .filter(|e| {
            matches!(
                e.binding,
                VerificationBinding::Exact | VerificationBinding::Possible
            )
        })
        .collect();
    let fresh: Vec<&VerificationEvidence> = matching
        .iter()
        .copied()
        .filter(|e| same_revision(&e.scope, scope))
        .collect();
    let stale: Vec<&VerificationEvidence> = matching
        .iter()
        .copied()
        .filter(|e| !same_revision(&e.scope, scope))
        .collect();
    for item in &stale {
        frontier.push(format!(
            "evidence '{}' has an obsolete source or build identity",
            item.id
        ));
    }

    let mut observed = Vec::<&VerificationEvidence>::new();
    let mut zeros = Vec::<&VerificationEvidence>::new();
    let mut configured = Vec::<&VerificationEvidence>::new();
    let mut unresolved = Vec::<&VerificationEvidence>::new();
    for item in fresh {
        match item.kind {
            VerificationEvidenceKind::Configuration
                if item.binding == VerificationBinding::Exact =>
            {
                configured.push(item);
            }
            VerificationEvidenceKind::StaticPath => {
                configured.push(item);
                if item.binding == VerificationBinding::Possible {
                    frontier.push(format!(
                        "static path '{}' is only a possible binding",
                        item.id
                    ));
                }
            }
            VerificationEvidenceKind::BranchCounter
                if typed.kind == VerificationChangeKind::EditedBranch =>
            {
                if item.binding == VerificationBinding::Exact
                    && item.valid_receipt
                    && item.exact_target_mapping
                    && item.counter_mapping == VerificationCounterMapping::ExactBranch
                    && item.hit_count.is_some_and(|hits| hits >= 0)
                    && is_completed(item.run_outcome)
                {
                    if item.hit_count.is_some_and(|hits| hits > 0) {
                        observed.push(item);
                    } else {
                        zeros.push(item);
                    }
                } else {
                    unresolved.push(item);
                    frontier.push(format!(
                        "branch counter '{}' lacks an exact verified, completed measurement",
                        item.id
                    ));
                }
            }
            VerificationEvidenceKind::ContractExecution
                if typed.kind != VerificationChangeKind::EditedBranch =>
            {
                if item.binding == VerificationBinding::Exact
                    && item.valid_receipt
                    && item.exact_target_mapping
                    && item.executed_behavior
                    && item.relevant_assertion
                    && is_completed(item.run_outcome)
                {
                    observed.push(item);
                } else {
                    unresolved.push(item);
                    frontier.push(format!("contract execution '{}' lacks an attested behavior/assertion or valid receipt", item.id));
                }
            }
            _ => {
                unresolved.push(item);
                frontier.push(format!(
                    "evidence '{}' does not measure the required behavior",
                    item.id
                ));
            }
        }
    }

    let detectors = sorted_distinct(
        scoped_searches
            .iter()
            .map(|s| s.detector_id.clone())
            .chain(scoped_evidence.iter().map(|e| e.detector_id.clone())),
    );
    let searched = sorted_distinct(
        fresh_searches
            .iter()
            .flat_map(|s| s.searched_targets.iter().cloned()),
    );
    let excluded = sorted_distinct(
        fresh_searches
            .iter()
            .flat_map(|s| s.excluded_targets.iter().cloned()),
    );
    let evidence_ids = sorted_distinct(matching.iter().map(|e| e.id.clone()));
    let mut outcomes: Vec<VerificationRunOutcome> = matching
        .iter()
        .filter(|e| {
            same_revision(&e.scope, scope)
                && e.valid_receipt
                && e.run_outcome != VerificationRunOutcome::NotRun
        })
        .map(|e| e.run_outcome)
        .collect();
    outcomes.sort();
    outcomes.dedup();
    let asserted = observed
        .iter()
        .any(|e| e.kind == VerificationEvidenceKind::ContractExecution);

    let (state, strength, action);
    if !observed.is_empty() {
        state = VerificationState::Observed;
        strength = VerificationStrength::MeasuredExecution;
        reasons.push(format!(
            "Verified matching behavior measurement: {}.",
            ids(&observed)
        ));
        action = if outcomes.contains(&VerificationRunOutcome::Failed) {
            format!(
                "Investigate the failed test result for target '{}', then rerun and import current measurement evidence.",
                scope.target_id
            )
        } else {
            format!(
                "Retain the verified receipt for target '{}' and re-evaluate after its inputs change.",
                scope.target_id
            )
        };
    } else if !zeros.is_empty() {
        state = VerificationState::Uncovered;
        strength = VerificationStrength::VerifiedMeasurement;
        reasons.push(format!(
            "Exact mapped branch counters recorded zero hits: {}.",
            ids(&zeros)
        ));
        action = format!(
            "Add a test executing branch '{}' in target '{}', then collect a new exact branch counter.",
            scope.behavior_id, scope.target_id
        );
    } else if !configured.is_empty() {
        state = VerificationState::Configured;
        strength = if configured.iter().any(|e| {
            e.kind == VerificationEvidenceKind::StaticPath
                && e.binding == VerificationBinding::Exact
        }) {
            VerificationStrength::ExactStaticPath
        } else if configured
            .iter()
            .any(|e| e.kind == VerificationEvidenceKind::StaticPath)
        {
            VerificationStrength::PossibleStaticPath
        } else {
            VerificationStrength::Declared
        };
        reasons.push(format!(
            "Scoped configuration/static path exists: {}; execution is not established.",
            ids(&configured)
        ));
        action = format!(
            "Run the configured {category} check for target '{}' and import a current, exactly mapped result.",
            scope.target_id
        );
    } else if !stale.is_empty() || unreplaced_stale_search {
        state = VerificationState::Stale;
        strength = VerificationStrength::None;
        reasons.push(
            "Only obsolete source/build evidence or search receipts apply to this behavior.".into(),
        );
        action = format!(
            "Rerun the {category} detector for target '{}' at source '{}' and build '{}'.",
            scope.target_id, scope.source_digest, scope.build_id
        );
    } else {
        let complete = !expected.is_empty()
            && unresolved.is_empty()
            && frontier.is_empty()
            && expected
                .iter()
                .all(|detector| fresh_searches.iter().any(|s| &s.detector_id == detector))
            && fresh_searches.iter().all(|s| {
                s.complete
                    && !s.budget_exhausted
                    && s.frontier.is_empty()
                    && s.searched_targets.contains(&scope.target_id)
                    && !s.excluded_targets.contains(&scope.target_id)
            });
        if complete {
            state = VerificationState::Missing;
            reasons.push("Every required detector completed the current applicable scope without linked evidence.".into());
            action = format!(
                "Add a {category} check for '{}/{}' in target '{}' and repeat the detector search.",
                scope.symbol_id, scope.behavior_id, scope.target_id
            );
        } else {
            state = VerificationState::Unknown;
            if frontier.is_empty() {
                frontier.push("no complete applicable evidence search was supplied".into());
            }
            reasons.push("No confirmed behavior measurement; the evidence search or mappings are unresolved.".into());
            action = format!(
                "Complete the {category} detector search and exact behavior mapping for target '{}', then re-evaluate.",
                scope.target_id
            );
        }
        strength = VerificationStrength::None;
    }

    decision(
        typed,
        policy_version,
        mode,
        state,
        strength,
        detectors,
        searched,
        excluded,
        evidence_ids,
        outcomes,
        asserted,
        reasons,
        sorted_distinct(frontier),
        action,
    )
}
