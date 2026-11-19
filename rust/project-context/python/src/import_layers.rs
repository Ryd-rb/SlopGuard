//! Opt-in comparison of two admitted, declared direct-import graphs.
//!
//! This module does not extract Python imports or resolve names. A trusted adapter
//! must bind every fact to original bytes and prove complete extraction, unique
//! regular-package roots, absence of shadowing/symlinks/path aliases, and any
//! `ProvenTypeOnly` binding. Unknowns must be supplied, never dropped. The separate
//! `import-producer` crate supplies a narrow opt-in original-byte adapter; project
//! package hints and native clone requests do not admit it. Digests bind observations;
//! their equality does not authenticate the adapter's assertions.
//! Without raw bytes this core checks digest format/equality and range bounds;
//! the adapter must verify actual hashes and original UTF-8 span boundaries.
//!
//! Edges describe declared imports, including optional/runtime conditional imports.
//! They do not prove execution or model implicit parent-package initialization.
//! A package initializer is the module `p`, not `p.__init__`. Runtime loader cycles,
//! external-module internals, and arbitrary Python resolution are outside this
//! contract. There is no filesystem access, execution, clock, or persistent cache.

use crate::digest::DigestInput;
use std::collections::{BTreeMap, BTreeSet, VecDeque};

pub const RULE_ID: &str = "python.import-layer-regressions/v1";
pub const SEMANTICS: &str = "declared-direct-imports; optional-included; proven-type-only-excluded; no-implicit-loader-edges";
const MAX_NAME: usize = 256;
const MAX_PATH: usize = 1_024;
const MAX_ROOTS: usize = 128;
const MAX_PREFIXES: usize = 128;
const MAX_SOURCE_BYTES: u32 = 4 * 1024 * 1024;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SnapshotIdentity {
    pub revision: String,
    pub source_sha256: String,
    /// Stable digest of extractor/schema, Python target, resolution environment,
    /// root-admission rules and import semantics; excludes the source revision.
    pub context_sha256: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SnapshotPair {
    pub before: SnapshotIdentity,
    pub after: SnapshotIdentity,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AdmittedRoot {
    /// A single top-level ASCII Python package identifier, uniquely admitted.
    pub package: String,
    /// Canonical ASCII workspace-relative POSIX directory, or `.`.
    pub source_root: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Module {
    pub name: String,
    pub path: String,
    pub source_sha256: String,
    pub byte_len: u32,
    pub is_package: bool,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct Span {
    pub start: u32,
    pub end: u32,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum UnsupportedKind {
    DynamicImport,
    AmbiguousReexport,
    NamespacePackage,
    ShadowedRoot,
    PathAlias,
    UnsupportedSyntax,
    UnsupportedName,
    UnresolvedGuard,
    UnresolvedTarget,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ImportTarget {
    Internal(String),
    /// Independently resolved outside the admitted module universe.
    External,
    Unknown(UnsupportedKind),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ImportUse {
    Runtime,
    Optional,
    /// Requires a bounded binding proof, not just `TYPE_CHECKING` spelling.
    ProvenTypeOnly,
    UnresolvedGuard,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DirectImport {
    pub importer: String,
    pub source_sha256: String,
    pub target: ImportTarget,
    pub use_kind: ImportUse,
    /// Nonempty half-open range in the importer's original bytes, including BOM.
    pub span: Span,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct UnsupportedFact {
    pub module: Option<String>,
    pub span: Option<Span>,
    pub kind: UnsupportedKind,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ImportSnapshot {
    pub identity: SnapshotIdentity,
    /// Certifies all declared-import facts in the admitted universe were covered.
    /// This is stronger than project-context's `Inventory::Complete`.
    pub complete: bool,
    pub roots: Vec<AdmittedRoot>,
    pub modules: Vec<Module>,
    pub imports: Vec<DirectImport>,
    pub unsupported: Vec<UnsupportedFact>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LayerPolicy {
    /// Highest layer first. Prefixes must be globally disjoint at dot boundaries.
    /// Same-layer imports are allowed. Unassigned modules remain traversable.
    /// A prefix may match no current module, allowing declarations for future
    /// modules; callers must review policy spelling and coverage explicitly.
    /// Cycles are checked on every admitted module, regardless of layer assignment.
    pub layers: Vec<Vec<String>>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Limits {
    /// Combined count across both snapshots, not a per-snapshot allowance.
    pub max_modules: usize,
    pub max_imports: usize,
    /// Sum of input string bytes, including expectations, roots and policy.
    pub max_input_bytes: usize,
    pub max_work: u64,
    pub max_findings: usize,
    pub max_witness_edges: usize,
    pub max_frontiers: usize,
}

impl Limits {
    pub const fn hard_maxima() -> Self {
        Self {
            max_modules: 4_096, max_imports: 32_768, max_input_bytes: 8 * 1024 * 1024,
            max_work: 20_000_000, max_findings: 64, max_witness_edges: 128,
            max_frontiers: 64,
        }
    }

    fn valid(self) -> bool {
        let hard = Self::hard_maxima();
        (1..=hard.max_modules).contains(&self.max_modules)
            && (1..=hard.max_imports).contains(&self.max_imports)
            && (1..=hard.max_input_bytes).contains(&self.max_input_bytes)
            && (1..=hard.max_work).contains(&self.max_work)
            && (1..=hard.max_findings).contains(&self.max_findings)
            && (1..=hard.max_witness_edges).contains(&self.max_witness_edges)
            && (1..=hard.max_frontiers).contains(&self.max_frontiers)
    }
}

impl Default for Limits {
    fn default() -> Self {
        Self { max_work: 1_000_000, max_findings: 32, max_witness_edges: 64,
            max_frontiers: 32, ..Self::hard_maxima() }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Usage {
    pub input_bytes: usize,
    pub modules: usize,
    pub imports: usize,
    /// Raw fact counts across both snapshots, before semantic deduplication;
    /// zero if record-count preflight rejected the input before inspecting facts.
    pub excluded_type_only: usize,
    pub external_imports: usize,
    pub optional_imports: usize,
    pub work: u64,
    pub bfs: u64,
    pub visited_nodes: u64,
    pub visited_edges: u64,
    pub pairs: u64,
    pub witness_edges: u64,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum SnapshotSide { Before, After, Both }

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum LimitKind { Work, Findings, WitnessEdges }

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum Reason {
    InvalidLimits,
    InputLimit,
    InvalidInput,
    InvalidPolicy,
    IncompleteSnapshot,
    StaleSnapshot,
    IncompatibleContext,
    RootMappingChanged,
    Unsupported(UnsupportedKind),
    Limit(LimitKind),
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct Frontier {
    pub side: SnapshotSide,
    pub reason: Reason,
    pub module: Option<String>,
    pub span: Option<Span>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ComparisonStatus { Complete, Unknown }

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FindingKind { ForbiddenPath, Cycle }

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WitnessEdge {
    pub from: String,
    pub to: String,
    pub path: String,
    pub source_sha256: String,
    pub span: Span,
    /// True exactly when this semantic ordered pair is absent from the baseline.
    pub added: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Finding {
    pub kind: FindingKind,
    /// For cycles, source/target identify the added edge anchoring the witness.
    pub source: String,
    pub target: String,
    pub witness: Vec<WitnessEdge>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Comparison {
    pub rule_id: &'static str,
    pub semantics: &'static str,
    pub status: ComparisonStatus,
    /// Observed identities; may be present on a stale/unknown result. They do not
    /// certify equality to `expected` or authorize publication as current.
    pub binding: Option<SnapshotPair>,
    pub policy_sha256: Option<String>,
    pub findings: Vec<Finding>,
    pub frontiers: Vec<Frontier>,
    /// Number of distinct known frontier records omitted by the output cap.
    pub omitted_frontiers: usize,
    /// Requested limits are effective only when `limits_accepted` is true.
    pub limits: Limits,
    pub limits_accepted: bool,
    pub usage: Usage,
}

/// Compare two immutable observations against independently supplied expectations.
/// Input errors/frontiers produce no findings. Exhaustion during graph analysis
/// retains earlier proven findings and returns `Unknown`, never a clean verdict.
/// Complete means complete for the declared graph/explicit policy, not Python
/// runtime correctness. No result is automatically enabled or published.
pub fn compare(
    before: &ImportSnapshot, after: &ImportSnapshot, expected: &SnapshotPair,
    policy: &LayerPolicy, limits: Limits,
) -> Comparison {
    let mut result = Comparison {
        rule_id: RULE_ID, semantics: SEMANTICS, status: ComparisonStatus::Complete,
        binding: None, policy_sha256: None, findings: Vec::new(), frontiers: Vec::new(),
        omitted_frontiers: 0, limits, limits_accepted: limits.valid(), usage: Usage::default(),
    };
    if !result.limits_accepted {
        result.status = ComparisonStatus::Unknown;
        result.frontiers.push(gap(SnapshotSide::Both, Reason::InvalidLimits));
        return result;
    }
    if !preflight(before, after, expected, policy, limits, &mut result.usage) {
        finish_frontiers(&mut result, BTreeSet::from([gap(SnapshotSide::Both, Reason::InputLimit)]));
        return result;
    }
    let mut gaps = BTreeSet::new();
    let before_valid = validate_snapshot(before);
    let after_valid = validate_snapshot(after);
    for (snapshot, side, valid) in [
        (before, SnapshotSide::Before, before_valid),
        (after, SnapshotSide::After, after_valid),
    ] {
        if !valid { gaps.insert(gap(side, Reason::InvalidInput)); }
        if snapshot.modules.iter().any(|module| !module_name(&module.name) || !module.path.is_ascii())
            || snapshot.roots.iter().any(|root| !identifier(&root.package) || !root.source_root.is_ascii())
        { gaps.insert(gap(side, Reason::Unsupported(UnsupportedKind::UnsupportedName))); }
        if !snapshot.complete { gaps.insert(gap(side, Reason::IncompleteSnapshot)); }
    }
    if !valid_identity(&expected.before) || !valid_identity(&expected.after) {
        gaps.insert(gap(SnapshotSide::Both, Reason::InvalidInput));
    }
    if before.identity != expected.before { gaps.insert(gap(SnapshotSide::Before, Reason::StaleSnapshot)); }
    if after.identity != expected.after { gaps.insert(gap(SnapshotSide::After, Reason::StaleSnapshot)); }
    if before.identity.context_sha256 != after.identity.context_sha256 {
        gaps.insert(gap(SnapshotSide::Both, Reason::IncompatibleContext));
    }
    if before_valid { collect_frontiers(before, SnapshotSide::Before, &mut gaps); }
    if after_valid { collect_frontiers(after, SnapshotSide::After, &mut gaps); }
    if before_valid && after_valid {
        result.binding = Some(SnapshotPair { before: before.identity.clone(), after: after.identity.clone() });
        if roots(before) != roots(after) { gaps.insert(gap(SnapshotSide::Both, Reason::RootMappingChanged)); }
        if cross_snapshot_alias(before, after) {
            gaps.insert(gap(SnapshotSide::Both, Reason::Unsupported(UnsupportedKind::PathAlias)));
        }
    }
    let prefixes = policy_prefixes(policy, &before.roots);
    if prefixes.is_none() { gaps.insert(gap(SnapshotSide::Both, Reason::InvalidPolicy)); }
    if !gaps.is_empty() {
        finish_frontiers(&mut result, gaps);
        return result;
    }
    let Some(prefixes) = prefixes else { return result; };
    let mut digest = DigestInput::new("slophawk/python/import-layers/policy/v1");
    digest.append(&(policy.layers.len() as u64).to_be_bytes());
    for (prefix, layer) in &prefixes {
        digest.append(prefix.as_bytes());
        digest.append(&(*layer as u64).to_be_bytes());
    }
    result.policy_sha256 = Some(digest.finish());
    let names: Vec<_> = before.modules.iter().chain(&after.modules)
        .map(|module| module.name.as_str()).collect::<BTreeSet<_>>().into_iter().collect();
    let ids: BTreeMap<_, _> = names.iter().enumerate().map(|(id, name)| (*name, id)).collect();
    let old = Graph::new(before, &ids);
    let new = Graph::new(after, &ids);
    let ranks: Vec<_> = names.iter().map(|name| prefixes.iter()
        .find_map(|(prefix, layer)| under(name, prefix).then_some(*layer))).collect();
    if let Err(reason) = compare_graphs(&old, &new, &names, &ranks, &mut result) {
        finish_frontiers(&mut result, BTreeSet::from([gap(SnapshotSide::Both, reason)]));
    }
    result
}

fn gap(side: SnapshotSide, reason: Reason) -> Frontier {
    Frontier { side, reason, module: None, span: None }
}

fn finish_frontiers(result: &mut Comparison, gaps: BTreeSet<Frontier>) {
    result.status = ComparisonStatus::Unknown;
    result.omitted_frontiers = gaps.len().saturating_sub(result.limits.max_frontiers);
    result.frontiers = gaps.into_iter().take(result.limits.max_frontiers).collect();
}

// Preflight is allocation-free. All structural work after it is bounded by these
// record/byte caps, including validation and ordered-map construction; those steps
// are not counted as graph work. Caller-owned input allocations are outside our cap.
fn preflight(
    before: &ImportSnapshot, after: &ImportSnapshot, expected: &SnapshotPair,
    policy: &LayerPolicy, limits: Limits, usage: &mut Usage,
) -> bool {
    usage.modules = before.modules.len().saturating_add(after.modules.len());
    usage.imports = before.imports.len().saturating_add(after.imports.len());
    if usage.modules > limits.max_modules || usage.imports > limits.max_imports
        || before.roots.len().saturating_add(after.roots.len()) > MAX_ROOTS
        || before.unsupported.len().saturating_add(after.unsupported.len()) > Limits::hard_maxima().max_imports
        || policy.layers.len() > 32
        || policy.layers.iter().map(Vec::len).fold(0usize, usize::saturating_add) > MAX_PREFIXES
    { return false; }
    let mut fields_bounded = true;
    let mut count = |text: &str, max: usize| {
        usage.input_bytes = usage.input_bytes.saturating_add(text.len());
        fields_bounded &= text.len() <= max;
    };
    for identity in [&before.identity, &after.identity, &expected.before, &expected.after] {
        count(&identity.revision, 160); count(&identity.source_sha256, 64); count(&identity.context_sha256, 64);
    }
    for snapshot in [before, after] {
        for root in &snapshot.roots { count(&root.package, MAX_NAME); count(&root.source_root, MAX_PATH); }
        for module in &snapshot.modules { count(&module.name, MAX_NAME); count(&module.path, MAX_PATH); count(&module.source_sha256, 64); }
        for import in &snapshot.imports {
            count(&import.importer, MAX_NAME); count(&import.source_sha256, 64);
            if let ImportTarget::Internal(target) = &import.target { count(target, MAX_NAME); }
        }
        for unsupported in &snapshot.unsupported {
            if let Some(module) = &unsupported.module { count(module, MAX_NAME); }
        }
    }
    for prefix in policy.layers.iter().flatten() { count(prefix, MAX_NAME); }
    for import in before.imports.iter().chain(&after.imports) {
        usage.excluded_type_only += usize::from(import.use_kind == ImportUse::ProvenTypeOnly);
        usage.optional_imports += usize::from(import.use_kind == ImportUse::Optional);
        usage.external_imports += usize::from(matches!(&import.target, ImportTarget::External));
    }
    fields_bounded && usage.input_bytes <= limits.max_input_bytes
}

fn valid_text(text: &str, max: usize) -> bool {
    !text.is_empty() && text.len() <= max && !text.chars().any(char::is_control)
}

fn sha256(text: &str) -> bool {
    text.len() == 64 && text.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

fn valid_identity(identity: &SnapshotIdentity) -> bool {
    valid_text(&identity.revision, 160) && sha256(&identity.source_sha256) && sha256(&identity.context_sha256)
}

fn identifier(text: &str) -> bool {
    let mut bytes = text.bytes();
    bytes.next().is_some_and(|b| b.is_ascii_alphabetic() || b == b'_')
        && bytes.all(|b| b.is_ascii_alphanumeric() || b == b'_')
}

fn module_name(name: &str) -> bool {
    name.len() <= MAX_NAME && name.split('.').all(|part| identifier(part) && part != "__init__")
}

fn path(path: &str, directory: bool) -> bool {
    (directory && path == ".") || (valid_text(path, MAX_PATH) && path.is_ascii()
        && !path.contains('\\') && !path.contains(':') && !path.starts_with('/')
        && path.split('/').all(|part| !part.is_empty() && part != "." && part != ".."))
}

fn under(name: &str, prefix: &str) -> bool {
    name == prefix || name.strip_prefix(prefix).is_some_and(|suffix| suffix.starts_with('.'))
}

fn valid_span(span: Span, module: &Module) -> bool {
    span.start < span.end && span.end <= module.byte_len
}

fn roots(snapshot: &ImportSnapshot) -> BTreeMap<&str, &str> {
    snapshot.roots.iter().map(|root| (root.package.as_str(), root.source_root.as_str())).collect()
}

fn cross_snapshot_alias(before: &ImportSnapshot, after: &ImportSnapshot) -> bool {
    let mut paths = BTreeMap::new();
    let mut names = BTreeMap::new();
    for module in before.modules.iter().chain(&after.modules) {
        if names.insert(module.name.to_ascii_lowercase(), module.name.as_str())
            .is_some_and(|previous| previous != module.name.as_str())
        { return true; }
        if paths.insert(module.path.to_ascii_lowercase(), module.name.as_str())
            .is_some_and(|previous| previous != module.name.as_str())
        { return true; }
    }
    false
}

fn validate_snapshot(snapshot: &ImportSnapshot) -> bool {
    // Reject unbounded individual fields before creating maps or derived paths.
    if !valid_identity(&snapshot.identity) || snapshot.roots.is_empty()
        || snapshot.roots.iter().any(|root| !identifier(&root.package)
            || root.package.len() > MAX_NAME || !path(&root.source_root, true))
        || snapshot.modules.iter().any(|module| !module_name(&module.name)
            || !path(&module.path, false) || !sha256(&module.source_sha256)
            || module.byte_len > MAX_SOURCE_BYTES)
        || snapshot.imports.iter().any(|import| !module_name(&import.importer)
            || !sha256(&import.source_sha256)
            || matches!(&import.target, ImportTarget::Internal(target) if !module_name(target)))
        || snapshot.unsupported.iter().any(|fact| fact.module.as_ref()
            .is_some_and(|name| !valid_text(name, MAX_NAME)))
    { return false; }
    let mut valid = true;
    let mut root_map = BTreeMap::new();
    for root in &snapshot.roots {
        valid &= root_map.insert(root.package.as_str(), root.source_root.as_str()).is_none();
    }
    let mut modules = BTreeMap::new();
    let mut paths = BTreeSet::new();
    for module in &snapshot.modules {
        valid &= modules.insert(module.name.as_str(), module).is_none();
        // Reject ASCII case aliases on every host; no filesystem canonicalization.
        valid &= paths.insert(module.path.to_ascii_lowercase());
        let top = module.name.split('.').next().unwrap_or("");
        if let Some(source_root) = root_map.get(top) {
            let prefix = if *source_root == "." { String::new() } else { format!("{source_root}/") };
            let suffix = if module.is_package { "/__init__.py" } else { ".py" };
            valid &= module.path == format!("{prefix}{}{suffix}", module.name.replace('.', "/"));
        } else { valid = false; }
    }
    for root in &snapshot.roots {
        valid &= modules.get(root.package.as_str()).is_some_and(|module| module.is_package);
    }
    for module in &snapshot.modules {
        let mut name = module.name.as_str();
        while let Some((parent, _)) = name.rsplit_once('.') {
            valid &= modules.get(parent).is_some_and(|module| module.is_package);
            name = parent;
        }
    }
    for import in &snapshot.imports {
        if let Some(module) = modules.get(import.importer.as_str()) {
            valid &= import.source_sha256 == module.source_sha256 && valid_span(import.span, module);
        } else { valid = false; }
        if let ImportTarget::Internal(target) = &import.target {
            valid &= modules.contains_key(target.as_str());
        }
    }
    for unsupported in &snapshot.unsupported {
        if let Some(name) = &unsupported.module { valid &= valid_text(name, MAX_NAME); }
        if let Some(span) = unsupported.span {
            valid &= unsupported.module.as_deref().and_then(|name| modules.get(name))
                .is_some_and(|module| valid_span(span, module));
        }
    }
    valid
}

fn policy_prefixes<'a>(policy: &'a LayerPolicy, roots: &[AdmittedRoot]) -> Option<BTreeMap<&'a str, usize>> {
    let mut valid = !policy.layers.is_empty();
    let mut prefixes = BTreeMap::new();
    for (layer, names) in policy.layers.iter().enumerate() {
        valid &= !names.is_empty();
        for name in names {
            valid &= module_name(name) && roots.iter().any(|root| under(name, &root.package));
            valid &= prefixes.insert(name.as_str(), layer).is_none();
        }
    }
    for a in prefixes.keys() {
        for b in prefixes.keys() {
            if a != b && under(a, b) { valid = false; }
        }
    }
    valid.then_some(prefixes)
}

fn collect_frontiers(snapshot: &ImportSnapshot, side: SnapshotSide, gaps: &mut BTreeSet<Frontier>) {
    for unsupported in &snapshot.unsupported {
        gaps.insert(Frontier { side, reason: Reason::Unsupported(unsupported.kind),
            module: unsupported.module.clone(), span: unsupported.span });
    }
    for import in &snapshot.imports {
        let target = match &import.target { ImportTarget::Unknown(kind) => Some(*kind), _ => None };
        let guard = (import.use_kind == ImportUse::UnresolvedGuard).then_some(UnsupportedKind::UnresolvedGuard);
        for kind in [target, guard].into_iter().flatten() {
            gaps.insert(Frontier { side, reason: Reason::Unsupported(kind),
                module: Some(import.importer.clone()), span: Some(import.span) });
        }
    }
}

struct Graph<'a> {
    next: Vec<Vec<usize>>,
    edges: BTreeMap<(usize, usize), &'a DirectImport>,
    modules: BTreeMap<&'a str, &'a Module>,
}

impl<'a> Graph<'a> {
    fn new(snapshot: &'a ImportSnapshot, ids: &BTreeMap<&str, usize>) -> Self {
        let mut graph = Self { next: vec![Vec::new(); ids.len()], edges: BTreeMap::new(),
            modules: snapshot.modules.iter().map(|module| (module.name.as_str(), module)).collect() };
        for import in &snapshot.imports {
            if import.use_kind == ImportUse::ProvenTypeOnly { continue; }
            if let ImportTarget::Internal(target) = &import.target {
                let key = (ids[import.importer.as_str()], ids[target.as_str()]);
                match graph.edges.get(&key) {
                    Some(previous) if previous.span <= import.span => {}
                    _ => { graph.edges.insert(key, import); }
                }
            }
        }
        for &(source, target) in graph.edges.keys() { graph.next[source].push(target); }
        graph
    }
}

struct Search {
    seen: Vec<bool>,
    parent: Vec<Option<usize>>,
    positive_cycle: bool,
}

fn charge(usage: &mut Usage, limits: Limits, count: u64) -> Result<(), Reason> {
    if count > limits.max_work.saturating_sub(usage.work) { return Err(Reason::Limit(LimitKind::Work)); }
    usage.work += count;
    Ok(())
}

fn bfs(graph: &Graph<'_>, source: usize, usage: &mut Usage, limits: Limits) -> Result<Search, Reason> {
    charge(usage, limits, 2 * graph.next.len() as u64)?;
    usage.bfs += 1;
    let mut search = Search { seen: vec![false; graph.next.len()], parent: vec![None; graph.next.len()], positive_cycle: false };
    let mut queue = VecDeque::from([source]);
    search.seen[source] = true;
    while let Some(node) = queue.pop_front() {
        charge(usage, limits, 1)?;
        usage.visited_nodes += 1;
        for &next in &graph.next[node] {
            charge(usage, limits, 1)?;
            usage.visited_edges += 1;
            if next == source { search.positive_cycle = true; }
            if !search.seen[next] {
                search.seen[next] = true;
                search.parent[next] = Some(node);
                queue.push_back(next);
            }
        }
    }
    Ok(search)
}

fn route(source: usize, target: usize, search: &Search, usage: &mut Usage, limits: Limits) -> Result<Vec<(usize, usize)>, Reason> {
    let mut edges = Vec::new();
    let mut node = target;
    while node != source {
        if edges.len() == limits.max_witness_edges { return Err(Reason::Limit(LimitKind::WitnessEdges)); }
        charge(usage, limits, 1)?;
        let parent = search.parent[node].ok_or(Reason::InvalidInput)?;
        edges.push((parent, node));
        node = parent;
    }
    charge(usage, limits, edges.len() as u64)?;
    edges.reverse();
    Ok(edges)
}

fn emit(
    kind: FindingKind, endpoints: (usize, usize), route: Vec<(usize, usize)>,
    graphs: (&Graph<'_>, &Graph<'_>), names: &[&str], result: &mut Comparison,
) -> Result<(), Reason> {
    let (source, target) = endpoints;
    let (old, new) = graphs;
    if result.findings.len() == result.limits.max_findings { return Err(Reason::Limit(LimitKind::Findings)); }
    if route.len() > result.limits.max_witness_edges { return Err(Reason::Limit(LimitKind::WitnessEdges)); }
    let mut witness = Vec::new();
    let mut has_added = false;
    for (from, to) in route {
        charge(&mut result.usage, result.limits, 1)?;
        result.usage.witness_edges += 1;
        let fact = new.edges.get(&(from, to)).ok_or(Reason::InvalidInput)?;
        let module = new.modules.get(names[from]).ok_or(Reason::InvalidInput)?;
        let added = !old.edges.contains_key(&(from, to));
        has_added |= added;
        witness.push(WitnessEdge { from: names[from].into(), to: names[to].into(),
            path: module.path.clone(), source_sha256: module.source_sha256.clone(),
            span: fact.span, added });
    }
    // Reachability novelty implies an added pair; retain an explicit invariant check.
    if !has_added { return Err(Reason::InvalidInput); }
    result.findings.push(Finding { kind, source: names[source].into(), target: names[target].into(), witness });
    Ok(())
}

// No transitive closure or simple-cycle enumeration is retained. Two BFS searches
// per source serve all forbidden endpoints; changed-edge cycle checks use bounded
// additional searches. Worst case O((V+A)(V+E) + V^2), each graph operation charged.
// Ordered-map lookup adds log(E) overhead. Input maps and at most three live BFS
// results use O(V+E) storage, plus bounded frontiers and capped findings/witnesses.
// Frontier staging is O(U+I), for supplied unsupported records U and import
// occurrences I, before the output cap; no recursive traversal is used.
fn compare_graphs(
    old: &Graph<'_>, new: &Graph<'_>, names: &[&str], ranks: &[Option<usize>], result: &mut Comparison,
) -> Result<(), Reason> {
    let mut added_sources = BTreeSet::new();
    for edge in new.edges.keys() {
        charge(&mut result.usage, result.limits, 1)?;
        result.usage.pairs += 1;
        if !old.edges.contains_key(edge) { added_sources.insert(edge.0); }
    }
    // Deletions, moved source spans and byte-only edits cannot introduce a graph
    // relation under a fixed policy. This exact delta check needs no traversal.
    if added_sources.is_empty() { return Ok(()); }
    for source in 0..names.len() {
        charge(&mut result.usage, result.limits, 1)?;
        if new.next[source].is_empty() { continue; }
        if (ranks[source].is_none() || ranks[source] == Some(0))
            && !added_sources.contains(&source)
        { continue; }
        let before = bfs(old, source, &mut result.usage, result.limits)?;
        let after = bfs(new, source, &mut result.usage, result.limits)?;
        for target in 0..names.len() {
            charge(&mut result.usage, result.limits, 1)?;
            result.usage.pairs += 1;
            if ranks[source].zip(ranks[target]).is_some_and(|(from, to)| from > to)
                && after.seen[target] && !before.seen[target]
            {
                let path = route(source, target, &after, &mut result.usage, result.limits)?;
                emit(FindingKind::ForbiddenPath, (source, target), path, (old, new), names, result)?;
            }
        }
        for &target in &new.next[source] {
            charge(&mut result.usage, result.limits, 1)?;
            result.usage.pairs += 1;
            if old.edges.contains_key(&(source, target)) { continue; }
            if source == target {
                // An added self-edge inside ANY old cycle is pre-existing debt.
                if !before.positive_cycle {
                    emit(FindingKind::Cycle, (source, target), vec![(source, target)], (old, new), names, result)?;
                }
                continue;
            }
            if before.seen[target] {
                let reverse_before = bfs(old, target, &mut result.usage, result.limits)?;
                if reverse_before.seen[source] { continue; }
            }
            let reverse_after = bfs(new, target, &mut result.usage, result.limits)?;
            if reverse_after.seen[source] {
                let reverse = route(target, source, &reverse_after, &mut result.usage, result.limits)?;
                if reverse.len() >= result.limits.max_witness_edges { return Err(Reason::Limit(LimitKind::WitnessEdges)); }
                charge(&mut result.usage, result.limits, reverse.len() as u64 + 1)?;
                let mut path = vec![(source, target)];
                path.extend(reverse);
                emit(FindingKind::Cycle, (source, target), path, (old, new), names, result)?;
            }
        }
    }
    Ok(())
}
