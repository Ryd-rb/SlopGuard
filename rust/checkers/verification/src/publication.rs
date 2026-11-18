//! Opt-in, model-only checking of the scheduled-checker current-report contract.
//!
//! The declared effect is returning/attaching a deferred checker report as current
//! (`Scheduler::current_report` -> `Workspace::results`), not exporting results.json.
//! This module performs no extraction, I/O, scheduling, admission, or observation.
//! Nothing in the existing verification rules calls it or advertises this contract.
//!
//! A producer must supply the complete relevant control flow and interference facts,
//! exact identities, and source/provenance mapping for every numeric ID. A successful
//! `Validate` fact means current-versus-captured equality, including the associated
//! epoch and required grant/pin predicates. Similar names or equal-looking variables
//! are insufficient. At the pinned resident seam the key is marker.epoch,
//! snapshot.revision, physical marker.root_identity, and Observed.scope; the epoch
//! association is through the authorized per-epoch scheduler, not a scope-hash field.
//! An unproved composite check, alias, helper, or external writer is a frontier.
//!
//! Reachable acyclic paths use a forward must-analysis: validation generates a fact,
//! invalidation kills it, and joins intersect predecessors. Capturing immutable input
//! does not establish freshness. A modeled unguarded path is not an observed stale
//! publication, and even `GuardedInModel` establishes only the supplied abstraction.
//! Locks over the resident object do not by themselves exclude filesystem writers.

use std::collections::{BTreeMap, BTreeSet};
use std::error::Error;
use std::fmt;

/// Deliberately separate from the existing verification rule IDs and policy defaults.
pub const CONTRACT_ID: &str = "slophawk.scheduled-checks.current-report";
pub const CONTRACT_VERSION: u32 = 1;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct NodeId(pub u32);

/// Interned, exactly correlated symbolic value; this is not a runtime numeric value.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ValueId(pub u32);

/// Reference into the producer's independently reviewable evidence/source map.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct EvidenceId(pub u32);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PublicationIdentity {
    pub revision: ValueId,
    pub epoch: ValueId,
    pub root: ValueId,
    pub digest: ValueId,
}

/// These are producer attestations, not facts authenticated by this evaluator.
/// Default is deliberately incomplete in every dimension.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ModelCoverage {
    pub control_flow: bool,
    pub identities: bool,
    pub interference: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Invalidation {
    DeferredWork,
    QueueHandoff,
    IdentityMutation,
    ExclusionEnded,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum UnsupportedFact {
    Alias,
    OpaqueHelper,
    UnresolvedIdentity,
    UnmodeledControlFlow,
    UnmodeledInterference,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExclusionEvidence {
    /// The producer established a single shared critical section spanning successful
    /// validation and publication, without release, yield, or hidden intervening call,
    /// and established that every relevant invalidator obeys the same exclusion.
    /// An evidence ID or a lock name alone does not establish those obligations.
    Established(EvidenceId),
    Unknown,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Fact {
    /// A reviewed operation with no relevant effect; never use for an opaque helper.
    Noop,
    CaptureImmutable,
    /// Place only on the successful edge of the complete current-identity check.
    Validate(PublicationIdentity),
    Invalidate(Invalidation),
    /// The effect always concerns the request's declared captured identity.
    Publish,
    /// A whole, producer-attested helper summary: enter, validate, publish, exit.
    /// Incoming validation cannot justify it; its validation cannot escape it.
    SharedCriticalSection {
        validated: PublicationIdentity,
        exclusion: ExclusionEvidence,
    },
    Unsupported(UnsupportedFact),
}

impl Fact {
    fn publishes(self) -> bool {
        matches!(self, Self::Publish | Self::SharedCriticalSection { .. })
    }

    fn clears_validation(self) -> bool {
        matches!(self, Self::Invalidate(_) | Self::SharedCriticalSection { .. })
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PublicationNode {
    pub id: NodeId,
    pub fact: Fact,
    pub successors: Vec<NodeId>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PublicationRequest {
    pub identity: PublicationIdentity,
    pub coverage: ModelCoverage,
    pub entry: NodeId,
    pub nodes: Vec<PublicationNode>,
}

/// Bounds count all supplied nodes, edges (including duplicates), and effects before
/// analysis. No over-budget input prefix is evaluated. Result witness storage is at
/// most max_publications * max_nodes numeric IDs, with at most max_nodes + 5 frontiers.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PublicationLimits {
    pub max_nodes: usize,
    pub max_edges: usize,
    pub max_publications: usize,
}

impl Default for PublicationLimits {
    fn default() -> Self {
        Self {
            max_nodes: 256,
            max_edges: 1024,
            max_publications: 64,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PublicationState {
    GuardedInModel,
    UnguardedInModel,
    Unknown,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PublicationDecision {
    pub node: NodeId,
    pub state: PublicationState,
    /// A deterministic entry-to-effect modeled path, only for UnguardedInModel.
    /// The graph may overapproximate feasible execution; this is not a runtime trace.
    pub witness: Vec<NodeId>,
    pub exclusion_evidence: Option<EvidenceId>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Budget {
    Nodes,
    Edges,
    Publications,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum FrontierReason {
    IncompleteControlFlow,
    IncompleteIdentities,
    IncompleteInterference,
    Unsupported(UnsupportedFact),
    UnestablishedExclusion,
    ReachableCycle,
    NoReachablePublication,
    Budget(Budget),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Frontier {
    pub node: Option<NodeId>,
    pub reason: FrontierReason,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PublicationAnalysis {
    pub identity: PublicationIdentity,
    /// Means evaluation of the supplied model completed, not that publication is safe.
    pub complete: bool,
    pub budget_exhausted: bool,
    /// Sorted by NodeId. Unknown models never contain a guarded/unguarded decision.
    pub decisions: Vec<PublicationDecision>,
    pub frontier: Vec<Frontier>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PublicationInputError {
    NonPositiveLimit,
    DuplicateNode(NodeId),
    MissingEntry(NodeId),
    MissingSuccessor { from: NodeId, to: NodeId },
}

impl fmt::Display for PublicationInputError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NonPositiveLimit => write!(f, "All publication limits must be positive."),
            Self::DuplicateNode(id) => write!(f, "Duplicate publication node {}.", id.0),
            Self::MissingEntry(id) => write!(f, "Missing publication entry {}.", id.0),
            Self::MissingSuccessor { from, to } => {
                write!(
                    f,
                    "Publication node {} references missing node {}.",
                    from.0, to.0
                )
            }
        }
    }
}

impl Error for PublicationInputError {}

fn exhausted(identity: PublicationIdentity, budget: Budget) -> PublicationAnalysis {
    PublicationAnalysis {
        identity,
        complete: false,
        budget_exhausted: true,
        decisions: Vec::new(),
        frontier: vec![Frontier {
            node: None,
            reason: FrontierReason::Budget(budget),
        }],
    }
}

/// Explicit invocation is the only opt-in. Existing evaluator behavior is unchanged.
///
/// Bounds are checked before graph allocation. Malformed references return an error;
/// coverage gaps, reachable cycles/frontiers, and no reachable effect return Unknown.
/// Unreachable nodes cannot establish or invalidate a reachable guard. Duplicate
/// edges are canonicalized after budget counting. Ordering is independent of supplied
/// node/edge order: numeric NodeId, then FrontierReason declaration order.
///
/// For V nodes, E supplied edges and P effects, work is
/// O((V + E) log(V + E) + P*V), extra storage O(V + E + P*V), including witnesses.
/// There is no path enumeration, recursion, fixed point, I/O, or unbounded text output.
pub fn analyze(
    request: &PublicationRequest,
    limits: Option<&PublicationLimits>,
) -> Result<PublicationAnalysis, PublicationInputError> {
    let defaults = PublicationLimits::default();
    let limits = limits.unwrap_or(&defaults);
    if [limits.max_nodes, limits.max_edges, limits.max_publications].contains(&0) {
        return Err(PublicationInputError::NonPositiveLimit);
    }
    if request.nodes.len() > limits.max_nodes {
        return Ok(exhausted(request.identity, Budget::Nodes));
    }
    let mut edge_count = 0usize;
    let mut publication_count = 0usize;
    for node in &request.nodes {
        let Some(total) = edge_count.checked_add(node.successors.len()) else {
            return Ok(exhausted(request.identity, Budget::Edges));
        };
        edge_count = total;
        if edge_count > limits.max_edges {
            return Ok(exhausted(request.identity, Budget::Edges));
        }
        if node.fact.publishes() {
            publication_count += 1;
        }
    }
    if publication_count > limits.max_publications {
        return Ok(exhausted(request.identity, Budget::Publications));
    }

    let mut nodes: Vec<&PublicationNode> = request.nodes.iter().collect();
    nodes.sort_by_key(|node| node.id);
    let mut positions = BTreeMap::new();
    for (index, node) in nodes.iter().enumerate() {
        if positions.insert(node.id, index).is_some() {
            return Err(PublicationInputError::DuplicateNode(node.id));
        }
    }
    let Some(&entry) = positions.get(&request.entry) else {
        return Err(PublicationInputError::MissingEntry(request.entry));
    };
    let mut successors = vec![Vec::new(); nodes.len()];
    let mut predecessors = vec![Vec::new(); nodes.len()];
    for (index, node) in nodes.iter().enumerate() {
        let mut ids = node.successors.clone();
        ids.sort();
        ids.dedup();
        for id in ids {
            let Some(&next) = positions.get(&id) else {
                return Err(PublicationInputError::MissingSuccessor {
                    from: node.id,
                    to: id,
                });
            };
            successors[index].push(next);
            predecessors[next].push(index);
        }
    }

    let mut reachable = vec![false; nodes.len()];
    reachable[entry] = true;
    let mut pending = vec![entry];
    while let Some(index) = pending.pop() {
        for &next in &successors[index] {
            if !reachable[next] {
                reachable[next] = true;
                pending.push(next);
            }
        }
    }
    let publications: Vec<usize> = nodes
        .iter()
        .enumerate()
        .filter(|(index, node)| reachable[*index] && node.fact.publishes())
        .map(|(index, _)| index)
        .collect();
    let mut frontier = Vec::new();
    for (complete, reason) in [
        (
            request.coverage.control_flow,
            FrontierReason::IncompleteControlFlow,
        ),
        (request.coverage.identities, FrontierReason::IncompleteIdentities),
        (
            request.coverage.interference,
            FrontierReason::IncompleteInterference,
        ),
    ] {
        if !complete {
            frontier.push(Frontier { node: None, reason });
        }
    }
    for (_, node) in nodes
        .iter()
        .enumerate()
        .filter(|(index, _)| reachable[*index])
    {
        let reason = match node.fact {
            Fact::Unsupported(reason) => Some(FrontierReason::Unsupported(reason)),
            Fact::SharedCriticalSection {
                exclusion: ExclusionEvidence::Unknown,
                ..
            } => Some(FrontierReason::UnestablishedExclusion),
            _ => None,
        };
        if let Some(reason) = reason {
            frontier.push(Frontier {
                node: Some(node.id),
                reason,
            });
        }
    }
    if publications.is_empty() {
        frontier.push(Frontier {
            node: None,
            reason: FrontierReason::NoReachablePublication,
        });
    }

    let mut degree = vec![0usize; nodes.len()];
    let mut ready = BTreeSet::new();
    for index in 0..nodes.len() {
        if reachable[index] {
            degree[index] = predecessors[index]
                .iter()
                .filter(|&&p| reachable[p])
                .count();
            if degree[index] == 0 {
                ready.insert(index);
            }
        }
    }
    let mut order = Vec::new();
    while let Some(index) = ready.pop_first() {
        order.push(index);
        for &next in &successors[index] {
            degree[next] -= 1;
            if degree[next] == 0 {
                ready.insert(next);
            }
        }
    }
    if order.len() != reachable.iter().filter(|&&value| value).count() {
        frontier.push(Frontier {
            node: None,
            reason: FrontierReason::ReachableCycle,
        });
    }
    frontier.sort();
    frontier.dedup();
    if !frontier.is_empty() {
        return Ok(PublicationAnalysis {
            identity: request.identity,
            complete: false,
            budget_exhausted: false,
            decisions: publications
                .iter()
                .map(|&index| PublicationDecision {
                    node: nodes[index].id,
                    state: PublicationState::Unknown,
                    witness: Vec::new(),
                    exclusion_evidence: None,
                })
                .collect(),
            frontier,
        });
    }

    let mut incoming = vec![false; nodes.len()];
    let mut outgoing = vec![false; nodes.len()];
    let mut first_predecessor = vec![None; nodes.len()];
    let mut unguarded_predecessor = vec![None; nodes.len()];
    for index in order {
        // Predecessors are already in numeric NodeId order.
        first_predecessor[index] = predecessors[index]
            .iter()
            .copied()
            .find(|&p| reachable[p]);
        unguarded_predecessor[index] = predecessors[index]
            .iter()
            .copied()
            .find(|&p| reachable[p] && !outgoing[p]);
        incoming[index] = index != entry
            && first_predecessor[index].is_some()
            && unguarded_predecessor[index].is_none();
        outgoing[index] = match nodes[index].fact {
            Fact::Validate(identity) => incoming[index] || identity == request.identity,
            fact if fact.clears_validation() => false,
            _ => incoming[index],
        };
    }
    let mut decisions = Vec::new();
    for index in publications {
        let (guarded, exclusion_evidence, ordinary_publication) = match nodes[index].fact {
            Fact::SharedCriticalSection {
                validated,
                exclusion: ExclusionEvidence::Established(id),
            } => (validated == request.identity, Some(id), false),
            _ => (incoming[index], None, true),
        };
        let mut witness = Vec::new();
        if !guarded {
            let mut cursor = Some(index);
            let mut need_unguarded = ordinary_publication;
            while let Some(current) = cursor {
                witness.push(nodes[current].id);
                if current == entry {
                    break;
                }
                // A barrier establishes an unguarded suffix regardless of its prefix.
                if nodes[current].fact.clears_validation() {
                    need_unguarded = false;
                }
                cursor = if need_unguarded {
                    unguarded_predecessor[current]
                } else {
                    first_predecessor[current]
                };
            }
            witness.reverse();
        }
        decisions.push(PublicationDecision {
            node: nodes[index].id,
            state: if guarded {
                PublicationState::GuardedInModel
            } else {
                PublicationState::UnguardedInModel
            },
            witness,
            exclusion_evidence,
        });
    }
    Ok(PublicationAnalysis {
        identity: request.identity,
        complete: true,
        budget_exhausted: false,
        decisions,
        frontier,
    })
}
