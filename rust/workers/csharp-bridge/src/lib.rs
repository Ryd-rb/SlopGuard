//! An opt-in, one-shot client for the separate C# migration-job/1 worker.
//!
//! The caller must start an already-authorized, contained child. This crate
//! neither starts tools nor confers Evaluate, Build, Restore, Network, Test or
//! CompilerExtensions authority, and does not publish scheduler receipts.
//! Hold staged facts until: terminal identity matches, the stream ends, peer
//! exit/drain is confirmed, and the authority/fence is revalidated by the owner.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::fmt;
use std::io::{self, Read, Write};
use std::process::Child;

pub const SCHEMA: &str = "slophawk.migration-job/1";
pub const MAX_REQUEST: usize = 256 * 1024;
pub const MAX_FRAME: usize = 2 * 1024 * 1024;
pub const MAX_JOB_BYTES: usize = 8 * 1024 * 1024;
pub const MAX_FACTS: usize = 128;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Identity {
    pub job_id: String,
    pub root: String,
    pub root_identity: String,
    pub context: String,
    pub epoch: String,
    pub committed_revision: i64,
    pub fence_sequence: i64,
    pub input_sha256: String,
    pub tool_sha256: String,
    pub policy_sha256: String,
    pub grant_revision: String,
    pub budget_reference: String,
    pub attempt_id: i64,
    pub attempt_number: i32,
    pub executable_sha256: Option<String>,
    pub runner_sha256: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Operation {
    Evaluate,
    Build,
    Test,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct RequestFrame<'a> {
    schema: &'static str,
    kind: &'static str,
    operation: Operation,
    identity: &'a Identity,
    payload: &'a Value,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct FactFrame {
    schema: String,
    kind: String,
    operation: Operation,
    identity: Identity,
    payload: Fact,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct TerminalFrame {
    schema: String,
    kind: String,
    operation: Operation,
    identity: Identity,
    status: String,
    reason: String,
    omissions: Vec<String>,
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Fact {
    pub kind: String,
    pub data: Value,
}

#[derive(Debug, Clone)]
pub struct Accepted {
    pub identity: Identity,
    pub operation: Operation,
    pub facts: Vec<Fact>,
}

#[derive(Debug)]
pub enum Error {
    Io(io::Error),
    Malformed(&'static str),
    Limit(&'static str),
    IdentityChanged,
    MissingTerminal,
    PeerNotDrained,
    AuthorityChanged,
    Refused {
        status: String,
        reason: String,
        omissions: Vec<String>,
    },
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(e) => write!(f, "worker IO: {e}"),
            Self::Malformed(e) => write!(f, "malformed worker frame: {e}"),
            Self::Limit(e) => write!(f, "worker budget exceeded: {e}"),
            Self::IdentityChanged => write!(f, "worker identity mismatch"),
            Self::MissingTerminal => write!(f, "missing worker terminal"),
            Self::PeerNotDrained => write!(f, "peer exit or containment drainage unconfirmed"),
            Self::AuthorityChanged => write!(f, "publication authority or fence changed"),
            Self::Refused { status, reason, .. } => write!(f, "worker {status}: {reason}"),
        }
    }
}

impl std::error::Error for Error {}

impl From<io::Error> for Error {
    fn from(value: io::Error) -> Self {
        Self::Io(value)
    }
}

fn sha256(value: &[u8]) -> String {
    let output = Sha256::digest(value);
    let mut result = String::with_capacity(64);
    for byte in output {
        use std::fmt::Write as _;
        let _ = write!(result, "{byte:02X}");
    }
    result
}

fn bounded(value: &str, limit: usize) -> bool {
    !value.is_empty() && value.len() <= limit && !value.chars().any(char::is_control)
}

fn digest(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|b| b.is_ascii_hexdigit())
}

/// The host supplies identity fields from its committed revision, active epoch,
/// evaluated context, trusted tool inputs and current policy. This does not mint
/// an execution grant. SHA-256 covers the *serialized payload bytes* as sent.
pub fn encode_request(
    operation: Operation,
    identity: &Identity,
    payload: &Value,
) -> Result<Vec<u8>, Error> {
    if !bounded(&identity.job_id, 128)
        || !bounded(&identity.root, 4096)
        || !bounded(&identity.root_identity, 160)
        || !bounded(&identity.context, 128)
        || !bounded(&identity.epoch, 64)
        || identity.committed_revision < 0
        || identity.fence_sequence < 0
        || !digest(&identity.input_sha256)
        || !digest(&identity.tool_sha256)
        || !digest(&identity.policy_sha256)
        || !digest(&identity.grant_revision)
        || !bounded(&identity.budget_reference, 128)
        || identity.executable_sha256.as_deref().is_some_and(|s| !digest(s))
        || identity.runner_sha256.as_deref().is_some_and(|s| !digest(s))
        || !payload.is_object()
    {
        return Err(Error::Malformed("request identity"));
    }
    if !sha256(&serde_json::to_vec(payload).map_err(|_| Error::Malformed("payload"))?)
        .eq_ignore_ascii_case(&identity.input_sha256)
    {
        return Err(Error::IdentityChanged);
    }
    let bytes = serde_json::to_vec(&RequestFrame {
        schema: SCHEMA,
        kind: "request",
        operation,
        identity,
        payload,
    })
    .map_err(|_| Error::Malformed("request JSON"))?;
    if bytes.is_empty() || bytes.len() > MAX_REQUEST {
        return Err(Error::Limit("request"));
    }
    Ok(bytes)
}

fn write_frame<W: Write>(writer: &mut W, frame: &[u8]) -> Result<(), Error> {
    let length = u32::try_from(frame.len()).map_err(|_| Error::Limit("frame"))?;
    writer.write_all(&length.to_le_bytes())?;
    writer.write_all(frame)?;
    writer.flush()?;
    Ok(())
}

fn read_frame<R: Read>(reader: &mut R) -> Result<Option<Vec<u8>>, Error> {
    let mut header = [0u8; 4];
    let n = reader.read(&mut header[..1])?;
    if n == 0 {
        return Ok(None);
    }
    reader
        .read_exact(&mut header[1..])
        .map_err(|_| Error::Malformed("truncated header"))?;
    let length = u32::from_le_bytes(header) as usize;
    if length == 0 || length > MAX_FRAME {
        return Err(Error::Limit("frame"));
    }
    let mut bytes = vec![0u8; length];
    reader
        .read_exact(&mut bytes)
        .map_err(|_| Error::Malformed("truncated frame"))?;
    Ok(Some(bytes))
}

fn check_echo(
    schema: &str,
    kind: &str,
    operation: Operation,
    identity: &Identity,
    expected_operation: Operation,
    expected_identity: &Identity,
) -> Result<(), Error> {
    if schema != SCHEMA || operation != expected_operation || identity != expected_identity {
        return Err(Error::IdentityChanged);
    }
    if kind != "terminal" && kind != "fact" {
        return Err(Error::Malformed("unexpected kind"));
    }
    Ok(())
}

/// A one-shot exchange. `stdin` MUST be owned, not a borrowed writer: it is
/// dropped after the request so that the C# worker can reject extra input and
/// observe EOF. The child is only allowed to have been launched in a supervised
/// process scope by the native daemon. `peer_drained` must check confirmed exit
/// plus drained containment, and `fence_current` must re-read the root, epoch,
/// committed revision, input/tool/policy identities and the grant reference.
/// No fact escapes this function unless both callbacks return true.
pub fn exchange<R, W, P, A>(
    mut stdout: R,
    mut stdin: W,
    operation: Operation,
    identity: Identity,
    payload: Value,
    peer_drained: P,
    fence_current: A,
) -> Result<Accepted, Error>
where
    R: Read,
    W: Write,
    P: FnOnce() -> bool,
    A: FnOnce() -> bool,
{
    let request = encode_request(operation, &identity, &payload)?;
    write_frame(&mut stdin, &request)?;
    drop(stdin);

    let mut facts = Vec::new();
    let mut total_bytes = 0usize;
    let terminal = loop {
        let bytes = read_frame(&mut stdout)?.ok_or(Error::MissingTerminal)?;
        total_bytes = total_bytes.checked_add(bytes.len()).ok_or(Error::Limit("job"))?;
        if total_bytes > MAX_JOB_BYTES {
            return Err(Error::Limit("job"));
        }
        // Inspect kind first; deserialize the original bytes again into a strict
        // typed structure so duplicate keys and unknown top-level fields fail.
        let header: Value = serde_json::from_slice(&bytes)
            .map_err(|_| Error::Malformed("frame JSON"))?;
        match header.get("kind").and_then(Value::as_str) {
            Some("fact") => {
                let frame: FactFrame = serde_json::from_slice(&bytes)
                    .map_err(|_| Error::Malformed("fact structure"))?;
                check_echo(&frame.schema, &frame.kind, frame.operation, &frame.identity,
                    operation, &identity)?;
                if facts.len() == MAX_FACTS || !bounded(&frame.payload.kind, 128) {
                    return Err(Error::Limit("facts"));
                }
                facts.push(frame.payload);
            }
            Some("terminal") => {
                let frame: TerminalFrame = serde_json::from_slice(&bytes)
                    .map_err(|_| Error::Malformed("terminal structure"))?;
                check_echo(&frame.schema, &frame.kind, frame.operation, &frame.identity,
                    operation, &identity)?;
                if !bounded(&frame.reason, 256)
                    || frame.omissions.len() > 32
                    || frame.omissions.iter().any(|s| !bounded(s, 128))
                    || !matches!(frame.status.as_str(), "complete" | "partial" | "unsupported"
                        | "deferred" | "cancelled" | "stale" | "failed")
                {
                    return Err(Error::Malformed("terminal status, reason or omissions"));
                }
                break frame;
            }
            _ => return Err(Error::Malformed("unknown frame kind")),
        }
    };
    // In particular, an extra fact or a second terminal poisons the result.
    if read_frame(&mut stdout)?.is_some() {
        return Err(Error::Malformed("trailing frames"));
    }
    if !peer_drained() {
        return Err(Error::PeerNotDrained);
    }
    if !fence_current() {
        return Err(Error::AuthorityChanged);
    }
    if terminal.status != "complete" {
        return Err(Error::Refused {
            status: terminal.status,
            reason: terminal.reason,
            omissions: terminal.omissions,
        });
    }
    if facts.is_empty() || !terminal.omissions.is_empty() {
        return Err(Error::Malformed("complete result has no facts or has omissions"));
    }
    Ok(Accepted {
        identity,
        operation,
        facts,
    })
}

/// Convenience for an *already admitted and started* worker. The owner is still
/// responsible for supervising and draining all descendants, and for verifying
/// that the tool binary, request, profile and policy are current. A plain child
/// exit code alone does not attest descendant drainage: pass a closure that
/// checks the real process scope and current publication fence.
pub fn exchange_admitted_child<A, D>(
    child: &mut Child,
    operation: Operation,
    identity: Identity,
    payload: Value,
    containment_drained: D,
    fence_current: A,
) -> Result<Accepted, Error>
where
    D: FnOnce() -> bool,
    A: FnOnce() -> bool,
{
    let stdin = child.stdin.take().ok_or(Error::Malformed("child stdin not piped"))?;
    let stdout = child.stdout.take().ok_or(Error::Malformed("child stdout not piped"))?;
    exchange(stdout, stdin, operation, identity, payload,
        || child.wait().is_ok_and(|status| status.success()) && containment_drained(),
        fence_current)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::io::Cursor;

    fn identity(payload: &Value) -> Identity {
        Identity {
            job_id: "job-1".into(),
            root: "/checkout".into(),
            root_identity: "physical-root".into(),
            context: "workspace".into(),
            epoch: "b773f0ec-0309-459a-a9d9-78a181a2eeed".into(),
            committed_revision: 5,
            fence_sequence: 6,
            input_sha256: sha256(&serde_json::to_vec(payload).unwrap()),
            tool_sha256: "A".repeat(64),
            policy_sha256: "B".repeat(64),
            grant_revision: "B".repeat(64),
            budget_reference: "bounded-test".into(),
            attempt_id: 0,
            attempt_number: 0,
            executable_sha256: None,
            runner_sha256: None,
        }
    }

    fn framed(value: &Value) -> Vec<u8> {
        let bytes = serde_json::to_vec(value).unwrap();
        [u32::try_from(bytes.len()).unwrap().to_le_bytes().as_slice(), bytes.as_slice()].concat()
    }

    fn frames(identity: &Identity) -> Vec<u8> {
        [framed(&json!({"schema": SCHEMA, "kind": "fact", "operation": "evaluate",
            "identity": identity, "payload": {"kind":"evaluated_graph", "data":{"targets":[]}}})),
         framed(&json!({"schema": SCHEMA, "kind": "terminal", "operation": "evaluate",
            "identity": identity, "status":"complete", "reason":"ok", "omissions": []}))].concat()
    }

    #[test]
    fn accepts_only_drained_current_matching_terminal() {
        let payload = json!({"root":"/checkout","dotnetPath":"/usr/bin/dotnet"});
        let id = identity(&payload);
        let accepted = exchange(Cursor::new(frames(&id)), Vec::<u8>::new(),
            Operation::Evaluate, id, payload, || true, || true).unwrap();
        assert_eq!(accepted.facts.len(), 1);
    }

    #[test]
    fn malformed_missing_terminal_never_releases_staged_facts() {
        let payload = json!({"root":"/checkout"});
        let id = identity(&payload);
        let mut response = frames(&id);
        let terminal = framed(&json!({"schema": SCHEMA,"kind":"terminal",
            "operation":"evaluate","identity":id,"status":"complete","reason":"ok","omissions":[]}));
        response.truncate(response.len() - terminal.len());
        assert!(matches!(exchange(Cursor::new(response), Vec::<u8>::new(),
            Operation::Evaluate, id, payload, || true, || true), Err(Error::MissingTerminal)));
    }

    #[test]
    fn revocation_peer_failure_and_identity_change_discard_everything() {
        let payload = json!({"root":"/checkout"});
        let id = identity(&payload);
        assert!(matches!(exchange(Cursor::new(frames(&id)), Vec::<u8>::new(),
            Operation::Evaluate, id.clone(), payload.clone(), || false, || true),
            Err(Error::PeerNotDrained)));
        assert!(matches!(exchange(Cursor::new(frames(&id)), Vec::<u8>::new(),
            Operation::Evaluate, id.clone(), payload.clone(), || true, || false),
            Err(Error::AuthorityChanged)));
        let mut changed = id.clone();
        changed.committed_revision += 1;
        assert!(matches!(exchange(Cursor::new(frames(&id)), Vec::<u8>::new(),
            Operation::Evaluate, changed, payload, || true, || true),
            Err(Error::IdentityChanged)));
    }

    #[test]
    fn oversized_frame_and_failed_terminal_never_publish() {
        let payload = json!({"root":"/checkout"});
        let id = identity(&payload);
        let oversize = u32::try_from(MAX_FRAME + 1).unwrap().to_le_bytes().to_vec();
        assert!(matches!(exchange(Cursor::new(oversize), Vec::<u8>::new(),
            Operation::Evaluate, id.clone(), payload.clone(), || true, || true),
            Err(Error::Limit("frame"))));
        let refusal = framed(&json!({"schema":SCHEMA,"kind":"terminal",
            "operation":"evaluate","identity":id,"status":"partial",
            "reason":"evaluation_incomplete","omissions":["unknown_target"]}));
        assert!(matches!(exchange(Cursor::new(refusal), Vec::<u8>::new(),
            Operation::Evaluate, id, payload, || true, || true), Err(Error::Refused { .. })));
    }
}
