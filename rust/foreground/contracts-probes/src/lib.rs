//! Foreground qualification, probe JSON, and search/replay decisions for the
//! pinned SlopHawk C# commands. No compiler or adapter executes in this library.
//! The binary's explicit C# `check` peer owns executable observations; IL hints
//! are untrusted *search hints*, never sources of expected results.

pub mod passive;
pub mod command;
pub mod selection_audit;

#[cfg(test)]
mod selection_tests;

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, value::RawValue};
use sha2::{Digest, Sha256};
use slophawk_contracts::{ContractCases, ContractQualification, IntegerContract, ProbeExpectation, ProbeInput};
use slophawk_probe_replay::{
    JsonValue, ProbeCheckResult, check, inputs_for_check, parse_expectations, parse_inputs,
    parse_results,
};
use std::collections::HashSet;
use selection_audit::SelectionAudit;

pub const SOURCE_LIMIT: usize = 8 * 1024 * 1024;
pub const BATCH_SIZE: usize = 2048;
pub const FORMAT: &str = "slophawk-contract-search-v1";

pub type Result<T> = std::result::Result<T, String>;

/// Match the C# catalog's ordered `Let` and `Basis` dictionaries. serde_json's
/// `preserve_order` feature is intentional: locals are evaluated in source order.
/// Expected numbers are NOT interpreted with `serde_json::Number` during replay.
pub fn read_rules(source: &[u8]) -> Result<Vec<IntegerContract>> {
    let data = json_document(source)?;
    let rows = data.as_array().ok_or("Rules must be a JSON array.")?;
    if rows.len() > 64 {
        return Err("More than 64 contracts exceed the Rust contract scope.".into());
    }
    rows.iter().map(|row| {
        let fields = row.as_object().ok_or("A contract must be an object.")?;
        // C#'s positional record deserializer admits missing/null nullable
        // fields and lets ContractRule report qualification failures *per
        // operation*. Wrong JSON types are still global decode errors.
        let operation = string_field(fields, "Operation")?;
        let parameters = match field(fields, "Parameters") {
            None | Some(Value::Null) => Vec::new(),
            Some(Value::Array(values)) => values.iter().map(|item|
                item.as_str().map(str::to_owned)
                    .ok_or_else(|| "Contract Parameters must be strings.".into()))
                .collect::<Result<Vec<_>>>()?,
            Some(_) => return Err("Contract Parameters must be a string array.".into()),
        };
        let domain = string_field(fields, "Domain")?;
        let expected = string_field(fields, "Expected")?;
        let source = string_field(fields, "Source")?;
        let lets = ordered_map(fields, "Let")?.map(|(name, value)| {
            Ok((name.to_owned(), value.as_str().ok_or("A Let expression must be a string.")?.to_owned()))
        }).collect::<Result<Vec<_>>>()?;
        let basis = ordered_map(fields, "Basis")?.map(|(name, values)| {
            let numbers = values.as_array().ok_or("Each Basis entry must be an integer array.")?
                .iter().map(as_i64).collect::<Result<Vec<_>>>()?;
            Ok((name.to_owned(), numbers))
        }).collect::<Result<Vec<_>>>()?;
        Ok(IntegerContract {operation, parameters, domain, expected, source, lets, basis})
    }).collect()
}

fn field<'a>(fields: &'a Map<String, Value>, name: &str) -> Option<&'a Value> {
    fields.iter().rev().find(|(key, _)| key.eq_ignore_ascii_case(name)).map(|(_, value)| value)
}
fn string_field(fields: &Map<String, Value>, name: &str) -> Result<String> {
    match field(fields, name) {
        None | Some(Value::Null) => Ok(String::new()),
        Some(Value::String(value)) => Ok(value.clone()),
        Some(_) => Err(format!("Contract {name} must be a string.")),
    }
}
fn ordered_map<'a>(fields: &'a Map<String, Value>, name: &str)
    -> Result<impl Iterator<Item = (&'a str, &'a Value)>> {
    let obj = match field(fields, name) {
        None | Some(Value::Null) => None,
        Some(value) => Some(value.as_object().ok_or_else(|| format!("{name} must be an object."))?),
    };
    Ok(obj.into_iter().flat_map(|obj| obj.iter().map(|(k, v)| (k.as_str(), v))))
}
fn as_i64(value: &Value) -> Result<i64> {
    value.as_i64().ok_or_else(|| "Expected a signed 64-bit JSON integer.".into())
}
fn json_document(bytes: &[u8]) -> Result<Value> {
    if bytes.len() > SOURCE_LIMIT {
        return Err("JSON input exceeds the 8 MiB source budget.".into());
    }
    serde_json::from_slice(strip_bom(bytes)).map_err(|error| format!("Invalid JSON: {error}"))
}
pub fn strip_bom(bytes: &[u8]) -> &[u8] {
    bytes.strip_prefix(&[0xef, 0xbb, 0xbf]).unwrap_or(bytes)
}
pub fn qualify(rules: &[u8], published: &[u8]) -> Result<ContractCases> {
    let parsed = read_rules(rules)?;
    let cases = parse_expectations(strip_bom(published)).map_err(|e| e.to_string())?;
    ContractCases::new(&parsed, &cases).map_err(|error| error.to_string())
}

/// Qualify supplied rules against their examples and select cases without executing an adapter.
pub fn diversify_cases(rules: &[u8], published: &[u8], pool: &[u8], budget: usize) -> Result<Value> {
    let catalog = qualify(rules, published)?;
    let inputs = slophawk_probe_replay::parse_inputs(pool).map_err(|error| error.to_string())?;
    let cases = catalog.select_diverse(&inputs, budget).map_err(|error| error.to_string())?;
    let cases: Value = serde_json::from_str(&expectations_json(&cases)?)
        .map_err(|error| error.to_string())?;
    Ok(serde_json::json!({
        "qualification": qualifications_json(catalog.qualification()),
        "cases": cases,
    }))
}

pub fn sha256_hex(source: &[u8]) -> String {
    format!("{:X}", Sha256::digest(source))
}

pub fn read_scales(bytes: &[u8]) -> Result<Vec<i64>> {
    json_document(bytes)?.as_array().ok_or("Scales must be a JSON array.")?
        .iter().map(as_i64).collect()
}

#[derive(Serialize)]
#[allow(non_snake_case)]
struct OutputInput<'a> { Operation: &'a str, Args: &'a [i64] }
#[derive(Serialize)]
#[allow(non_snake_case)]
struct OutputCase<'a> { Operation: &'a str, Args: &'a [i64], Expected: Box<RawValue> }

pub fn inputs_json(inputs: &[ProbeInput]) -> Result<String> {
    let rows: Vec<_> = inputs.iter().map(|item| OutputInput {
        Operation: &item.operation, Args: &item.args,
    }).collect();
    serde_json::to_string(&rows).map_err(|e| e.to_string())
}
pub fn expectations_json(cases: &[ProbeExpectation]) -> Result<String> {
    let rows = cases.iter().map(|case| Ok(OutputCase {
        Operation: &case.operation, Args: &case.args,
        Expected: raw(case.expected.raw_json())?,
    })).collect::<Result<Vec<_>>>()?;
    serde_json::to_string(&rows).map_err(|e| e.to_string())
}
pub fn values_json(values: &[JsonValue]) -> String {
    format!("[{}]", values.iter().map(JsonValue::raw_json).collect::<Vec<_>>().join(","))
}
pub fn raw(text: &str) -> Result<Box<RawValue>> {
    RawValue::from_string(text.into()).map_err(|e| e.to_string())
}

/// IL literals are optional, separately supplied observations from a trusted
/// C# peer. The envelope binds the *exact* assembly SHA and operation arities.
/// They guide generated inputs but NEVER qualify rules or set expectations.
pub fn read_literal_hints(bytes: &[u8], qualified: &[(String, usize)],
    candidate_sha: &str) -> Result<Vec<(String, Vec<i64>)>> {
    let data = json_document(bytes)?;
    let envelope = data.as_object().ok_or("Literal hints must be a versioned envelope.")?;
    if field(envelope, "Format").and_then(Value::as_str) != Some("slophawk-il-literals-v1") {
        return Err("Unsupported IL literal hint format.".into());
    }
    if field(envelope, "AssemblySha256").and_then(Value::as_str) != Some(candidate_sha) {
        return Err("IL literal hints were derived from a different assembly.".into());
    }
    let entries = field(envelope, "Operations").and_then(Value::as_array)
        .ok_or("IL hints must contain ordered Operations.")?;
    if entries.len() > 64 { return Err("Too many hinted operations.".into()); }
    let mut hints = Vec::new();
    for entry in entries {
        let props = entry.as_object().ok_or("A literal hint entry must be an object.")?;
        let operation = field(props, "Operation").and_then(Value::as_str)
            .ok_or("Literal hint operation is required.")?;
        let arity = field(props, "Arity").and_then(Value::as_u64)
            .ok_or("Literal hint arity is required.")?;
        if !qualified.iter().any(|(name, count)| name == operation && *count as u64 == arity) {
            return Err(format!("Literal hints have an unqualified operation/arity: {operation}"));
        }
        if hints.iter().any(|(name, _): &(String, Vec<i64>)| name == operation) {
            return Err(format!("Duplicate literal hint operation: {operation}"));
        }
        let values = field(props, "Literals").and_then(Value::as_array)
            .ok_or("Literal hint values must be integer arrays.")?;
        if values.len() > 256 { return Err("More than 256 literals for one operation.".into()); }
        hints.push((operation.to_owned(), values.iter().map(as_i64)
            .collect::<Result<Vec<_>>>()?));
    }
    Ok(hints)
}

/// Validated receipt and raw observations from a completed C# foreground
/// `check` invocation. This is NOT an authorization or an execution grant.
#[derive(Clone, Debug)]
pub struct Observation {
    pub run: String,
    pub candidate: String,
    pub artifacts: String,
    pub report: String,
    pub observed: Vec<JsonValue>,
    pub result: ProbeCheckResult,
}

/// A peer must reject missing/bad receipts, input/output length changes,
/// authority changes, failed adapters, and invalid run identity. A false
/// observation is never interpreted as an unsupported contract case.
pub trait CheckPeer {
    fn check(&mut self, cases: &[ProbeExpectation]) -> Result<Observation>;
}

#[derive(Clone, Debug)]
pub struct ReplayChange {
    pub input: ProbeInput,
    pub expected: JsonValue,
    pub before: JsonValue,
    pub after: JsonValue,
    pub initial_run: String,
    pub initial_index: usize,
    pub replay_index: usize,
}

#[derive(Clone, Debug)]
pub struct OperationSummary {
    pub operation: String,
    pub source: Option<String>,
    pub error: Option<String>,
    pub compared: usize,
    pub difference: bool,
}

#[derive(Clone, Debug)]
pub enum Publication {
    None,
    /// Promote the *separately executed* replay's complete C# report/artifacts.
    Promote(Observation),
    /// Publish only stable comparisons, preserving their ORIGINAL replay bytes.
    Subset { cases: Vec<ProbeExpectation>, observed: Vec<JsonValue>, candidate: String },
}

pub struct SearchOutcome {
    pub qualification: Vec<ContractQualification>,
    pub operations: Vec<OperationSummary>,
    pub compared: usize,
    pub unsupported: usize,
    pub observed_discrepancies: usize,
    pub replay_unsupported: usize,
    pub replay_report: Option<String>,
    pub replay_changes: Vec<ReplayChange>,
    pub publication: Publication,
    /// Present only for the opt-in live diverse search, never read-only planning.
    pub selection_audit: Option<SelectionAudit>,
}

impl SearchOutcome {
    pub fn success(&self, report_present: bool, findings: usize) -> bool {
        report_present && self.replay_changes.is_empty() && self.observed_discrepancies == 0
            && findings == 0 && self.unsupported == 0
            && self.qualification.iter().all(|rule| rule.error.is_none())
            && self.operations.iter().all(|rule| rule.compared > 0)
    }
}

#[derive(Clone)]
struct Witness { case: ProbeExpectation, observed: JsonValue, run: String, index: usize }

struct SearchState {
    selected: Vec<(String, Witness)>,
    matched: Vec<(String, Witness)>,
    seen: HashSet<(String, Vec<i64>)>,
    remaining: HashSet<String>,
    processed: Vec<(String, usize)>,
    compared: usize,
    unsupported: usize,
    candidate: Option<String>,
    selection_audit: Option<SelectionAudit>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CaseSelection {
    Legacy,
    DiverseV1,
    BoundaryPairsV1,
}

/// Search and replay are kept separate even for a clean first result. In
/// particular, *new* discrepancies, vanished discrepancies and changed null
/// support in replay always survive in `ReplayChanges` verbatim.
pub fn search(catalog: &ContractCases, per_operation: usize, hints: &[(String, Vec<i64>)],
    peer: &mut impl CheckPeer) -> Result<SearchOutcome> {
    search_with_selection(catalog, per_operation, hints, peer, CaseSelection::Legacy)
}

pub fn search_with_selection(catalog: &ContractCases, per_operation: usize,
    hints: &[(String, Vec<i64>)], peer: &mut impl CheckPeer,
    selection: CaseSelection) -> Result<SearchOutcome> {
    if !(1..=8192).contains(&per_operation) {
        return Err("The per-operation budget must be between 1 and 8192.".into());
    }
    let qualification = catalog.qualification().to_vec();
    let remaining: HashSet<String> = qualification.iter().filter(|q| q.error.is_none())
        .map(|q| q.operation.clone()).collect();
    let mut state = SearchState {
        processed: qualification.iter().filter(|q| q.error.is_none())
            .map(|q| (q.operation.clone(), 0)).collect(),
        selected: Vec::new(), matched: Vec::new(), seen: HashSet::new(),
        remaining, compared: 0, unsupported: 0, candidate: None,
        selection_audit: (selection == CaseSelection::DiverseV1).then(SelectionAudit::new),
    };
    pass(catalog, per_operation.min(64), 0, &[], peer, &mut state, selection)?;
    if per_operation > 64 && !state.remaining.is_empty() {
        pass(catalog, per_operation - 64, 1, hints, peer, &mut state, selection)?;
    } else if per_operation > 64 {
        if let Some(audit) = &mut state.selection_audit {
            for (operation, _) in catalog.parameter_counts() {
                audit.skip_pass(operation, 1, per_operation - 64,
                    ((per_operation - 64) * 4).min(8192));
            }
        }
    }
    let mut witnesses: Vec<Witness> = state.selected.iter().map(|(_, witness)| witness.clone()).collect();
    witnesses.extend(state.matched.iter().filter(|(op, _)|
        !state.selected.iter().any(|(name, _)| name == op))
        .map(|(_, witness)| witness.clone()));
    let (publication, replay_report, replay_unsupported, replay_changes) = if witnesses.is_empty() {
        (Publication::None, None, 0, Vec::new())
    } else {
        let cases: Vec<_> = witnesses.iter().map(|w| w.case.clone()).collect();
        let replay = peer.check(&cases)?;
        validate_observation(&cases, &replay, &mut state.candidate)?;
        if let Some(audit) = &mut state.selection_audit {
            audit.record_replay(&cases, &replay)?;
        }
        let mut stable_cases = Vec::new();
        let mut stable_observed = Vec::new();
        let mut changes = Vec::new();
        for (i, (witness, actual)) in witnesses.iter().zip(&replay.observed).enumerate() {
            if witness.observed.deep_equals(actual).map_err(|e| e.to_string())? {
                stable_cases.push(witness.case.clone());
                stable_observed.push(actual.clone());
            } else {
                changes.push(ReplayChange {
                    input: ProbeInput {operation: witness.case.operation.clone(), args: witness.case.args.clone()},
                    expected: witness.case.expected.clone(), before: witness.observed.clone(),
                    after: actual.clone(), initial_run: witness.run.clone(),
                    initial_index: witness.index, replay_index: i,
                });
            }
        }
        let publication = if changes.is_empty() {
            Publication::Promote(replay.clone())
        } else if !stable_cases.is_empty() {
            Publication::Subset {cases: stable_cases, observed: stable_observed, candidate: replay.candidate.clone()}
        } else { Publication::None };
        (publication, Some(replay.report), replay.result.unsupported, changes)
    };
    let operations = qualification.iter().map(|q| OperationSummary {
        operation: q.operation.clone(), source: q.source.clone(), error: q.error.clone(),
        compared: state.processed.iter().find(|(op, _)| op == &q.operation).map_or(0, |(_, count)| *count),
        difference: state.selected.iter().any(|(op, _)| op == &q.operation),
    }).collect();
    Ok(SearchOutcome { qualification, operations, compared: state.compared,
        unsupported: state.unsupported, observed_discrepancies: state.selected.len(),
        replay_unsupported, replay_report, replay_changes, publication,
        selection_audit: state.selection_audit })
}

fn pass(catalog: &ContractCases, limit: usize, seed: i32, hints: &[(String, Vec<i64>)],
    peer: &mut impl CheckPeer, state: &mut SearchState, selection: CaseSelection) -> Result<()> {
    let mut batch = Vec::new();
    // The C# generator constructs a fresh Random(seed) for *each* rule. A
    // one-operation filter avoids the Rust planner's 65536 total-plan cap
    // without changing any per-rule RNG draw, interleave or operation order.
    for (operation, _) in catalog.parameter_counts() {
        let pool_limit = match selection {
            CaseSelection::Legacy | CaseSelection::BoundaryPairsV1 => limit,
            CaseSelection::DiverseV1 => (limit * 4).min(8192),
        };
        if !state.remaining.contains(operation) {
            if let Some(audit) = &mut state.selection_audit {
                audit.skip_pass(operation, seed, limit, pool_limit);
            }
            continue;
        }
        let only = HashSet::from([operation.clone()]);
        let generated = catalog.generate(pool_limit, seed, Some(&only), seed != 0, hints)
            .map_err(|error| error.to_string())?;
        let selected = if selection == CaseSelection::DiverseV1 {
            let pool = generated.iter().filter_map(|test| {
                let key = (test.operation.clone(), test.args.clone());
                (!state.seen.contains(&key)).then_some(ProbeInput {
                    operation: test.operation.clone(), args: test.args.clone(),
                })
            }).collect::<Vec<_>>();
            let selected = catalog.select_diverse(&pool, limit).map_err(|error| error.to_string())?;
            if let Some(audit) = &mut state.selection_audit {
                audit.nominate(operation, seed, limit, pool_limit, &generated,
                    generated.len() - pool.len(), &selected)?;
            }
            selected
        } else if selection == CaseSelection::BoundaryPairsV1 && seed == 0 {
            let mut boundary_cases = catalog.boundary_pair_cases(operation, limit.div_ceil(2));
            let mut seen: HashSet<_> = boundary_cases.iter().map(|case| case.args.clone()).collect();
            let remaining = limit - boundary_cases.len();
            boundary_cases.extend(generated.into_iter()
                .filter(|test| seen.insert(test.args.clone()))
                .take(remaining));
            boundary_cases
        } else {
            generated
        };
        for test in selected {
            batch.push(test);
            if batch.len() == BATCH_SIZE {
                flush(&mut batch, peer, state, seed)?;
            }
        }
    }
    flush(&mut batch, peer, state, seed)
}

fn flush(batch: &mut Vec<ProbeExpectation>, peer: &mut impl CheckPeer,
    state: &mut SearchState, pass: i32) -> Result<()> {
    let mut to_check = Vec::new();
    for test in batch.drain(..) {
        // Preserve the original short-circuit admission order: an operation
        // stopped by a prior batch never enters the cross-pass seen set.
        let after_finding = !state.remaining.contains(&test.operation);
        if after_finding || !state.seen.insert((test.operation.clone(), test.args.clone())) {
            if let Some(audit) = &mut state.selection_audit {
                audit.skip_case(pass, &test.operation, after_finding);
            }
        } else {
            to_check.push(test);
        }
    }
    if to_check.is_empty() { return Ok(()); }
    let receipt = peer.check(&to_check)?;
    validate_observation(&to_check, &receipt, &mut state.candidate)?;
    if let Some(audit) = &mut state.selection_audit {
        audit.record_batch(pass, &to_check, &receipt)?;
    }
    state.compared += receipt.result.compared;
    state.unsupported += receipt.result.unsupported;
    for (index, (test, actual)) in to_check.iter().zip(&receipt.observed).enumerate() {
        if actual.is_null() { continue; }
        if let Some((_, count)) = state.processed.iter_mut().find(|(op, _)| op == &test.operation) {
            *count += 1;
        }
        if actual.deep_equals(&test.expected).map_err(|e| e.to_string())?
            && !state.matched.iter().any(|(op, _)| op == &test.operation) {
            state.matched.push((test.operation.clone(), Witness {
                case: test.clone(), observed: actual.clone(), run: receipt.run.clone(), index,
            }));
        }
    }
    for mismatch in &receipt.result.findings {
        if !state.selected.iter().any(|(op, _)| op == &mismatch.input.operation) {
            let test = &to_check[mismatch.index];
            state.selected.push((test.operation.clone(), Witness {
                case: test.clone(), observed: mismatch.candidate.clone(),
                run: receipt.run.clone(), index: mismatch.index,
            }));
        }
        state.remaining.remove(&mismatch.input.operation);
    }
    Ok(())
}

fn validate_observation(cases: &[ProbeExpectation], observation: &Observation,
    candidate: &mut Option<String>) -> Result<()> {
    inputs_for_check(cases).map_err(|e| e.to_string())?;
    if observation.observed.len() != cases.len() {
        return Err("Peer returned an observation count inconsistent with the requests.".into());
    }
    let computed = check(cases, &observation.observed).map_err(|e| e.to_string())?;
    if computed.cases != observation.result.cases || computed.compared != observation.result.compared
        || computed.unsupported != observation.result.unsupported
        || computed.findings.len() != observation.result.findings.len() {
        return Err("Peer result counters or findings disagree with raw observations.".into());
    }
    for (left, right) in computed.findings.iter().zip(&observation.result.findings) {
        if left.index != right.index || left.input != right.input
            || !left.candidate.deep_equals(&right.candidate).map_err(|e| e.to_string())?
            || !left.reference.deep_equals(&right.reference).map_err(|e| e.to_string())? {
            return Err("Peer finding differs from raw replay comparison.".into());
        }
    }
    if observation.candidate.is_empty() || observation.run.is_empty() {
        return Err("Peer omitted candidate or run identity.".into());
    }
    if let Some(previous) = candidate.as_ref() {
        if previous != &observation.candidate { return Err("Candidate assembly changed during search.".into()); }
    } else { *candidate = Some(observation.candidate.clone()); }
    Ok(())
}

#[derive(Deserialize)]
#[allow(non_snake_case)]
pub struct PeerReceipt {
    pub Format: String,
    pub Run: String,
    pub Report: String,
    pub Artifacts: String,
    pub ObserveOnly: bool,
    pub Result: Box<RawValue>,
}
#[derive(Deserialize)]
#[allow(non_snake_case)]
struct PeerReport {
    Format: String,
    Run: String,
    Candidate: String,
    RunVerb: Option<String>,
    PrepareVerb: Option<String>,
    Result: Box<RawValue>,
    Expected: Option<Vec<Box<RawValue>>>,
}

/// Verify C# stdout receipt, report, requests and actual observations (all
/// within an isolated staging workspace), before search treats it as evidence.
/// The peer's Result/Expected nodes are kept as raw JSON, not floating-point
/// serde_json::Value: exact i64 and duplicate JSON keys must survive.
pub fn parse_peer_evidence(receipt_bytes: &[u8], report_bytes: &[u8],
    requests_bytes: &[u8], observed_bytes: &[u8], cases: &[ProbeExpectation],
    actual_candidate_sha: &str, run_verb: Option<&str>, prepare_verb: Option<&str>)
    -> Result<(PeerReceipt, Observation)> {
    if receipt_bytes.len() > SOURCE_LIMIT || report_bytes.len() > SOURCE_LIMIT {
        return Err("Peer terminal/report exceeded 8 MiB.".into());
    }
    let receipt: PeerReceipt = serde_json::from_slice(receipt_bytes).map_err(|e| e.to_string())?;
    if receipt.Format != "slophawk-check-v1" || receipt.ObserveOnly
        || receipt.Run.len() != 32 || !receipt.Run.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err("Invalid or incomplete C# check terminal receipt.".into());
    }
    let report: PeerReport = serde_json::from_slice(report_bytes).map_err(|e| e.to_string())?;
    if report.Format != receipt.Format || report.Run != receipt.Run
        || report.Candidate != actual_candidate_sha
        || report.RunVerb.as_deref() != run_verb
        || report.PrepareVerb.as_deref() != prepare_verb {
        return Err("C# report terminal identity, candidate, or verbs did not match.".into());
    }
    let requests = parse_inputs(requests_bytes).map_err(|e| e.to_string())?;
    let expected = inputs_for_check(cases).map_err(|e| e.to_string())?;
    if requests != expected { return Err("Peer rewrote the ordered check requests.".into()); }
    let observed = parse_results(observed_bytes).map_err(|e| e.to_string())?;
    let result = check(cases, &observed).map_err(|e| e.to_string())?;
    let computed = JsonValue::parse(slophawk_probe_replay::check_result_json(&result).as_bytes())
        .map_err(|e| e.to_string())?;
    for actual in [receipt.Result.get(), report.Result.get()] {
        let parsed = JsonValue::parse(actual.as_bytes()).map_err(|e| e.to_string())?;
        if !computed.deep_equals(&parsed).map_err(|e| e.to_string())? {
            return Err("Peer report/result disagrees with raw candidate observations.".into());
        }
    }
    let report_expected = report.Expected.ok_or("Peer report lacks expected values.")?;
    if report_expected.len() != cases.len() {
        return Err("Peer Expected count differs from requested cases.".into());
    }
    for (original, actual) in cases.iter().zip(&report_expected) {
        let value = JsonValue::parse(actual.get().as_bytes()).map_err(|e| e.to_string())?;
        if !original.expected.deep_equals(&value).map_err(|e| e.to_string())? {
            return Err("Peer replaced a contract expected value.".into());
        }
    }
    Ok((receipt, Observation {
        run: report.Run, candidate: actual_candidate_sha.into(),
        artifacts: String::new(), report: String::new(), observed, result,
    }))
}

#[derive(Serialize)]
#[serde(rename_all = "PascalCase")]
struct ReplayChangeJson<'a> {
    input: OutputInput<'a>, expected: Box<RawValue>, before: Box<RawValue>, after: Box<RawValue>,
    initial_run: &'a str, initial_index: usize, replay_index: usize,
}

pub fn replay_changes_json(changes: &[ReplayChange]) -> Result<String> {
    let rows = changes.iter().map(|c| Ok(ReplayChangeJson {
        input: OutputInput {Operation: &c.input.operation, Args: &c.input.args},
        expected: raw(c.expected.raw_json())?, before: raw(c.before.raw_json())?,
        after: raw(c.after.raw_json())?, initial_run: &c.initial_run,
        initial_index: c.initial_index, replay_index: c.replay_index,
    })).collect::<Result<Vec<_>>>()?;
    serde_json::to_string(&rows).map_err(|e| e.to_string())
}

pub fn qualifications_json(qualifications: &[ContractQualification]) -> Value {
    Value::Array(qualifications.iter().map(|q| serde_json::json!({
        "Operation": q.operation, "Source": q.source, "Error": q.error,
    })).collect())
}
pub fn operations_json(operations: &[OperationSummary]) -> Value {
    Value::Array(operations.iter().map(|o| serde_json::json!({
        "Operation": o.operation, "Source": o.source, "Error": o.error,
        "Compared": o.compared, "Difference": o.difference,
    })).collect())
}
