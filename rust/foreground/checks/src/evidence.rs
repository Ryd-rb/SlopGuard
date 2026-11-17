//! Immutable completed runs; incomplete/failed attempts never look completed.
use crate::{adapter, io, model::{self, CheckRun, CheckReceipt, Identity, CHECK_FORMAT, IDENTITY_FORMAT}};
use slophawk_probe_replay::{check, check_result_json, inputs_for_check, parse_inputs, parse_results,
    JsonValue, ProbeCheckResult, ProbeDifference, ProbeExpectation, ProbeInput};
use std::{fs, path::{Path, PathBuf}};

pub(crate) struct Evidence {
    pub report: CheckRun,
    pub inputs: Vec<ProbeInput>,
    pub observed: Vec<JsonValue>,
    pub findings: Vec<ProbeDifference>,
}
impl Evidence {
    pub fn adapter(&self) -> model::Adapter { model::Adapter::from(&self.report) }
}

pub(crate) struct Published {
    pub run: String,
    pub artifacts: PathBuf,
    pub report: CheckRun,
    pub observed: Vec<JsonValue>,
}
impl Published {
    pub fn receipt(&self) -> CheckReceipt {
        CheckReceipt {
            format: CHECK_FORMAT.to_owned(),
            run: self.run.clone(),
            report: self.artifacts.join("report.json").display().to_string(),
            artifacts: self.artifacts.display().to_string(),
            observe_only: self.report.expected.is_none(),
            result: self.report.result.clone(),
        }
    }
}

pub(crate) fn observe_result(count: usize, observed: &[JsonValue]) -> ProbeCheckResult {
    ProbeCheckResult {
        cases: count, compared: 0, unsupported: observed.iter().filter(|item| item.is_null()).count(),
        findings: vec![],
    }
}

pub(crate) fn execute(
    assembly: &Path, workspace: &Path, run_verb: Option<&str>, prepare_verb: Option<&str>,
    inputs: &[ProbeInput], cases: Option<&[ProbeExpectation]>, input_sha256: &str,
    original_input: Option<&Path>, parent: &Path, required_candidate: Option<&str>,
) -> Result<Published, String> {
    if !assembly.extension().and_then(|ext| ext.to_str()).is_some_and(|ext| ext.eq_ignore_ascii_case("dll")) {
        return Err("Pass a managed .NET assembly (.dll).".into());
    }
    if !workspace.is_dir() { return Err("The workspace directory must already exist.".into()); }
    if inputs.is_empty() || inputs.iter().any(|v| v.operation.chars().all(char::is_whitespace)) {
        return Err("Provide at least one request with an operation and arguments.".into());
    }
    if let Some(cases) = cases {
        let expected_inputs = inputs_for_check(cases).map_err(|e| e.to_string())?;
        if expected_inputs.as_slice() != inputs { // explicit ordered request/expectation binding
            return Err("The check inputs must match the ordered expectations.".into());
        }
    }
    let candidate = io::hash_assembly(assembly)?;
    if required_candidate.is_some_and(|required| required != candidate) {
        return Err("The retest requires the original candidate assembly.".into());
    }
    fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    let run = io::new_id();
    let pending = parent.join(format!(".pending-{run}"));
    fs::create_dir(&pending).map_err(|e| e.to_string())?;
    let result: Result<Published, String> = (|| -> Result<Published, String> {
        let request_bytes = io::serialize(&inputs.iter().map(model::Input::from).collect::<Vec<_>>())?;
        io::write_new(&pending.join("requests.json"), &request_bytes)?;
        let (adapter_input, prepared_digest) = if let Some(verb) = prepare_verb {
            let output = adapter::execute(assembly, Some(verb), &pending.join("requests.json"), &pending)?;
            // The C# prepare contract allows any valid JSON value, not only an array.
            JsonValue::parse(&output).map_err(|e| format!("Invalid prepared JSON: {e}"))?;
            io::write_new(&pending.join("prepared.json"), &output)?;
            (pending.join("prepared.json"), Some(io::hash(&output)))
        } else {
            (pending.join("requests.json"), None)
        };
        let output = adapter::execute(assembly, run_verb, &adapter_input, &pending)?;
        let observed = parse_results(&output).map_err(|e| format!("Adapter results: {e}"))?;
        if observed.len() != inputs.len() {
            return Err("The adapter must return exactly one result per request, in input order.".into());
        }
        if io::hash_assembly(assembly)? != candidate {
            return Err("The candidate assembly changed during this check.".into());
        }
        if io::hash(&io::read_limit(&pending.join("requests.json"), io::MAX_DOCUMENT)?) != io::hash(&request_bytes) {
            return Err("The adapter changed the ordered requests.".into());
        }
        if let Some(ref digest) = prepared_digest {
            if io::hash(&io::read_limit(&pending.join("prepared.json"), io::MAX_DOCUMENT)?) != *digest {
                return Err("The adapter changed the prepared requests.".into());
            }
        }
        if let Some(source) = original_input {
            if io::hash(&io::read_limit(source, io::MAX_DOCUMENT)?) != input_sha256 {
                return Err("The original input changed during this check.".into());
            }
        }
        io::write_new(&pending.join("observed.json"), &output)?;
        let comparison = if let Some(cases) = cases {
            check(cases, &observed).map_err(|e| e.to_string())?
        } else {
            observe_result(inputs.len(), &observed)
        };
        let report = CheckRun {
            format: CHECK_FORMAT.to_owned(), run: run.clone(), candidate: candidate.clone(),
            run_verb: run_verb.map(str::to_owned), prepare_verb: prepare_verb.map(str::to_owned),
            result: model::raw(&check_result_json(&comparison)),
            expected: cases.map(|cases| cases.iter().map(|v| model::raw(v.expected.raw_json())).collect()),
        };
        io::write_new(&pending.join("report.json"), &io::serialize(&report)?)?;
        let identity = Identity {
            format: IDENTITY_FORMAT.to_owned(), run: run.clone(),
            root: workspace.display().to_string(),
            mode: if cases.is_some() { "check" } else { "check-observe" }.into(),
            input_sha256: input_sha256.to_owned(), requests_sha256: io::hash(&request_bytes),
            prepared_sha256: prepared_digest, observed_sha256: io::hash(&output),
            candidate, run_verb: run_verb.map(str::to_owned), prepare_verb: prepare_verb.map(str::to_owned),
        };
        io::write_new(&pending.join("identity.json"), &io::serialize(&identity)?)?;
        let destination = parent.join(&run);
        fs::rename(&pending, &destination).map_err(|e| format!("Cannot publish check run: {e}"))?;
        Ok(Published { run: run.clone(), artifacts: destination, report, observed })
    })();
    match result {
        Ok(published) => Ok(published),
        Err(error) => {
            // Keep bounded non-authoritative diagnostics and the original request
            // for investigation; review deliberately ignores .failed directories.
            let details = serde_json::json!({"Run":run,"Error":error.chars().take(2048).collect::<String>()});
            if let Ok(bytes) = io::serialize(&details) { let _ = io::write_new(&pending.join("failure.json"), &bytes); }
            let failed = parent.join(format!(".failed-{run}"));
            let outcome = fs::rename(&pending, &failed);
            Err(match outcome {
                Ok(()) => format!("{error} (incomplete attempt: {})", failed.display()),
                Err(_) => format!("{error} (attempt: {})", pending.display()),
            })
        }
    }
}

pub(crate) fn read_run(workspace: &Path, path: &Path, run: &str) -> Result<Evidence, String> {
    if !io::is_run_id(run) { return Err("Invalid checker run ID.".into()); }
    let report: CheckRun = io::read_json(&path.join("report.json"))?;
    if report.format != CHECK_FORMAT || report.run != run || !valid_digest(&report.candidate) {
        return Err(format!("Inconsistent checker evidence in run {run}."));
    }
    let requests_bytes = io::read_limit(&path.join("requests.json"), io::MAX_DOCUMENT)?;
    let observed_bytes = io::read_limit(&path.join("observed.json"), io::MAX_DOCUMENT)?;
    let inputs = parse_inputs(&requests_bytes).map_err(|e| e.to_string())?;
    let observed = parse_results(&observed_bytes).map_err(|e| e.to_string())?;
    if inputs.len() != observed.len() || inputs.iter().any(|v| v.operation.chars().all(char::is_whitespace)) {
        return Err(format!("Inconsistent checker evidence in run {run}."));
    }
    let comparison = if let Some(expectations) = &report.expected {
        if inputs.len() != expectations.len() {
            return Err(format!("Inconsistent checker evidence in run {run}."));
        }
        let cases = inputs.iter().zip(expectations.iter()).map(|(input, value)| {
            JsonValue::parse(value.get().as_bytes()).map(|expected| ProbeExpectation {
                operation: input.operation.clone(), args: input.args.clone(), expected,
            })
        }).collect::<Result<Vec<_>,_>>().map_err(|e| e.to_string())?;
        check(&cases, &observed).map_err(|e| e.to_string())?
    } else { observe_result(inputs.len(), &observed) };
    let actual_result = JsonValue::parse(report.result.get().as_bytes()).map_err(|e| e.to_string())?;
    let expected_result = JsonValue::parse(check_result_json(&comparison).as_bytes()).map_err(|e| e.to_string())?;
    if !model::equal(&actual_result, &expected_result)? {
        return Err(format!("Checker report disagrees with the saved inputs and observations in run {run}."));
    }
    if path.join("identity.json").exists() {
        let identity: Identity = io::read_json(&path.join("identity.json"))?;
        let prepared_ok = match &identity.prepared_sha256 {
            Some(digest) => path.join("prepared.json").exists() &&
                io::read_limit(&path.join("prepared.json"), io::MAX_DOCUMENT)
                    .map(|b| io::hash(&b) == *digest).unwrap_or(false),
            None => !path.join("prepared.json").exists(),
        };
        let required_mode = if report.expected.is_some() { "check" } else { "check-observe" };
        if identity.format != IDENTITY_FORMAT || identity.run != run || identity.root != workspace.display().to_string() ||
            identity.candidate != report.candidate || identity.run_verb != report.run_verb ||
            identity.prepare_verb != report.prepare_verb || !valid_digest(&identity.input_sha256) ||
            identity.mode != required_mode ||
            identity.requests_sha256 != io::hash(&requests_bytes) ||
            identity.observed_sha256 != io::hash(&observed_bytes) || !prepared_ok
        {
            return Err(format!("Checker evidence identity changed in run {run}."));
        }
    }
    Ok(Evidence { report, inputs, observed, findings: comparison.findings })
}

pub(crate) fn valid_digest(digest: &str) -> bool {
    digest.len() == 64 && digest.bytes().all(|b| b.is_ascii_hexdigit())
}
