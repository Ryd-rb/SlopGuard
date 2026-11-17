//! Internal `slophawk.migration-job/1` transport. Not Roslyn frame-v1,
//! MCP stdio, or public CLI argv. Only an explicitly configured executable
//! peer can consume it; repository content never supplies a grant.
use crate::store::invalid;
use serde::{Deserialize, Serialize};
use std::io::{self, ErrorKind, Read, Write};

pub const JOB_PROTOCOL: &str = "slophawk.migration-job/1";
pub const MAX_FRAME: usize = 1 << 20; // 1 MiB per frame, checked before allocation
pub const MAX_OUTPUT: usize = 8 << 20;
pub const MAX_RECORDS: usize = 2_000;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct JobIdentity {
    pub protocol: String,
    pub operation: String,
    pub job_id: String,
    pub root_identity: String,
    pub context: String,
    pub epoch: String,
    pub committed_revision: u64,
    pub input_digest: String,
    pub tool_digest: String,
    pub policy_digest: String,
    pub budget_ref: String,
    pub grant_ref: String,
}
impl JobIdentity {
    pub fn validate(&self) -> io::Result<()> {
        fn bounded(s: &str, limit: usize) -> bool {
            !s.is_empty() && s.len() <= limit &&
            !s.chars().any(|c| c.is_control())
        }
        fn digest(s: &str) -> bool {
            s.len() == 64 && s.bytes().all(|b| b.is_ascii_hexdigit())
        }
        if self.protocol != JOB_PROTOCOL ||
           !bounded(&self.operation, 64) || !bounded(&self.job_id, 128) ||
           !bounded(&self.context, 512) || !bounded(&self.epoch, 128) ||
           !bounded(&self.budget_ref, 128) || !bounded(&self.grant_ref, 128) ||
           !digest(&self.root_identity) || !digest(&self.input_digest) ||
           !digest(&self.tool_digest) || !digest(&self.policy_digest) ||
           self.committed_revision == 0 {
            return Err(invalid("invalid internal job identity"));
        }
        Ok(())
    }
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct JobRequest {
    pub identity: JobIdentity,
    pub inputs: Vec<Input>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Input {
    pub relative_path: String,
    pub digest: String,
    pub language: String,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Outcome { Complete, Partial, Unsupported, Deferred, Cancelled, Stale, Failed }

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum WorkerFrame {
    Facts {
        identity: JobIdentity,
        provider: String,
        records: Vec<serde_json::Value>,
    },
    Terminal {
        identity: JobIdentity,
        outcome: Outcome,
        reason: Option<String>,
        omissions: Vec<String>,
    },
}
#[derive(Debug)]
pub struct CheckedTerminal {
    pub outcome: Outcome,
    pub reason: Option<String>,
    pub omissions: Vec<String>,
    /// Facts are *only* returned on a complete, identity-matched terminal.
    /// The caller must separately prove peer success and a current fence.
    pub accepted_records: Vec<serde_json::Value>,
}

pub fn write_frame<W: Write, T: Serialize>(out: &mut W, frame: &T) -> io::Result<()> {
    let payload = serde_json::to_vec(frame)
        .map_err(|_| invalid("cannot serialize internal frame"))?;
    if payload.is_empty() || payload.len() > MAX_FRAME {
        return Err(invalid("outbound frame exceeds budget"));
    }
    out.write_all(&(payload.len() as u32).to_le_bytes())?;
    out.write_all(&payload)
}
pub fn read_frame<R: Read>(input: &mut R) -> io::Result<Vec<u8>> {
    let mut length = [0u8; 4];
    input.read_exact(&mut length)?;
    let length = u32::from_le_bytes(length) as usize;
    if length == 0 || length > MAX_FRAME { return Err(invalid("internal frame exceeds budget")); }
    let mut bytes = vec![0u8; length];
    input.read_exact(&mut bytes)?;
    if std::str::from_utf8(&bytes).is_err() { return Err(invalid("invalid internal UTF-8")); }
    Ok(bytes)
}
pub fn checked_request<R: Read>(input: &mut R) -> io::Result<JobRequest> {
    let frame = read_frame(input)?;
    let request: JobRequest = serde_json::from_slice(&frame)
        .map_err(|_| invalid("malformed internal request"))?;
    request.identity.validate()?;
    if request.inputs.len() > MAX_RECORDS ||
       request.inputs.iter().any(|i| {
           !crate::store::valid_relative(&i.relative_path) ||
           i.digest.len() != 64 || !i.digest.bytes().all(|b| b.is_ascii_hexdigit()) ||
           crate::scan::language(&i.relative_path) != Some(i.language.as_str())
       }) {
        return Err(invalid("unbounded or mismatched source inputs"));
    }
    Ok(request)
}

/// Collect until exactly one terminal and EOF. No prefix of a failed or
/// incomplete stream is publishable; never return staged facts on errors.
pub fn checked_output<R: Read>(input: &mut R, expected: &JobIdentity) -> io::Result<CheckedTerminal> {
    expected.validate()?;
    let mut budget = MAX_OUTPUT;
    let mut records = Vec::new();
    loop {
        let bytes = read_frame(input)?;
        budget = budget.checked_sub(bytes.len() + 4)
            .ok_or_else(|| invalid("internal total output budget exceeded"))?;
        let frame: WorkerFrame = serde_json::from_slice(&bytes)
            .map_err(|_| invalid("malformed worker frame"))?;
        match frame {
            WorkerFrame::Facts { identity, provider, records: batch } => {
                if identity != *expected || provider.is_empty() || provider.len() > 128 ||
                   records.len() > MAX_RECORDS.saturating_sub(records.len()) {
                    return Err(invalid("untrusted worker facts identity or count"));
                }
                records.extend(batch);
            }
            WorkerFrame::Terminal { identity, outcome, reason, omissions } => {
                if identity != *expected || reason.as_ref().is_some_and(|s| s.len() > 512) ||
                   omissions.len() > 64 || omissions.iter().any(|s| s.is_empty() || s.len() > 128) ||
                   (outcome == Outcome::Complete && (!omissions.is_empty() || reason.is_some())) {
                    return Err(invalid("bad terminal identity or coverage"));
                }
                // The peer must close stdout; a second terminal, trailing
                // garbage or an absent EOF fails rather than blessing a prefix.
                let mut trailing = [0u8; 1];
                if input.read(&mut trailing)? != 0 { return Err(invalid("extra worker output")); }
                if outcome != Outcome::Complete { records.clear(); }
                return Ok(CheckedTerminal {
                    outcome, reason, omissions, accepted_records: records,
                });
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn identity() -> JobIdentity {
        JobIdentity {
            protocol: JOB_PROTOCOL.into(), operation: "facts".into(),
            job_id: "j-7".into(), root_identity: "a".repeat(64),
            context: "rust:unresolved:src".into(), epoch: "e-1".into(),
            committed_revision: 7, input_digest: "b".repeat(64),
            tool_digest: "c".repeat(64), policy_digest: "d".repeat(64),
            budget_ref: "budget-1".into(), grant_ref: "granted-1".into(),
        }
    }
    #[test]
    fn strict_framing_and_identity() {
        let id = identity();
        let mut stream = Vec::new();
        write_frame(&mut stream, &WorkerFrame::Facts {
            identity: id.clone(), provider: "rust".into(),
            records: vec![serde_json::json!({"span": [0,2]})],
        }).unwrap();
        write_frame(&mut stream, &WorkerFrame::Terminal {
            identity: id.clone(), outcome: Outcome::Complete, reason: None, omissions: vec![],
        }).unwrap();
        let checked = checked_output(&mut stream.as_slice(), &id).unwrap();
        assert_eq!(checked.accepted_records.len(), 1);
        assert_eq!(checked.outcome, Outcome::Complete);
        let mut stale = id.clone(); stale.epoch = "e-2".into();
        assert!(checked_output(&mut stream.as_slice(), &stale).is_err());
    }
    #[test]
    fn no_terminal_bad_terminal_and_partial_discard() {
        let id = identity();
        let mut stream = Vec::new();
        write_frame(&mut stream, &WorkerFrame::Facts {
            identity: id.clone(), provider: "rust".into(), records: vec![serde_json::json!({})],
        }).unwrap();
        assert_eq!(checked_output(&mut stream.as_slice(), &id)
            .err().unwrap().kind(), ErrorKind::UnexpectedEof);
        write_frame(&mut stream, &WorkerFrame::Terminal {
            identity: id.clone(), outcome: Outcome::Partial,
            reason: Some("unknown macro".into()), omissions: vec!["cfg".into()],
        }).unwrap();
        assert!(checked_output(&mut stream.as_slice(), &id).unwrap().accepted_records.is_empty());
        stream.extend_from_slice(b"extra");
        assert!(checked_output(&mut stream.as_slice(), &id).is_err());
        let mut oversized = (&((MAX_FRAME as u32) + 1).to_le_bytes()[..]).to_vec();
        assert!(read_frame(&mut oversized.as_slice()).is_err());
    }
}
