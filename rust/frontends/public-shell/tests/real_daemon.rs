#![cfg(any(windows, target_os = "linux"))]

use serde_json::{Value, json};
use std::fs;
use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

struct Fixture {
    container: PathBuf,
    root: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let mut nonce = [0_u8; 16];
        getrandom::fill(&mut nonce).unwrap();
        let container = std::env::temp_dir().join(format!(
            "slophawk-public-{}", slophawk_public_shell::identity::uppercase_hex(&nonce)
        ));
        let root = container.join("workspace");
        fs::create_dir_all(&root).unwrap();
        fs::write(root.join("Source.cs"), "class Source { }").unwrap();
        Self { container, root }
    }

    fn command(&self, verb: &str) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_slophawk"));
        command.arg(verb).arg("--root").arg(&self.root);
        #[cfg(windows)]
        command.env("LOCALAPPDATA", self.container.join("app-data"));
        #[cfg(target_os = "linux")]
        command.env("XDG_DATA_HOME", self.container.join("app-data"));
        command
    }

    fn request(&self, verb: &str) -> Value {
        let output = self.command(verb).output().unwrap();
        assert!(output.status.success(), "{verb}: {}", String::from_utf8_lossy(&output.stderr));
        serde_json::from_slice(&output.stdout).unwrap()
    }

    fn await_state(&self, verb: &str, accept: impl Fn(&Value) -> bool) -> Value {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let output = self.command(verb).output().unwrap();
            if output.status.success() {
                let value = serde_json::from_slice(&output.stdout).unwrap();
                if accept(&value) { return value; }
            }
            assert!(Instant::now() < deadline, "{verb}: {} {}",
                String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
            thread::sleep(Duration::from_millis(50));
        }
    }

    fn wait_stopped(&self) {
        let deadline = Instant::now() + Duration::from_secs(10);
        while self.command("status").output().unwrap().status.success() {
            assert!(Instant::now() < deadline, "daemon stayed alive");
            thread::sleep(Duration::from_millis(50));
        }
    }

    fn mcp(&self) -> Vec<Value> {
        self.mcp_with(&[])
    }

    fn mcp_with(&self, extra: &[Value]) -> Vec<Value> {
        let mut child = self.command("mcp").stdin(Stdio::piped()).stdout(Stdio::piped())
            .stderr(Stdio::piped()).spawn().unwrap();
        let mut input = child.stdin.take().unwrap();
        let commands = [
            json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-11-25"}}),
            json!({"jsonrpc":"2.0","method":"notifications/initialized"}),
            json!({"jsonrpc":"2.0","id":2,"method":"tools/list"}),
            json!({"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"get_findings","arguments":{}}}),
            json!({"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"list_checkers","arguments":{}}}),
        ];
        for message in commands.iter().chain(extra) {
            writeln!(input, "{message}").unwrap();
        }
        drop(input);
        let output = child.wait_with_output().unwrap();
        assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
        String::from_utf8(output.stdout).unwrap().lines()
            .map(|line| serde_json::from_str(line).unwrap()).collect()
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        if self.command("stop").output().is_ok_and(|output| output.status.success()) {
            self.wait_stopped();
        }
        let _ = fs::remove_dir_all(&self.container);
    }
}

fn tool_result(messages: &[Value], id: u64) -> Value {
    let message = messages.iter().find(|message| message["id"] == id).unwrap();
    let text = message["result"]["content"][0]["text"].as_str()
        .unwrap_or_else(|| panic!("unexpected MCP response: {message}"));
    serde_json::from_str(text).unwrap()
}

#[test]
fn installed_rust_daemon_starts_reconciles_restarts_and_serves_mcp() {
    let fixture = Fixture::new();
    assert!(!fixture.command("status").output().unwrap().status.success());
    fixture.request("start");
    let initial = fixture.await_state("results", |report| report["fileCount"] == 1);
    assert_eq!(initial["schemaVersion"], 2);
    fs::write(fixture.root.join("Other.cs"), "class Other {}").unwrap();
    let updated = fixture.await_state("results", |report| report["fileCount"] == 2);
    assert!(updated["committedRevision"].as_i64() > initial["committedRevision"].as_i64());
    let revision = fixture.request("recheck")["revision"].as_u64().unwrap();
    let current = fixture.await_state("results", |report| report["revision"] == revision);

    let messages = fixture.mcp();
    assert_eq!(messages[1]["result"]["tools"].as_array().unwrap().len(), 5);
    assert_eq!(tool_result(&messages, 3)["payload"], current);
    let listed = tool_result(&messages, 4);
    assert_eq!(listed, fixture.request("list-checkers"));
    let languages: Vec<_> = listed.as_array().unwrap().iter()
        .map(|checker| checker["language"].as_str().unwrap()).collect();
    assert_eq!(languages, ["csharp", "python", "rust", "fluxr"]);
    fixture.request("stop");
    fixture.wait_stopped();
    fixture.request("start");
    let restarted = fixture.await_state("results", |report| report["fileCount"] == 2);
    assert_eq!(current["committedRevision"], restarted["committedRevision"]);
    assert_ne!(current["epoch"], restarted["epoch"]);
}

#[test]
fn installed_rust_daemon_recovers_the_writer_after_process_exit() {
    let fixture = Fixture::new();
    let mut first = fixture.command("serve").stdin(Stdio::null()).stdout(Stdio::null())
        .stderr(Stdio::null()).spawn().unwrap();
    let before = fixture.await_state("results", |report| report["fileCount"] == 1);
    first.kill().unwrap();
    first.wait().unwrap();
    assert!(!fixture.command("status").output().unwrap().status.success());
    fixture.request("start");
    let recovered = fixture.await_state("results", |report| report["fileCount"] == 1);
    assert_eq!(before["committedRevision"], recovered["committedRevision"]);
    assert_ne!(before["epoch"], recovered["epoch"]);
}

#[test]
fn installed_rust_daemon_publishes_roslyn_findings_through_cli_and_mcp() {
    let (Ok(host), Ok(worker), Ok(pack)) = (std::env::var("VARO_DOTNET"),
        std::env::var("VARO_WORKER_DLL"), std::env::var("VARO_REFERENCE_PACK")) else {
        assert!(std::env::var_os("VARO_REQUIRE_REAL_WORKER").is_none(), "Roslyn fixture needs all three pins");
        return;
    };
    use sha2::{Digest, Sha256};
    let fixture = Fixture::new();
    let source = "partial class A { static int F(int x) => x + 1; }";
    fs::write(fixture.root.join("Source.cs"), source).unwrap();
    fs::write(fixture.root.join("Duplicate.cs"), source).unwrap();
    let status = fixture.request("start");
    let execution = &status["executionProfile"];
    let policy = PathBuf::from(execution["path"].as_str().unwrap()).with_file_name("native-checkers.json");
    let pin = |path: &str| json!({"path":path, "sha256":format!("{:X}", Sha256::digest(fs::read(path).unwrap()))});
    let registration = json!({"version":1,"rootIdentity":execution["rootIdentity"],
        "context":{"key":"net10-fixture","pythonMinor":12,"rustEdition":"2024","fluxrModule":"fixture",
            "targetFramework":"net10.0","assemblyName":"Fixture","languageVersion":"CSharp14",
            "referencePack":pack,"referenceClosureComplete":true,"sourceClosureComplete":true},
        "worker":{"dotnet":pin(&host),"assembly":pin(&worker)}});
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    serde_json::to_writer(options.open(&policy).unwrap(), &registration).unwrap();
    let current = fixture.await_state("results", |report| report["checkerReport"]["pairs"]
        .as_array().is_some_and(|pairs| pairs.iter().any(|pair| pair["classification"] == "exact")));
    assert_eq!(current["checkerReport"]["workerInvocations"], 2);
    assert_eq!(tool_result(&fixture.mcp(), 3)["payload"]["checkerReport"], current["checkerReport"]);
    fs::write(fixture.root.join("Duplicate.cs"), source.replace("+ 1", "+ 2")).unwrap();
    let replaced = fixture.await_state("results", |report|
        report["committedRevision"].as_i64() > current["committedRevision"].as_i64() &&
        report["checkerReport"]["pairs"].as_array().is_some_and(Vec::is_empty));
    assert!(replaced["checkerReport"]["documents"].as_array().is_some_and(|docs| docs.len() == 2));
}

#[test]
fn near_miss_profile_is_opt_in_and_publishes_current_matches_to_cli_and_mcp() {
    let fixture = Fixture::new();
    fs::remove_file(fixture.root.join("Source.cs")).unwrap();
    let source = "fn compute(x: i32) -> i32 { let y = x + 1; y + 2 }";
    fs::write(fixture.root.join("First.rs"), source).unwrap();
    fs::write(fixture.root.join("Second.rs"), source.replace("+ 2", "+ 3")).unwrap();
    let python_source = "def f(value):\n    total = value + 1\n    total = total * 2\n    return total + 3\n";
    fs::write(fixture.root.join("First.py"), python_source).unwrap();
    fs::write(fixture.root.join("Second.py"), python_source.replace("+ 3", "+ 4")).unwrap();
    fixture.request("start");
    let ready = fixture.await_state("status", |status| status["state"] == "ready");
    let execution = &ready["executionProfile"];
    let policy = PathBuf::from(execution["path"].as_str().unwrap()).with_file_name("native-checkers.json");
    let mut registration = json!({
        "version": 1, "rootIdentity": execution["rootIdentity"],
        "context": {"key": "near-fixture", "rustEdition": "2024"}
    });
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    serde_json::to_writer(options.open(&policy).unwrap(), &registration).unwrap();

    let baseline = fixture.await_state("results", |report| report["checkerReport"].is_object());
    assert!(baseline["checkerReport"].get("nearMissClones").is_none());
    registration["nearMissClones"] = json!(true);
    fs::write(&policy, serde_json::to_vec(&registration).unwrap()).unwrap();
    let current = fixture.await_state("results", |report| {
        report["checkerReport"]["nearMissClones"].is_array()
    });
    let report = &current["checkerReport"];
    assert_eq!(report["pairs"], baseline["checkerReport"]["pairs"]);
    let rows = report["nearMissClones"].as_array().unwrap();
    let rust = rows.iter().find(|row| row["language"] == "rust").unwrap();
    assert_eq!(rust["status"], "complete");
    let pairs = rust["pairs"].as_array().unwrap();
    assert_eq!(pairs.len(), 1, "{rust}");
    assert_eq!(pairs[0]["editDistance"], 2);
    assert_ne!(pairs[0]["left"]["documentId"], pairs[0]["right"]["documentId"]);
    let bytes = &pairs[0]["left"]["bytes"];
    let start = bytes["start"].as_u64().unwrap() as usize;
    let end = bytes["end"].as_u64().unwrap() as usize;
    assert!(source[start..end].starts_with('{'));
    let python = rows.iter().find(|row| row["language"] == "python").unwrap();
    assert_eq!(python["ruleVersion"], "sg10.python-layout-near-misses.v1");
    assert_eq!(python["status"], "complete");
    let python_pairs = python["pairs"].as_array().unwrap();
    assert_eq!(python_pairs.len(), 1, "{python}");
    assert_eq!(python_pairs[0]["editDistance"], 2);
    let bytes = &python_pairs[0]["left"]["bytes"];
    let start = bytes["start"].as_u64().unwrap() as usize;
    let end = bytes["end"].as_u64().unwrap() as usize;
    assert_eq!(&python_source[start..end], python_source.trim_end());
    assert_eq!(tool_result(&fixture.mcp(), 3)["payload"]["checkerReport"], *report);

    registration["nearMissClones"] = json!(false);
    fs::write(&policy, serde_json::to_vec(&registration).unwrap()).unwrap();
    fixture.await_state("results", |report| {
        report["checkerReport"].is_object() && report["checkerReport"].get("nearMissClones").is_none()
    });
}

#[test]
fn public_cli_and_mcp_share_opt_in_checker_plans() {
    let fixture = Fixture::new();
    fixture.request("start");
    let scalar = json!({
        "checker": "scalar_correlated",
        "facts": include_str!("../../../checkers/scalar-paths/fixtures/trace.facts")
    });
    let input = |args: [i64; 3]| json!({"Operation": "Ordered", "Args": args});
    let pool = [[1,10,20], [2,10,20], [3,10,20], [10,10,20]].map(input);
    let contract = json!({
        "checker": "contract_diversity",
        "rules": [{
            "Operation": "Ordered", "Parameters": ["a", "b", "c"], "Domain": "true",
            "Expected": "a < b && b < c ? 1 : 0", "Source": "ordered.md"
        }],
        "published": [{"Operation": "Ordered", "Args": [0, 10, 20], "Expected": 1}],
        "pool": pool,
        "perOperation": 2
    });
    let invalid = json!({"checker": "scalar_correlated", "facts": "not a fact stream"});
    let requests: Vec<_> = [&scalar, &contract, &invalid].into_iter().enumerate().map(|(i, args)|
        json!({"jsonrpc":"2.0","id":i+5,"method":"tools/call",
               "params":{"name":"plan_checker","arguments":args}})).collect();
    let messages = fixture.mcp_with(&requests);
    assert!(messages[1]["result"]["tools"].as_array().unwrap().iter()
        .any(|tool| tool["name"] == "plan_checker"));
    let from_mcp = [tool_result(&messages, 5), tool_result(&messages, 6)];
    assert_eq!(from_mcp[0]["policy"], "graph-correlated-v1");
    assert_eq!(from_mcp[0]["result"]["bound"], true);
    assert!(!from_mcp[0]["result"]["candidates"].as_array().unwrap().is_empty());
    assert_eq!(from_mcp[1]["policy"], "diverse-v1");
    assert_eq!(from_mcp[1]["result"]["cases"], json!([
        {"Operation":"Ordered","Args":[1,10,20],"Expected":1},
        {"Operation":"Ordered","Args":[10,10,20],"Expected":0}
    ]));
    assert_eq!(messages.iter().find(|m| m["id"] == 7).unwrap()["error"]["data"]["code"],
        "checker_plan_input");

    let path = fixture.container.join("checker-plan.json");
    for (request, mcp_plan) in [&scalar, &contract].into_iter().zip(from_mcp) {
        fs::write(&path, serde_json::to_vec(request).unwrap()).unwrap();
        let output = Command::new(env!("CARGO_BIN_EXE_slophawk"))
            .args(["checker-plan", "--request"]).arg(&path).output().unwrap();
        assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
        assert_eq!(serde_json::from_slice::<Value>(&output.stdout).unwrap(), mcp_plan);
    }
}
