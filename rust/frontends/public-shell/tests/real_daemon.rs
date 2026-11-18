//! Windows-only integration source against the *real published* C# `serve`
//! entry point. No daemon is started merely by compiling this crate. These
//! ignored tests are explicitly owned by the integrator's runtime pass.
#![cfg(windows)]
use slophawk_public_shell::identity::WorkspaceIdentity;
use slophawk_public_shell::mcp;
use slophawk_public_shell::protocol::{Command, DaemonClient};
use slophawk_public_shell::transport::NativeTransport;
use serde_json::{Value, json};
use std::fs::{self, OpenOptions};
use std::io::{Cursor, Read, Write};
use std::path::PathBuf;
use std::process::{Child, Command as ProcessCommand, Stdio};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

struct Live {
    root: PathBuf,
    daemon: Child,
    client: DaemonClient<NativeTransport>,
    pipe: String,
}

impl Live {
    fn start() -> Self {
        let executable = std::env::var_os("SLOPHAWK_TEST_CSHARP_APPHOST")
            .expect("Set SLOPHAWK_TEST_CSHARP_APPHOST to a built, trusted SlopHawk.Cli apphost (not dotnet).");
        let unique = format!("slophawk-native-ipc-{}-{}", std::process::id(),
            SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos());
        let root = std::env::temp_dir().join(unique);
        fs::create_dir(&root).expect("create isolated workspace");
        let identity = WorkspaceIdentity::resolve(&root).expect("physical workspace identity");
        let mut daemon = ProcessCommand::new(executable)
            .args(["serve", "--root", root.to_str().unwrap()])
            .stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null())
            .spawn().expect("published C# serve launches");
        let pipe = format!(r"\\.\pipe\{}", identity.instance_name);
        let client = DaemonClient::new(NativeTransport::new(identity.instance_name),
            root.to_string_lossy().into_owned());
        let deadline = Instant::now() + Duration::from_secs(12);
        loop {
            if client.negotiate().is_ok() { break; }
            if let Some(status) = daemon.try_wait().unwrap() {
                panic!("C# daemon exited during readiness: {status}");
            }
            assert!(Instant::now() < deadline, "real daemon never became ready");
            thread::sleep(Duration::from_millis(60));
        }
        Self { root, daemon, client, pipe }
    }
}

impl Drop for Live {
    fn drop(&mut self) {
        let _ = self.client.command(Command::Stop);
        let deadline = Instant::now() + Duration::from_secs(5);
        while self.daemon.try_wait().ok().flatten().is_none() && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(50));
        }
        if self.daemon.try_wait().ok().flatten().is_none() { let _ = self.daemon.kill(); }
        let _ = self.daemon.wait();
        let _ = fs::remove_dir_all(&self.root);
    }
}

#[test]
#[ignore = "owner-only: requires built C# SlopHawk.Cli apphost"]
fn real_daemon_handshake_status_recheck_results_and_stop() {
    let live = Live::start();
    let handshake = live.client.negotiate().unwrap();
    assert_eq!(handshake.wire_version, 1);
    assert_eq!(handshake.report_schema_version, 2);
    assert_eq!(handshake.capabilities,
        ["status", "results", "recheck", "stop", "list_checkers"]
            .map(str::to_owned).to_vec());
    let status = live.client.command(Command::Status).unwrap();
    assert!(status.ok);
    assert_eq!(status.payload.unwrap()["reportSchemaVersion"], 2);
    let recheck = live.client.command(Command::Recheck).unwrap();
    assert!(recheck.ok);
    assert!(recheck.payload.unwrap()["revision"].as_i64().unwrap() > 0);
    // The watcher is live: a read racing a new reconciliation is explicitly stale.
    match live.client.command(Command::Results) {
        Ok(results) if results.ok => assert_eq!(results.payload.unwrap()["schemaVersion"], 2),
        Ok(results) => assert_eq!(results.code.as_deref(), Some("no_current_report")),
        Err(error) => assert_eq!(error.code, "stale_revision"),
    }
    let stopped = live.client.command(Command::Stop).unwrap();
    assert!(stopped.ok);
}

#[test]
#[ignore = "owner-only: requires built C# SlopHawk.Cli apphost"]
fn real_daemon_slow_client_and_late_ack_cannot_starve_status() {
    let live = Live::start();
    let mut slow = OpenOptions::new().read(true).write(true).open(&live.pipe).unwrap();
    slow.write_all(&[6]).unwrap(); // incomplete version envelope: 750 ms server read deadline
    thread::sleep(Duration::from_millis(100));
    let started = Instant::now();
    assert!(live.client.command(Command::Status).unwrap().ok);
    assert!(started.elapsed() < Duration::from_secs(2));
    drop(slow);

    let mut unacked = OpenOptions::new().read(true).write(true).open(&live.pipe).unwrap();
    unacked.write_all(&[1]).unwrap(); // legacy status, deliberately no ACK
    let mut length = [0u8; 4];
    unacked.read_exact(&mut length).unwrap();
    let size = i32::from_le_bytes(length);
    assert!((1..=8 * 1024 * 1024 + 4096).contains(&size));
    let mut reply = vec![0u8; size as usize];
    unacked.read_exact(&mut reply).unwrap();
    let started = Instant::now();
    assert!(live.client.command(Command::Status).unwrap().ok);
    assert!(started.elapsed() < Duration::from_secs(2));
    drop(unacked);
}

#[test]
#[ignore = "owner-only: requires built C# SlopHawk.Cli apphost"]
fn real_daemon_mcp_projection_is_schema_v2_and_list_checkers_is_empty() {
    let live = Live::start();
    let input = concat!(
        "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"initialize\",\"params\":{\"protocolVersion\":\"2025-11-25\"}}\n",
        "{\"jsonrpc\":\"2.0\",\"method\":\"notifications/initialized\"}\n",
        "{\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"tools/list\"}\n",
        "{\"jsonrpc\":\"2.0\",\"id\":3,\"method\":\"tools/call\",\"params\":{\"name\":\"list_checkers\"}}\n",
        "{\"jsonrpc\":\"2.0\",\"id\":4,\"method\":\"tools/call\",\"params\":{\"name\":\"workspace_status\"}}\n",
    );
    let mut output = Vec::new();
    mcp::serve(&live.client, Cursor::new(input), &mut output).unwrap();
    let messages: Vec<Value> = output.split(|b| *b == b'\n').filter(|m| !m.is_empty())
        .map(|m| serde_json::from_slice(m).unwrap()).collect();
    assert_eq!(messages.len(), 4);
    let tools: Vec<_> = messages[1]["result"]["tools"].as_array().unwrap()
        .iter().map(|t| t["name"].as_str().unwrap()).collect();
    assert_eq!(tools, ["workspace_status", "get_findings", "run_checks", "list_checkers"]);
    assert_eq!(messages[2]["result"], json!({"content":[{"type":"text","text":"[]"}]}));
    let text = messages[3]["result"]["content"][0]["text"].as_str().unwrap();
    assert_eq!(serde_json::from_str::<Value>(text).unwrap()["payload"]["reportSchemaVersion"], 2);
}
