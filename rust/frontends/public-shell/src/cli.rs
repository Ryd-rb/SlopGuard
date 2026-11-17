//! Pure CLI projection, independently testable without starting a daemon.
use crate::protocol::DaemonReply;
use crate::{Result, failure};

#[derive(Debug, Eq, PartialEq)]
pub struct Projection {
    pub exit_code: i32,
    pub stdout: Option<String>,
    pub stderr: Option<String>,
}

pub fn project(reply: &DaemonReply) -> Result<Projection> {
    if !reply.ok {
        let mut message = reply.error.as_deref().unwrap_or("No workspace daemon is listening.").to_owned();
        if let Some(code) = &reply.code { message.push_str(&format!("\ncode: {code}")); }
        return Ok(Projection { exit_code: 1, stdout: None, stderr: Some(message) });
    }
    // Keep schema-v2 payload unchanged. Do not project roadmap-only marks.
    let output = serde_json::to_string_pretty(&reply.payload)
        .map_err(|e| failure("invalid_reply", e))?;
    Ok(Projection { exit_code: 0, stdout: Some(output), stderr: None })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn status_preserves_the_report_schema_two_projection() {
        let reply = DaemonReply { ok: true, payload: Some(json!({
            "root":"C:\\source", "reportSchemaVersion":2, "epoch":"epoch",
            "revision":4, "committedRevision":3, "state":"pending",
            "testScheduler":{"state":"awaiting_adapter"}
        })), error: None, code: None };
        let projected = project(&reply).unwrap();
        assert_eq!(projected.exit_code, 0);
        assert_eq!(projected.stderr, None);
        let output = projected.stdout.unwrap();
        let payload: serde_json::Value = serde_json::from_str(&output).unwrap();
        assert_eq!(payload["reportSchemaVersion"], 2);
        assert_eq!(payload["testScheduler"]["state"], "awaiting_adapter");
        assert!(payload.get("marks").is_none());
    }
    #[test]
    fn current_report_failure_keeps_the_daemon_code_and_nonzero_exit() {
        let reply = DaemonReply { ok: false, payload: None,
            error: Some("No current inventory report.".into()),
            code: Some("no_current_report".into()) };
        let projected = project(&reply).unwrap();
        assert_eq!(projected.exit_code, 1);
        assert!(projected.stdout.is_none());
        assert!(projected.stderr.unwrap().contains("no_current_report"));
    }
}
