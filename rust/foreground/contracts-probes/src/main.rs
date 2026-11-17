//! A separate, opt-in Rust entry point. The shipped `slophawk` binary and its
//! MCP / private-IPC interfaces remain unchanged until owner integration.
use serde::Serialize;
use serde_json::{Value, json, value::RawValue};
use sha2::{Digest, Sha256};
use slophawk_contracts::{ContractCases, plan_inputs};
use slophawk_contracts_probes::{
    CheckPeer, FORMAT, Observation, Publication, Result, SearchOutcome, SOURCE_LIMIT,
    expectations_json, inputs_json, operations_json, parse_peer_evidence, qualify,
    qualifications_json, raw, read_literal_hints, read_scales, replay_changes_json,
    search, sha256_hex, values_json,
};
use slophawk_probe_replay::{
    ProbeExpectation, check, check_result_json, compare_sources,
    operation_results_json, parse_inputs,
};
use std::collections::hash_map::RandomState;
use std::fs::{self, File};
use std::hash::{BuildHasher, Hasher};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode, Stdio};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

fn main() -> ExitCode {
    let args = std::env::args().skip(1).collect::<Vec<_>>();
    match execute(&args) {
        Ok(code) => ExitCode::from(code),
        Err(error) => {
            eprintln!("{error}");
            ExitCode::from(2)
        }
    }
}

fn execute(args: &[String]) -> Result<u8> {
    let parts = args.iter().map(String::as_str).collect::<Vec<_>>();
    match parts.as_slice() {
        ["contracts", "qualify", rules, published] => {
            let catalog = qualify(&read_limited(rules)?, &read_limited(published)?)?;
            println!("{}", qualifications_json(catalog.qualification()));
            Ok(if catalog.qualification().iter().all(|q| q.error.is_none()) {0} else {1})
        }
        ["probes", "plan", seeds, limit, rest @ ..] if rest.len() <= 2 => {
            let count = positive_count(limit)?;
            let seed = rest.first().map_or(Ok(0), |s| signed_seed(s))?;
            let scales = if let Some(path) = rest.get(1) { read_scales(&read_limited(path)?)? }
                else { Vec::new() };
            let inputs = parse_inputs(&read_limited(seeds)?).map_err(|e| e.to_string())?;
            let result = plan_inputs(&inputs, count, seed, &scales).map_err(|e| e.to_string())?;
            println!("{}", inputs_json(&result)?);
            Ok(0)
        }
        ["probes", "compare", inputs, candidate, reference] => {
            let result = compare_sources(&read_limited(inputs)?, &read_limited(candidate)?,
                &read_limited(reference)?).map_err(|e| e.to_string())?;
            println!("{}", operation_results_json(&result));
            Ok(0)
        }
        // The managed PE/CLI comparison is still the published C# authority.
        ["probes", "methods", candidate, reference] => {
            let code = Command::new(peer_executable()).args(["probes", "methods", candidate, reference])
                .status().map_err(|e| format!("C# managed-method peer could not start: {e}"))?;
            Ok(if code.success() {0} else {2})
        }
        ["check-contracts", assembly, rules, published, workspace, rest @ ..] if rest.len() <= 3 => {
            let count = rest.first().map_or(Ok(512), |s| positive_count(s))?;
            if count > 8192 { return Err("The per-operation budget must be between 1 and 8192.".into()); }
            let run_verb = rest.get(1).map(|s| (*s).to_owned());
            let prepare_verb = rest.get(2).map(|s| (*s).to_owned());
            check_contracts(Path::new(assembly), Path::new(rules), Path::new(published),
                Path::new(workspace), count, run_verb, prepare_verb)
        }
        _ => Err("Usage: contracts qualify <rules.json> <published.json> | probes plan <seeds.json> <count> [seed [scales.json]] | probes compare <inputs.json> <candidate.json> <reference.json> | probes methods <candidate.dll> <reference.dll> | check-contracts <assembly.dll> <rules.json> <published.json> <workspace> [per-operation [run-verb [prepare-verb]]]".into()),
    }
}

fn signed_seed(text: &str) -> Result<i32> {
    text.trim().parse::<i32>().map_err(|_| "The optional random seed must be a signed 32-bit integer.".into())
}
fn positive_count(text: &str) -> Result<usize> {
    if text.is_empty() || !text.bytes().all(|b| b.is_ascii_digit()) {
        return Err("The probe count must be a positive integer.".into());
    }
    let value: usize = text.parse().map_err(|_| "The probe count is too large.".to_string())?;
    if value == 0 { return Err("The probe count must be positive.".into()); }
    Ok(value)
}
fn read_limited(path: impl AsRef<Path>) -> Result<Vec<u8>> {
    let path = path.as_ref();
    let len = fs::metadata(path).map_err(|e| format!("{}: {e}", path.display()))?.len();
    if len > SOURCE_LIMIT as u64 { return Err(format!("{} exceeds the 8 MiB input budget.", path.display())); }
    // A file may grow after metadata: cap the read itself before allocating.
    let mut bytes = Vec::with_capacity((len as usize).min(SOURCE_LIMIT));
    File::open(path).map_err(|e| format!("{}: {e}", path.display()))?
        .take(SOURCE_LIMIT as u64 + 1).read_to_end(&mut bytes)
        .map_err(|e| format!("{}: {e}", path.display()))?;
    if bytes.len() > SOURCE_LIMIT { return Err("File grew beyond the input budget.".into()); }
    Ok(bytes)
}
fn peer_executable() -> String {
    std::env::var("SLOPHAWK_CSHARP_CLI").unwrap_or_else(|_| "slophawk".into())
}

/// A run identifier is a non-authorizing filename, never a workspace grant.
/// Collisions are rejected with create_new/create_dir, not overwritten.
fn new_run_id() -> String {
    let time = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_nanos();
    let mut a = RandomState::new().build_hasher();
    a.write_u128(time); a.write_u32(std::process::id());
    let mut b = RandomState::new().build_hasher();
    b.write_u64(a.finish()); b.write_u128(time.rotate_left(53));
    format!("{:016x}{:016x}", a.finish(), b.finish())
}
fn make_new_dir(base: &Path) -> Result<PathBuf> {
    fs::create_dir_all(base).map_err(|e| e.to_string())?;
    for _ in 0..16 {
        let path = base.join(new_run_id());
        match fs::create_dir(&path) {
            Ok(()) => return Ok(path),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e.to_string()),
        }
    }
    Err("Could not allocate a distinct search/run directory.".into())
}
fn write_new(path: &Path, contents: &[u8]) -> Result<()> {
    let mut file = File::create_new(path).map_err(|e| format!("{}: {e}", path.display()))?;
    file.write_all(contents).map_err(|e| e.to_string())?;
    file.sync_all().map_err(|e| e.to_string())
}
fn publish_json(path: &Path, value: &[u8]) -> Result<()> {
    // Like C# PublishJsonAsync, replace the visible file only after closing the pending file.
    let parent = path.parent().ok_or("Missing publication parent.")?;
    fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    let pending = parent.join(format!("{}.{}.pending", path.file_name().unwrap().to_string_lossy(), new_run_id()));
    let write = write_new(&pending, value);
    if write.is_err() { let _ = fs::remove_file(&pending); return write; }
    let result = fs::rename(&pending, path).map_err(|e| format!("Cannot publish {}: {e}", path.display()));
    if result.is_err() { let _ = fs::remove_file(&pending); }
    result
}

struct CliPeer {
    csharp: String,
    assembly: PathBuf,
    scratch: PathBuf,
    candidate_sha: String,
    run_verb: Option<String>,
    prepare_verb: Option<String>,
    ordinal: usize,
}
impl CheckPeer for CliPeer {
    fn check(&mut self, cases: &[ProbeExpectation]) -> Result<Observation> {
        self.ordinal += 1;
        let scratch = self.scratch.join(format!("attempt-{}", self.ordinal));
        fs::create_dir(&scratch).map_err(|e| format!("Create staged peer workspace: {e}"))?;
        let attempt = (|| {
            let input = scratch.join("cases.json");
            write_new(&input, expectations_json(cases)?.as_bytes())?;
            // Public argv, NOT a new private wire protocol. 0=complete match,
            // 1=valid discrepancy/unsupported; 2 or process failure is invalid.
            let mut command = Command::new(&self.csharp);
            command.arg("check").arg(&self.assembly).arg(&input).arg(&scratch);
            if let Some(verb) = &self.run_verb { command.arg(verb); }
            if let Some(verb) = &self.prepare_verb { command.arg(verb); }
            // Capture to files: never accumulate unbounded child stdout/stderr in RAM.
            // The peer itself enforces its adapter budgets; a missing/bad terminal fails closed.
            let terminal_path = scratch.join("peer-terminal.json");
            let diagnostic_path = scratch.join("peer-stderr.txt");
            command.stdout(Stdio::from(File::create_new(&terminal_path)
                .map_err(|e| format!("Peer terminal capture: {e}"))?));
            command.stderr(Stdio::from(File::create_new(&diagnostic_path)
                .map_err(|e| format!("Peer diagnostic capture: {e}"))?));
            let status = command.status().map_err(|e| format!("C# check peer could not start: {e}"))?;
            if status.code() != Some(0) && status.code() != Some(1) {
                let diagnostic = read_limited(&diagnostic_path).unwrap_or_default();
                return Err(format!("C# check peer did not complete: status={status}; stderr={}",
                    String::from_utf8_lossy(&diagnostic).chars().take(1024).collect::<String>()));
            }
            let terminal = read_limited(&terminal_path)?;
            let stdout: slophawk_contracts_probes::PeerReceipt = serde_json::from_slice(&terminal)
                .map_err(|e| format!("Peer receipt is malformed: {e}"))?;
            if stdout.Run.len() != 32 || !stdout.Run.bytes().all(|c| c.is_ascii_hexdigit()) {
                return Err("Peer run ID is not a GUID-N hex identifier.".into());
            }
            let artifacts = scratch.join(".slophawk/checks").join(&stdout.Run);
            let report = artifacts.join("report.json");
            // Reject malicious/path-substituted artifacts regardless of CLI stdout.
            if Path::new(&stdout.Artifacts) != artifacts || Path::new(&stdout.Report) != report {
                return Err("Peer receipt paths escape or differ from the staged workspace.".into());
            }
            let req = read_limited(artifacts.join("requests.json"))?;
            let obs = read_limited(artifacts.join("observed.json"))?;
            let rep = read_limited(&report)?;
            let (_, mut verified) = parse_peer_evidence(&terminal, &rep, &req, &obs, cases,
                &self.candidate_sha, self.run_verb.as_deref(), self.prepare_verb.as_deref())?;
            if assembly_digest(&self.assembly)? != self.candidate_sha {
                return Err("Candidate assembly changed during a peer check.".into());
            }
            verified.report = report.to_string_lossy().into_owned();
            verified.artifacts = artifacts.to_string_lossy().into_owned();
            Ok(verified)
        })();
        if attempt.is_err() {
            // A failed/ambiguous terminal grants no staged output publication.
            let _ = fs::remove_dir_all(&scratch);
        }
        attempt
    }
}

fn assembly_digest(path: &Path) -> Result<String> {
    use std::io::Read;
    let mut file = File::open(path).map_err(|e| e.to_string())?;
    let mut hasher = Sha256::new();
    let mut block = [0u8; 16 * 1024];
    loop {
        let n = file.read(&mut block).map_err(|e| e.to_string())?;
        if n == 0 { break; }
        hasher.update(&block[..n]);
    }
    Ok(format!("{:X}", hasher.finalize()))
}

#[derive(Serialize)]
#[allow(non_snake_case)]
struct CheckRun<'a> {
    Format: &'static str, Run: &'a str, Candidate: &'a str,
    RunVerb: Option<&'a str>, PrepareVerb: Option<&'a str>,
    Result: Box<RawValue>, Expected: Vec<Box<RawValue>>,
}

struct Published {report: Option<String>, run: Option<String>, findings: usize, replay_report: Option<String>}

fn publish_replay(workspace: &Path, outcome: &SearchOutcome,
    run_verb: Option<&str>, prepare_verb: Option<&str>) -> Result<Published> {
    let checks = workspace.join(".slophawk/checks");
    let mut published = Published {
        report: None, run: None, findings: 0, replay_report: outcome.replay_report.clone(),
    };
    match &outcome.publication {
        Publication::None => {}
        Publication::Promote(original) => {
            let target = checks.join(&original.run);
            fs::create_dir_all(&checks).map_err(|e| e.to_string())?;
            fs::rename(&original.artifacts, &target).map_err(|e| format!("Promote stable replay: {e}"))?;
            let path = target.join("report.json").to_string_lossy().into_owned();
            published.report = Some(path.clone());
            published.replay_report = Some(path);
            published.run = Some(original.run.clone());
            published.findings = original.result.findings.len();
        }
        Publication::Subset {cases, observed, candidate} => {
            let root = make_new_dir(&checks)?;
            let run = root.file_name().unwrap().to_string_lossy().into_owned();
            let checked = check(cases, observed).map_err(|e| e.to_string())?;
            write_new(&root.join("requests.json"), inputs_json(
                &cases.iter().map(|c| slophawk_contracts::ProbeInput {
                    operation: c.operation.clone(), args: c.args.clone(),
                }).collect::<Vec<_>>())?.as_bytes())?;
            write_new(&root.join("observed.json"), values_json(observed).as_bytes())?;
            let report = CheckRun {
                Format: "slophawk-check-v1", Run: &run, Candidate: candidate,
                RunVerb: run_verb, PrepareVerb: prepare_verb,
                Result: raw(&check_result_json(&checked))?,
                Expected: cases.iter().map(|case| raw(case.expected.raw_json()))
                    .collect::<Result<Vec<_>>>()?,
            };
            write_new(&root.join("report.json"), serde_json::to_string(&report)
                .map_err(|e| e.to_string())?.as_bytes())?;
            published.report = Some(root.join("report.json").to_string_lossy().into_owned());
            published.run = Some(run);
            published.findings = checked.findings.len();
        }
    }
    Ok(published)
}

#[derive(Serialize)]
#[allow(non_snake_case)]
struct Summary<'a> {
    Format: &'static str, Search: &'a str, Qualification: &'a str,
    QualificationBasis: &'static str, ExpectationBasis: &'static str,
    RulesSha256: &'a str, PublishedSha256: &'a str,
    Summary: &'a str, Report: &'a Option<String>, Run: &'a Option<String>,
    Compared: usize, PerOperation: usize, ElapsedMilliseconds: u128,
    Unsupported: usize, Findings: usize, ObservedDiscrepancies: usize,
    ReplayUnsupported: usize, ReplayReport: &'a Option<String>,
    ReplayChanges: Box<RawValue>, Operations: Value,
}

fn check_contracts(assembly: &Path, rules: &Path, published: &Path, workspace: &Path,
    per_operation: usize, run_verb: Option<String>, prepare_verb: Option<String>) -> Result<u8> {
    let started = Instant::now();
    let rules_bytes = read_limited(rules)?;
    let published_bytes = read_limited(published)?;
    let catalog: ContractCases = qualify(&rules_bytes, &published_bytes)?;
    let workspace = fs::canonicalize(workspace).map_err(|_| "The workspace directory must already exist.".to_owned())?;
    if !workspace.is_dir() { return Err("The workspace directory must already exist.".into()); }
    // Like C#, qualification can complete with zero runnable rules even if no
    // adapter assembly is available. Do not demand executable evidence then.
    let has_runnable_rules = !catalog.parameter_counts().is_empty();
    let assembly = if has_runnable_rules {
        let path = fs::canonicalize(assembly).map_err(|e| format!("Candidate assembly: {e}"))?;
        if !path.extension().and_then(|ext| ext.to_str()).is_some_and(|ext| ext.eq_ignore_ascii_case("dll")) {
            return Err("Pass a managed .NET assembly (.dll).".into());
        }
        path
    } else { assembly.to_path_buf() };
    let candidate_sha = if has_runnable_rules { assembly_digest(&assembly)? } else { String::new() };
    let rule_sha = sha256_hex(&rules_bytes);
    let published_sha = sha256_hex(&published_bytes);
    let search_path = make_new_dir(&workspace.join(".slophawk/contract-searches"))?;
    let search_string = search_path.to_string_lossy().into_owned();
    let qualification_path = search_path.join("qualification.json");
    publish_json(&qualification_path, qualifications_json(catalog.qualification()).to_string().as_bytes())?;

    let qualified = catalog.parameter_counts();
    let hints = match std::env::var("SLOPHAWK_IL_HINTS_JSON") {
        Ok(path) => {
            let source = read_limited(path)?;
            let wanted = std::env::var("SLOPHAWK_IL_HINTS_SHA256")
                .map_err(|_| "An IL hints file requires SLOPHAWK_IL_HINTS_SHA256 (C# peer digest).".to_owned())?;
            if sha256_hex(&source) != wanted.to_uppercase() {
                return Err("IL hint digest does not match its peer-supplied identity.".into());
            }
            read_literal_hints(&source, qualified, &candidate_sha)?
        }
        Err(std::env::VarError::NotPresent) => Vec::new(),
        Err(error) => return Err(format!("Invalid IL hints environment setting: {error}")),
    };
    let mut peer = CliPeer { csharp: peer_executable(), assembly,
        scratch: search_path.clone(), candidate_sha, run_verb: run_verb.clone(),
        prepare_verb: prepare_verb.clone(), ordinal: 0 };
    let outcome = search(&catalog, per_operation, &hints, &mut peer)?;
    let evidence = publish_replay(&workspace, &outcome, run_verb.as_deref(), prepare_verb.as_deref())?;
    let summary_path = search_path.join("summary.json");
    let summary_string = summary_path.to_string_lossy().into_owned();
    let qualification_string = qualification_path.to_string_lossy().into_owned();
    let summary = Summary {
        Format: FORMAT, Search: &search_string, Qualification: &qualification_string,
        QualificationBasis: "published-examples", ExpectationBasis: "supplied-rules",
        RulesSha256: &rule_sha, PublishedSha256: &published_sha,
        Summary: &summary_string, Report: &evidence.report, Run: &evidence.run,
        Compared: outcome.compared, PerOperation: per_operation,
        ElapsedMilliseconds: started.elapsed().as_millis(),
        Unsupported: outcome.unsupported, Findings: evidence.findings,
        ObservedDiscrepancies: outcome.observed_discrepancies,
        ReplayUnsupported: outcome.replay_unsupported, ReplayReport: &evidence.replay_report,
        ReplayChanges: raw(&replay_changes_json(&outcome.replay_changes)?)?,
        Operations: operations_json(&outcome.operations),
    };
    publish_json(&summary_path, serde_json::to_string(&summary)
        .map_err(|e| e.to_string())?.as_bytes())?;
    println!("{}", json!({
        "Format": FORMAT, "Summary": summary_string,
        "Report": evidence.report, "Run": evidence.run, "Compared": outcome.compared,
        "Unsupported": outcome.unsupported, "Findings": evidence.findings,
        "ObservedDiscrepancies": outcome.observed_discrepancies,
        "ReplayUnsupported": outcome.replay_unsupported,
        "ElapsedMilliseconds": summary.ElapsedMilliseconds,
        "ReplayReport": evidence.replay_report,
        "ReplayChanges": outcome.replay_changes.len(),
        "QualificationBasis": "published-examples", "ExpectationBasis": "supplied-rules",
        "RulesSha256": rule_sha, "PublishedSha256": published_sha,
        "Unqualified": outcome.qualification.iter().filter(|q| q.error.is_some()).count(),
    }));
    Ok(if outcome.success(evidence.report.is_some(), evidence.findings) {0} else {1})
}
