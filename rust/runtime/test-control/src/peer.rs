//! Bounded, strict one-job transport. The caller owns authorization, output
//! leases, direct-child/process-tree supervision and deadlines. An untrusted
//! reply (including an EOF, timeout or mismatched terminal) is never evidence.
//!
//! This is a *new* migration-internal protocol only. It does not replace the
//! shipped daemon private IPC, MCP, artifact format or Roslyn frame-v1.

use crate::{ControlError, Job, MAX_FRAME, Result, Terminal};
use serde::{Serialize, de::DeserializeOwned};
use std::io::{Read, Write};

fn encode<T: Serialize>(value: &T) -> Result<Vec<u8>> {
    let body = serde_json::to_vec(value)?;
    if body.is_empty() || body.len() > MAX_FRAME {
        return Err(ControlError::Invalid("job frame size"));
    }
    let mut output = Vec::with_capacity(4 + body.len());
    output.extend_from_slice(&(body.len() as u32).to_le_bytes());
    output.extend_from_slice(&body);
    Ok(output)
}

pub fn write_frame<T: Serialize>(stream: &mut impl Write, value: &T) -> Result<()> {
    let bytes = encode(value)?;
    stream.write_all(&bytes)?;
    stream.flush()?;
    Ok(())
}

/// Check the little-endian length before allocating the response buffer.
/// A truncated frame or invalid UTF-8 is rejected, never interpreted as partial.
pub fn read_frame<T: DeserializeOwned>(stream: &mut impl Read) -> Result<T> {
    let mut length = [0_u8; 4];
    stream.read_exact(&mut length)?;
    let count = u32::from_le_bytes(length) as usize;
    if count == 0 || count > MAX_FRAME {
        return Err(ControlError::Invalid("unbounded or empty peer frame"));
    }
    let mut data = vec![0; count];
    stream.read_exact(&mut data)?;
    let text = std::str::from_utf8(&data)
        .map_err(|_| ControlError::Invalid("invalid UTF-8 peer frame"))?;
    Ok(serde_json::from_str(text)?)
}

/// Write a full request and read precisely one terminal on a dedicated,
/// externally supervised stream. The supervisor must enforce a deadline and
/// total output/diagnostic quotas; `Read` and `Write` alone cannot do so.
pub fn exchange(stream: &mut (impl Read + Write), job: &Job) -> Result<Terminal> {
    if job.max_response_bytes as usize != MAX_FRAME {
        return Err(ControlError::Invalid("unexpected frame budget"));
    }
    write_frame(stream, job)?;
    let terminal: Terminal = read_frame(stream)?;
    terminal.validate(&job.identity)?;
    Ok(terminal)
}

/// Supervisor contract for a single authorized, pre-reserved worker scope.
/// The host must enroll the process in an OS process-tree containment domain
/// *before* repository code runs, enforce the operation/grant/budget, and
/// terminate/drain that same domain on revocation and restart. No fallback to
/// an unsupervised child is permitted.
pub trait ScopedHost {
    type Stream: Read + Write;
    fn reserve_scope(&mut self, fence: &crate::Fence, plan: &crate::Plan) -> Result<String>;
    fn connect(&mut self, job: &Job) -> Result<Self::Stream>;
    fn revoke_and_drain(&mut self, scope: &str) -> Result<()>;
    fn confirm_drained(&mut self, scope: &str) -> Result<()>;
}

/// Production-facing bounded frame adapter; the separately installed host
/// supplies authorization and OS-specific lifetime enforcement. A missing host
/// means no admission, never silent fallback to direct `Command::spawn`.
pub struct FramedExecutor<H> { pub host: H }
impl<H: ScopedHost> crate::Executor for FramedExecutor<H> {
    fn reserve_scope(&mut self, fence: &crate::Fence, plan: &crate::Plan) -> Result<String> {
        self.host.reserve_scope(fence, plan)
    }
    fn execute(&mut self, job: &Job) -> Result<Terminal> {
        let mut stream = self.host.connect(job)?;
        exchange(&mut stream, job)
    }
    fn revoke_and_drain(&mut self, scope: &str) -> Result<()> {
        self.host.revoke_and_drain(scope)
    }
    fn confirm_drained(&mut self, scope: &str) -> Result<()> {
        self.host.confirm_drained(scope)
    }
}
