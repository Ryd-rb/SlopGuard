//! Pure, bounded evaluated-project test impact projection at the SG-CHECK-06 boundary.
//!
//! Graph evaluation, reference resolution, path normalization, source digests, worker
//! admission, test execution, and receipt validation belong to upstream providers.
//! The output is derived from the diagnostic projection of C# `EvaluatedTestAdapter`,
//! with consulted predecessor issues also preventing complete selection. It is not
//! an independent authority to launch tests. No automatic filter is inferred.

mod sha256;

pub mod lost_routes;

use std::cmp::Ordering;
use std::collections::{HashMap, HashSet, VecDeque};
use std::fmt;

const MAX_SELECTED: usize = 16;
const MAX_DEFERRED_EXPLAINED: usize = 16;
const MAX_UNAPPROVED_EXPLAINED: usize = 16;
const MAX_REASONS: usize = 4;
const MAX_FRONTIERS: usize = 16;
const MAX_CHAIN: usize = 8;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CompileMember {
    /// Evaluated, physical source path; linked logical name is separate.
    pub path: String,
    pub link: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProjectTarget {
    /// Concrete evaluated target identity, including framework/configuration/RID context.
    pub id: String,
    pub project_path: String,
    pub framework: String,
    pub configuration: String,
    pub platform: String,
    pub runtime_identifier: String,
    pub sdk_version: String,
    pub is_test_project: bool,
    pub compile: Vec<CompileMember>,
    /// *Resolved concrete* evaluated project target IDs, not textual `.csproj` guesses.
    pub references: Vec<String>,
    pub evaluation_fingerprint: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProjectEvaluationIssue {
    pub path: String,
    pub reason: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProjectGraph {
    pub targets: Vec<ProjectTarget>,
    pub issues: Vec<ProjectEvaluationIssue>,
}

impl ProjectGraph {
    pub fn complete(&self) -> bool {
        self.issues.is_empty()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PathComparison {
    /// Linux and case-sensitive repository identity semantics.
    Ordinal,
    /// For ASCII paths only; other inputs fail closed rather than guessing .NET casefolding.
    OrdinalIgnoreAsciiCase,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TestCause {
    pub revision: i64,
    pub path_key: String,
    pub old_path: Option<String>,
    pub old_digest: Option<String>,
    pub new_path: Option<String>,
    pub new_digest: Option<String>,
    pub predecessor: Option<String>,
    pub fallback_first_revision: Option<i64>,
}

impl TestCause {
    /// C# `TestCause.Identifier`: uppercase SHA-256 of UTF-8 revision + ':' + path key.
    pub fn id(&self) -> String {
        sha256::hash_upper_hex(format!("{}:{}", self.revision, self.path_key).as_bytes())
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SelectionInput {
    pub current: ProjectGraph,
    pub predecessors: HashMap<String, ProjectGraph>,
    pub source_digests: HashMap<String, String>,
    pub approved_unit_projects: Vec<String>,
    pub causes: Vec<TestCause>,
    pub path_comparison: PathComparison,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum TestSelectionReasonKind {
    CurrentMembership,
    CurrentDependency,
    PredecessorMembership,
    PredecessorDependency,
    FirstRevisionFallback,
    MissingPredecessorFallback,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TestSelectionTargetScope {
    Selected,
    BudgetDeferred,
    Unapproved,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TestSelectionReason {
    pub cause_id: String,
    pub kind: TestSelectionReasonKind,
    pub source_path: Option<String>,
    pub target_reference_chain: Vec<String>,
    pub chain_truncated: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TestTargetExplanation {
    pub target_id: String,
    pub scope: TestSelectionTargetScope,
    pub reasons: Vec<TestSelectionReason>,
    pub omitted_reasons: usize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TestSelectionFrontier {
    pub predecessor: Option<String>,
    pub path: String,
    pub reason: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TestSelectionProjection {
    pub targets: Vec<TestTargetExplanation>,
    pub unresolved_frontiers: Vec<TestSelectionFrontier>,
    pub omitted_target_explanations: usize,
    pub omitted_frontiers: usize,
    pub approved_affected_targets: usize,
    pub budget_deferred_targets: usize,
    pub unapproved_affected_targets: usize,
    pub conservative_fallback: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SelectedTarget {
    pub id: String,
    pub input_fingerprint: String,
    pub cause_ids: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TestSelection {
    pub complete: bool,
    pub targets: Vec<SelectedTarget>,
    pub projection: TestSelectionProjection,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SelectionError {
    DuplicateTargetId(String),
    MissingReference(String),
    NonAsciiCaseInsensitivePath(String),
    AmbiguousSourceDigest(String),
}

impl fmt::Display for SelectionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::DuplicateTargetId(id) => write!(formatter, "duplicate evaluated target ID: {id}"),
            Self::MissingReference(id) => write!(
                formatter,
                "selected target has unresolved reference ID: {id}"
            ),
            Self::NonAsciiCaseInsensitivePath(path) => write!(
                formatter,
                "case-insensitive path comparison is not verified for non-ASCII input: {path}"
            ),
            Self::AmbiguousSourceDigest(path) => write!(
                formatter,
                "multiple source digests for normalized path: {path}"
            ),
        }
    }
}
impl std::error::Error for SelectionError {}

fn ordinal(left: &str, right: &str) -> Ordering {
    left.encode_utf16().cmp(right.encode_utf16())
}
fn opt_ordinal(left: Option<&str>, right: Option<&str>) -> Ordering {
    match (left, right) {
        (None, None) => Ordering::Equal,
        (None, Some(_)) => Ordering::Less,
        (Some(_), None) => Ordering::Greater,
        (Some(left), Some(right)) => ordinal(left, right),
    }
}
fn sort_ordinal<T>(items: &mut [T], field: impl Fn(&T) -> &str) {
    items.sort_by(|a, b| ordinal(field(a), field(b)));
}
fn path_key(path: &str, comparison: PathComparison) -> Result<String, SelectionError> {
    match comparison {
        PathComparison::Ordinal => Ok(path.into()),
        PathComparison::OrdinalIgnoreAsciiCase if path.is_ascii() => Ok(path.to_ascii_lowercase()),
        PathComparison::OrdinalIgnoreAsciiCase => {
            Err(SelectionError::NonAsciiCaseInsensitivePath(path.into()))
        }
    }
}

struct GraphIndex<'a> {
    graph: &'a ProjectGraph,
    targets: HashMap<&'a str, &'a ProjectTarget>,
    owners: HashMap<String, Vec<String>>,
    dependents: HashMap<&'a str, Vec<String>>,
}

impl<'a> GraphIndex<'a> {
    fn new(graph: &'a ProjectGraph, comparison: PathComparison) -> Result<Self, SelectionError> {
        let mut targets = HashMap::new();
        let mut owners = HashMap::<String, Vec<String>>::new();
        let mut dependents = HashMap::<&str, Vec<String>>::new();
        for target in &graph.targets {
            if targets.insert(target.id.as_str(), target).is_some() {
                return Err(SelectionError::DuplicateTargetId(target.id.clone()));
            }
            for member in &target.compile {
                owners
                    .entry(path_key(&member.path, comparison)?)
                    .or_default()
                    .push(target.id.clone());
            }
            for reference in &target.references {
                dependents
                    .entry(reference)
                    .or_default()
                    .push(target.id.clone());
            }
        }
        for roots in owners.values_mut() {
            sort_ordinal(roots, String::as_str);
            roots.dedup();
        }
        for dependents in dependents.values_mut() {
            sort_ordinal(dependents, String::as_str);
        }
        Ok(Self {
            graph,
            targets,
            owners,
            dependents,
        })
    }

    fn dependents_of(
        &self,
        path: Option<&str>,
        comparison: PathComparison,
    ) -> Result<DependencyWalk, SelectionError> {
        let mut parents = HashMap::<String, Option<String>>::new();
        let Some(path) = path else {
            return Ok(DependencyWalk { parents });
        };
        let key = path_key(path, comparison)?;
        let mut pending = VecDeque::new();
        if let Some(roots) = self.owners.get(&key) {
            for root in roots {
                parents.insert(root.clone(), None);
                pending.push_back(root.clone());
            }
        }
        while let Some(current) = pending.pop_front() {
            if let Some(dependents) = self.dependents.get(current.as_str()) {
                for related in dependents {
                    if !parents.contains_key(related) {
                        parents.insert(related.clone(), Some(current.clone()));
                        pending.push_back(related.clone());
                    }
                }
            }
        }
        Ok(DependencyWalk { parents })
    }

    fn is_test_target(&self, id: &str) -> bool {
        self.targets.get(id).is_some_and(|t| t.is_test_project)
    }

    /// Match `GraphIndex.Fingerprint`: issues and forward closure in a deterministic
    /// anonymous-record JSON representation, then uppercase SHA-256.
    fn fingerprint(
        &self,
        target_id: &str,
        digests: &HashMap<String, String>,
        comparison: PathComparison,
    ) -> Result<String, SelectionError> {
        let mut seen = HashSet::<String>::new();
        let mut pending = VecDeque::from([target_id.to_owned()]);
        while let Some(id) = pending.pop_front() {
            if !seen.insert(id.clone()) {
                continue;
            }
            let target = self
                .targets
                .get(id.as_str())
                .ok_or_else(|| SelectionError::MissingReference(id.clone()))?;
            for reference in &target.references {
                pending.push_back(reference.clone());
            }
        }
        let mut contexts: Vec<&str> = seen.iter().map(String::as_str).collect();
        contexts.sort_by(|a, b| ordinal(a, b));
        let mut json = String::from("{\"Issues\":[");
        for (i, issue) in self.graph.issues.iter().enumerate() {
            if i != 0 {
                json.push(',');
            }
            json.push_str("{\"Path\":");
            write_json_str(&mut json, &issue.path);
            json.push_str(",\"Reason\":");
            write_json_str(&mut json, &issue.reason);
            json.push('}');
        }
        json.push_str("],\"Contexts\":[");
        for (i, id) in contexts.iter().enumerate() {
            if i != 0 {
                json.push(',');
            }
            let target = self.targets[id];
            json.push_str("{\"Id\":");
            write_json_str(&mut json, &target.id);
            json.push_str(",\"EvaluationFingerprint\":");
            write_json_str(&mut json, &target.evaluation_fingerprint);
            json.push_str(",\"References\":[");
            for (j, reference) in target.references.iter().enumerate() {
                if j != 0 {
                    json.push(',');
                }
                write_json_str(&mut json, reference);
            }
            json.push_str("],\"Sources\":[");
            for (j, member) in target.compile.iter().enumerate() {
                if j != 0 {
                    json.push(',');
                }
                json.push_str("{\"Path\":");
                write_json_str(&mut json, &member.path);
                json.push_str(",\"Link\":");
                write_json_opt(&mut json, member.link.as_deref());
                json.push_str(",\"Digest\":");
                let digest = digests
                    .get(&path_key(&member.path, comparison)?)
                    .map(String::as_str);
                write_json_opt(&mut json, digest);
                json.push('}');
            }
            json.push_str("]}");
        }
        json.push_str("]}");
        Ok(sha256::hash_upper_hex(json.as_bytes()))
    }
}

struct DependencyWalk {
    parents: HashMap<String, Option<String>>,
}
impl DependencyWalk {
    fn chain_to(&self, target_id: &str) -> (Vec<String>, bool) {
        let mut chain = Vec::new();
        let mut current = target_id;
        while chain.len() < MAX_CHAIN {
            chain.push(current.to_owned());
            match self.parents.get(current).and_then(Option::as_deref) {
                Some(parent) => current = parent,
                None => {
                    chain.reverse();
                    return (chain, false);
                }
            }
        }
        chain.reverse();
        (chain, true)
    }
}

#[derive(Default)]
struct CapturedTarget {
    reasons: Vec<TestSelectionReason>,
    total: usize,
}
impl CapturedTarget {
    fn add(&mut self, reason: TestSelectionReason) {
        self.total += 1;
        self.reasons.push(reason);
        self.reasons.sort_by(|a, b| {
            ordinal(&a.cause_id, &b.cause_id)
                .then_with(|| a.kind.cmp(&b.kind))
                .then_with(|| opt_ordinal(a.source_path.as_deref(), b.source_path.as_deref()))
        });
        if self.reasons.len() > MAX_REASONS {
            self.reasons.pop();
        }
    }
    fn project(self, target_id: String, scope: TestSelectionTargetScope) -> TestTargetExplanation {
        TestTargetExplanation {
            target_id,
            scope,
            omitted_reasons: self.total - self.reasons.len(),
            reasons: self.reasons,
        }
    }
}

#[derive(Default)]
struct ExplanationBuilder {
    captured: HashMap<String, CapturedTarget>,
    frontiers: HashMap<String, TestSelectionFrontier>,
    frontier_events: usize,
    explained_predecessors: HashSet<String>,
    conservative_fallback: bool,
}
impl ExplanationBuilder {
    fn frontier(&mut self, predecessor: Option<&str>, path: &str, reason: &str) {
        let key = format!("{}\0{}\0{}", predecessor.unwrap_or(""), path, reason);
        if self.frontiers.contains_key(&key) {
            return;
        }
        // Match the bounded C# presentation contract: a discarded event counts
        // again if it reappears, while a retained duplicate does not.
        self.frontier_events += 1;
        if self.frontiers.len() >= MAX_FRONTIERS {
            let last = self
                .frontiers
                .keys()
                .max_by(|a, b| ordinal(a, b))
                .expect("full frontier map")
                .clone();
            if ordinal(&key, &last) != Ordering::Less {
                return;
            }
            self.frontiers.remove(&last);
        }
        self.frontiers.insert(
            key,
            TestSelectionFrontier {
                predecessor: predecessor.map(str::to_owned),
                path: path.into(),
                reason: reason.into(),
            },
        );
    }
    fn issues(&mut self, predecessor: Option<&str>, issues: &[ProjectEvaluationIssue]) {
        if let Some(name) = predecessor
            && !self.explained_predecessors.insert(name.into())
        {
            return;
        }
        for issue in issues {
            self.frontier(predecessor, &issue.path, &issue.reason);
        }
    }
    fn record_walk(
        &mut self,
        cause_id: &str,
        path: Option<&str>,
        index: &GraphIndex<'_>,
        walk: &DependencyWalk,
        predecessor: Option<&str>,
        current_tests: &HashSet<String>,
    ) {
        let Some(path) = path else {
            return;
        };
        for target_id in walk.parents.keys() {
            if !current_tests.contains(target_id) {
                if predecessor.is_some() && index.is_test_target(target_id) {
                    self.frontier(predecessor, path, "predecessor_test_target_absent_current");
                }
                continue;
            }
            let (chain, chain_truncated) = walk.chain_to(target_id);
            let kind = match (predecessor.is_some(), chain.len() == 1) {
                (true, true) => TestSelectionReasonKind::PredecessorMembership,
                (true, false) => TestSelectionReasonKind::PredecessorDependency,
                (false, true) => TestSelectionReasonKind::CurrentMembership,
                (false, false) => TestSelectionReasonKind::CurrentDependency,
            };
            self.captured
                .entry(target_id.clone())
                .or_default()
                .add(TestSelectionReason {
                    cause_id: cause_id.into(),
                    kind,
                    source_path: Some(path.into()),
                    target_reference_chain: chain,
                    chain_truncated,
                });
        }
    }
    fn fallback(
        &mut self,
        cause_id: &str,
        cause: &TestCause,
        kind: TestSelectionReasonKind,
        eligible: &HashSet<String>,
    ) {
        self.conservative_fallback = true;
        if kind == TestSelectionReasonKind::MissingPredecessorFallback {
            self.frontier(
                cause.predecessor.as_deref(),
                cause.old_path.as_deref().unwrap_or("."),
                "missing_predecessor",
            );
        }
        for target_id in eligible {
            self.captured
                .entry(target_id.clone())
                .or_default()
                .add(TestSelectionReason {
                    cause_id: cause_id.into(),
                    kind,
                    source_path: cause.old_path.clone(),
                    target_reference_chain: vec![],
                    chain_truncated: false,
                });
        }
    }
    fn finish(self, selected_count: usize, eligible: &HashSet<String>) -> TestSelectionProjection {
        let mut approved = Vec::<(String, CapturedTarget)>::new();
        let mut unapproved = Vec::<(String, CapturedTarget)>::new();
        for (target_id, captured) in self.captured {
            if eligible.contains(&target_id) {
                approved.push((target_id, captured));
            } else {
                unapproved.push((target_id, captured));
            }
        }
        sort_ordinal(&mut approved, |pair| &pair.0);
        sort_ordinal(&mut unapproved, |pair| &pair.0);
        let approved_count = approved.len();
        let unapproved_count = unapproved.len();
        let mut explanations = Vec::new();
        for (idx, (id, captured)) in approved
            .into_iter()
            .take(MAX_SELECTED + MAX_DEFERRED_EXPLAINED)
            .enumerate()
        {
            let scope = if idx < MAX_SELECTED {
                TestSelectionTargetScope::Selected
            } else {
                TestSelectionTargetScope::BudgetDeferred
            };
            explanations.push(captured.project(id, scope));
        }
        let approved_retained = approved_count.min(MAX_SELECTED + MAX_DEFERRED_EXPLAINED);
        for (id, captured) in unapproved.into_iter().take(MAX_UNAPPROVED_EXPLAINED) {
            explanations.push(captured.project(id, TestSelectionTargetScope::Unapproved));
        }
        let mut frontiers: Vec<_> = self.frontiers.into_iter().collect();
        sort_ordinal(&mut frontiers, |pair| &pair.0);
        let retained_frontiers = frontiers.len();
        TestSelectionProjection {
            targets: explanations,
            unresolved_frontiers: frontiers
                .into_iter()
                .take(MAX_FRONTIERS)
                .map(|(_, v)| v)
                .collect(),
            omitted_target_explanations: selected_count.saturating_sub(approved_retained)
                + unapproved_count.saturating_sub(MAX_UNAPPROVED_EXPLAINED),
            omitted_frontiers: self.frontier_events.saturating_sub(retained_frontiers),
            approved_affected_targets: selected_count,
            budget_deferred_targets: selected_count.saturating_sub(MAX_SELECTED),
            unapproved_affected_targets: unapproved_count,
            conservative_fallback: self.conservative_fallback,
        }
    }
}

fn write_json_opt(json: &mut String, value: Option<&str>) {
    if let Some(value) = value {
        write_json_str(json, value);
    } else {
        json.push_str("null");
    }
}

/// `System.Text.Json` default encoder escapes non-ASCII and HTML-sensitive characters.
fn write_json_str(json: &mut String, value: &str) {
    json.push('"');
    for code in value.encode_utf16() {
        match code {
            0x0022 => json.push_str("\\u0022"),
            0x005c => json.push_str("\\\\"),
            0x0008 => json.push_str("\\b"),
            0x0009 => json.push_str("\\t"),
            0x000a => json.push_str("\\n"),
            0x000c => json.push_str("\\f"),
            0x000d => json.push_str("\\r"),
            0x0000..=0x001f
            | 0x007f..=0xffff
            | 0x0026
            | 0x0027
            | 0x002b
            | 0x003c
            | 0x003e
            | 0x0060 => {
                use std::fmt::Write;
                write!(json, "\\u{code:04X}").expect("writing to string cannot fail");
            }
            _ => json.push(char::from_u32(u32::from(code)).expect("ASCII scalar")),
        }
    }
    json.push('"');
}

/// Project evaluated-project selection and diagnostics from the pinned C# selector,
/// retaining partial scope for every predecessor consulted for an old path.
/// An unresolved selected fingerprint reference or unsupported path comparison is
/// an explicit error, never an authoritative empty selection.
pub fn select(input: &SelectionInput) -> Result<TestSelection, SelectionError> {
    let comparison = input.path_comparison;
    let current = GraphIndex::new(&input.current, comparison)?;
    let mut normalized_digests = HashMap::new();
    for (path, digest) in &input.source_digests {
        let key = path_key(path, comparison)?;
        if normalized_digests
            .insert(key.clone(), digest.clone())
            .is_some()
        {
            return Err(SelectionError::AmbiguousSourceDigest(key));
        }
    }
    let mut previous = HashMap::<&str, GraphIndex<'_>>::new();
    for (name, graph) in &input.predecessors {
        previous.insert(name.as_str(), GraphIndex::new(graph, comparison)?);
    }
    let current_tests: HashMap<&str, &ProjectTarget> = input
        .current
        .targets
        .iter()
        .filter(|target| target.is_test_project)
        .map(|target| (target.id.as_str(), target))
        .collect();
    let mut eligible = HashSet::<String>::new();
    let mut present_projects = HashSet::<String>::new();
    for target in current_tests.values() {
        let key = path_key(&target.project_path, comparison)?;
        present_projects.insert(key.clone());
        if input
            .approved_unit_projects
            .iter()
            .any(|approved| path_key(approved, comparison).as_deref() == Ok(key.as_str()))
        {
            eligible.insert(target.id.clone());
        }
    }
    let mut missing_approved = Vec::new();
    for approved in &input.approved_unit_projects {
        if !present_projects.contains(&path_key(approved, comparison)?) {
            missing_approved.push(approved.clone());
        }
    }
    let mut complete = input.current.complete() && missing_approved.is_empty();
    let current_ids: HashSet<String> = current_tests.keys().map(|id| (*id).into()).collect();
    let mut explanations = ExplanationBuilder::default();
    explanations.issues(None, &input.current.issues);
    for path in missing_approved {
        explanations.frontier(None, &path, "approved_test_project_absent_current");
    }
    let mut selected = HashMap::<String, Vec<String>>::new();
    for cause in &input.causes {
        let id = cause.id();
        let new = current.dependents_of(cause.new_path.as_deref(), comparison)?;
        let mut impacted: HashSet<String> = new.parents.keys().cloned().collect();
        explanations.record_walk(
            &id,
            cause.new_path.as_deref(),
            &current,
            &new,
            None,
            &current_ids,
        );
        if cause.fallback_first_revision.is_some() {
            impacted.extend(eligible.iter().cloned());
            explanations.fallback(
                &id,
                cause,
                TestSelectionReasonKind::FirstRevisionFallback,
                &eligible,
            );
        } else if let Some(old_path) = &cause.old_path {
            if let Some(prior) = cause
                .predecessor
                .as_ref()
                .and_then(|key| previous.get(key.as_str()))
            {
                // Missing old membership in an explicitly partial graph is not
                // evidence that no current test is affected. Preserve known
                // impacts and frontiers, but do not authorize cause retirement.
                complete &= prior.graph.complete();
                let prior_walk = prior.dependents_of(Some(old_path), comparison)?;
                impacted.extend(prior_walk.parents.keys().cloned());
                explanations.record_walk(
                    &id,
                    Some(old_path),
                    prior,
                    &prior_walk,
                    cause.predecessor.as_deref(),
                    &current_ids,
                );
                explanations.issues(cause.predecessor.as_deref(), &prior.graph.issues);
            } else {
                impacted.extend(eligible.iter().cloned());
                explanations.fallback(
                    &id,
                    cause,
                    TestSelectionReasonKind::MissingPredecessorFallback,
                    &eligible,
                );
            }
        }
        for target_id in impacted {
            if eligible.contains(&target_id) {
                selected.entry(target_id).or_default().push(id.clone());
            }
        }
    }
    let selected_count = selected.len();
    let mut selected: Vec<(String, Vec<String>)> = selected.into_iter().collect();
    sort_ordinal(&mut selected, |pair| &pair.0);
    let targets = selected
        .into_iter()
        .take(MAX_SELECTED)
        .map(|(id, cause_ids)| {
            Ok(SelectedTarget {
                input_fingerprint: current.fingerprint(&id, &normalized_digests, comparison)?,
                id,
                cause_ids,
            })
        })
        .collect::<Result<Vec<_>, SelectionError>>()?;
    Ok(TestSelection {
        complete: complete && selected_count == targets.len(),
        targets,
        projection: explanations.finish(selected_count, &eligible),
    })
}
