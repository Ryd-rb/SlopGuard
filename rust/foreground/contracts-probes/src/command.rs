//! Shared foreground entry point for contract search and its evidence.
use crate::passive::{self, positive_count, read_limited};
use crate::selection_audit::SelectionAudit;
use crate::{
    CheckPeer, FORMAT, Observation, Publication, Result, SearchOutcome, inputs_json,
    operations_json, qualifications_json, qualify, raw, read_literal_hints, replay_changes_json,
    CaseSelection, search_with_selection, sha256_hex, values_json,
};
use serde::Serialize;
use serde_json::{Value, json, value::RawValue};
use sha2::{Digest, Sha256};
use slophawk_contracts::{ContractCases, ProbeInput};
use slophawk_probe_replay::{ProbeCheckResult, ProbeExpectation, check, check_result_json};
use std::collections::hash_map::RandomState;
use std::fs::{self, File};
use std::hash::{BuildHasher, Hasher};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

pub fn run(args: &[String]) -> Result<u8> {
    let parts = args.iter().map(String::as_str).collect::<Vec<_>>();
    let (parts, selection) = match parts.as_slice() {
        ["check-contracts", "--diverse", rest @ ..] => {
            let command = std::iter::once("check-contracts").chain(rest.iter().copied()).collect::<Vec<_>>();
            (command, CaseSelection::DiverseV1)
        }
        _ => (parts, CaseSelection::Legacy),
    };
    match parts.as_slice() {
        ["contracts" | "probes", ..] => {
            let (output, code) = passive::run(args)?;
            println!("{output}");
            Ok(code)
        }
        ["check-contracts", assembly, rules, published, workspace, rest @ ..] if rest.len() <= 3 => {
            let count = rest.first().map_or(Ok(512), |s| positive_count(s))?;
            if count > 8192 { return Err("The per-operation budget must be between 1 and 8192.".into()); }
            let run_verb = rest.get(1).map(|s| (*s).to_owned());
            let prepare_verb = rest.get(2).map(|s| (*s).to_owned());
            check_contracts(Path::new(assembly), Path::new(rules), Path::new(published),
                Path::new(workspace), count, run_verb, prepare_verb, selection)
        }
        _ => Err("Usage: contracts qualify <rules.json> <published.json> | probes plan <seeds.json> <count> [seed [scales.json]] | probes compare <inputs.json> <candidate.json> <reference.json> | probes methods <candidate.dll> <reference.dll> | check-contracts [--diverse] <assembly.dll> <rules.json> <published.json> <workspace> [per-operation [run-verb [prepare-verb]]]".into()),
    }
}

/// A run identifier is a non-authorizing filename, never a workspace grant.
/// Collisions are rejected with create_new/create_dir, not overwritten.
fn new_run_id() -> String {
    let time = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let mut a = RandomState::new().build_hasher();
    a.write_u128(time);
    a.write_u32(std::process::id());
    let mut b = RandomState::new().build_hasher();
    b.write_u64(a.finish());
    b.write_u128(time.rotate_left(53));
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
    let pending = parent.join(format!(
        "{}.{}.pending",
        path.file_name().unwrap().to_string_lossy(),
        new_run_id()
    ));
    let write = write_new(&pending, value);
    if write.is_err() {
        let _ = fs::remove_file(&pending);
        return write;
    }
    let result =
        fs::rename(&pending, path).map_err(|e| format!("Cannot publish {}: {e}", path.display()));
    if result.is_err() {
        let _ = fs::remove_file(&pending);
    }
    result
}

struct CliPeer {
    assembly: PathBuf,
    workspace: PathBuf,
    scratch: PathBuf,
    candidate_sha: String,
    run_verb: Option<String>,
    prepare_verb: Option<String>,
}
impl CheckPeer for CliPeer {
    fn check(&mut self, cases: &[ProbeExpectation]) -> Result<Observation> {
        let (run, artifacts, observed) = slophawk_foreground_checks::contract_batch(
            &self.assembly,
            &self.workspace,
            &self.scratch,
            &self.candidate_sha,
            self.run_verb.as_deref(),
            self.prepare_verb.as_deref(),
            cases,
        )?;
        let result = check(cases, &observed).map_err(|error| error.to_string())?;
        Ok(Observation {
            run,
            candidate: self.candidate_sha.clone(),
            report: artifacts.join("report.json").display().to_string(),
            artifacts: artifacts.display().to_string(),
            observed,
            result,
        })
    }
}

fn assembly_digest(path: &Path) -> Result<String> {
    use std::io::Read;
    let mut file = File::open(path).map_err(|e| e.to_string())?;
    let mut hasher = Sha256::new();
    let mut block = [0u8; 16 * 1024];
    loop {
        let n = file.read(&mut block).map_err(|e| e.to_string())?;
        if n == 0 {
            break;
        }
        hasher.update(&block[..n]);
    }
    Ok(format!("{:X}", hasher.finalize()))
}

#[derive(Serialize)]
#[allow(non_snake_case)]
struct CheckRun<'a> {
    Format: &'static str,
    Run: &'a str,
    Candidate: &'a str,
    RunVerb: Option<&'a str>,
    PrepareVerb: Option<&'a str>,
    Result: &'a RawValue,
    Expected: Vec<Box<RawValue>>,
}

struct Published {
    report: Option<String>,
    run: Option<String>,
    result: Option<ProbeCheckResult>,
    replay_report: Option<String>,
}

fn publish_replay(
    workspace: &Path,
    outcome: &SearchOutcome,
    run_verb: Option<&str>,
    prepare_verb: Option<&str>,
) -> Result<Published> {
    let checks = workspace.join(".slophawk/checks");
    let mut published = Published {
        report: None,
        run: None,
        result: None,
        replay_report: outcome.replay_report.clone(),
    };
    match &outcome.publication {
        Publication::None => {}
        Publication::Promote(original) => {
            let target = checks.join(&original.run);
            fs::create_dir_all(&checks).map_err(|e| e.to_string())?;
            fs::rename(&original.artifacts, &target)
                .map_err(|e| format!("Promote stable replay: {e}"))?;
            let path = target.join("report.json").to_string_lossy().into_owned();
            published.report = Some(path.clone());
            published.replay_report = Some(path);
            published.run = Some(original.run.clone());
            published.result = Some(original.result.clone());
        }
        Publication::Subset {
            cases,
            observed,
            candidate,
        } => {
            let root = make_new_dir(&checks)?;
            let run = root.file_name().unwrap().to_string_lossy().into_owned();
            let checked = check(cases, observed).map_err(|e| e.to_string())?;
            let result = raw(&check_result_json(&checked))?;
            write_new(
                &root.join("requests.json"),
                inputs_json(
                    &cases
                        .iter()
                        .map(|c| ProbeInput {
                            operation: c.operation.clone(),
                            args: c.args.clone(),
                        })
                        .collect::<Vec<_>>(),
                )?
                .as_bytes(),
            )?;
            write_new(
                &root.join("observed.json"),
                values_json(observed).as_bytes(),
            )?;
            let report = CheckRun {
                Format: "slophawk-check-v1",
                Run: &run,
                Candidate: candidate,
                RunVerb: run_verb,
                PrepareVerb: prepare_verb,
                Result: &result,
                Expected: cases
                    .iter()
                    .map(|case| raw(case.expected.raw_json()))
                    .collect::<Result<Vec<_>>>()?,
            };
            write_new(
                &root.join("report.json"),
                serde_json::to_string(&report)
                    .map_err(|e| e.to_string())?
                    .as_bytes(),
            )?;
            published.report = Some(root.join("report.json").to_string_lossy().into_owned());
            published.run = Some(run);
            published.result = Some(checked);
        }
    }
    Ok(published)
}

#[derive(Serialize)]
#[allow(non_snake_case)]
struct Summary<'a> {
    Format: &'static str,
    Search: &'a str,
    Qualification: &'a str,
    QualificationBasis: &'static str,
    ExpectationBasis: &'static str,
    RulesSha256: &'a str,
    PublishedSha256: &'a str,
    Summary: &'a str,
    Review: &'a str,
    ReplayRequests: &'a Option<String>,
    Report: &'a Option<String>,
    Run: &'a Option<String>,
    Compared: usize,
    PerOperation: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    CaseSelection: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    SelectionAudit: Option<&'a SelectionAudit>,
    ElapsedMilliseconds: u128,
    Unsupported: usize,
    Findings: usize,
    ObservedDiscrepancies: usize,
    ReplayUnsupported: usize,
    ReplayReport: &'a Option<String>,
    ReplayChanges: Box<RawValue>,
    Operations: Value,
}

#[derive(Serialize)]
#[allow(non_snake_case)]
struct Review<'a> {
    Format: &'static str,
    Run: &'a Option<String>,
    Candidate: &'a str,
    QualificationBasis: &'static str,
    ExpectationBasis: &'static str,
    RulesSha256: &'a str,
    PublishedSha256: &'a str,
    Compared: usize,
    Unsupported: usize,
    ReplayUnsupported: usize,
    ReplayChanges: usize,
    Operations: &'a Value,
    Result: Option<&'a RawValue>,
    ReplayRequests: &'a Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    SelectionAudit: Option<&'a SelectionAudit>,
}

fn check_contracts(
    assembly: &Path,
    rules: &Path,
    published: &Path,
    workspace: &Path,
    per_operation: usize,
    run_verb: Option<String>,
    prepare_verb: Option<String>,
    selection: CaseSelection,
) -> Result<u8> {
    let started = Instant::now();
    let rules_bytes = read_limited(rules)?;
    let published_bytes = read_limited(published)?;
    let catalog: ContractCases = qualify(&rules_bytes, &published_bytes)?;
    let workspace = fs::canonicalize(workspace)
        .map_err(|_| "The workspace directory must already exist.".to_owned())?;
    if !workspace.is_dir() {
        return Err("The workspace directory must already exist.".into());
    }
    // Like C#, qualification can complete with zero runnable rules even if no
    // adapter assembly is available. Do not demand executable evidence then.
    let has_runnable_rules = !catalog.parameter_counts().is_empty();
    let assembly = if has_runnable_rules {
        let path = fs::canonicalize(assembly).map_err(|e| format!("Candidate assembly: {e}"))?;
        if !path
            .extension()
            .and_then(|ext| ext.to_str())
            .is_some_and(|ext| ext.eq_ignore_ascii_case("dll"))
        {
            return Err("Pass a managed .NET assembly (.dll).".into());
        }
        path
    } else {
        assembly.to_path_buf()
    };
    let candidate_sha = if has_runnable_rules {
        assembly_digest(&assembly)?
    } else {
        String::new()
    };
    let rule_sha = sha256_hex(&rules_bytes);
    let published_sha = sha256_hex(&published_bytes);
    let search_path = make_new_dir(&workspace.join(".slophawk/contract-searches"))?;
    let search_string = search_path.to_string_lossy().into_owned();
    let qualification_path = search_path.join("qualification.json");
    publish_json(
        &qualification_path,
        qualifications_json(catalog.qualification())
            .to_string()
            .as_bytes(),
    )?;

    let qualified = catalog.parameter_counts();
    let hints = match std::env::var("SLOPHAWK_IL_HINTS_JSON") {
        Ok(path) => {
            let source = read_limited(path)?;
            let wanted = std::env::var("SLOPHAWK_IL_HINTS_SHA256").map_err(|_| {
                "An IL hints file requires SLOPHAWK_IL_HINTS_SHA256 (C# peer digest).".to_owned()
            })?;
            if sha256_hex(&source) != wanted.to_uppercase() {
                return Err("IL hint digest does not match its peer-supplied identity.".into());
            }
            read_literal_hints(&source, qualified, &candidate_sha)?
        }
        Err(std::env::VarError::NotPresent) if has_runnable_rules => {
            let bytes = slophawk_managed_methods::read_assembly(&assembly)
                .map_err(|error| error.to_string())?;
            if sha256_hex(&bytes) != candidate_sha {
                return Err("Candidate changed before literal extraction.".into());
            }
            slophawk_managed_methods::integer_literals_for_operations(&bytes, qualified)
                .map_err(|error| error.to_string())?
                .into_iter()
                .map(|row| (row.operation, row.literals))
                .collect()
        }
        Err(std::env::VarError::NotPresent) => Vec::new(),
        Err(error) => return Err(format!("Invalid IL hints environment setting: {error}")),
    };
    let mut peer = CliPeer {
        assembly,
        workspace: workspace.clone(),
        scratch: search_path.clone(),
        candidate_sha,
        run_verb: run_verb.clone(),
        prepare_verb: prepare_verb.clone(),
    };
    let outcome = search_with_selection(&catalog, per_operation, &hints, &mut peer, selection)?;
    let evidence = publish_replay(
        &workspace,
        &outcome,
        run_verb.as_deref(),
        prepare_verb.as_deref(),
    )?;
    let mut selection_audit = outcome.selection_audit.clone();
    if let Some(replay) = selection_audit.as_mut().and_then(|audit| audit.replay.as_mut()) {
        // Promotion moves the replay directory. A subset keeps the original
        // replay report; never relabel it with the subset's new run identity.
        if let Some(report) = &evidence.replay_report {
            replay.report = report.clone();
        }
    }
    let summary_path = search_path.join("summary.json");
    let summary_string = summary_path.to_string_lossy().into_owned();
    let review_path = search_path.join("review.json");
    let review_string = review_path.to_string_lossy().into_owned();
    let qualification_string = qualification_path.to_string_lossy().into_owned();
    let findings = evidence.result.as_ref().map_or(&[][..], |result| result.findings.as_slice());
    let replay_requests = if findings.is_empty() {
        None
    } else {
        let path = search_path.join("replay-requests.json");
        let requests: Vec<ProbeInput> = findings.iter().map(|finding| finding.input.clone()).collect();
        publish_json(&path, inputs_json(&requests)?.as_bytes())?;
        Some(path.to_string_lossy().into_owned())
    };
    let summary = Summary {
        Format: FORMAT,
        Search: &search_string,
        Qualification: &qualification_string,
        QualificationBasis: "published-examples",
        ExpectationBasis: "supplied-rules",
        RulesSha256: &rule_sha,
        PublishedSha256: &published_sha,
        Summary: &summary_string,
        Review: &review_string,
        ReplayRequests: &replay_requests,
        Report: &evidence.report,
        Run: &evidence.run,
        Compared: outcome.compared,
        PerOperation: per_operation,
        CaseSelection: (selection == CaseSelection::DiverseV1).then_some("diverse-v1"),
        SelectionAudit: selection_audit.as_ref(),
        ElapsedMilliseconds: started.elapsed().as_millis(),
        Unsupported: outcome.unsupported,
        Findings: findings.len(),
        ObservedDiscrepancies: outcome.observed_discrepancies,
        ReplayUnsupported: outcome.replay_unsupported,
        ReplayReport: &evidence.replay_report,
        ReplayChanges: raw(&replay_changes_json(&outcome.replay_changes)?)?,
        Operations: operations_json(&outcome.operations),
    };
    let result = evidence.result.as_ref()
        .map(|result| raw(&check_result_json(result)))
        .transpose()?;
    let review = Review {
        Format: "slophawk-contract-review-v1",
        Run: &evidence.run,
        Candidate: &peer.candidate_sha,
        QualificationBasis: summary.QualificationBasis,
        ExpectationBasis: summary.ExpectationBasis,
        RulesSha256: &rule_sha,
        PublishedSha256: &published_sha,
        Compared: outcome.compared,
        Unsupported: outcome.unsupported,
        ReplayUnsupported: outcome.replay_unsupported,
        ReplayChanges: outcome.replay_changes.len(),
        Operations: &summary.Operations,
        Result: result.as_deref(),
        ReplayRequests: &replay_requests,
        SelectionAudit: selection_audit.as_ref(),
    };
    publish_json(
        &review_path,
        serde_json::to_string_pretty(&review)
            .map_err(|e| e.to_string())?
            .as_bytes(),
    )?;
    publish_json(
        &summary_path,
        serde_json::to_string(&summary)
            .map_err(|e| e.to_string())?
            .as_bytes(),
    )?;
    println!(
        "{}",
        json!({
            "Format": FORMAT, "Summary": summary_string, "Review": review_string,
            "ReplayRequests": replay_requests,
            "Report": evidence.report, "Run": evidence.run, "Compared": outcome.compared,
            "Unsupported": outcome.unsupported, "Findings": findings.len(),
            "ObservedDiscrepancies": outcome.observed_discrepancies,
            "ReplayUnsupported": outcome.replay_unsupported,
            "ElapsedMilliseconds": summary.ElapsedMilliseconds,
            "ReplayReport": evidence.replay_report,
            "ReplayChanges": outcome.replay_changes.len(),
            "QualificationBasis": "published-examples", "ExpectationBasis": "supplied-rules",
            "RulesSha256": rule_sha, "PublishedSha256": published_sha,
            "Unqualified": outcome.qualification.iter().filter(|q| q.error.is_some()).count(),
        })
    );
    Ok(
        if outcome.success(evidence.report.is_some(), findings.len()) {
            0
        } else {
            1
        },
    )
}
