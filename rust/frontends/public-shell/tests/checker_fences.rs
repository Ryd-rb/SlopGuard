//! Deterministic request-boundary schedules; no processes, clocks, or sleeps.
use serde_json::{Value, json};
use slophawk_public_shell::{Result, cli, mcp};
use slophawk_public_shell::protocol::{
    Command, DaemonClient, HELLO, Transport, VERSIONED_REQUEST, WIRE_VERSION,
};
use std::cell::RefCell;
use std::collections::VecDeque;
use std::io::Cursor;
use std::rc::Rc;

const ROOT: &str = "C:\\work";
type Step = (Vec<u8>, Value);

#[derive(Clone)]
struct Wire(Rc<RefCell<VecDeque<Step>>>);

impl Transport for Wire {
    fn exchange(&self, request: &[u8]) -> Result<Option<Vec<u8>>> {
        let (expected, reply) = self.0.borrow_mut().pop_front().expect("unexpected extra exchange");
        assert_eq!(request, expected);
        Ok(Some(serde_json::to_vec(&reply).unwrap()))
    }
}

fn hello() -> Step {
    (vec![HELLO], json!({"ok":true,"payload":{
        "wireVersion":1,"reportSchemaVersion":2,"capabilities":["status","results"]}}))
}

fn exchange(command: Command, payload: Value) -> [Step; 2] {
    [hello(), (vec![VERSIONED_REQUEST, WIRE_VERSION, command as u8],
        json!({"ok":true,"payload":payload}))]
}

fn fence(profile: char, receipt: char) -> Value {
    json!({"version":1,"profileSha256":profile.to_string().repeat(64),
        "receiptSha256":receipt.to_string().repeat(64)})
}

fn add_fence(mut value: Value, token: Option<Value>) -> Value {
    if let Some(token) = token { value["checkerFence"] = token; }
    value
}

fn status(token: Option<Value>) -> Value {
    add_fence(json!({"root":ROOT,"epoch":"epoch-1","revision":9,
        "committedRevision":9,"reportSchemaVersion":2,"state":"ready"}), token)
}

fn report(checks: Option<Value>, token: Option<Value>) -> Value {
    let mut report = add_fence(json!({"root":ROOT,"epoch":"epoch-1","revision":9,
        "committedRevision":9,"schemaVersion":2,"producer":"SlopHawk",
        "fileCount":2,"partial":false}), token);
    if let Some(checks) = checks { report["checkerReport"] = checks; }
    report
}

fn checked(token: Value) -> Value {
    report(Some(json!({"status":"complete","pairs":[],"documents":[]})), Some(token))
}

fn script(before: Value, report: Value, after: Value) -> Vec<Step> {
    [(Command::Status, before), (Command::Results, report), (Command::Status, after)]
        .into_iter().flat_map(|(command, payload)| exchange(command, payload)).collect()
}

fn cli_result(steps: Vec<Step>) -> Result<cli::Projection> {
    let wire = Wire(Rc::new(RefCell::new(steps.into())));
    let client = DaemonClient::new(wire.clone(), ROOT.into());
    let result = client.command(Command::Results).and_then(|reply| cli::project(&reply));
    assert!(wire.0.borrow().is_empty(), "CLI skipped a required fence");
    result
}

fn mcp_result(steps: Vec<Step>) -> Value {
    // initialize and tools/call refresh each negotiate before the six read exchanges.
    let mut frames = vec![hello(), hello()];
    frames.extend(steps);
    let wire = Wire(Rc::new(RefCell::new(frames.into())));
    let client = DaemonClient::new(wire.clone(), ROOT.into());
    let input = concat!(
        "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"initialize\",\"params\":{\"protocolVersion\":\"2025-11-25\"}}\n",
        "{\"jsonrpc\":\"2.0\",\"method\":\"notifications/initialized\"}\n",
        "{\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"tools/call\",\"params\":{\"name\":\"get_findings\",\"arguments\":{}}}\n",
    );
    let mut output = Vec::new();
    mcp::serve(&client, Cursor::new(input), &mut output).unwrap();
    assert!(wire.0.borrow().is_empty(), "MCP skipped a required fence");
    let messages: Vec<Value> = output.split(|byte| *byte == b'\n').filter(|line| !line.is_empty())
        .map(|line| serde_json::from_slice(line).unwrap()).collect();
    assert_eq!(messages.len(), 2);
    assert_eq!(messages[0]["result"]["protocolVersion"], "2025-11-25");
    assert_eq!(messages[1]["id"], 2);
    let content = messages[1]["result"]["content"].as_array().unwrap();
    assert_eq!(content.len(), 1);
    assert_eq!(content[0]["type"], "text");
    serde_json::from_str(content[0]["text"].as_str().unwrap()).unwrap()
}

fn assert_current(before: Value, report: Value, after: Value) {
    let steps = script(before, report.clone(), after);
    let projected = cli_result(steps.clone()).unwrap();
    assert_eq!(projected.exit_code, 0);
    assert!(projected.stderr.is_none());
    let payload: Value = serde_json::from_str(&projected.stdout.unwrap()).unwrap();
    assert_eq!(payload, report);
    assert_eq!(mcp_result(steps), json!({"ok":true,"payload":report}));
}

fn assert_rejected(before: Value, report: Value, after: Value, code: &str) {
    let steps = script(before, report, after);
    let error = cli_result(steps.clone()).unwrap_err();
    assert_eq!(error.code, code);
    assert_eq!(mcp_result(steps), json!({"ok":false,"error":error.message,"code":code}));
}

#[test]
fn stable_positive_negative_and_partial_evidence_is_preserved_by_both_readers() {
    for checks in [
        json!({"status":"complete","pairs":[{"left":"A.py","right":"B.py"}]}),
        json!({"status":"complete","pairs":[],"documents":[{"path":"A.py"}]}),
        json!({"status":"partial","pairs":[],"unsupported":[{"path":"B.py"}]}),
    ] {
        let token = fence('a', 'B'); // Both ASCII hex cases are valid.
        let report = report(Some(checks), Some(token.clone()));
        assert_current(status(Some(token.clone())), report, status(Some(token)));
    }
}

#[test]
fn same_source_profile_and_receipt_replacement_cannot_pass_the_read_fence() {
    let old = fence('a', 'b');
    for replacement in [fence('c', 'b'), fence('a', 'd')] {
        // Replacement before Results, after Results, and a different middle observation.
        for (before, middle, after) in [
            (old.clone(), replacement.clone(), replacement.clone()),
            (old.clone(), old.clone(), replacement.clone()),
            (replacement.clone(), old.clone(), old.clone()),
            (old.clone(), replacement.clone(), old.clone()),
        ] {
            assert_rejected(status(Some(before)), checked(middle), status(Some(after)),
                "stale_checker_receipt");
        }
    }
}

#[test]
fn profile_revocation_or_missing_status_attestation_withdraws_checker_evidence() {
    let token = fence('a', 'b');
    for missing in [None, Some(Value::Null)] {
        assert_rejected(status(Some(token.clone())), checked(token.clone()), status(missing.clone()),
            "stale_checker_receipt");
        assert_rejected(status(missing), checked(token.clone()), status(Some(token.clone())),
            "stale_checker_receipt");
    }
}

#[test]
fn older_checker_reports_without_attestation_are_unavailable() {
    for missing in [None, Some(Value::Null)] {
        let mut report = checked(fence('a', 'b'));
        report.as_object_mut().unwrap().remove("checkerFence");
        report = add_fence(report, missing.clone());
        assert_rejected(status(missing.clone()), report.clone(), status(missing),
            "checker_fence_unavailable");
        assert_rejected(status(Some(fence('a', 'b'))), report, status(Some(fence('a', 'b'))),
            "checker_fence_unavailable");
    }
    assert_rejected(status(None), report(Some(json!({})), None), status(None),
        "checker_fence_unavailable");
}

#[test]
fn older_inventory_without_checker_evidence_still_projects() {
    for missing in [None, Some(Value::Null)] {
        for checks in [None, Some(Value::Null)] {
            assert_current(status(missing.clone()), report(checks, missing.clone()),
                status(missing.clone()));
        }
    }
}

#[test]
fn failed_or_incomplete_checker_does_not_turn_inventory_into_a_clean_check() {
    for state in ["failed", "blocked"] {
        let mut before = status(Some(fence('a', 'b')));
        before["checkerScheduler"] = json!({"state":state,"error":"no current checker evidence"});
        let mut after = before.clone();
        after["checkerFence"] = fence('a', 'c');
        let mut inventory = report(None, None);
        inventory["partial"] = json!(state == "blocked");
        assert_current(before, inventory, after);
    }
}

#[test]
fn malformed_or_unsupported_tokens_fail_at_each_observation() {
    let token = fence('a', 'b');
    let mut malformed = vec![json!(false), json!([]), json!({})];
    for (key, value) in [
        ("version", json!(2)), ("version", json!("1")),
        ("profileSha256", json!("a".repeat(63))),
        ("profileSha256", json!("é".repeat(32))),
        ("receiptSha256", json!("g".repeat(64))),
        ("receiptSha256", Value::Null), ("extra", json!(true)),
    ] {
        let mut bad = token.clone();
        bad[key] = value;
        malformed.push(bad);
    }
    for bad in malformed {
        for position in 0..3 {
            let mut frames = [status(Some(token.clone())), checked(token.clone()), status(Some(token.clone()))];
            frames[position]["checkerFence"] = bad.clone();
            let [before, report, after] = frames;
            assert_rejected(before, report, after, "invalid_checker_fence");
        }
    }
}

#[test]
fn malformed_checker_report_is_not_treated_as_inventory_only() {
    for checks in [json!(false), json!([]), json!("invalid"), json!(42)] {
        let token = fence('a', 'b');
        assert_rejected(status(Some(token.clone())), report(Some(checks), Some(token.clone())),
            status(Some(token)), "invalid_reply");
    }
}

#[test]
fn source_and_publication_fences_remain_authoritative() {
    let token = fence('a', 'b');
    for (key, value) in [("revision", json!(10)), ("epoch", json!("new-epoch")),
        ("committedRevision", json!(10)), ("state", json!("pending"))] {
        let mut after = status(Some(fence('a', 'c')));
        after[key] = value;
        assert_rejected(status(Some(token.clone())), checked(token.clone()), after, "stale_revision");
    }
    for (key, value) in [("root", json!("other-root")), ("schemaVersion", json!(3)),
        ("producer", json!("other-producer"))] {
        let mut report = checked(token.clone());
        report[key] = value;
        assert_rejected(status(Some(token.clone())), report, status(Some(token.clone())), "stale_revision");
    }
}

#[test]
fn no_current_report_keeps_its_code_and_does_not_request_a_post_fence() {
    let reply = json!({"ok":false,"error":"No current inventory report.","code":"no_current_report"});
    let mut steps = exchange(Command::Status, status(None)).to_vec();
    steps.push(hello());
    steps.push((vec![VERSIONED_REQUEST, WIRE_VERSION, Command::Results as u8], reply.clone()));
    let projected = cli_result(steps.clone()).unwrap();
    assert_eq!(projected.exit_code, 1);
    assert!(projected.stdout.is_none());
    assert!(projected.stderr.unwrap().contains("no_current_report"));
    assert_eq!(mcp_result(steps), reply);
}
