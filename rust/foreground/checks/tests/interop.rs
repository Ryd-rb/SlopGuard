//! Two-way v1 artifact and argv interop. These tests are not self-building:
//! the owner explicitly builds the C# CLI and adapter, then sets the two
//! environment variables documented in ../README.md before executing them.
use std::{env, fs, path::{Path, PathBuf}, process::{Command, Output}};
use serde_json::{Value, json};
use uuid::Uuid;

struct Scratch(PathBuf);
impl Scratch {
    fn new() -> Self {
        let path = env::temp_dir().join(format!("sg-mig-06-{}", Uuid::new_v4().simple()));
        fs::create_dir(&path).unwrap();
        Self(path)
    }
    fn file(&self, name: &str, value: Value) -> PathBuf {
        let path = self.0.join(name);
        fs::write(&path, serde_json::to_vec(&value).unwrap()).unwrap();
        path
    }
    fn checks(&self) -> PathBuf { self.0.join(".slophawk").join("checks") }
    fn artifact(&self, run: &str, name: &str) -> Value {
        serde_json::from_slice(&fs::read(self.checks().join(run).join(name)).unwrap()).unwrap()
    }
}
impl Drop for Scratch { fn drop(&mut self) { let _ = fs::remove_dir_all(&self.0); } }

fn tools() -> Option<(PathBuf, PathBuf)> {
    Some((PathBuf::from(env::var_os("SLOPHAWK_CSHARP_CLI")?),
          PathBuf::from(env::var_os("SLOPHAWK_FOREGROUND_ADAPTER")?)))
}
fn rust(args: &[String]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_slophawk-foreground-checks"))
        .args(args).output().unwrap()
}
fn csharp(cli: &Path, args: &[String]) -> Output {
    Command::new("dotnet").arg("exec").arg(cli).args(args).output().unwrap()
}
fn command(prefix: &[&str], rest: &[&Path]) -> Vec<String> {
    prefix.iter().map(|v| v.to_string()).chain(rest.iter().map(|p| p.display().to_string())).collect()
}
fn json_output(output: &Output) -> Value {
    serde_json::from_slice(&output.stdout).unwrap_or_else(|e| panic!(
        "Expected JSON receipt: {e}; stderr={}", String::from_utf8_lossy(&output.stderr)))
}
fn success(output: &Output) {
    assert_eq!(output.status.code(), Some(0), "{}", String::from_utf8_lossy(&output.stderr));
}
fn failed(output: &Output) { assert_eq!(output.status.code(), Some(1), "{output:?}"); }
fn input(op: &str, args: &[i64], expected: i64) -> Value {
    json!({"Operation":op,"Args":args,"Expected":expected})
}

#[test]
fn rust_check_to_csharp_review_and_csharp_check_to_rust_review() {
    let Some((cli, adapter)) = tools() else { return };
    let space = Scratch::new();
    let cases = space.file("cases.json", json!([input("steady", &[3], 3)]));
    let args = command(&["check"], &[&adapter, &cases, &space.0]);
    let args = [args, vec!["probe-batch".into(), "prepare-batch".into()]].concat();
    let port = rust(&args);
    success(&port);
    let port_run = json_output(&port)["Run"].as_str().unwrap().to_owned();
    assert_eq!(space.artifact(&port_run, "report.json")["Format"], "slophawk-check-v1");
    assert!(space.checks().join(&port_run).join("identity.json").exists());
    let review = csharp(&cli, &command(&["check-review"], &[&space.0]));
    success(&review);
    assert_eq!(json_output(&review)["Complete"], true);
    let legacy = csharp(&cli, &args);
    success(&legacy);
    let legacy_run = json_output(&legacy)["Run"].as_str().unwrap().to_owned();
    let rust_review = rust(&[command(&["check-review"], &[&space.0]),
        vec!["--run".into(), legacy_run]].concat());
    success(&rust_review);
    assert_eq!(json_output(&rust_review)["Complete"], true);
    let report = space.artifact(&port_run, "report.json");
    assert_eq!(report["Candidate"].as_str().unwrap().len(), 64);
    assert_eq!(space.artifact(&port_run, "requests.json")[0]["Args"], json!([3]));
    assert_eq!(space.artifact(&port_run, "observed.json"), json!([3]));
}

#[test]
fn both_retest_directions_substantiate_corrected_and_confirmed_expectations() {
    let Some((cli, adapter)) = tools() else { return };
    let space = Scratch::new();
    let cases = space.file("cases.json", json!([input("wrong", &[3], 3)]));
    let args = [command(&["check"], &[&adapter, &cases, &space.0]), vec!["probe-batch".into()]].concat();
    let legacy = csharp(&cli, &args);
    failed(&legacy);
    let legacy_run = json_output(&legacy)["Run"].as_str().unwrap().to_owned();
    let decision = space.file("corrected.json", json!([{"Index":0,"Reason":"Contract says x+1","Expected":4}]));
    let ack = space.file("acks.json", json!([]));
    let retest = rust(&command(&["check-retest"], &[&adapter, &space.0, Path::new(&legacy_run), &decision, &ack]));
    success(&retest);
    assert_eq!(json_output(&retest)["Added"], 1);
    let review = csharp(&cli, &[command(&["check-review"], &[&space.0, &ack]),
        vec!["--run".into(), legacy_run]].concat());
    success(&review);

    let space = Scratch::new();
    let cases = space.file("cases.json", json!([input("wrong", &[3], 3)]));
    let initial = rust(&[command(&["check"], &[&adapter, &cases, &space.0]), vec!["probe-batch".into()]].concat());
    failed(&initial);
    let run = json_output(&initial)["Run"].as_str().unwrap().to_owned();
    let decisions = space.file("confirmed.json", json!([{"Index":0,"Reason":"Independent rule requires x"}]));
    let acks = space.file("acks.json", json!([]));
    let legacy_retest = csharp(&cli,
        &command(&["check-retest"], &[&adapter, &space.0, Path::new(&run), &decisions, &acks]));
    success(&legacy_retest);
    let rust_review = rust(&[command(&["check-review"], &[&space.0, &acks]),
        vec!["--run".into(), run]].concat());
    success(&rust_review);
    assert_eq!(json_output(&rust_review)["Outstanding"], 0);
}

#[test]
fn vanished_discrepancy_keeps_attempt_separate_and_does_not_acknowledge() {
    let Some((_, adapter)) = tools() else { return };
    let space = Scratch::new();
    let cases = space.file("original.json", json!([
        input("changes", &[0], 1), input("steady", &[1], 1), input("steady", &[2], 2),
        input("steady", &[3], 3), input("steady", &[4], 4)]));
    let original = rust(&[command(&["check"], &[&adapter, &cases, &space.0]), vec!["probe-batch".into()]].concat());
    failed(&original);
    let run = json_output(&original)["Run"].as_str().unwrap().to_owned();
    let decisions = space.file("decisions.json", json!([{"Index":0,"Reason":"Expected one"}]));
    let ack = space.file("ack.json", json!([]));
    let retest = rust(&command(&["check-retest"], &[&adapter, &space.0, Path::new(&run), &decisions, &ack]));
    failed(&retest);
    assert!(String::from_utf8_lossy(&retest.stderr).contains("did not reproduce"));
    assert_eq!(serde_json::from_slice::<Value>(&fs::read(&ack).unwrap()).unwrap(), json!([]));
    let attempts = space.0.join(".slophawk").join("retest-attempts");
    assert_eq!(fs::read_dir(attempts).unwrap().count(), 1);
}

#[test]
fn truncated_and_reordered_arrays_never_silently_pass() {
    let Some((_, adapter)) = tools() else { return };
    let space = Scratch::new();
    let short = space.file("short.json", json!([input("short", &[3], 3)]));
    let output = rust(&[command(&["check"], &[&adapter, &short, &space.0]), vec!["probe-batch".into()]].concat());
    failed(&output);
    assert!(String::from_utf8_lossy(&output.stderr).contains("exactly one result"));
    let incomplete: Vec<_> = fs::read_dir(space.checks()).unwrap().map(|e|e.unwrap().file_name()).collect();
    assert!(incomplete.iter().any(|name| name.to_string_lossy().starts_with(".failed-")));
    let reordered = space.file("reordered.json", json!([input("reorder", &[1], 1), input("steady", &[2], 2)]));
    let output = rust(&[command(&["check"], &[&adapter, &reordered, &space.0]), vec!["probe-batch".into()]].concat());
    failed(&output);
    assert_eq!(json_output(&output)["Result"]["Findings"].as_array().unwrap().len(), 2);
}

#[test]
fn stale_acknowledgment_and_competing_lock_block_publication() {
    let Some((_, adapter)) = tools() else { return };
    let space = Scratch::new();
    let cases = space.file("cases.json", json!([input("wrong", &[3], 3)]));
    let output = rust(&[command(&["check"], &[&adapter, &cases, &space.0]), vec!["probe-batch".into()]].concat());
    failed(&output);
    let run = json_output(&output)["Run"].as_str().unwrap().to_owned();
    let ack = space.file("ack.json", json!([{"Run":run,"Index":0,"RetestRun":"123456781234123412341234567890ab",
        "RetestIndex":0,"Disposition":"confirmed","Reason":"stale"}]));
    let invalid = rust(&[command(&["check-review"], &[&space.0, &ack]), vec!["--run".into(), run.clone()]].concat());
    failed(&invalid);
    assert!(String::from_utf8_lossy(&invalid.stderr).contains("acknowledgment"));
    fs::write(&ack, b"[]").unwrap();
    let lock_path = PathBuf::from(format!("{}.lock", ack.display()));
    let _lock = fs::OpenOptions::new().write(true).create_new(true).open(&lock_path).unwrap();
    let decision = space.file("decisions.json", json!([{"Index":0,"Reason":"Independent rule x"}]));
    let retest = rust(&command(&["check-retest"], &[&adapter, &space.0, Path::new(&run), &decision, &ack]));
    failed(&retest);
    assert_eq!(serde_json::from_slice::<Value>(&fs::read(&ack).unwrap()).unwrap(), json!([]));
    drop(_lock);
    fs::remove_file(lock_path).unwrap();
}

#[test]
fn independent_adapter_sets_require_run_selection_and_scoped_coverage() {
    let Some((_, adapter)) = tools() else { return };
    let space = Scratch::new();
    let first = space.file("first.json", json!([input("steady", &[3], 3)]));
    let second = space.file("second.json", json!([input("constant", &[4], 7)]));
    let a = rust(&[command(&["check"], &[&adapter, &first, &space.0]), vec!["probe-batch".into()]].concat());
    let b = rust(&[command(&["check"], &[&adapter, &second, &space.0]), vec!["alternate-probe-batch".into()]].concat());
    success(&a); success(&b);
    let ack = space.file("ack.json", json!([]));
    let scope = space.file("scope.json", json!({"Operations":["steady","constant"],"Ordinary":[],
        "Sources":[{"Operation":"constant","Component":"fees","Contract":"fees.md","Implementation":"Fees.cs"}]}));
    let combined = rust(&command(&["check-review"], &[&space.0, &ack, &scope]));
    failed(&combined);
    assert_eq!(json_output(&combined)["RequiresRunSelection"], true);
    assert_eq!(json_output(&combined)["EvidenceSets"].as_array().unwrap().len(), 2);
    let first_receipt = json_output(&a);
    let first_run = first_receipt["Run"].as_str().unwrap();
    let scoped = rust(&[command(&["check-review"], &[&space.0, &ack, &scope]),
        vec!["--run".into(), first_run.into(), "--progress".into()]].concat());
    success(&scoped);
    assert_eq!(json_output(&scoped)["MissingCoverage"], 1); // constant is not in the selected adapter
    assert_eq!(json_output(&scoped)["NextComponent"], "fees");
}

#[test]
fn empty_scoped_progress_observe_only_and_unsupported_keep_coverage_open() {
    let Some((_, adapter)) = tools() else { return };
    let space = Scratch::new();
    let acks = space.file("ack.json", json!([]));
    let scope = space.file("scope.json", json!({"Operations":["steady"],"Ordinary":[{"Operation":"steady","Args":[3]}],
        "MinimumOutsideOrdinary":1,"Sources":[{"Operation":"steady","Component":"core","Contract":"contract.md",
        "Implementation":"Source.cs"}]}));
    let progress = rust(&[command(&["check-review"], &[&space.0, &acks, &scope]), vec!["--progress".into()]].concat());
    success(&progress);
    assert_eq!(json_output(&progress)["MissingCoverage"], 1);
    let observation = space.file("requests.json", json!([{"Operation":"steady","Args":[4]},
        {"Operation":"unsupported","Args":[2]}]));
    let run = rust(&[command(&["check-observe"], &[&adapter, &observation, &space.0]), vec!["probe-batch".into()]].concat());
    failed(&run);
    assert_eq!(json_output(&run)["ObserveOnly"], true);
    assert_eq!(json_output(&run)["Result"]["Compared"], 0);
    let incomplete = rust(&command(&["check-review"], &[&space.0, &acks, &scope]));
    failed(&incomplete);
    assert_eq!(json_output(&incomplete)["MissingCoverage"], 1);
    let cases = space.file("checked.json", json!([input("steady", &[4], 4)]));
    let checked = rust(&[command(&["check"], &[&adapter, &cases, &space.0]), vec!["probe-batch".into()]].concat());
    success(&checked);
    let complete = rust(&command(&["check-review"], &[&space.0, &acks, &scope]));
    success(&complete); // observe-only unsupported is not a comparison blocker
    assert_eq!(json_output(&complete)["MissingCoverage"], 0);
}

#[test]
fn unstable_supported_observations_and_mutated_identity_are_not_approved() {
    let Some((_, adapter)) = tools() else { return };
    let space = Scratch::new();
    let one = space.file("one.json", json!([input("changes", &[0], 3)]));
    let five = space.file("five.json", json!([input("changes", &[0], 2), input("steady", &[1], 1),
        input("steady", &[2], 2),input("steady", &[3], 3),input("steady", &[4], 4)]));
    let a = rust(&[command(&["check"], &[&adapter, &one, &space.0]), vec!["probe-batch".into()]].concat());
    let b = rust(&[command(&["check"], &[&adapter, &five, &space.0]), vec!["probe-batch".into()]].concat());
    success(&a); success(&b);
    let review = rust(&command(&["check-review"], &[&space.0]));
    failed(&review);
    assert_eq!(json_output(&review)["UnstableInputs"], 1);
    let run = json_output(&a)["Run"].as_str().unwrap().to_owned();
    let path = space.checks().join(run).join("identity.json");
    let mut identity: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    identity["ObservedSha256"] = json!("0".repeat(64));
    fs::write(&path, serde_json::to_vec(&identity).unwrap()).unwrap();
    let review = rust(&command(&["check-review"], &[&space.0]));
    failed(&review);
    assert!(String::from_utf8_lossy(&review.stderr).contains("identity"));
}
