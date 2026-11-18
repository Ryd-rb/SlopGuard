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
        for message in commands {
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
    assert_eq!(messages[1]["result"]["tools"].as_array().unwrap().len(), 4);
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
