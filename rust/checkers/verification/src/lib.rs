//! Pure v1 decision engine for SlopHawk's typed verification rules.
//!
//! Ported from `SlopHawk.Core.Verification.VerificationRuleEvaluator.Evaluate`
//! at commit `978a6464a6e5013fb1ad3a56c4e2877e62c18ac4`.
//! Callers supply facts *after* receipt, provenance, artifact, and target-mapping
//! validation. The evaluator performs no I/O and makes no new evidence claims.
//! `evaluate` retains the existing edited-branch API; `typed` adds mixed-rule input.

use std::cmp::Ordering;
use std::error::Error;
use std::fmt;

pub mod publication;
pub mod typed;

/// Versioned rule IDs for already validated, typed producer facts.
pub const EDITED_BRANCH_RULE_ID: &str = "sg.verification.edited-branch";
pub const PUBLIC_CONTRACT_RULE_ID: &str = "sg.verification.public-contract";
pub const SERIALIZATION_RULE_ID: &str = "sg.verification.serialization";
/// The version of the pinned rule semantics, not the Cargo package version.
pub const RULE_VERSION: u32 = 1;
/// Advertise only capabilities actually implemented by this standalone crate.
pub const SUPPORTED_RULE_IDS: &[&str] = &[
    EDITED_BRANCH_RULE_ID,
    PUBLIC_CONTRACT_RULE_ID,
    SERIALIZATION_RULE_ID,
];

/// Identity is contextual: an overload, branch arm, target, or source/build
/// revision cannot be substituted for another.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VerificationScope {
    pub context_id: String,
    pub target_id: String,
    pub symbol_id: String,
    pub behavior_id: String,
    pub source_digest: String,
    pub build_id: String,
}

/// All changes accepted by this crate are edited-branch changes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VerificationChange {
    pub id: String,
    pub scope: VerificationScope,
    pub changed_range: String,
    pub reasons: Vec<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VerificationPolicyMode {
    Disabled,
    Advisory,
    Required,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VerificationRuleSetting {
    pub rule_id: String,
    pub mode: VerificationPolicyMode,
    /// Full applicable negative-search boundary, not a selected sample.
    pub required_detectors: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VerificationPolicy {
    pub version: String,
    pub default_mode: VerificationPolicyMode,
    pub rules: Vec<VerificationRuleSetting>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VerificationSearchFact {
    pub id: String,
    pub rule_id: String,
    pub detector_id: String,
    pub scope: VerificationScope,
    pub complete: bool,
    pub searched_targets: Vec<String>,
    pub excluded_targets: Vec<String>,
    pub frontier: Vec<String>,
    pub budget_exhausted: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VerificationEvidenceKind {
    Configuration,
    StaticPath,
    TestRun,
    BranchCounter,
    /// Accepted as an input fact but cannot measure an *edited branch*.
    ContractExecution,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VerificationBinding {
    Unresolved,
    NameCandidate,
    Possible,
    Exact,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VerificationCounterMapping {
    None,
    LineOnly,
    PartialBranch,
    Ambiguous,
    ExactBranch,
}

/// Declaration order matches the C# enum numeric order for `OrderBy(o => o)`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum VerificationRunOutcome {
    NotRun,
    Passed,
    Failed,
    Skipped,
    Incomplete,
}

/// Input facts have already passed artifact validation by their producers.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VerificationEvidence {
    pub id: String,
    pub rule_id: String,
    pub detector_id: String,
    pub scope: VerificationScope,
    pub kind: VerificationEvidenceKind,
    pub binding: VerificationBinding,
    pub valid_receipt: bool,
    pub exact_target_mapping: bool,
    pub counter_mapping: VerificationCounterMapping,
    pub hit_count: Option<i32>,
    pub run_outcome: VerificationRunOutcome,
    pub executed_behavior: bool,
    pub relevant_assertion: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VerificationRequest {
    pub policy: VerificationPolicy,
    pub changes: Vec<VerificationChange>,
    pub searches: Vec<VerificationSearchFact>,
    pub evidence: Vec<VerificationEvidence>,
}

/// Hard bounds; no input-order prefix of an oversized request is evaluated.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VerificationLimits {
    pub max_changes: usize,
    pub max_searches: usize,
    pub max_evidence: usize,
    pub max_policy_rules: usize,
    pub max_list_items: usize,
}

impl Default for VerificationLimits {
    fn default() -> Self {
        Self {
            max_changes: 128,
            max_searches: 512,
            max_evidence: 2048,
            max_policy_rules: 32,
            max_list_items: 128,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VerificationCategory {
    CoverageTests,
    ContractTests,
}

impl VerificationCategory {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::CoverageTests => "coverage-tests",
            Self::ContractTests => "contract-tests",
        }
    }

    /// Match the pinned C# enum `ToString()` in contract action messages.
    pub const fn display_name(self) -> &'static str {
        match self {
            Self::CoverageTests => "CoverageTests",
            Self::ContractTests => "ContractTests",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VerificationState {
    Observed,
    Uncovered,
    Configured,
    Stale,
    Missing,
    Unknown,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VerificationStrength {
    None,
    Declared,
    PossibleStaticPath,
    ExactStaticPath,
    VerifiedMeasurement,
    MeasuredExecution,
}

/// Complete v1 decision, not just its headline disposition.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VerificationDecision {
    pub change_id: String,
    pub rule_id: String,
    pub rule_version: u32,
    pub category: VerificationCategory,
    pub policy_version: String,
    pub applied_policy: VerificationPolicyMode,
    pub scope: VerificationScope,
    pub changed_range: String,
    pub trigger_reasons: Vec<String>,
    pub state: VerificationState,
    pub strength: VerificationStrength,
    pub consulted_detectors: Vec<String>,
    pub searched_targets: Vec<String>,
    pub excluded_targets: Vec<String>,
    pub evidence_ids: Vec<String>,
    pub run_outcomes: Vec<VerificationRunOutcome>,
    pub relevant_assertion_established: bool,
    pub reasons: Vec<String>,
    pub frontier: Vec<String>,
    pub next_action: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VerificationBatch {
    pub decisions: Vec<VerificationDecision>,
    pub budget_exhausted: bool,
    pub frontier: Vec<String>,
}

/// Equivalent to C#'s out-of-range exception for a zero/negative limit.
/// Rust limits use `usize`, so negative values cannot be represented.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VerificationInputError {
    NonPositiveLimit,
}

impl fmt::Display for VerificationInputError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NonPositiveLimit => write!(f, "All verification limits must be positive."),
        }
    }
}

impl Error for VerificationInputError {}

/// Compare valid Unicode Rust strings by their UTF-16 code units as .NET
/// `StringComparer.Ordinal` does. Rust's ordinary `str::cmp` would order a
/// supplementary-plane ID differently from a high BMP/private-use ID.
fn ordinal(left: &str, right: &str) -> Ordering {
    left.encode_utf16().cmp(right.encode_utf16())
}

fn sorted_distinct(items: impl IntoIterator<Item = String>) -> Vec<String> {
    let mut values: Vec<String> = items.into_iter().collect();
    values.sort_by(|a, b| ordinal(a, b));
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

fn completed(outcome: VerificationRunOutcome) -> bool {
    matches!(
        outcome,
        VerificationRunOutcome::Passed | VerificationRunOutcome::Failed
    )
}

#[allow(clippy::too_many_arguments)]
fn make_decision(
    change: &VerificationChange,
    policy_version: &str,
    mode: VerificationPolicyMode,
    state: VerificationState,
    strength: VerificationStrength,
    consulted_detectors: Vec<String>,
    searched_targets: Vec<String>,
    excluded_targets: Vec<String>,
    evidence_ids: Vec<String>,
    run_outcomes: Vec<VerificationRunOutcome>,
    relevant_assertion_established: bool,
    reasons: Vec<String>,
    frontier: Vec<String>,
    next_action: String,
) -> VerificationDecision {
    VerificationDecision {
        change_id: change.id.clone(),
        rule_id: EDITED_BRANCH_RULE_ID.to_owned(),
        rule_version: RULE_VERSION,
        category: VerificationCategory::CoverageTests,
        policy_version: policy_version.to_owned(),
        applied_policy: mode,
        scope: change.scope.clone(),
        changed_range: change.changed_range.clone(),
        trigger_reasons: sorted_distinct(change.reasons.iter().cloned()),
        state,
        strength,
        consulted_detectors,
        searched_targets,
        excluded_targets,
        evidence_ids,
        run_outcomes,
        relevant_assertion_established,
        reasons,
        frontier,
        next_action,
    }
}

/// Evaluate a batch using C# v1 count and nested-list limits by default.
///
/// Whole-request rejection applies to over-limit change/policy counts and to
/// applicable nested lists. An over-limit search/evidence count returns one
/// `unknown` decision per in-budget change, rather than sampling the facts.
/// There is no interpretation or verification of external receipt bytes here.
pub fn evaluate(
    request: &VerificationRequest,
    limits: Option<&VerificationLimits>,
) -> Result<VerificationBatch, VerificationInputError> {
    let default_limits = VerificationLimits::default();
    let limits = limits.unwrap_or(&default_limits);
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

    // Deliberately mirror the C# conditional: when search counts already exceed
    // their cap, inspecting their nested lists is neither required nor permitted.
    if request.changes.len() <= limits.max_changes
        && request.policy.rules.len() <= limits.max_policy_rules
        && (request
            .changes
            .iter()
            .any(|c| c.reasons.len() > limits.max_list_items)
            || request
                .policy
                .rules
                .iter()
                .any(|r| r.required_detectors.len() > limits.max_list_items)
            || (request.searches.len() <= limits.max_searches
                && request.searches.iter().any(|s| {
                    s.searched_targets.len() > limits.max_list_items
                        || s.excluded_targets.len() > limits.max_list_items
                        || s.frontier.len() > limits.max_list_items
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

    // Stable sort, equivalent to LINQ's OrderBy/ThenBy chain.
    let mut changes: Vec<&VerificationChange> = request.changes.iter().collect();
    changes.sort_by(|a, b| {
        ordinal(&a.scope.context_id, &b.scope.context_id)
            .then_with(|| ordinal(&a.scope.target_id, &b.scope.target_id))
            .then_with(|| ordinal(&a.scope.symbol_id, &b.scope.symbol_id))
            .then_with(|| ordinal(&a.scope.behavior_id, &b.scope.behavior_id))
            .then_with(|| ordinal(&a.id, &b.id))
    });

    let mut decisions = Vec::with_capacity(changes.len());
    for change in changes {
        // First matching setting wins, as in C# FirstOrDefault; policy rules
        // with unrelated IDs still participate in the count/nested caps.
        let setting = request
            .policy
            .rules
            .iter()
            .find(|p| p.rule_id == EDITED_BRANCH_RULE_ID);
        let mode = setting.map_or(request.policy.default_mode, |p| p.mode);
        decisions.push(if limit_frontier.is_empty() {
            evaluate_rule(change, &request.policy.version, setting, mode, request)
        } else {
            make_decision(
                change,
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
                    change.scope.target_id
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

fn evaluate_rule(
    change: &VerificationChange,
    policy_version: &str,
    setting: Option<&VerificationRuleSetting>,
    mode: VerificationPolicyMode,
    request: &VerificationRequest,
) -> VerificationDecision {
    let scope = &change.scope;
    if mode == VerificationPolicyMode::Disabled {
        return make_decision(
            change,
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
                "Enable rule '{}' for target '{}' to request verification.",
                EDITED_BRANCH_RULE_ID, scope.target_id
            ),
        );
    }

    // Match the four behavior keys, *then* compare source/build revisions.
    let mut scoped_searches: Vec<&VerificationSearchFact> = request
        .searches
        .iter()
        .filter(|s| s.rule_id == EDITED_BRANCH_RULE_ID && same_behavior(&s.scope, scope))
        .collect();
    scoped_searches.sort_by(|a, b| {
        ordinal(&a.detector_id, &b.detector_id).then_with(|| ordinal(&a.id, &b.id))
    });
    let mut scoped_evidence: Vec<&VerificationEvidence> = request
        .evidence
        .iter()
        .filter(|e| e.rule_id == EDITED_BRANCH_RULE_ID && same_behavior(&e.scope, scope))
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
        .filter(|s| same_revision(&s.scope, scope))
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
        if !fresh_searches.iter().any(|s| s.detector_id == *detector) {
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
            VerificationEvidenceKind::BranchCounter => {
                if item.binding == VerificationBinding::Exact
                    && item.valid_receipt
                    && item.exact_target_mapping
                    && item.counter_mapping == VerificationCounterMapping::ExactBranch
                    && item.hit_count.is_some_and(|n| n >= 0)
                    && completed(item.run_outcome)
                {
                    if item.hit_count.is_some_and(|n| n > 0) {
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
            _ => {
                // A completed test run, static candidate, or contract-only
                // execution cannot supply an individual branch-arm hit.
                unresolved.push(item);
                frontier.push(format!(
                    "evidence '{}' does not measure the required behavior",
                    item.id
                ));
            }
        }
    }

    let consulted_detectors = sorted_distinct(
        scoped_searches
            .iter()
            .map(|s| s.detector_id.clone())
            .chain(scoped_evidence.iter().map(|e| e.detector_id.clone())),
    );
    let searched_targets = sorted_distinct(
        fresh_searches
            .iter()
            .flat_map(|s| s.searched_targets.iter().cloned()),
    );
    let excluded_targets = sorted_distinct(
        fresh_searches
            .iter()
            .flat_map(|s| s.excluded_targets.iter().cloned()),
    );
    let evidence_ids = sorted_distinct(matching.iter().map(|e| e.id.clone()));
    let mut run_outcomes: Vec<VerificationRunOutcome> = matching
        .iter()
        .filter(|e| {
            same_revision(&e.scope, scope)
                && e.valid_receipt
                && e.run_outcome != VerificationRunOutcome::NotRun
        })
        .map(|e| e.run_outcome)
        .collect();
    run_outcomes.sort();
    run_outcomes.dedup();
    // For this rule a branch hit is execution, never an attested assertion.
    let assertion_relevant = false;

    let state;
    let strength;
    let action;
    if !observed.is_empty() {
        state = VerificationState::Observed;
        strength = VerificationStrength::MeasuredExecution;
        reasons.push(format!(
            "Verified matching behavior measurement: {}.",
            join_evidence_ids(&observed)
        ));
        action = if run_outcomes.contains(&VerificationRunOutcome::Failed) {
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
            join_evidence_ids(&zeros)
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
            join_evidence_ids(&configured)
        ));
        action = format!(
            "Run the configured {} check for target '{}' and import a current, exactly mapped result.",
            VerificationCategory::CoverageTests.as_str(),
            scope.target_id
        );
    } else if !stale.is_empty() || unreplaced_stale_search {
        state = VerificationState::Stale;
        strength = VerificationStrength::None;
        reasons.push(
            "Only obsolete source/build evidence or search receipts apply to this behavior.".into(),
        );
        action = format!(
            "Rerun the {} detector for target '{}' at source '{}' and build '{}'.",
            VerificationCategory::CoverageTests.as_str(),
            scope.target_id,
            scope.source_digest,
            scope.build_id
        );
    } else {
        let complete = !expected.is_empty()
            && unresolved.is_empty()
            && frontier.is_empty()
            && expected
                .iter()
                .all(|d| fresh_searches.iter().any(|s| &s.detector_id == d))
            && fresh_searches.iter().all(|s| {
                s.complete
                    && !s.budget_exhausted
                    && s.frontier.is_empty()
                    && s.searched_targets.contains(&scope.target_id)
                    && !s.excluded_targets.contains(&scope.target_id)
            });
        if complete {
            state = VerificationState::Missing;
            reasons.push(
                "Every required detector completed the current applicable scope without linked evidence."
                    .into(),
            );
            action = format!(
                "Add a {} check for '{}/{}' in target '{}' and repeat the detector search.",
                VerificationCategory::CoverageTests.as_str(),
                scope.symbol_id,
                scope.behavior_id,
                scope.target_id
            );
        } else {
            state = VerificationState::Unknown;
            if frontier.is_empty() {
                frontier.push("no complete applicable evidence search was supplied".into());
            }
            reasons.push(
                "No confirmed behavior measurement; the evidence search or mappings are unresolved."
                    .into(),
            );
            action = format!(
                "Complete the {} detector search and exact behavior mapping for target '{}', then re-evaluate.",
                VerificationCategory::CoverageTests.as_str(),
                scope.target_id
            );
        }
        strength = VerificationStrength::None;
    }

    make_decision(
        change,
        policy_version,
        mode,
        state,
        strength,
        consulted_detectors,
        searched_targets,
        excluded_targets,
        evidence_ids,
        run_outcomes,
        assertion_relevant,
        reasons,
        sorted_distinct(frontier),
        action,
    )
}

fn join_evidence_ids(evidence: &[&VerificationEvidence]) -> String {
    evidence
        .iter()
        .map(|e| e.id.as_str())
        .collect::<Vec<_>>()
        .join(", ")
}
