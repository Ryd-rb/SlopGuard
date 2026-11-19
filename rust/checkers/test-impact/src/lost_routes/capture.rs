//! Pure integrity seam for owner-collected evaluated captures, version 1.
//!
//! This is a typed API, not a wire parser or a collector. The owner must retain
//! expectations independently of the candidate, qualify evaluation-only inputs,
//! enumerate the entire supported universe and affected items, and observe an
//! immutable complete input snapshot. Equal before/after stamps do not rule out
//! ABA or mixed reads. Hashes bind supplied facts; they do not authenticate their
//! producer or establish semantic completeness. No CLI/MCP path uses this seam.
//! Canonical physical path/symlink identity is also a collector responsibility;
//! lexical path validation below cannot inspect the filesystem or resolve aliases.

use super::{Capture, CompileMember, Declaration, Identity, Input, Limits, PathComparison,
    ProjectGraph, Report, Request, SurvivingSource, Unknown, ordinal};
use crate::sha256::hash_upper_hex;
use std::collections::{HashMap, HashSet};

pub const VERSION: u32 = 1;
pub const MAX_TARGETS: usize = 128;
pub const MAX_MEMBERS: usize = 20_000;
pub const MAX_REFERENCES: usize = 8_192;
pub const MAX_FILES: usize = 20_000;
pub const MAX_FILE_BYTES: usize = 2 * 1024 * 1024;
pub const MAX_OBSERVATION_BYTES: usize = 8 * 1024 * 1024;
pub const MAX_GRAPH_BYTES: usize = 4 * 1024 * 1024;
pub const MAX_STRING_BYTES: usize = 2_048;
pub const MAX_REQUESTS: usize = 256;
pub const MAX_PROPERTIES: usize = 256;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Stamp {
    pub root: String,
    /// Owner token for a sealed, complete immutable input snapshot; not HEAD alone.
    pub source_snapshot: String,
    /// Owner-retained commit/tree/worktree context, including dirty state if allowed.
    pub git_identity: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Property { pub name: String, pub value: String }

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EvaluationKey {
    pub sdk_version: String,
    pub provider_sha256: String,
    /// Full resolved host, SDK, MSBuild, resolver and imported toolchain closure.
    pub toolchain_sha256: String,
    pub environment_sha256: String,
    /// External non-workspace inputs. Changing workspace inputs belong to Stamp.
    pub external_inputs_sha256: String,
    /// Full explicit global-property map, not the evaluator's selected properties.
    pub global_properties: Vec<Property>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TargetContext {
    pub project_path: String,
    pub framework: String,
    pub configuration: String,
    pub platform: String,
    pub runtime_identifier: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConfiguredGroup { AllEvaluatedTests }

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SupportPolicy {
    /// Independently retained owner qualification, including universe/group rules.
    pub policy_sha256: String,
    pub immutable_inputs: bool,
    /// Relevant execution-created items/references are excluded by qualified policy.
    pub evaluation_only: bool,
    pub paths: PathComparison,
    pub configured_group: ConfiguredGroup,
}

#[derive(Clone, Debug)]
pub struct CapturedEvaluation<'a> {
    pub version: u32,
    pub graph: &'a ProjectGraph,
    pub graph_sha256: String,
    pub before: Stamp,
    pub after: Stamp,
    pub evaluation: EvaluationKey,
    pub complete: bool,
}

/// Trusted collector record. Copying these fields from a candidate is not trust.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExpectedCapture {
    pub stamp: Stamp,
    pub policy_sha256: String,
    pub graph_sha256: String,
    pub observation_sha256: String,
    pub evaluation: EvaluationKey,
    /// Full owner-declared evaluated universe at this snapshot, one target/project.
    pub targets: Vec<TargetContext>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SourceFile<'a> { pub path: String, pub bytes: &'a [u8] }

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SourceObservation<'a> {
    pub stamp: Stamp,
    /// Independently observed physical files, including survivors absent from Compile.
    pub files: Vec<SourceFile<'a>>,
}

/// Trusted owner plan; supplying this declares the exact predecessor tuple supported.
/// Complete affected-item enumeration and declaration authority remain owner duties.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OwnerRequest {
    pub key: String,
    pub previous_target: String,
    pub previous_item: CompileMember,
    pub current_item: CompileMember,
    pub rename: Option<Declaration>,
    pub retirement: Option<Declaration>,
}

pub struct VerificationInput<'a> {
    pub previous: CapturedEvaluation<'a>,
    pub current: CapturedEvaluation<'a>,
    pub expected_previous: Option<&'a ExpectedCapture>,
    pub expected_current: Option<&'a ExpectedCapture>,
    pub policy: Option<&'a SupportPolicy>,
    pub previous_sources: Option<&'a SourceObservation<'a>>,
    pub current_sources: Option<&'a SourceObservation<'a>>,
    pub requests: &'a [OwnerRequest],
    pub limits: Limits,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Unavailable {
    UnsupportedSchema, MissingExpectation, MissingPolicy, MissingObservation,
    UnsupportedPolicy, ImmutableInputsRequired, ExecutionItems, StaleCapture,
    InvalidEvidence, GraphMismatch, ObservationMismatch, EvaluationMismatch,
    UniverseMismatch, MissingSource, InvalidDeclaration, InputLimit, PartialGraph,
    Core(Unknown),
}

struct Budget { used: usize, maximum: usize }
impl Budget {
    fn new(maximum: usize) -> Self { Self { used: 0, maximum } }
    fn add(&mut self, amount: usize) -> Result<(), Unavailable> {
        self.used = self.used.checked_add(amount).filter(|n| *n <= self.maximum)
            .ok_or(Unavailable::InputLimit)?;
        Ok(())
    }
    fn text(&mut self, value: &str, empty: bool) -> Result<(), Unavailable> {
        if value.len() > MAX_STRING_BYTES { return Err(Unavailable::InputLimit); }
        if (!empty && value.is_empty()) || value.contains('\0') {
            return Err(Unavailable::InvalidEvidence);
        }
        self.add(8 + value.len())
    }
}

fn digest(value: &str) -> Result<(), Unavailable> {
    if value.len() == 64 && value.bytes().all(|b| b.is_ascii_hexdigit() && !b.is_ascii_lowercase()) {
        Ok(())
    } else { Err(Unavailable::InvalidEvidence) }
}

fn path(value: &str) -> Result<(), Unavailable> {
    if value.is_empty() || value.contains('\\') || value.contains(':') || value.chars().any(char::is_control)
        || value.split('/').any(|part| matches!(part, "" | "." | ".."))
    { Err(Unavailable::InvalidEvidence) } else { Ok(()) }
}

fn stamp(value: &Stamp, budget: &mut Budget) -> Result<(), Unavailable> {
    for s in [&value.root, &value.source_snapshot, &value.git_identity] { budget.text(s, false)?; }
    Ok(())
}

fn member(value: &CompileMember, budget: &mut Budget) -> Result<(), Unavailable> {
    budget.text(&value.path, false)?; path(&value.path)?;
    budget.add(1)?;
    if let Some(link) = &value.link { budget.text(link, false)?; path(link)?; }
    Ok(())
}

fn key_preflight(key: &EvaluationKey, budget: &mut Budget) -> Result<(), Unavailable> {
    if key.global_properties.len() > MAX_PROPERTIES { return Err(Unavailable::InputLimit); }
    budget.text(&key.sdk_version, false)?;
    for s in [&key.provider_sha256, &key.toolchain_sha256, &key.environment_sha256,
        &key.external_inputs_sha256] { budget.text(s, false)?; digest(s)?; }
    for p in &key.global_properties {
        budget.text(&p.name, false)?; budget.text(&p.value, true)?;
        if !p.name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_') {
            return Err(Unavailable::InvalidEvidence);
        }
    }
    Ok(())
}

fn graph_preflight(graph: &ProjectGraph) -> Result<(), Unavailable> {
    if !graph.complete() { return Err(Unavailable::PartialGraph); }
    if graph.targets.len() > MAX_TARGETS { return Err(Unavailable::InputLimit); }
    let mut budget = Budget::new(MAX_GRAPH_BYTES);
    let mut members = Budget::new(MAX_MEMBERS);
    let mut references = Budget::new(MAX_REFERENCES);
    budget.add(64 + graph.targets.len() * 17)?;
    for t in &graph.targets {
        members.add(t.compile.len())?; references.add(t.references.len())?;
        for s in [&t.id, &t.project_path, &t.framework, &t.configuration, &t.platform,
            &t.sdk_version, &t.evaluation_fingerprint] { budget.text(s, false)?; }
        budget.text(&t.runtime_identifier, true)?; path(&t.project_path)?;
        for m in &t.compile { member(m, &mut budget)?; }
        for r in &t.references { budget.text(r, false)?; }
    }
    Ok(())
}

fn observation_preflight(observation: &SourceObservation<'_>) -> Result<(), Unavailable> {
    if observation.files.len() > MAX_FILES { return Err(Unavailable::InputLimit); }
    let mut budget = Budget::new(MAX_OBSERVATION_BYTES);
    stamp(&observation.stamp, &mut budget)?;
    budget.add(64 + observation.files.len() * 80)?;
    for file in &observation.files {
        if file.bytes.len() > MAX_FILE_BYTES { return Err(Unavailable::InputLimit); }
        budget.text(&file.path, false)?; path(&file.path)?; budget.add(file.bytes.len())?;
    }
    Ok(())
}

/// Length-prefixed UTF-8, u64 big-endian counts and tagged optionals. Internal
/// encoding only; bounded preflights precede every proportional allocation.
struct Encoding(Vec<u8>);
impl Encoding {
    fn new(domain: &str) -> Self { let mut e = Self(Vec::new()); e.text(domain); e }
    fn number(&mut self, value: usize) { self.0.extend_from_slice(&(value as u64).to_be_bytes()); }
    fn text(&mut self, value: &str) { self.number(value.len()); self.0.extend_from_slice(value.as_bytes()); }
    fn flag(&mut self, value: bool) { self.0.push(u8::from(value)); }
    fn stamp(&mut self, s: &Stamp) { self.text(&s.root); self.text(&s.source_snapshot); self.text(&s.git_identity); }
    fn member(&mut self, m: &CompileMember) {
        self.text(&m.path); self.flag(m.link.is_some());
        if let Some(link) = &m.link { self.text(link); }
    }
    fn finish(self) -> String { hash_upper_hex(&self.0) }
}

fn member_order(a: &CompileMember, b: &CompileMember) -> std::cmp::Ordering {
    ordinal(&a.path, &b.path).then_with(|| match (&a.link, &b.link) {
        (None, None) => std::cmp::Ordering::Equal,
        (None, Some(_)) => std::cmp::Ordering::Less,
        (Some(_), None) => std::cmp::Ordering::Greater,
        (Some(a), Some(b)) => ordinal(a, b),
    })
}

/// Integrity of an entire supported graph, independent of collection order.
pub fn graph_fingerprint(graph: &ProjectGraph) -> Result<String, Unavailable> {
    graph_preflight(graph)?;
    let mut targets: Vec<_> = graph.targets.iter().collect();
    targets.sort_by(|a, b| ordinal(&a.id, &b.id));
    let mut projects = HashSet::new();
    let ids: HashSet<_> = targets.iter().map(|t| t.id.as_str()).collect();
    if ids.len() != targets.len() { return Err(Unavailable::InvalidEvidence); }
    let mut e = Encoding::new("slophawk.lost-routes.graph/1");
    e.flag(true); e.number(0); e.number(targets.len());
    for t in targets {
        if !projects.insert(t.project_path.as_str()) { return Err(Unavailable::UniverseMismatch); }
        for s in [&t.id, &t.project_path, &t.framework, &t.configuration, &t.platform,
            &t.runtime_identifier, &t.sdk_version, &t.evaluation_fingerprint] { e.text(s); }
        e.flag(t.is_test_project);
        let mut members: Vec<_> = t.compile.iter().collect();
        members.sort_by(|a, b| member_order(a, b));
        if members.windows(2).any(|pair| pair[0] == pair[1]) { return Err(Unavailable::InvalidEvidence); }
        e.number(members.len());
        for m in members { e.member(m); }
        let mut references: Vec<_> = t.references.iter().collect();
        references.sort_by(|a, b| ordinal(a, b));
        if references.windows(2).any(|pair| pair[0] == pair[1])
            || references.iter().any(|r| !ids.contains(r.as_str()))
        { return Err(Unavailable::InvalidEvidence); }
        e.number(references.len());
        for r in references { e.text(r); }
    }
    Ok(e.finish())
}

fn observation_seal<'a>(observation: &'a SourceObservation<'_>)
    -> Result<(String, HashMap<&'a str, String>), Unavailable>
{
    observation_preflight(observation)?;
    let mut files: Vec<_> = observation.files.iter().collect();
    files.sort_by(|a, b| ordinal(&a.path, &b.path));
    if files.windows(2).any(|pair| pair[0].path == pair[1].path) {
        return Err(Unavailable::InvalidEvidence);
    }
    let mut e = Encoding::new("slophawk.lost-routes.sources/1");
    e.stamp(&observation.stamp); e.number(files.len());
    let mut digests = HashMap::new();
    for file in files {
        let sha256 = hash_upper_hex(file.bytes);
        e.text(&file.path); e.number(file.bytes.len()); e.text(&sha256);
        digests.insert(file.path.as_str(), sha256);
    }
    Ok((e.finish(), digests))
}

/// Collector helper. Seals every supplied path and its raw bytes, never Compile.
pub fn observation_fingerprint(observation: &SourceObservation<'_>) -> Result<String, Unavailable> {
    observation_seal(observation).map(|(seal, _)| seal)
}

fn evaluation_fingerprint(key: &EvaluationKey) -> Result<String, Unavailable> {
    let mut properties: Vec<_> = key.global_properties.iter().collect();
    properties.sort_by(|a, b| ordinal(&a.name, &b.name));
    let mut names = HashSet::new();
    let mut e = Encoding::new("slophawk.lost-routes.evaluation/1");
    for s in [&key.sdk_version, &key.provider_sha256, &key.toolchain_sha256,
        &key.environment_sha256, &key.external_inputs_sha256] { e.text(s); }
    e.number(properties.len());
    for p in properties {
        if !names.insert(p.name.to_ascii_lowercase()) { return Err(Unavailable::InvalidEvidence); }
        e.text(&p.name); e.text(&p.value);
    }
    Ok(e.finish())
}

fn expected_preflight(expected: &ExpectedCapture, budget: &mut Budget) -> Result<(), Unavailable> {
    stamp(&expected.stamp, budget)?;
    digest(&expected.policy_sha256)?;
    digest(&expected.graph_sha256)?; digest(&expected.observation_sha256)?;
    key_preflight(&expected.evaluation, budget)?;
    if expected.targets.len() > MAX_TARGETS { return Err(Unavailable::InputLimit); }
    for t in &expected.targets {
        for s in [&t.project_path, &t.framework, &t.configuration, &t.platform] { budget.text(s, false)?; }
        path(&t.project_path)?;
        budget.text(&t.runtime_identifier, true)?;
    }
    Ok(())
}

fn universe(graph: &ProjectGraph, expected: &ExpectedCapture) -> Result<(), Unavailable> {
    let mut seen = HashSet::new();
    if graph.targets.len() != expected.targets.len() { return Err(Unavailable::UniverseMismatch); }
    for t in &expected.targets {
        if !seen.insert(&t.project_path) { return Err(Unavailable::UniverseMismatch); }
        if !graph.targets.iter().any(|g| g.project_path == t.project_path && g.framework == t.framework
            && g.configuration == t.configuration && g.platform == t.platform
            && g.runtime_identifier == t.runtime_identifier)
        { return Err(Unavailable::UniverseMismatch); }
    }
    Ok(())
}

fn verify_capture<'a>(capture: &CapturedEvaluation<'_>, expected: &ExpectedCapture,
    sources: &'a SourceObservation<'_>) -> Result<HashMap<&'a str, String>, Unavailable>
{
    if capture.before != capture.after || capture.after != expected.stamp || sources.stamp != expected.stamp {
        return Err(Unavailable::StaleCapture);
    }
    if evaluation_fingerprint(&capture.evaluation)? != evaluation_fingerprint(&expected.evaluation)?
        || capture.graph.targets.iter().any(|t| t.sdk_version != capture.evaluation.sdk_version)
    { return Err(Unavailable::EvaluationMismatch); }
    let graph = graph_fingerprint(capture.graph)?;
    if graph != capture.graph_sha256 || graph != expected.graph_sha256 { return Err(Unavailable::GraphMismatch); }
    universe(capture.graph, expected)?;
    let (observation, files) = observation_seal(sources)?;
    if observation != expected.observation_sha256 { return Err(Unavailable::ObservationMismatch); }
    if capture.graph.targets.iter().flat_map(|t| &t.compile).any(|m| !files.contains_key(m.path.as_str())) {
        return Err(Unavailable::MissingSource);
    }
    Ok(files)
}

/// Fields are private so an unverified graph cannot construct this adapter.
#[derive(Debug)]
pub struct VerifiedPair<'a> {
    previous_graph: &'a ProjectGraph,
    current_graph: &'a ProjectGraph,
    previous_identity: Identity,
    current_identity: Identity,
    current_stamp: Stamp,
    previous_tests: Vec<String>,
    current_tests: Vec<String>,
    requests: Vec<Request>,
    limits: Limits,
}

impl VerifiedPair<'_> {
    pub fn requests(&self) -> &[Request] { &self.requests }
    pub fn previous_configured_tests(&self) -> &[String] { &self.previous_tests }
    pub fn current_configured_tests(&self) -> &[String] { &self.current_tests }

    fn input(&self) -> Input<'_> {
        Input {
            previous: Capture { identity: &self.previous_identity, graph: self.previous_graph,
                complete: true, evaluation_sufficient: true },
            current: Capture { identity: &self.current_identity, graph: self.current_graph,
                complete: true, evaluation_sufficient: true },
            expected_previous: &self.previous_identity, expected_current: &self.current_identity,
            previous_configured_tests: &self.previous_tests, current_configured_tests: &self.current_tests,
            requests: &self.requests, limits: self.limits,
        }
    }

    /// Revalidate against the owner's independent current observation at use time.
    /// This comparison cannot turn a stale or forged owner stamp into fresh truth.
    pub fn check(&self, current: &Stamp) -> Result<Report<'_>, Unavailable> {
        stamp(current, &mut Budget::new(MAX_GRAPH_BYTES))?;
        if current != &self.current_stamp { return Err(Unavailable::StaleCapture); }
        Ok(super::check(&self.input()))
    }
}

/// No I/O, project evaluation, publication or execution permission is performed.
pub fn verify_pair<'a>(input: &VerificationInput<'a>) -> Result<VerifiedPair<'a>, Unavailable> {
    if input.previous.version != VERSION || input.current.version != VERSION { return Err(Unavailable::UnsupportedSchema); }
    let previous = input.expected_previous.ok_or(Unavailable::MissingExpectation)?;
    let current = input.expected_current.ok_or(Unavailable::MissingExpectation)?;
    let policy = input.policy.ok_or(Unavailable::MissingPolicy)?;
    let previous_sources = input.previous_sources.ok_or(Unavailable::MissingObservation)?;
    let current_sources = input.current_sources.ok_or(Unavailable::MissingObservation)?;
    if policy.paths != PathComparison::Ordinal { return Err(Unavailable::UnsupportedPolicy); }
    if !policy.immutable_inputs { return Err(Unavailable::ImmutableInputsRequired); }
    if !policy.evaluation_only { return Err(Unavailable::ExecutionItems); }
    digest(&policy.policy_sha256)?;
    // All cardinality/string/byte checks precede sorting, copying and hashing.
    let mut budget = Budget::new(MAX_OBSERVATION_BYTES);
    for c in [&input.previous, &input.current] {
        if !c.complete { return Err(Unavailable::PartialGraph); }
        graph_preflight(c.graph)?; stamp(&c.before, &mut budget)?; stamp(&c.after, &mut budget)?;
        digest(&c.graph_sha256)?; key_preflight(&c.evaluation, &mut budget)?;
    }
    expected_preflight(previous, &mut budget)?; expected_preflight(current, &mut budget)?;
    if previous.policy_sha256 != policy.policy_sha256 || current.policy_sha256 != policy.policy_sha256 {
        return Err(Unavailable::UnsupportedPolicy);
    }
    observation_preflight(previous_sources)?; observation_preflight(current_sources)?;
    if input.requests.len() > MAX_REQUESTS { return Err(Unavailable::InputLimit); }
    for r in input.requests {
        budget.text(&r.key, false)?; budget.text(&r.previous_target, false)?;
        member(&r.previous_item, &mut budget)?; member(&r.current_item, &mut budget)?;
        for d in [&r.rename, &r.retirement].into_iter().flatten() {
            for text in [&d.source_snapshot, &d.provenance] {
                if text.len() > MAX_STRING_BYTES { return Err(Unavailable::InputLimit); }
                if text.is_empty() || text.contains('\0') { return Err(Unavailable::InvalidDeclaration); }
                budget.add(8 + text.len())?;
            }
        }
    }
    if previous.stamp.root != current.stamp.root { return Err(Unavailable::Core(Unknown::ScopeMismatch)); }
    let evaluation = evaluation_fingerprint(&previous.evaluation)?;
    if evaluation != evaluation_fingerprint(&current.evaluation)? { return Err(Unavailable::EvaluationMismatch); }
    drop(verify_capture(&input.previous, previous, previous_sources)?);
    let current_files = verify_capture(&input.current, current, current_sources)?;
    let mut scope = Encoding::new("slophawk.lost-routes.scope/1");
    scope.text(&evaluation); scope.text(&policy.policy_sha256);
    let evaluation_scope = scope.finish();
    let mut group = Encoding::new("slophawk.lost-routes.configured/1");
    match policy.configured_group { ConfiguredGroup::AllEvaluatedTests => group.text("all-evaluated-tests") }
    group.text(&policy.policy_sha256);
    let configured_scope = group.finish();
    let identity = |e: &ExpectedCapture| Identity {
        root: e.stamp.root.clone(), source_snapshot: e.stamp.source_snapshot.clone(),
        graph_snapshot: e.graph_sha256.clone(), evaluation_scope: evaluation_scope.clone(),
        configured_scope: configured_scope.clone(), paths: PathComparison::Ordinal,
    };
    let tests = |graph: &ProjectGraph| {
        let mut ids: Vec<_> = graph.targets.iter().filter(|t| t.is_test_project).map(|t| t.id.clone()).collect();
        ids.sort_by(|a, b| ordinal(a, b)); ids
    };
    let mut pair = VerifiedPair {
        previous_graph: input.previous.graph, current_graph: input.current.graph,
        previous_identity: identity(previous), current_identity: identity(current),
        current_stamp: current.stamp.clone(), previous_tests: tests(input.previous.graph),
        current_tests: tests(input.current.graph), requests: Vec::new(), limits: input.limits,
    };
    // Reuse the graph/context/toolchain/group defenses without route traversal.
    if let Some(problem) = super::check(&pair.input()).problem { return Err(Unavailable::Core(problem)); }
    let mut names = HashSet::new();
    let mut old_items = HashSet::new();
    let mut new_items = HashSet::new();
    for r in input.requests {
        if !names.insert(&r.key) || !old_items.insert((&r.previous_target, &r.previous_item.path, &r.previous_item.link))
            || !new_items.insert((&r.previous_target, &r.current_item.path, &r.current_item.link))
        { return Err(Unavailable::Core(Unknown::InvalidRequests)); }
        let old = input.previous.graph.targets.iter().find(|t| t.id == r.previous_target)
            .ok_or(Unavailable::Core(Unknown::TargetNotComparable))?;
        if !old.compile.contains(&r.previous_item) { return Err(Unavailable::Core(Unknown::PriorItemAbsent)); }
        if r.rename.is_some() && r.retirement.is_some()
            || [&r.rename, &r.retirement].into_iter().flatten().any(|d|
                d.source_snapshot != current.stamp.source_snapshot || d.provenance.is_empty())
            || r.retirement.is_none() && r.previous_item != r.current_item && r.rename.is_none()
        { return Err(Unavailable::InvalidDeclaration); }
        let source = if r.retirement.is_some() { None } else {
            Some(SurvivingSource { source_snapshot: current.stamp.source_snapshot.clone(),
                path: r.current_item.path.clone(), sha256: current_files.get(r.current_item.path.as_str())
                    .ok_or(Unavailable::MissingSource)?.clone() })
        };
        pair.requests.push(Request { key: r.key.clone(), previous_target: r.previous_target.clone(),
            previous_item: r.previous_item.clone(), current_item: r.current_item.clone(),
            surviving_source: source, rename: r.rename.clone(), retirement: r.retirement.clone() });
    }
    // Core limits may be tighter than the hard capture caps, including copy expansion.
    super::preflight(&pair.input()).map_err(Unavailable::Core)?;
    Ok(pair)
}
