//! MCP 2025-11-25 stdio JSON-RPC adapter. Public messages are newline-separated
//! UTF-8 JSON, NEVER private daemon frames. All stdout writes happen here.
//! The published C# SDK bridge remains the verified fallback on Unix.
use crate::protocol::{Command, DaemonClient, DaemonReply, Handshake, Transport};
use crate::{Result, failure};
use serde_json::{Value, json};
use std::io::{BufRead, Write};

const VERSION: &str = "2025-11-25";
const MAX_MCP_LINE: usize = 1024 * 1024;
const SERVER_NAME: &str = "SlopHawk";
const SERVER_VERSION: &str = "0.2.0-preview.1";

struct Session {
    initialized: bool,
    ready: bool,
    tools: Vec<&'static str>,
}

pub fn serve<T: Transport, R: BufRead, W: Write>(
    daemon: &DaemonClient<T>, mut input: R, mut output: W,
) -> Result<()> {
    let mut session = Session { initialized: false, ready: false, tools: Vec::new() };
    loop {
        let Some(line) = read_line(&mut input)? else { return Ok(()); };
        let message: Value = match serde_json::from_slice(&line) {
            Ok(value) => value,
            Err(_) => {
                write_message(&mut output, &rpc_error(Value::Null, -32700, "Parse error", None))?;
                continue;
            }
        };
        let id = message.get("id").cloned();
        let Some(object) = message.as_object() else {
            write_message(&mut output, &rpc_error(Value::Null, -32600, "Invalid Request", None))?;
            continue;
        };
        let valid_id = id.as_ref().is_some_and(|id| id.is_string() || id.is_i64() || id.is_u64());
        if object.get("jsonrpc").and_then(Value::as_str) != Some("2.0") ||
            object.get("method").and_then(Value::as_str).is_none() ||
            (id.is_some() && !valid_id) {
            write_message(&mut output, &rpc_error(id.unwrap_or(Value::Null), -32600,
                "Invalid Request", None))?;
            continue;
        }
        let method = object["method"].as_str().unwrap();
        if !valid_id {
            match method {
                "notifications/initialized" if session.initialized => { session.ready = true; }
                "notifications/cancelled" | "notifications/initialized" => (),
                _ => (), // Other notifications receive no JSON-RPC response.
            }
            continue;
        }
        let id = id.unwrap();
        let params = object.get("params").unwrap_or(&Value::Null);
        let answer = match method {
            "initialize" => initialize(&mut session, daemon, params, &id),
            "ping" => Ok(json!({"jsonrpc":"2.0","id":id,"result":{}})),
            "tools/list" | "tools/call" if !session.ready =>
                Ok(rpc_error(id.clone(), -32002, "Server not initialized", None)),
            "tools/list" => {
                match refresh(&mut session, daemon, &mut output) {
                    Ok(()) => Ok(json!({"jsonrpc":"2.0","id":id,"result":{
                        "tools":session.tools.iter().map(|name| tool_spec(name)).collect::<Vec<_>>()}})),
                    Err(e) => Err(e),
                }
            }
            "tools/call" => call(&mut session, daemon, &mut output, params, &id),
            _ => Ok(rpc_error(id.clone(), -32601, "Method not found", None)),
        };
        match answer {
            Ok(response) => write_message(&mut output, &response)?,
            Err(error) => write_message(&mut output, &rpc_error(id, -32000,
                &error.message, Some(&error.code)))?,
        }
    }
}

fn initialize<T: Transport>(session: &mut Session, daemon: &DaemonClient<T>,
    params: &Value, id: &Value) -> Result<Value> {
    if session.initialized {
        return Ok(rpc_error(id.clone(), -32600, "Already initialized", None));
    }
    if params.get("protocolVersion").and_then(Value::as_str) != Some(VERSION) {
        return Ok(json!({"jsonrpc":"2.0","id":id,"error":{"code":-32602,
            "message":"Unsupported MCP protocol version", "data":{"supported":[VERSION]}}}));
    }
    let handshake = daemon.negotiate()?;
    session.tools = projected_tools(&handshake);
    session.initialized = true;
    Ok(json!({"jsonrpc":"2.0","id":id,"result":{
        "protocolVersion":VERSION,
        "capabilities":{"tools":{"listChanged":true}},
        "serverInfo":{"name":SERVER_NAME,"version":SERVER_VERSION}
    }}))
}

fn refresh<T: Transport, W: Write>(session: &mut Session, daemon: &DaemonClient<T>,
    output: &mut W) -> Result<()> {
    let handshake = daemon.negotiate()?;
    let available = projected_tools(&handshake);
    if session.tools != available {
        session.tools = available;
        write_message(output, &json!({"jsonrpc":"2.0","method":"notifications/tools/list_changed"}))?;
    }
    Ok(())
}

fn projected_tools(handshake: &Handshake) -> Vec<&'static str> {
    let mut result = Vec::new();
    for (name, feature) in [
        ("workspace_status", "status"),
        ("get_findings", "results"),
        ("run_checks", "recheck"),
        ("list_checkers", "list_checkers"),
    ] {
        if handshake.capabilities.iter().any(|c| c == feature) { result.push(name); }
    }
    result
}

fn tool_spec(name: &str) -> Value {
    let (description, read_only) = match name {
        "workspace_status" => ("Read source reconciliation and test scheduling status.", true),
        "get_findings" => ("Read the current validated schema-v2 source inventory.", true),
        "run_checks" => ("Queue a full source inventory reconciliation.", false),
        "list_checkers" => ("List registered checkers (empty in the published preview).", true),
        _ => unreachable!(),
    };
    json!({"name":name,"description":description,
        "inputSchema":{"type":"object","properties":{},"additionalProperties":false},
        "annotations":{"readOnlyHint":read_only}})
}

fn call<T: Transport, W: Write>(session: &mut Session, daemon: &DaemonClient<T>,
    output: &mut W, params: &Value, id: &Value) -> Result<Value> {
    let Some(name) = params.get("name").and_then(Value::as_str) else {
        return Ok(rpc_error(id.clone(), -32602, "Missing tool name", None));
    };
    if !params.get("arguments").is_none_or(|v| v.as_object().is_some_and(|m| m.is_empty())) {
        return Ok(rpc_error(id.clone(), -32602, "Tool arguments must be an empty object", None));
    }
    refresh(session, daemon, output)?;
    if !session.tools.iter().any(|tool| *tool == name) {
        return Ok(rpc_error(id.clone(), -32602, "Tool not advertised by the connected daemon", Some("unsupported_feature")));
    }
    let payload = match name {
        "workspace_status" => from_daemon(daemon.command(Command::Status)),
        "get_findings" => from_daemon(daemon.command(Command::Results)),
        "run_checks" => from_daemon(daemon.command(Command::Recheck)),
        "list_checkers" => (json!([]), false), // Published wire 1 has no checker listing command.
        _ => unreachable!(),
    };
    let text = serde_json::to_string(&payload.0).map_err(|e| failure("serialization", e))?;
    Ok(json!({"jsonrpc":"2.0","id":id,"result":{
        "content":[{"type":"text","text":text}],
        "structuredContent":{"result":payload.0},
        "isError":payload.1
    }}))
}

fn from_daemon(reply: Result<DaemonReply>) -> (Value, bool) {
    match reply {
        Ok(reply) => {
            let failed = !reply.ok;
            (serde_json::to_value(reply).expect("DaemonReply is always JSON serializable"), failed)
        }
        Err(error) => (json!({"ok":false,"payload":null,
            "error":error.message,"code":error.code}), true),
    }
}

fn rpc_error(id: Value, code: i64, message: &str, tag: Option<&str>) -> Value {
    let mut error = json!({"code":code,"message":message});
    if let Some(tag) = tag { error["data"] = json!({"code":tag}); }
    json!({"jsonrpc":"2.0","id":id,"error":error})
}

fn write_message<W: Write>(output: &mut W, value: &Value) -> Result<()> {
    serde_json::to_writer(&mut *output, value).map_err(|e| failure("mcp_output", e))?;
    output.write_all(b"\n").map_err(|e| failure("mcp_output", e))?;
    output.flush().map_err(|e| failure("mcp_output", e))
}

fn read_line<R: BufRead>(input: &mut R) -> Result<Option<Vec<u8>>> {
    let mut bytes = Vec::new();
    loop {
        let available = input.fill_buf().map_err(|e| failure("mcp_input", e))?;
        if available.is_empty() {
            if bytes.is_empty() { return Ok(None); }
            return Err(failure("mcp_truncated", "MCP stdio input ended without a JSON line delimiter."));
        }
        let found = available.iter().position(|&byte| byte == b'\n');
        let count = found.map_or(available.len(), |i| i + 1);
        if count > MAX_MCP_LINE.saturating_sub(bytes.len()) {
            return Err(failure("mcp_oversize", "MCP JSON line exceeds the one-MiB byte budget."));
        }
        bytes.extend_from_slice(&available[..count]);
        input.consume(count);
        if found.is_some() {
            // A line may have CRLF framing but cannot contain embedded newline.
            if bytes.ends_with(b"\r\n") { bytes.pop(); bytes.pop(); }
            else { bytes.pop(); }
            return Ok(Some(bytes));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::{HELLO, VERSIONED_REQUEST};
    use std::cell::RefCell;
    use std::collections::VecDeque;
    use std::io::{BufReader, Cursor};
    struct Wire(RefCell<VecDeque<(Vec<u8>, Value)>>);
    impl Transport for Wire {
        fn exchange(&self, bytes: &[u8]) -> Result<Option<Vec<u8>>> {
            let (expected, reply) = self.0.borrow_mut().pop_front().expect("scripted frame");
            assert_eq!(expected, bytes);
            Ok(Some(serde_json::to_vec(&reply).unwrap()))
        }
    }
    fn hello(features: &[&str]) -> (Vec<u8>, Value) {
        (vec![HELLO], json!({"ok":true,"payload":{
            "wireVersion":1,"reportSchemaVersion":2,"capabilities":features},"error":null}))
    }
    #[test]
    fn publishes_only_capable_tools_and_an_empty_baseline_checker_list() {
        let wire = Wire(RefCell::new(VecDeque::from(vec![
            hello(&["status","list_checkers"]), hello(&["status","list_checkers"]),
            hello(&["status","list_checkers"]),
        ])));
        let client = DaemonClient::new(wire, "C:\\work".into());
        let input = concat!(
            "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"initialize\",\"params\":{\"protocolVersion\":\"2025-11-25\"}}\n",
            "{\"jsonrpc\":\"2.0\",\"method\":\"notifications/initialized\"}\n",
            "{\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"tools/list\"}\n",
            "{\"jsonrpc\":\"2.0\",\"id\":3,\"method\":\"tools/call\",\"params\":{\"name\":\"list_checkers\",\"arguments\":{}}}\n",
        );
        let mut output = Vec::new();
        serve(&client, BufReader::new(Cursor::new(input)), &mut output).unwrap();
        let lines: Vec<Value> = output.split(|b| *b == b'\n').filter(|x| !x.is_empty())
            .map(|x| serde_json::from_slice(x).unwrap()).collect();
        assert_eq!(lines.len(), 3);
        assert_eq!(lines[1]["result"]["tools"].as_array().unwrap().len(), 2);
        assert_eq!(lines[2]["result"]["structuredContent"]["result"], json!([]));
        assert_eq!(lines[2]["result"]["isError"], false);
    }
    #[test]
    fn replaced_daemon_renegotiates_and_notifies_list_changes() {
        let wire = Wire(RefCell::new(VecDeque::from(vec![
            hello(&["status","list_checkers"]), hello(&["status"]),
        ])));
        let client = DaemonClient::new(wire, "C:\\work".into());
        let input = concat!(
            "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"initialize\",\"params\":{\"protocolVersion\":\"2025-11-25\"}}\n",
            "{\"jsonrpc\":\"2.0\",\"method\":\"notifications/initialized\"}\n",
            "{\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"tools/list\"}\n",
        );
        let mut output = Vec::new();
        serve(&client, Cursor::new(input), &mut output).unwrap();
        let lines: Vec<Value> = output.split(|b| *b == b'\n').filter(|x| !x.is_empty())
            .map(|x| serde_json::from_slice(x).unwrap()).collect();
        assert_eq!(lines.len(), 3);
        assert_eq!(lines[1]["method"], "notifications/tools/list_changed");
        assert_eq!(lines[2]["result"]["tools"].as_array().unwrap().len(), 1);
    }
    #[test]
    fn unsupported_protocol_and_tool_call_before_initialize_are_errors() {
        let client = DaemonClient::new(Wire(RefCell::new(VecDeque::new())), "C:\\work".into());
        let input = concat!(
            "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"initialize\",\"params\":{\"protocolVersion\":\"1900-01-01\"}}\n",
            "{\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"tools/list\"}\n",
        );
        let mut out = Vec::new();
        serve(&client, Cursor::new(input), &mut out).unwrap();
        let lines: Vec<Value> = out.split(|b| *b == b'\n').filter(|x| !x.is_empty())
            .map(|x| serde_json::from_slice(x).unwrap()).collect();
        assert_eq!(lines[0]["error"]["code"], -32602);
        assert_eq!(lines[1]["error"]["code"], -32002);
    }
    #[test]
    fn bounded_stdio_rejects_oversized_lines_before_deserialization() {
        let mut input = Cursor::new(vec![b'a'; MAX_MCP_LINE + 5]);
        assert_eq!(read_line(&mut input).unwrap_err().code, "mcp_oversize");
    }
    #[test]
    fn daemon_reply_maps_to_structured_mcp_content_not_stdout_diagnostics() {
        let status = json!({"root":"C:\\work","epoch":"e","revision":1,
            "committedRevision":1,"reportSchemaVersion":2,"state":"ready"});
        let wire = Wire(RefCell::new(VecDeque::from(vec![
            hello(&["status"]), hello(&["status"]), hello(&["status"]),
            (vec![VERSIONED_REQUEST,1,1],json!({"ok":true,"payload":status,"error":null})),
        ])));
        let client = DaemonClient::new(wire, "C:\\work".into());
        let input = concat!(
            "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"initialize\",\"params\":{\"protocolVersion\":\"2025-11-25\"}}\n",
            "{\"jsonrpc\":\"2.0\",\"method\":\"notifications/initialized\"}\n",
            "{\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"tools/call\",\"params\":{\"name\":\"workspace_status\"}}\n",
        );
        let mut out = Vec::new();
        serve(&client, Cursor::new(input), &mut out).unwrap();
        let lines: Vec<Value> = out.split(|b| *b == b'\n').filter(|x| !x.is_empty())
            .map(|x| serde_json::from_slice(x).unwrap()).collect();
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[1]["result"]["structuredContent"]["result"]["payload"]["revision"], 1);
    }
}
