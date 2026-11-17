//! Published one-request daemon pipe protocol, not MCP and not migration-job/1.
//! `DaemonConnection.cs` and `DaemonWire.cs` at the VODA pinned base are the
//! interoperability contract. No private command is invented here.
use crate::{Result, ShellError, failure};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::io::{Read, Write};

pub const WIRE_VERSION: u8 = 1;
pub const REPORT_SCHEMA: u64 = 2;
pub const HELLO: u8 = 5;
pub const VERSIONED_REQUEST: u8 = 6;
pub const MAX_REPLY_BYTES: usize = 8 * 1024 * 1024 + 4096;

#[derive(Copy, Clone, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum Command { Status = 1, Results = 2, Recheck = 3, Stop = 4 }

impl Command {
    pub fn capability(self) -> &'static str {
        match self {
            Self::Status => "status",
            Self::Results => "results",
            Self::Recheck => "recheck",
            Self::Stop => "stop",
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DaemonReply {
    pub ok: bool,
    pub payload: Option<Value>,
    pub error: Option<String>,
    #[serde(default)]
    pub code: Option<String>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Handshake {
    pub wire_version: i64,
    pub report_schema_version: u64,
    pub capabilities: Vec<String>,
}

/// A single exchange opens a new same-user connection, writes a short command,
/// reads an entirely bounded frame, ACKs and closes it. `None` means that no
/// listener exists, not that a listener returned an error or malformed bytes.
pub trait Transport {
    fn exchange(&self, request: &[u8]) -> Result<Option<Vec<u8>>>;
}

pub struct DaemonClient<T> {
    transport: T,
    root: String,
}

impl<T: Transport> DaemonClient<T> {
    pub fn new(transport: T, root: String) -> Self { Self { transport, root } }

    pub fn negotiate(&self) -> Result<Handshake> {
        let reply = self.send_once(&[HELLO])?;
        if !reply.ok {
            if reply.code.as_deref() == Some("unknown_command") ||
                (reply.code.is_none() && reply.error.as_deref() == Some("Unknown daemon command.")) {
                return Err(failure("upgrade_required", "A legacy daemon lacks IPC hello; stop it with its matching SlopHawk build. It will not be replaced automatically."));
            }
            return Err(typed_failure(reply));
        }
        let payload = reply.payload.ok_or_else(|| failure("invalid_handshake", "Missing handshake payload."))?;
        let handshake: Handshake = serde_json::from_value(payload)
            .map_err(|e| failure("invalid_handshake", e))?;
        if handshake.capabilities.len() > 16 || handshake.capabilities.iter().any(|c| {
            c.is_empty() || c.len() > 32 || c.trim() != c
        }) || has_duplicates(&handshake.capabilities) {
            return Err(failure("invalid_handshake", "Invalid daemon capability list."));
        }
        if handshake.wire_version != WIRE_VERSION as i64 {
            return Err(failure("unsupported_wire_version", format!(
                "Daemon wire version {} is incompatible with version {WIRE_VERSION}; use matching builds.",
                handshake.wire_version)));
        }
        if handshake.report_schema_version != REPORT_SCHEMA {
            return Err(failure("unsupported_report_schema", format!(
                "Daemon report schema {} is incompatible with schema {REPORT_SCHEMA}; use matching builds.",
                handshake.report_schema_version)));
        }
        Ok(handshake)
    }

    pub fn command(&self, command: Command) -> Result<DaemonReply> {
        if command == Command::Results { return self.current_results(); }
        self.send_versioned(command)
    }

    fn send_versioned(&self, command: Command) -> Result<DaemonReply> {
        let handshake = self.negotiate()?;
        if !handshake.capabilities.iter().any(|c| c == command.capability()) {
            return Err(failure("unsupported_feature", format!(
                "The connected daemon does not advertise '{}'.", command.capability())));
        }
        let reply = self.send_once(&[VERSIONED_REQUEST, WIRE_VERSION, command as u8])?;
        if command == Command::Status && reply.ok {
            let status = reply.payload.as_ref().ok_or_else(|| failure("invalid_status", "Missing status payload."))?;
            self.validate_status(status)?;
        }
        Ok(reply)
    }

    fn current_results(&self) -> Result<DaemonReply> {
        // A report is current only if its publication fence matches two fresh
        // daemon status observations. A changed revision between replies is a
        // stale read, never a successful current finding projection.
        let before = self.send_versioned(Command::Status)?;
        if !before.ok { return Ok(before); }
        let report = self.send_versioned(Command::Results)?;
        if !report.ok { return Ok(report); } // Preserve `no_current_report`.
        let after = self.send_versioned(Command::Status)?;
        if !after.ok { return Err(typed_failure(after)); }
        let first = before.payload.ok_or_else(|| failure("invalid_status", "Missing pre-read status."))?;
        let second = after.payload.ok_or_else(|| failure("invalid_status", "Missing post-read status."))?;
        let payload = report.payload.as_ref().ok_or_else(|| failure("invalid_reply", "Missing report."))?;
        for key in ["epoch", "revision", "committedRevision"] {
            if first.get(key).is_none() || first.get(key) != second.get(key) ||
                first.get(key) != payload.get(key) {
                return Err(failure("stale_revision", format!("The report's {key} changed during the read.")));
            }
        }
        if first.get("state").and_then(Value::as_str) != Some("ready") ||
            second.get("state").and_then(Value::as_str) != Some("ready") ||
            payload.get("root").and_then(Value::as_str) != Some(self.root.as_str()) ||
            payload.get("producer").and_then(Value::as_str) != Some("SlopHawk") ||
            payload.get("schemaVersion").and_then(Value::as_u64) != Some(REPORT_SCHEMA) {
            return Err(failure("stale_revision", "The report is not a current schema-v2 SlopHawk projection."));
        }
        Ok(report)
    }

    fn validate_status(&self, status: &Value) -> Result<()> {
        if status.get("root").and_then(Value::as_str) != Some(self.root.as_str()) ||
            status.get("reportSchemaVersion").and_then(Value::as_u64) != Some(REPORT_SCHEMA) ||
            status.get("epoch").and_then(Value::as_str).is_none() ||
            status.get("revision").and_then(Value::as_u64).is_none() ||
            !matches!(status.get("state").and_then(Value::as_str), Some("ready" | "pending" | "degraded")) ||
            (status.get("state").and_then(Value::as_str) == Some("ready") &&
                status.get("committedRevision").and_then(Value::as_u64).is_none()) {
            return Err(failure("invalid_status", "Daemon returned a status for a different root or schema."));
        }
        Ok(())
    }

    fn send_once(&self, request: &[u8]) -> Result<DaemonReply> {
        let bytes = self.transport.exchange(request)?
            .ok_or_else(|| failure("daemon_unavailable", "No workspace daemon is listening."))?;
        if bytes.is_empty() || bytes.len() > MAX_REPLY_BYTES {
            return Err(failure("invalid_frame", "Daemon reply is empty or oversized."));
        }
        serde_json::from_slice(&bytes).map_err(|e| failure("invalid_reply", e))
    }
}

fn has_duplicates(values: &[String]) -> bool {
    values.iter().enumerate().any(|(i, v)| values[..i].contains(v))
}

fn read_failure(error: std::io::Error) -> ShellError {
    if error.kind() == std::io::ErrorKind::TimedOut {
        failure("daemon_timeout", error)
    } else {
        failure("truncated_frame", error)
    }
}

fn typed_failure(reply: DaemonReply) -> ShellError {
    ShellError::new(reply.code.unwrap_or_else(|| "daemon_error".into()),
        reply.error.unwrap_or_else(|| "The daemon rejected the request.".into()))
}

/// Shared frame codec. Called after sending exactly one request. The same
/// transport is read and written, so no second connection is ACKed by mistake.
/// Length is rejected *before* allocating any response bytes.
pub fn read_reply_and_ack<S: Read + Write>(stream: &mut S) -> Result<Vec<u8>> {
    let mut header = [0u8; 4];
    stream.read_exact(&mut header).map_err(read_failure)?;
    let size = i32::from_le_bytes(header);
    if size <= 0 || size as usize > MAX_REPLY_BYTES {
        return Err(failure("invalid_frame", "Invalid daemon response length."));
    }
    let mut payload = vec![0u8; size as usize];
    stream.read_exact(&mut payload).map_err(read_failure)?;
    stream.write_all(&[1]).map_err(|e| failure("ack_failed", e))?;
    Ok(payload)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::cell::RefCell;
    use std::collections::VecDeque;
    use std::io;

    struct Duplex { input: io::Cursor<Vec<u8>>, output: Vec<u8> }
    impl Read for Duplex { fn read(&mut self, dest: &mut [u8]) -> io::Result<usize> { self.input.read(dest) } }
    impl Write for Duplex {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> { self.output.extend_from_slice(buf); Ok(buf.len()) }
        fn flush(&mut self) -> io::Result<()> { Ok(()) }
    }
    fn duplex(length: i32, body: &[u8]) -> Duplex {
        let mut bytes = length.to_le_bytes().to_vec(); bytes.extend(body);
        Duplex { input: io::Cursor::new(bytes), output: vec![] }
    }

    #[test]
    fn bound_is_checked_before_allocation_and_no_ack_is_sent() {
        for n in [0, -1, MAX_REPLY_BYTES as i32 + 1] {
            let mut stream = duplex(n, &[]);
            assert_eq!(read_reply_and_ack(&mut stream).unwrap_err().code, "invalid_frame");
            assert!(stream.output.is_empty());
        }
    }
    #[test]
    fn truncation_is_not_acknowledged_and_exact_payload_is() {
        let mut short = duplex(4, b"ab");
        assert_eq!(read_reply_and_ack(&mut short).unwrap_err().code, "truncated_frame");
        assert!(short.output.is_empty());
        let mut full = duplex(3, b"abc");
        assert_eq!(read_reply_and_ack(&mut full).unwrap(), b"abc");
        assert_eq!(full.output, [1]);
    }

    struct Scripted { steps: RefCell<VecDeque<(Vec<u8>, Option<Vec<u8>>)>> }
    impl Scripted {
        fn new(steps: Vec<(Vec<u8>, Value)>) -> Self {
            Self { steps: RefCell::new(steps.into_iter().map(|(req, val)|
                (req, Some(serde_json::to_vec(&val).unwrap()))).collect()) }
        }
    }
    impl Transport for Scripted {
        fn exchange(&self, request: &[u8]) -> Result<Option<Vec<u8>>> {
            let (expected, reply) = self.steps.borrow_mut().pop_front().unwrap();
            assert_eq!(request, expected);
            Ok(reply)
        }
    }
    fn hello(caps: Vec<&str>, wire: u8, schema: u64) -> Value {
        json!({"ok":true,"payload":{"wireVersion":wire,"reportSchemaVersion":schema,"capabilities":caps},"error":null})
    }
    fn reply(payload: Value) -> Value { json!({"ok":true,"payload":payload,"error":null}) }
    fn status(revision: u64) -> Value {
        json!({"root":"C:\\work","epoch":"epoch-1","revision":revision,"committedRevision":9,
            "reportSchemaVersion":2,"state":"ready"})
    }
    #[test]
    fn version_and_capabilities_are_checked_before_operation() {
        let bad = DaemonClient::new(Scripted::new(vec![(vec![HELLO], hello(vec!["status"], 99, 2))]), "C:\\work".into());
        assert_eq!(bad.command(Command::Recheck).unwrap_err().code, "unsupported_wire_version");
        let missing = DaemonClient::new(Scripted::new(vec![(vec![HELLO], hello(vec!["status"], 1, 2))]), "C:\\work".into());
        assert_eq!(missing.command(Command::Recheck).unwrap_err().code, "unsupported_feature");
        let old = DaemonClient::new(Scripted::new(vec![(vec![HELLO],
            json!({"ok":false,"payload":null,"error":"Unknown daemon command."}))]), "C:\\work".into());
        assert_eq!(old.command(Command::Status).unwrap_err().code, "upgrade_required");
    }
    #[test]
    fn incompatible_report_schema_and_duplicate_capabilities_fail_closed() {
        let schema = DaemonClient::new(Scripted::new(vec![
            (vec![HELLO], hello(vec!["status"], 1, 3))]), "C:\\work".into());
        assert_eq!(schema.command(Command::Status).unwrap_err().code, "unsupported_report_schema");
        let duplicate = DaemonClient::new(Scripted::new(vec![
            (vec![HELLO], hello(vec!["status", "status"], 1, 2))]), "C:\\work".into());
        assert_eq!(duplicate.command(Command::Status).unwrap_err().code, "invalid_handshake");
    }
    #[test]
    fn status_and_recheck_use_the_published_three_byte_request() {
        let steps = vec![
            (vec![HELLO], hello(vec!["status","recheck"], 1, 2)),
            (vec![VERSIONED_REQUEST, 1, 1], reply(status(11))),
            (vec![HELLO], hello(vec!["status","recheck"], 1, 2)),
            (vec![VERSIONED_REQUEST, 1, 3], reply(json!({"revision":12}))),
        ];
        let client = DaemonClient::new(Scripted::new(steps), "C:\\work".into());
        assert!(client.command(Command::Status).unwrap().ok);
        assert_eq!(client.command(Command::Recheck).unwrap().payload.unwrap()["revision"], 12);
    }
    #[test]
    fn a_stale_report_is_discarded_after_status_changes() {
        let report = json!({"schemaVersion":2,"producer":"SlopHawk","root":"C:\\work",
            "epoch":"epoch-1","revision":11,"committedRevision":9});
        let mut steps = Vec::new();
        for (op, payload) in [(1, status(11)), (2, report), (1, status(12))] {
            steps.push((vec![HELLO], hello(vec!["status","results"], 1, 2)));
            steps.push((vec![VERSIONED_REQUEST, 1, op], reply(payload)));
        }
        let client = DaemonClient::new(Scripted::new(steps), "C:\\work".into());
        assert_eq!(client.command(Command::Results).unwrap_err().code, "stale_revision");
    }
    #[test]
    fn report_is_returned_when_a_fence_is_stable() {
        let report = json!({"schemaVersion":2,"producer":"SlopHawk","root":"C:\\work",
            "epoch":"epoch-1","revision":11,"committedRevision":9,"partial":true});
        let mut steps = Vec::new();
        for (op, payload) in [(1, status(11)), (2, report.clone()), (1, status(11))] {
            steps.push((vec![HELLO], hello(vec!["status","results"], 1, 2)));
            steps.push((vec![VERSIONED_REQUEST, 1, op], reply(payload)));
        }
        let client = DaemonClient::new(Scripted::new(steps), "C:\\work".into());
        assert!(client.command(Command::Results).unwrap().ok);
    }
}
