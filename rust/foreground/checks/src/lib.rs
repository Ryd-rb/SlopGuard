//! Standalone Rust port of the pinned C# foreground check family.
//!
//! Invocation is identical to the shipped `slophawk check`, `check-observe`,
//! `check-retest`, and `check-review` subcommands, without rewriting private
//! daemon IPC or inferring permission to execute from repository artifacts.
//! The explicit assembly supplied by the user is the only runnable adapter.
mod adapter;
mod evidence;
mod io;
mod model;
pub mod paired_regression;
mod retest;
mod review;

use slophawk_probe_replay::{inputs_for_check, parse_expectations, parse_inputs};
use std::{fs, path::Path};

/// Run an ordered contract batch through the same scoped adapter as `check`.
pub fn contract_batch(
    assembly: &Path, workspace: &Path, output: &Path, candidate: &str,
    run_verb: Option<&str>, prepare_verb: Option<&str>, cases: &[slophawk_probe_replay::ProbeExpectation],
) -> Result<(String, std::path::PathBuf, Vec<slophawk_probe_replay::JsonValue>), String> {
    let inputs = inputs_for_check(cases).map_err(|error| error.to_string())?;
    let serialized = io::serialize(&inputs.iter().map(model::Input::from).collect::<Vec<_>>())?;
    let result = evidence::execute(assembly, workspace, run_verb, prepare_verb, &inputs,
        Some(cases), &io::hash(&serialized), None, output, Some(candidate))?;
    Ok((result.run, result.artifacts, result.observed))
}

/// Parse the shipped argv and print the shipped JSON receipt/summary to stdout.
/// Returns 0 for complete or progress, 1 for unsupported/finding/incomplete.
/// Input and execution faults return an error for the caller to print to stderr.
pub fn run(args: &[String]) -> Result<u8, String> {
    match args.first().map(String::as_str) {
        Some("check") | Some("check-observe") => check(args).map(|v| v as u8),
        Some("check-retest") => retest::run(args).map(|v| v as u8),
        Some("check-review") => review::run(args).map(|v| v as u8),
        _ => Err("Usage: slophawk <check|check-observe|check-retest|check-review> ...".into()),
    }
}

fn check(args: &[String]) -> Result<i32, String> {
    if args.len() < 4 || args.len() > 6 {
        return Err(format!("Usage: slophawk {} <assembly.dll> <input.json> <workspace> [run-verb [prepare-verb]]", args[0]));
    }
    let workspace = io::ensure_workspace(&args[3])?;
    let assembly = fs::canonicalize(&args[1]).map_err(|e| format!("Candidate assembly: {e}"))?;
    let input_path = Path::new(&args[2]);
    let bytes = io::read_limit(input_path, io::MAX_DOCUMENT)?;
    let observe = args[0] == "check-observe";
    let cases = if observe { None } else { Some(parse_expectations(&bytes).map_err(|e| e.to_string())?) };
    let inputs = if let Some(cases) = &cases {
        inputs_for_check(cases).map_err(|e| e.to_string())?
    } else { parse_inputs(&bytes).map_err(|e| e.to_string())? };
    let run_verb = args.get(4).map(String::as_str);
    let prepare_verb = args.get(5).map(String::as_str);
    let published = evidence::execute(&assembly, &workspace, run_verb, prepare_verb,
        &inputs, cases.as_deref(), &io::hash(&bytes), Some(input_path),
        &io::checks_dir(&workspace), None)?;
    let comparison: serde_json::Value = serde_json::from_str(published.report.result.get())
        .map_err(|e| e.to_string())?;
    let unsupported = comparison["Unsupported"].as_u64().ok_or("Missing unsupported count")?;
    let findings = comparison["Findings"].as_array().ok_or("Missing findings")?.len();
    println!("{}", serde_json::to_string(&published.receipt()).map_err(|e| e.to_string())?);
    Ok(if unsupported == 0 && findings == 0 { 0 } else { 1 })
}
