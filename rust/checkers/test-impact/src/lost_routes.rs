//! Opt-in comparison of two captured evaluated graphs; no scheduling or I/O.
//!
//! The caller owns capture authenticity, canonical paths, complete affected-item
//! enumeration, and support policy. An identity is an immutable owner attestation,
//! not a digest verified by this function. Evaluation sufficiency must be established
//! separately: an issue-free evaluation does not include execution-created items.
//! Findings concern configured graph reachability, never execution or coverage.

use super::{CompileMember, GraphIndex, PathComparison, ProjectGraph, ProjectTarget, ordinal, path_key};
use std::collections::{HashMap, HashSet, VecDeque};

/// Bind the graph bytes and source observation to an owner-qualified capture.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Identity {
    pub root: String,
    pub source_snapshot: String,
    pub graph_snapshot: String,
    /// Provider/tool/global-property/entry-point universe, excluding changing inputs.
    pub evaluation_scope: String,
    /// Immutable configured test group policy, independent of execution approval.
    /// The caller supplies its full resolved endpoint set in each capture below.
    pub configured_scope: String,
    pub paths: PathComparison,
}

#[derive(Clone, Copy, Debug)]
pub struct Capture<'a> {
    pub identity: &'a Identity,
    pub graph: &'a ProjectGraph,
    /// Entire evaluated universe, not a selected subset or explanation projection.
    pub complete: bool,
    /// False if relevant execution-created items/references may be absent.
    pub evaluation_sufficient: bool,
}

/// Owner-validated policy or rename declaration, bound to the current snapshot.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Declaration {
    pub source_snapshot: String,
    pub provenance: String,
}

/// Independent current source observation; missing Compile membership is not one.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SurvivingSource {
    pub source_snapshot: String,
    pub path: String,
    pub sha256: String,
}

/// Supplying a request declares this exact predecessor item/context supported.
/// Target replacements are deliberately unsupported; exact context correspondence
/// is required. Item renames are explicit and one-to-one within the supplied scope.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Request {
    pub key: String,
    pub previous_target: String,
    pub previous_item: CompileMember,
    pub current_item: CompileMember,
    pub surviving_source: Option<SurvivingSource>,
    pub rename: Option<Declaration>,
    pub retirement: Option<Declaration>,
}

/// Counts cover both graphs together. Charged bytes include requests, identities
/// and GraphIndex's repeated target-ID copies, not just serialized input bytes.
/// The edge-visit limit covers reverse searches; validation and attribution have
/// separate finite input bounds. Limits change no execution or selection budget.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Limits {
    pub targets: usize,
    pub members: usize,
    pub references: usize,
    pub requests: usize,
    pub input_bytes: usize,
    pub string_bytes: usize,
    pub edge_visits: usize,
    pub results: usize,
    pub witness_nodes: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            targets: 256,
            members: 40_000,
            references: 16_384,
            requests: 256,
            input_bytes: 8 * 1024 * 1024,
            string_bytes: 2048,
            edge_visits: 262_144,
            results: 64,
            witness_nodes: 16,
        }
    }
}

pub struct Input<'a> {
    pub previous: Capture<'a>,
    pub current: Capture<'a>,
    pub expected_previous: &'a Identity,
    pub expected_current: &'a Identity,
    /// Complete resolved configured groups for the respective captures. Include
    /// unapproved/deferred tests; never pass selected IDs only. Added current
    /// configured endpoints can preserve a route without any target equivalence.
    pub previous_configured_tests: &'a [String],
    pub current_configured_tests: &'a [String],
    pub requests: &'a [Request],
    pub limits: Limits,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Unknown {
    InputLimit,
    CaptureIdentity,
    ScopeMismatch,
    PartialGraph,
    ExecutionItems,
    UnsupportedPathCase,
    InvalidGraph,
    AmbiguousContext,
    ContextChanged,
    ToolchainChanged,
    InvalidRequests,
    InvalidConfiguredGroup,
    TargetNotComparable,
    PriorItemAbsent,
    SurvivalUnproven,
    RenameUnproven,
    RetirementUnproven,
    EdgeBudget,
    AttributionUnavailable,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    CompileMembershipLost,
    LastConfiguredRouteLost,
    Preserved,
    NoPriorConfiguredRoute,
    Retired,
    Unknown(Unknown),
}

/// Exact typed attribution in the captured graph, not a guessed XML location or
/// proof that this edge alone cuts every old route. Defining files are unavailable
/// in ProjectGraph; an owner may enrich this record from separately bound evidence.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MissingFact {
    CompileItem { previous_target: String, current_target: String },
    Target { previous_target: String },
    Reference {
        previous_referencing: String,
        previous_referenced: String,
        current_referencing: String,
        current_referenced: String,
    },
    TestClassification { previous_target: String, current_target: String },
    ConfiguredMembership { previous_target: String, current_target: String },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Witness {
    /// Source owner first, configured test last when not truncated.
    pub targets: Vec<String>,
    pub omitted_nodes: usize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ItemResult<'a> {
    pub request: &'a Request,
    pub outcome: Outcome,
    pub previous_route: Option<Witness>,
    pub current_route: Option<Witness>,
    pub first_missing: Option<MissingFact>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Report<'a> {
    pub previous: &'a Identity,
    pub current: &'a Identity,
    pub limits: Limits,
    /// All supplied requests decided, no unknowns or omitted results. Does not
    /// claim that the caller enumerated every affected item in the repository.
    pub complete: bool,
    pub problem: Option<Unknown>,
    pub results: Vec<ItemResult<'a>>,
    pub omitted_results: usize,
    pub edge_visits: usize,
}

fn field(value: &str) -> bool {
    !value.is_empty() && !value.contains('\0')
}

fn charge(total: &mut usize, amount: usize, maximum: usize) -> Result<(), Unknown> {
    *total = total.checked_add(amount).filter(|sum| *sum <= maximum)
        .ok_or(Unknown::InputLimit)?;
    Ok(())
}

/// No allocation proportional to caller input occurs before these checks.
fn preflight(input: &Input<'_>) -> Result<(), Unknown> {
    let limit = input.limits;
    if input.requests.len() > limit.requests {
        return Err(Unknown::InputLimit);
    }
    let mut groups = 0;
    charge(&mut groups, input.previous_configured_tests.len(), limit.targets)?;
    charge(&mut groups, input.current_configured_tests.len(), limit.targets)?;
    let mut bytes = 0;
    let mut expansion = 0;
    let mut add = |text: &str| {
        if text.len() > limit.string_bytes { return Err(Unknown::InputLimit); }
        charge(&mut bytes, text.len(), limit.input_bytes)
    };
    for identity in [input.previous.identity, input.current.identity,
        input.expected_previous, input.expected_current]
    {
        for text in [&identity.root, &identity.source_snapshot, &identity.graph_snapshot,
            &identity.evaluation_scope, &identity.configured_scope]
        {
            add(text)?;
        }
    }
    let mut targets = 0;
    let mut members = 0;
    let mut references = 0;
    for capture in [input.previous, input.current] {
        charge(&mut targets, capture.graph.targets.len(), limit.targets)?;
        // Issues need no indexing or copying: any issue makes the pair unknown.
        if !capture.complete || !capture.graph.complete() {
            return Err(Unknown::PartialGraph);
        }
        for target in &capture.graph.targets {
            charge(&mut members, target.compile.len(), limit.members)?;
            charge(&mut references, target.references.len(), limit.references)?;
            let copies = target.compile.len().checked_add(target.references.len())
                .and_then(|count| count.checked_mul(target.id.len()))
                .ok_or(Unknown::InputLimit)?;
            charge(&mut expansion, copies, limit.input_bytes)?;
            for text in [&target.id, &target.project_path, &target.framework,
                &target.configuration, &target.platform, &target.runtime_identifier,
                &target.sdk_version, &target.evaluation_fingerprint]
            {
                add(text)?;
            }
            for member in &target.compile {
                add(&member.path)?;
                if let Some(link) = &member.link { add(link)?; }
            }
            for reference in &target.references { add(reference)?; }
        }
    }
    for test in input.previous_configured_tests.iter().chain(input.current_configured_tests) {
        add(test)?;
    }
    for request in input.requests {
        add(&request.key)?;
        add(&request.previous_target)?;
        for item in [&request.previous_item, &request.current_item] {
            add(&item.path)?;
            if let Some(link) = &item.link { add(link)?; }
        }
        if let Some(source) = &request.surviving_source {
            add(&source.source_snapshot)?;
            add(&source.path)?;
            add(&source.sha256)?;
        }
        for declaration in [&request.rename, &request.retirement].into_iter().flatten() {
            add(&declaration.source_snapshot)?;
            add(&declaration.provenance)?;
        }
    }
    charge(&mut bytes, expansion, limit.input_bytes)
}

fn identity_valid(identity: &Identity) -> bool {
    [&identity.root, &identity.source_snapshot, &identity.graph_snapshot,
        &identity.evaluation_scope, &identity.configured_scope].into_iter().all(|s| field(s))
}

fn normalized(path: &str, comparison: PathComparison) -> Result<String, Unknown> {
    path_key(path, comparison).map_err(|_| Unknown::UnsupportedPathCase)
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct Context<'a> {
    project: String,
    framework: &'a str,
    configuration: &'a str,
    platform: &'a str,
    rid: &'a str,
}

fn context(target: &ProjectTarget, paths: PathComparison) -> Result<Context<'_>, Unknown> {
    Ok(Context {
        project: normalized(&target.project_path, paths)?,
        framework: &target.framework,
        configuration: &target.configuration,
        platform: &target.platform,
        rid: &target.runtime_identifier,
    })
}

struct CheckedGraph<'a> {
    index: GraphIndex<'a>,
    contexts: HashMap<Context<'a>, &'a ProjectTarget>,
    keys: HashMap<&'a str, Context<'a>>,
    members: HashMap<&'a str, HashSet<(String, Option<&'a str>)>>,
    references: HashSet<(&'a str, &'a str)>,
}

impl<'a> CheckedGraph<'a> {
    fn new(graph: &'a ProjectGraph, paths: PathComparison) -> Result<Self, Unknown> {
        // Fixed validation precedence keeps invalid-input results order independent.
        if graph.targets.iter().any(|t| {
            [&t.id, &t.project_path, &t.framework, &t.configuration, &t.platform,
                &t.sdk_version, &t.evaluation_fingerprint].into_iter().any(|s| !field(s))
                || t.runtime_identifier.contains('\0')
                || t.references.iter().any(|r| !field(r))
                || t.compile.iter().any(|m| !field(&m.path)
                    || m.link.as_deref().is_some_and(|link| !field(link)))
        }) {
            return Err(Unknown::InvalidGraph);
        }
        for target in &graph.targets {
            normalized(&target.project_path, paths)?;
            for member in &target.compile { normalized(&member.path, paths)?; }
        }
        let index = GraphIndex::new(graph, paths).map_err(|_| Unknown::InvalidGraph)?;
        let mut contexts = HashMap::new();
        let mut keys = HashMap::new();
        let mut members = HashMap::new();
        let mut references = HashSet::new();
        for target in &graph.targets {
            let key = context(target, paths)?;
            if contexts.insert(key.clone(), target).is_some() {
                return Err(Unknown::AmbiguousContext);
            }
            keys.insert(target.id.as_str(), key);
            let mut items = HashSet::new();
            for item in &target.compile {
                // Set membership, not duplicate MSBuild item multiplicity.
                items.insert((normalized(&item.path, paths)?, item.link.as_deref()));
            }
            members.insert(target.id.as_str(), items);
            for reference in &target.references {
                references.insert((target.id.as_str(), reference.as_str()));
            }
        }
        if references.iter().any(|(_, to)| !index.targets.contains_key(to)) {
            return Err(Unknown::InvalidGraph);
        }
        Ok(Self { index, contexts, keys, members, references })
    }

    fn contains(&self, target: &str, item: &CompileMember, paths: PathComparison)
        -> Result<bool, Unknown>
    {
        let key = (normalized(&item.path, paths)?, item.link.as_deref());
        Ok(self.members.get(target).is_some_and(|members| members.contains(&key)))
    }
}

fn declaration_valid(declaration: &Declaration, current: &Identity) -> bool {
    declaration.source_snapshot == current.source_snapshot && field(&declaration.provenance)
}

fn requests_valid(input: &Input<'_>, previous: &CheckedGraph<'_>) -> Result<(), Unknown> {
    let paths = input.current.identity.paths;
    let mut names = HashSet::new();
    let mut old_items = HashSet::new();
    let mut new_items = HashSet::new();
    if input.requests.iter().any(|r| !field(&r.key) || !field(&r.previous_target)
        || [&r.previous_item, &r.current_item].into_iter().any(|m| !field(&m.path)
            || m.link.as_deref().is_some_and(|link| !field(link))))
    {
        return Err(Unknown::InvalidRequests);
    }
    // Validate all path semantics before returning any mapping ambiguity.
    for request in input.requests {
        normalized(&request.previous_item.path, paths)?;
        normalized(&request.current_item.path, paths)?;
        if let Some(source) = &request.surviving_source { normalized(&source.path, paths)?; }
    }
    for request in input.requests {
        if !names.insert(request.key.as_str()) { return Err(Unknown::InvalidRequests); }
        if let Some(key) = previous.keys.get(request.previous_target.as_str()) {
            let old = (key, normalized(&request.previous_item.path, paths)?,
                request.previous_item.link.as_deref());
            let new = (key, normalized(&request.current_item.path, paths)?,
                request.current_item.link.as_deref());
            if !old_items.insert(old) || !new_items.insert(new) {
                return Err(Unknown::InvalidRequests);
            }
        }
    }
    Ok(())
}

fn witness(targets: Vec<String>, limit: usize) -> Witness {
    let omitted_nodes = targets.len().saturating_sub(limit);
    // Do not retain the full route's Vec capacity behind a short presentation.
    let mut retained = Vec::with_capacity(targets.len().min(limit));
    for target in targets.into_iter().take(limit) { retained.push(target); }
    Witness { targets: retained, omitted_nodes }
}

fn find_route(index: &GraphIndex<'_>, root: &str, tests: &HashSet<String>,
    used: &mut usize, limit: usize) -> Result<Option<Vec<String>>, Unknown>
{
    if tests.contains(root) { return Ok(Some(vec![root.to_owned()])); }
    let mut parents = HashMap::from([(root, None::<&str>)]);
    let mut pending = VecDeque::from([root]);
    while let Some(node) = pending.pop_front() {
        if let Some(dependents) = index.dependents.get(node) {
            for dependent in dependents {
                if *used == limit { return Err(Unknown::EdgeBudget); }
                *used += 1; // Charge even duplicate edges and already visited nodes.
                let next = dependent.as_str();
                if parents.contains_key(next) { continue; }
                parents.insert(next, Some(node));
                if tests.contains(next) {
                    let mut route = vec![next.to_owned()];
                    let mut at = next;
                    while let Some(Some(parent)) = parents.get(at) {
                        route.push((*parent).to_owned());
                        at = parent;
                    }
                    route.reverse();
                    return Ok(Some(route));
                }
                pending.push_back(next);
            }
        }
    }
    Ok(None)
}

fn first_missing(route: &[String], previous: &CheckedGraph<'_>, current: &CheckedGraph<'_>,
    current_tests: &HashSet<String>)
    -> Option<MissingFact>
{
    let mut prior_current: Option<&ProjectTarget> = None;
    for (position, old_id) in route.iter().enumerate() {
        let key = previous.keys.get(old_id.as_str())?;
        let Some(mapped) = current.contexts.get(key) else {
            return Some(MissingFact::Target { previous_target: old_id.clone() });
        };
        if let Some(lower) = prior_current {
            if !current.references.contains(&(mapped.id.as_str(), lower.id.as_str())) {
                return Some(MissingFact::Reference {
                    previous_referencing: old_id.clone(),
                    previous_referenced: route[position - 1].clone(),
                    current_referencing: mapped.id.clone(),
                    current_referenced: lower.id.clone(),
                });
            }
        }
        prior_current = Some(mapped);
        if position + 1 == route.len() && !mapped.is_test_project {
            return Some(MissingFact::TestClassification {
                previous_target: old_id.clone(), current_target: mapped.id.clone(),
            });
        }
        if position + 1 == route.len() && !current_tests.contains(&mapped.id) {
            return Some(MissingFact::ConfiguredMembership {
                previous_target: old_id.clone(), current_target: mapped.id.clone(),
            });
        }
    }
    None
}

fn compare_item<'a>(request: &'a Request, input: &Input<'_>, previous: &CheckedGraph<'_>,
    current: &CheckedGraph<'_>, old_tests: &HashSet<String>, new_tests: &HashSet<String>,
    used: &mut usize) -> ItemResult<'a>
{
    let mut result = ItemResult { request, outcome: Outcome::Unknown(Unknown::TargetNotComparable),
        previous_route: None, current_route: None, first_missing: None };
    let decision = (|| -> Result<Outcome, Unknown> {
        let paths = input.current.identity.paths;
        let old = previous.index.targets.get(request.previous_target.as_str())
            .ok_or(Unknown::TargetNotComparable)?;
        if !previous.contains(&old.id, &request.previous_item, paths)? {
            return Err(Unknown::PriorItemAbsent);
        }
        if let Some(retirement) = &request.retirement {
            return if declaration_valid(retirement, input.current.identity) {
                Ok(Outcome::Retired)
            } else { Err(Unknown::RetirementUnproven) };
        }
        let key = &previous.keys[old.id.as_str()];
        let now = current.contexts.get(key).ok_or(Unknown::TargetNotComparable)?;
        let source = request.surviving_source.as_ref().ok_or(Unknown::SurvivalUnproven)?;
        if source.source_snapshot != input.current.identity.source_snapshot
            || !field(&source.path)
            || normalized(&source.path, paths)? != normalized(&request.current_item.path, paths)?
            || source.sha256.len() != 64
            || !source.sha256.bytes().all(|b| b.is_ascii_hexdigit() && !b.is_ascii_lowercase())
        {
            return Err(Unknown::SurvivalUnproven);
        }
        let changed_item = normalized(&request.previous_item.path, paths)?
            != normalized(&request.current_item.path, paths)?
            || request.previous_item.link != request.current_item.link;
        if changed_item && !request.rename.as_ref()
            .is_some_and(|d| declaration_valid(d, input.current.identity))
        {
            return Err(Unknown::RenameUnproven);
        }
        if !current.contains(&now.id, &request.current_item, paths)? {
            result.first_missing = Some(MissingFact::CompileItem {
                previous_target: old.id.clone(), current_target: now.id.clone(),
            });
            return Ok(Outcome::CompileMembershipLost);
        }
        let Some(old_route) = find_route(&previous.index, &old.id, old_tests,
            used, input.limits.edge_visits)? else
        {
            return Ok(Outcome::NoPriorConfiguredRoute);
        };
        // Compute attribution on the entire old witness before presentation caps.
        let missing = first_missing(&old_route, previous, current, new_tests);
        result.previous_route = Some(witness(old_route, input.limits.witness_nodes));
        if let Some(new_route) = find_route(&current.index, &now.id, new_tests,
            used, input.limits.edge_visits)?
        {
            result.current_route = Some(witness(new_route, input.limits.witness_nodes));
            Ok(Outcome::Preserved)
        } else {
            result.first_missing = Some(missing.ok_or(Unknown::AttributionUnavailable)?);
            Ok(Outcome::LastConfiguredRouteLost)
        }
    })();
    result.outcome = decision.unwrap_or_else(Outcome::Unknown);
    result
}

/// Pure bounded advisory. Membership loss takes precedence; retained members
/// are checked for a prior route and then a current route. Only an exhausted
/// complete current frontier proves route absence. Retired/unknown/omitted work
/// is explicit; nothing here grants execution or changes selection/retirement.
pub fn check<'a>(input: &Input<'a>) -> Report<'a> {
    let mut report = Report { previous: input.previous.identity, current: input.current.identity,
        limits: input.limits, complete: false, problem: None, results: Vec::new(),
        omitted_results: input.requests.len(), edge_visits: 0 };
    let checked = (|| -> Result<_, Unknown> {
        preflight(input)?;
        let old_identity = input.previous.identity;
        let new_identity = input.current.identity;
        if !identity_valid(old_identity) || !identity_valid(new_identity)
            || old_identity != input.expected_previous || new_identity != input.expected_current
        {
            return Err(Unknown::CaptureIdentity);
        }
        if old_identity.root != new_identity.root
            || old_identity.evaluation_scope != new_identity.evaluation_scope
            || old_identity.configured_scope != new_identity.configured_scope
            || old_identity.paths != new_identity.paths
        {
            return Err(Unknown::ScopeMismatch);
        }
        if !input.previous.evaluation_sufficient || !input.current.evaluation_sufficient {
            return Err(Unknown::ExecutionItems);
        }
        let paths = new_identity.paths;
        let previous = CheckedGraph::new(input.previous.graph, paths)?;
        let current = CheckedGraph::new(input.current.graph, paths)?;
        // Provider IDs can be rebound to another configuration. Do not silently
        // interpret that as removing one context and adding another.
        if previous.keys.iter().any(|(id, key)| current.keys.get(id).is_some_and(|now| now != key)) {
            return Err(Unknown::ContextChanged);
        }
        if previous.contexts.iter().any(|(key, old)| current.contexts.get(key)
            .is_some_and(|now| old.sdk_version != now.sdk_version))
        {
            return Err(Unknown::ToolchainChanged);
        }
        requests_valid(input, &previous)?;
        let mut old_tests = HashSet::new();
        let mut new_tests = HashSet::new();
        for id in input.previous_configured_tests {
            let old = previous.index.targets.get(id.as_str())
                .filter(|target| target.is_test_project).ok_or(Unknown::InvalidConfiguredGroup)?;
            old_tests.insert(old.id.clone());
        }
        for id in input.current_configured_tests {
            let now = current.index.targets.get(id.as_str())
                .filter(|target| target.is_test_project).ok_or(Unknown::InvalidConfiguredGroup)?;
            new_tests.insert(now.id.clone());
        }
        Ok((previous, current, old_tests, new_tests))
    })();
    let (previous, current, old_tests, new_tests) = match checked {
        Ok(value) => value,
        Err(problem) => { report.problem = Some(problem); return report; }
    };
    let mut requests: Vec<_> = input.requests.iter().collect();
    requests.sort_by(|a, b| ordinal(&a.key, &b.key));
    for request in requests.into_iter().take(input.limits.results) {
        report.results.push(compare_item(request, input, &previous, &current,
            &old_tests, &new_tests, &mut report.edge_visits));
    }
    report.omitted_results -= report.results.len();
    report.complete = report.omitted_results == 0
        && report.results.iter().all(|r| !matches!(r.outcome, Outcome::Unknown(_)));
    report
}
