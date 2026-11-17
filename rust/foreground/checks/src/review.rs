//! Scope review and acknowledgment validation over immutable v1 evidence sets.
use crate::{evidence::{self, Evidence}, io, model::{self, Adapter, Acknowledgment, Difference, Finding, Input, Observation, Scope, Source, REVIEW_FORMAT}};
use serde::Serialize;
use slophawk_probe_replay::{JsonValue, ProbeDifference, ProbeInput};
use std::{collections::{BTreeMap, BTreeSet, HashMap, HashSet}, fs, path::{Path, PathBuf}};

const MAX_RUNS: usize = 2048;
const MAX_REVIEW_BYTES: u64 = 64 * 1024 * 1024;
const MAX_OBSERVATIONS: usize = 65_536;

#[derive(Debug, Serialize)]
#[serde(rename_all = "PascalCase")]
struct EvidenceSet { adapter: Adapter, runs: Vec<String> }

#[derive(Debug, Serialize)]
#[serde(rename_all = "PascalCase")]
struct Coverage {
    operation: String, compared: usize, outside_ordinary: Option<usize>,
    observed_only: usize, unsupported: usize,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "PascalCase")]
struct Unstable {
    input: Input, observations: Vec<Observation>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "PascalCase")]
struct WorkGroup { component: String, operations: Vec<Source> }

struct Sample { input: ProbeInput, run: String, value: JsonValue, observe_only: bool }
struct Samples { input: Input, observations: Vec<Observation>, first: JsonValue, unstable: bool }

pub(crate) fn run(args: &[String]) -> Result<i32, String> {
    // C# accepts --progress last, and --run with a 32-character GUID after
    // the workspace argument. Other positionals are acknowledgments and scope.
    if args.len() < 2 || args.len() > 7 {
        return Err("Usage: slophawk check-review <workspace> [acknowledgments.json [scope.json]] [--run <run>] [--progress]".into());
    }
    let workspace = io::ensure_workspace(&args[1])?;
    let progress = args.last().is_some_and(|v| v == "--progress");
    let tail = if progress { &args[2..args.len() - 1] } else { &args[2..] };
    let mut requested = None;
    let mut positionals = Vec::new();
    let mut index = 0;
    while index < tail.len() {
        if tail[index] == "--run" {
            if requested.is_some() || index + 1 == tail.len() || !io::is_run_id(&tail[index + 1]) {
                return Err("Supply a completed check run ID after --run.".into());
            }
            requested = Some(tail[index + 1].clone());
            index += 2;
        } else if tail[index].starts_with("--") {
            return Err("Unknown check-review option.".into());
        } else {
            positionals.push(tail[index].as_str());
            index += 1;
        }
    }
    if positionals.len() > 2 { return Err("Too many check-review arguments.".into()); }
    let scope: Option<Scope> = positionals.get(1).map(|v| io::read_json(Path::new(v))).transpose()?;
    if let Some(scope) = &scope { validate_scope(scope)?; }
    let directory = io::checks_dir(&workspace);
    let headers = load_headers(&directory)?;
    if headers.is_empty() && requested.is_none() && (!progress || scope.is_none()) {
        return Err("No completed checker reports are available.".into());
    }
    let sets = evidence_sets(&headers);
    let adapter = if let Some(run) = &requested {
        Some(headers.get(run).ok_or_else(|| format!("Check run {run} was not found in this workspace."))?.clone())
    } else if sets.len() == 1 { Some(sets[0].adapter.clone()) } else { None };
    let selection_required = sets.len() > 1 && requested.is_none();
    let mut selected = BTreeMap::<String, Evidence>::new();
    let mut total = 0_u64;
    let mut observation_count = 0_usize;
    for (run, candidate) in &headers {
        if adapter.as_ref() != Some(candidate) { continue; }
        let root = directory.join(run);
        for name in ["report.json", "requests.json", "observed.json", "identity.json"] {
            let path = root.join(name);
            if path.exists() { total = total.saturating_add(fs::metadata(path).map_err(|e| e.to_string())?.len()); }
        }
        if total > MAX_REVIEW_BYTES { return Err("Check-review evidence exceeds 64 MiB.".into()); }
        let evidence = evidence::read_run(&workspace, &root, run)?;
        if &evidence.adapter() != candidate { return Err(format!("Checker evidence changed during review of run {run}.")); }
        observation_count = observation_count.saturating_add(evidence.inputs.len());
        if observation_count > MAX_OBSERVATIONS { return Err("Check-review exceeds its 65536 observation budget.".into()); }
        selected.insert(run.clone(), evidence);
    }
    let ack_path = positionals.first().map(Path::new);
    let ack_before = if !selection_required {
        if let Some(path) = ack_path { Some(io::read_limit(path, io::MAX_DOCUMENT)?) } else { None }
    } else { None };
    let acknowledged: Vec<Acknowledgment> = if !selection_required {
        let all: Vec<Acknowledgment> = if let Some(bytes) = &ack_before {
            serde_json::from_slice(bytes).map_err(|e| format!("Invalid acknowledgments: {e}"))?
        } else { vec![] };
        all.into_iter().filter(|item| selected.contains_key(&item.run) || !headers.contains_key(&item.run)).collect()
    } else { vec![] };
    let resolved = resolve_acknowledgments(&selected, &acknowledged)?;
    let mut findings = Vec::new();
    let mut samples = Vec::<Sample>::new();
    let mut unsupported_comparisons = 0;
    for (run, evidence) in &selected {
        for difference in &evidence.findings {
            if !resolved.contains_key(&(run.clone(), difference.index)) {
                findings.push(Finding { run: run.clone(), difference: Difference::from(difference) });
            }
        }
        for (input, observed) in evidence.inputs.iter().zip(&evidence.observed) {
            let observe_only = evidence.report.expected.is_none();
            if !observe_only && observed.is_null() { unsupported_comparisons += 1; }
            samples.push(Sample {input: input.clone(), run: run.clone(), value: observed.clone(), observe_only });
        }
    }
    let mut by_input: HashMap<Input, usize> = HashMap::new();
    let mut groups = Vec::<Samples>::new();
    for sample in &samples {
        let input = Input::from(&sample.input);
        if let Some(&pos) = by_input.get(&input) {
            let group = &mut groups[pos];
            if !model::equal(&sample.value, &group.first)? { group.unstable = true; }
            group.observations.push(Observation {run:sample.run.clone(), observed:model::raw(sample.value.raw_json())});
        } else {
            by_input.insert(input.clone(), groups.len());
            groups.push(Samples { input, first: sample.value.clone(), unstable: false,
                observations: vec![Observation {run: sample.run.clone(), observed: model::raw(sample.value.raw_json())}] });
        }
    }
    let unstable: Vec<Unstable> = groups.into_iter().filter(|g| g.unstable)
        .map(|g| Unstable {input:g.input, observations:g.observations}).collect();

    let mut aggregate = BTreeMap::<String, (usize, usize, usize, BTreeSet<Vec<i64>>)>::new();
    for sample in &samples {
        let row = aggregate.entry(sample.input.operation.clone()).or_default();
        if sample.value.is_null() { row.2 += 1; }
        else if sample.observe_only { row.1 += 1; }
        else {
            row.0 += 1;
            if scope.as_ref().is_some_and(|scope| !scope.ordinary.iter().any(|v| {
                v.operation == sample.input.operation && v.args == sample.input.args
            })) { row.3.insert(sample.input.args.clone()); }
        }
    }
    let coverage: Vec<Coverage> = aggregate.into_iter().map(|(operation,(compared, observed_only, unsupported, outside))| Coverage {
        operation, compared, observed_only, unsupported,
        outside_ordinary: scope.as_ref().map(|_| outside.len()),
    }).collect();
    let missing: Vec<String> = scope.as_ref().map(|scope| {
        scope.operations.iter().filter(|operation| {
            !coverage.iter().any(|row| &row.operation == *operation &&
                row.outside_ordinary.unwrap_or(0) >= scope.minimum_outside_ordinary)
        }).cloned().collect::<BTreeSet<_>>().into_iter().collect()
    }).unwrap_or_default();
    let worklist = scope.as_ref().and_then(|scope| scope.sources.as_ref()).map(|sources| {
        let mut groups = Vec::<WorkGroup>::new();
        for source in sources.iter().filter(|v| missing.contains(&v.operation)) {
            if let Some(group) = groups.iter_mut().find(|group| group.component == source.component) {
                group.operations.push(source.clone());
            } else { groups.push(WorkGroup { component: source.component.clone(), operations: vec![source.clone()] }); }
        }
        groups
    });
    let complete = !selection_required && unstable.is_empty() && findings.is_empty() &&
        unsupported_comparisons == 0 && missing.is_empty() && coverage.iter().any(|row| row.compared > 0);
    fs::create_dir_all(&directory).map_err(|e| e.to_string())?;
    let review_path = directory.join("review.json");
    // The lock also prevents two review publishers from partially replacing a
    // review. It does not constitute execution authority over any adapter.
    let _lock = io::Lock::acquire(&review_path)?;
    if load_headers(&directory)? != headers { return Err("Check evidence changed during review publication.".into()); }
    if let (Some(path), Some(bytes)) = (ack_path, &ack_before) {
        if io::read_limit(path, io::MAX_DOCUMENT)? != *bytes {
            return Err("Acknowledgments changed during review publication.".into());
        }
    }
    for run in selected.keys() {
        let refreshed = evidence::read_run(&workspace, &directory.join(run), run)?;
        let original = &selected[run];
        if serde_json::to_string(&refreshed.report).map_err(|e| e.to_string())? !=
                serde_json::to_string(&original.report).map_err(|e| e.to_string())? ||
            refreshed.inputs != original.inputs ||
            refreshed.observed.len() != original.observed.len() ||
            refreshed.observed.iter().zip(&original.observed).any(|(a,b)| a.raw_json() != b.raw_json()) {
            return Err(format!("Evidence changed before review publication in {run}."));
        }
    }
    let report = serde_json::json!({
        "Format": REVIEW_FORMAT,
        "Complete": complete,
        "ScopeProvided": scope.is_some(),
        "RequiresRunSelection": selection_required,
        "EvidenceSets": sets,
        "Adapter": adapter,
        "Runs": selected.keys().collect::<Vec<_>>(),
        "Coverage": coverage,
        "UnsupportedComparisons": unsupported_comparisons,
        "UnstableInputs": unstable,
        "MinimumOutsideOrdinary": scope.as_ref().map(|s| s.minimum_outside_ordinary),
        "MissingCoverage": missing,
        "Worklist": worklist,
        "Acknowledged": acknowledged,
        "Outstanding": findings,
    });
    io::atomic_write(&review_path, &io::serialize(&report)?)?;
    let response = serde_json::json!({
        "Review": review_path.display().to_string(),
        "Complete": complete,
        "RequiresRunSelection": selection_required,
        "ScopeProvided": scope.is_some(),
        "EvidenceSets": if selection_required { Some(sets) } else { None },
        "UnstableInputs": unstable.len(),
        "UnsupportedComparisons": unsupported_comparisons,
        "Outstanding": findings.len(),
        "MissingCoverage": missing.len(),
        "NextComponent": worklist.as_ref().and_then(|v| v.first()).map(|v| &v.component),
    });
    println!("{}", serde_json::to_string(&response).map_err(|e| e.to_string())?);
    Ok(if complete || progress { 0 } else { 1 })
}

fn validate_scope(scope: &Scope) -> Result<(), String> {
    if scope.operations.is_empty() || scope.operations.iter().any(|v| v.chars().all(char::is_whitespace)) ||
        scope.ordinary.iter().any(|v| !model::input_valid(v)) || scope.minimum_outside_ordinary == 0 {
        return Err("The scope needs Operations, an Ordinary array of public examples, and a positive minimum if specified.".into());
    }
    if let Some(sources) = &scope.sources {
        let mut seen = HashSet::new();
        for source in sources {
            if !scope.operations.contains(&source.operation) || !seen.insert(&source.operation) ||
                [source.component.as_str(), source.contract.as_str(), source.implementation.as_str()]
                    .iter().any(|v| v.chars().all(char::is_whitespace)) {
                return Err("Each source entry needs a unique listed operation and its component, contract and implementation.".into());
            }
        }
    }
    Ok(())
}

pub(crate) fn load_headers(directory: &Path) -> Result<BTreeMap<String, Adapter>, String> {
    let mut runs = BTreeMap::new();
    if !directory.exists() { return Ok(runs); }
    for item in fs::read_dir(directory).map_err(|e| e.to_string())? {
        let item = item.map_err(|e| e.to_string())?;
        let path = item.path();
        if !item.file_type().map_err(|e| e.to_string())?.is_dir() { continue; }
        let run = item.file_name().to_string_lossy().to_string();
        if !io::is_run_id(&run) || !path.join("report.json").exists() { continue; }
        if runs.len() >= MAX_RUNS { return Err("Check review exceeds its 2048 completed-run budget.".into()); }
        let report: model::CheckRun = io::read_json(&path.join("report.json"))?;
        if report.format != model::CHECK_FORMAT || report.run != run || !evidence::valid_digest(&report.candidate) {
            return Err(format!("Inconsistent checker evidence in run {run}."));
        }
        runs.insert(run, Adapter::from(&report));
    }
    Ok(runs)
}

fn evidence_sets(headers: &BTreeMap<String, Adapter>) -> Vec<EvidenceSet> {
    let mut sets = Vec::<EvidenceSet>::new();
    for (run, adapter) in headers {
        if let Some(set) = sets.iter_mut().find(|set| &set.adapter == adapter) {
            set.runs.push(run.clone());
        } else {
            sets.push(EvidenceSet {adapter: adapter.clone(), runs: vec![run.clone()]});
        }
    }
    sets
}

/// Reusable for retest's locked acknowledgment update, and for check-review.
pub(crate) fn resolve_acknowledgments(
    selected: &BTreeMap<String, Evidence>, acks: &[Acknowledgment],
) -> Result<HashMap<(String, usize), String>, String> {
    let mut resolved = HashMap::new();
    for item in acks {
        let original = selected.get(&item.run).ok_or("Stale acknowledgment: missing original run.")?;
        let retest = selected.get(&item.retest_run).ok_or("Stale acknowledgment: missing retest run.")?;
        if item.run == item.retest_run || original.adapter() != retest.adapter() ||
            retest.report.expected.is_none() || item.retest_index >= retest.inputs.len() ||
            item.reason.chars().all(char::is_whitespace) {
            return Err("Each acknowledgment needs a finding, a separate retest of the same candidate and adapter, and a reason.".into());
        }
        let finding = original.findings.iter().find(|v| v.index == item.index)
            .ok_or("Stale acknowledgment: original finding no longer exists.")?;
        if !model::same_input(&finding.input, &retest.inputs[item.retest_index]) ||
            !model::equal(&finding.candidate, &retest.observed[item.retest_index])? {
            return Err(format!("The retest must reproduce the input and actual output for {}/{}.", item.run, item.index));
        }
        let expected = retest.report.expected.as_ref().expect("checked")
            .get(item.retest_index).ok_or("Retest expected value is missing.")?;
        let expected = JsonValue::parse(expected.get().as_bytes()).map_err(|e| e.to_string())?;
        let established = match item.disposition.as_str() {
            "confirmed" => model::equal(&finding.reference, &expected)?,
            "corrected-expectation" => model::equal(&finding.candidate, &expected)?,
            _ => false,
        };
        if !established {
            return Err(format!("The retest does not substantiate the disposition for {}/{}.", item.run, item.index));
        }
        for (run, evidence) in selected {
            for equivalent in &evidence.findings {
                if !model::same_input(&finding.input, &equivalent.input) ||
                    !model::equal(&finding.candidate, &equivalent.candidate)? ||
                    !model::equal(&finding.reference, &equivalent.reference)? { continue; }
                let key = (run.clone(), equivalent.index);
                if resolved.get(&key).is_some_and(|old| old != &item.disposition) {
                    return Err(format!("Conflicting dispositions for {}/{}.", run, equivalent.index));
                }
                resolved.insert(key, item.disposition.clone());
            }
        }
    }
    Ok(resolved)
}

pub(crate) fn validate_prior_acknowledgments(workspace: &Path, acks: &[Acknowledgment]) -> Result<(), String> {
    if acks.is_empty() { return Ok(()); }
    let directory = io::checks_dir(workspace);
    let headers = load_headers(&directory)?;
    let mut groups: Vec<(Adapter, Vec<Acknowledgment>)> = Vec::new();
    for ack in acks {
        let original = headers.get(&ack.run).ok_or_else(|| format!("Stale acknowledgment run {}.", ack.run))?;
        let retest = headers.get(&ack.retest_run)
            .ok_or_else(|| format!("Stale acknowledgment run {}.", ack.retest_run))?;
        if original != retest { return Err("Stale acknowledgment adapter mismatch.".into()); }
        if let Some((_, members)) = groups.iter_mut().find(|(adapter, _)| adapter == original) {
            members.push(ack.clone());
        } else { groups.push((original.clone(), vec![ack.clone()])); }
    }
    for (adapter, group) in groups {
        let mut selected = BTreeMap::new();
        for ack in &group {
            for run in [&ack.run, &ack.retest_run] {
                if !selected.contains_key(run) {
                    let evidence = evidence::read_run(workspace, &directory.join(run), run)?;
                    if evidence.adapter() != adapter { return Err("Stale acknowledgment adapter.".into()); }
                    selected.insert(run.clone(), evidence);
                }
            }
        }
        let _ = resolve_acknowledgments(&selected, &group)?;
    }
    Ok(())
}
