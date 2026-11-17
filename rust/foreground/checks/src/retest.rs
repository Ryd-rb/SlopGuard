use crate::{evidence, io, model::{self, Acknowledgment, Decision}, review};
use slophawk_probe_replay::{JsonValue, ProbeExpectation, ProbeInput};
use std::{collections::HashSet, fs, path::Path};

pub(crate) fn run(args: &[String]) -> Result<i32, String> {
    if args.len() != 6 || !io::is_run_id(&args[3]) {
        return Err("Usage: slophawk check-retest <assembly.dll> <workspace> <run> <decisions.json> <acknowledgments.json>".into());
    }
    let workspace = io::ensure_workspace(&args[2])?;
    let assembly = fs::canonicalize(&args[1]).map_err(|e| format!("Candidate assembly: {e}"))?;
    let run = &args[3];
    let path = io::checks_dir(&workspace).join(run);
    let original = evidence::read_run(&workspace, &path, run)?;
    if original.report.expected.is_none() {
        return Err("Provide a completed comparison report for retesting.".into());
    }
    let decisions_path = Path::new(&args[4]);
    let decision_bytes = io::read_limit(decisions_path, io::MAX_DOCUMENT)?;
    let decisions: Vec<Decision> = serde_json::from_slice(&decision_bytes).map_err(|e| e.to_string())?;
    let mut unique = HashSet::new();
    if decisions.is_empty() || decisions.iter().any(|v| v.reason.chars().all(char::is_whitespace) || !unique.insert(v.index)) {
        return Err("Provide unique finding indices and a reason for each decision.".into());
    }
    let mut cases = Vec::<ProbeExpectation>::new();
    let mut dispositions = Vec::<String>::new();
    for decision in &decisions {
        let finding = original.findings.iter().find(|v| v.index == decision.index).ok_or_else(||
            format!("Index {} is not a finding in run {run}.", decision.index))?;
        let (expected, disposition) = if let Some(corrected) = &decision.expected {
            let value = JsonValue::parse(corrected.get().as_bytes()).map_err(|e| e.to_string())?;
            if !model::equal(&value, &finding.candidate)? {
                return Err(format!("The corrected expectation at index {} must agree with the observed result.", decision.index));
            }
            (value, "corrected-expectation")
        } else { (finding.reference.clone(), "confirmed") };
        cases.push(ProbeExpectation { operation: finding.input.operation.clone(),
            args: finding.input.args.clone(), expected });
        dispositions.push(disposition.to_owned());
    }
    let inputs: Vec<ProbeInput> = cases.iter().map(|v| ProbeInput {operation: v.operation.clone(), args: v.args.clone()}).collect();
    let attempts = workspace.join(".slophawk").join("retest-attempts");
    let attempt = evidence::execute(&assembly, &workspace, original.report.run_verb.as_deref(),
        original.report.prepare_verb.as_deref(), &inputs, Some(&cases), &io::hash(&decision_bytes),
        Some(decisions_path), &attempts, Some(&original.report.candidate))?;
    for (index, decision) in decisions.iter().enumerate() {
        let initial = original.findings.iter().find(|v| v.index == decision.index).expect("validated finding");
        if !model::equal(&attempt.observed[index], &initial.candidate)? {
            return Err(format!("The retest did not reproduce the previously observed value. See {}.",
                attempt.artifacts.join("report.json").display()));
        }
    }
    let destination = Path::new(&args[5]);
    let _write_lock = io::Lock::acquire(destination)?;
    let existing: Vec<Acknowledgment> = if destination.exists() { io::read_json(destination)? } else { vec![] };
    // No promotion or acknowledgment may reuse a stale or cross-adapter receipt.
    review::validate_prior_acknowledgments(&workspace, &existing)?;
    let refreshed = evidence::read_run(&workspace, &path, run)?;
    if serde_json::to_string(&original.report).map_err(|e| e.to_string())? !=
            serde_json::to_string(&refreshed.report).map_err(|e| e.to_string())? ||
       original.inputs != refreshed.inputs || original.observed.iter().map(|v| v.raw_json()).collect::<Vec<_>>() !=
            refreshed.observed.iter().map(|v| v.raw_json()).collect::<Vec<_>>() {
        return Err("The original check changed while the retest was running.".into());
    }
    // Read the completed attempt again, including the extra Rust identity and
    // the exact ordered requests, before it may enter normal review evidence.
    let saved = evidence::read_run(&workspace, &attempt.artifacts, &attempt.run)?;
    if saved.adapter() != original.adapter() || saved.report.expected.is_none() ||
        saved.inputs != inputs || saved.observed.iter().map(|v| v.raw_json()).collect::<Vec<_>>() !=
            attempt.observed.iter().map(|v| v.raw_json()).collect::<Vec<_>>() {
        return Err("The retest attempt changed before promotion.".into());
    }
    let completed = io::checks_dir(&workspace).join(&attempt.run);
    fs::rename(&attempt.artifacts, &completed)
        .map_err(|e| format!("Cannot promote the completed retest: {e}"))?;
    let mut updated: Vec<Acknowledgment> = existing.into_iter().filter(|v| {
        !(v.run == *run && decisions.iter().any(|decision| decision.index == v.index))
    }).collect();
    for (index, decision) in decisions.iter().enumerate() {
        updated.push(Acknowledgment {run: run.clone(), index: decision.index,
            retest_run: attempt.run.clone(), retest_index: index,
            disposition: dispositions[index].clone(), reason: decision.reason.clone() });
    }
    io::atomic_write(destination, &io::serialize(&updated)?)?;
    let stdout = serde_json::json!({
        "Run": attempt.run,
        "Report": completed.join("report.json").display().to_string(),
        "Acknowledgments": destination.display().to_string(),
        "Added": decisions.len(),
    });
    println!("{}", serde_json::to_string(&stdout).map_err(|e| e.to_string())?);
    Ok(0)
}
