//! Supervised adapter to the existing C# worker's *own* frame-v1 protocol.
//! No shell, no worker for non-C# sources, no facts committed before terminal validation.
use crate::contract::{ByteSpan, Language, Loaded, ScopeRequest};
use serde_json::{Value, json};
use std::sync::atomic::AtomicBool;
use crate::supervisor;

const FRAME_MAX: usize = 1024 * 1024;
const TERMINAL_MAX: usize = 64 * 1024;

// The C# worker's SourceByteMap excludes an optional UTF-8 BOM from Roslyn's
// UTF-16 coordinate space, but includes that BOM in original byte offsets.
fn original_utf16_at(bytes: &[u8], byte_position: usize) -> Option<u64> {
    let bom = if bytes.starts_with(&[0xef, 0xbb, 0xbf]) { 3 } else { 0 };
    if byte_position == 0 && bom == 3 { return Some(0); }
    if byte_position < bom { return None; }
    let prefix = std::str::from_utf8(bytes.get(bom..byte_position)?).ok()?;
    u64::try_from(prefix.encode_utf16().count()).ok()
}
fn verify_worker_utf16(frame: &Value, bytes: &[u8], mapped: &ByteSpan) -> Result<(), String> {
    let utf16 = frame.pointer("/span/utf16").ok_or("missing worker UTF-16 token span")?;
    let start = utf16.get("start").and_then(Value::as_u64).ok_or("invalid worker UTF-16 start")?;
    let length = utf16.get("length").and_then(Value::as_u64).ok_or("invalid worker UTF-16 length")?;
    let expected_start = original_utf16_at(bytes, mapped.start).ok_or("unmapped UTF-16 start")?;
    let expected_end = original_utf16_at(bytes, mapped.end).ok_or("unmapped UTF-16 end")?;
    if start != expected_start || start.checked_add(length) != Some(expected_end) {
        return Err("worker UTF-16/byte source mapping disagreed with original bytes".into());
    }
    Ok(())
}

#[derive(Debug)]
pub struct WorkerToken { pub text: String, pub category: String, pub span: Option<ByteSpan>, pub truncated: bool }

#[derive(Debug)]
pub struct WorkerFacts {
    pub tokens: Vec<WorkerToken>, pub partial: bool, pub gaps: Vec<String>,
    pub tokens_complete: bool, pub spans_complete: bool,
}

fn take_text<'a>(v: &'a Value, pointer: &str) -> Result<&'a str, String> {
    v.pointer(pointer).and_then(Value::as_str).ok_or_else(|| format!("missing worker field {pointer}"))
}
fn check_scope(frame: &Value, job: &ScopeRequest, source: &Loaded) -> Result<(), String> {
    let pairs = [
        ("/scope/workspaceEpoch", job.workspace_epoch.as_str()),
        ("/scope/committedRevision", job.committed_revision.as_str()),
        ("/scope/contextKey", job.context.key.as_str()),
        ("/scope/documentId", source.source.document_id.as_str()),
        ("/scope/sourceSha256", source.digest.as_str()),
    ];
    for (pointer, value) in pairs {
        let got = take_text(frame, pointer)?;
        if if pointer.ends_with("Sha256") { !got.eq_ignore_ascii_case(value) } else { got != value } {
            return Err(format!("stale or wrong C# worker scope: {pointer}"));
        }
    }
    Ok(())
}
fn validate_terminal(v: &Value, job: &ScopeRequest, sources: &[&Loaded], target: &Loaded, count: usize, token_count: usize) -> Result<(bool, Vec<String>, bool, bool), String> {
    if v.get("version").and_then(Value::as_u64) != Some(1) || take_text(v, "/kind")? != "result" {
        return Err("missing frame-v1 terminal result".into());
    }
    check_scope(v, job, target)?;
    if take_text(v, "/provider/id")? != Language::Csharp.provider() { return Err("wrong C# provider identity".into()); }
    if take_text(v, "/provider/parserVersion")?.is_empty() { return Err("missing worker parser version".into()); }
    if take_text(v, "/input/targetFramework")? != "net10.0" || take_text(v, "/input/languageVersion")? != "CSharp14" {
        return Err("wrong worker target or language version".into());
    }
    if take_text(v, "/input/assemblyName")? != job.context.assembly_name.as_deref().unwrap_or("") {
        return Err("wrong evaluated assembly".into());
    }
    let defines = v.pointer("/input/defines").and_then(Value::as_array).ok_or("missing worker defines")?;
    if defines.iter().filter_map(Value::as_str).collect::<Vec<_>>() != job.context.defines.iter().map(String::as_str).collect::<Vec<_>>() {
        return Err("wrong worker define set".into());
    }
    for (field, expected) in [("checked", job.context.checked), ("optimize", job.context.optimize),
        ("referenceClosureComplete", job.context.reference_closure_complete), ("sourceClosureComplete", job.context.source_closure_complete)] {
        if v.pointer(&format!("/input/{field}")).and_then(Value::as_bool) != Some(expected) { return Err(format!("worker {field} differs from evaluated context")); }
    }
    let pack = take_text(v, "/input/referencePackSha256")?;
    if pack.len() != 64 || !pack.bytes().all(|b| b.is_ascii_hexdigit()) { return Err("missing digested C# reference pack".into()); }
    let documents = v.pointer("/input/documents").and_then(Value::as_array).ok_or("missing enumerated source closure")?;
    if documents.len() != sources.len() || v.pointer("/input/sourceCount").and_then(Value::as_u64) != Some(sources.len() as u64) {
        return Err("worker enumerated source closure differs".into());
    }
    for (actual, expected) in documents.iter().zip(sources) {
        if take_text(actual, "/documentId")? != expected.source.document_id
            || !take_text(actual, "/sourceSha256")?.eq_ignore_ascii_case(&expected.digest)
            || take_text(actual, "/physicalPath")? != expected.source.physical_path
            || take_text(actual, "/logicalPath")? != expected.source.logical_path
            || take_text(actual, "/provenance")? != expected.source.provenance.as_str()
            || take_text(actual, "/byteMapping")? != expected.source.byte_mapping {
            return Err("worker source identity, provenance, mapping or hash mismatched".into());
        }
    }
    if v.get("records").and_then(Value::as_u64) != Some(count as u64)
        || v.get("tokenRecords").and_then(Value::as_u64) != Some(token_count as u64) {
        return Err("worker fact/token record totals do not match frames".into());
    }
    let status = take_text(v, "/status")?;
    if !matches!(status, "complete" | "partial") || v.get("complete").and_then(Value::as_bool) != Some(status == "complete") {
        return Err("C# worker failed, reported a bad status or inconsistent completeness".into());
    }
    let capabilities = v.pointer("/capabilities").and_then(Value::as_object).ok_or("missing worker capabilities")?;
    for key in ["declarations", "references", "calls", "tokens", "byteSpans"] {
        if !matches!(capabilities.get(key).and_then(Value::as_str), Some("complete" | "partial" | "failed")) {
            return Err(format!("invalid worker capability {key}"));
        }
    }
    let gaps = v.get("gaps").and_then(Value::as_array).ok_or("missing worker gaps")?
        .iter().map(|x| x.as_str().map(str::to_owned).ok_or_else(|| "invalid worker gap".to_owned()))
        .collect::<Result<Vec<_>, String>>()?;
    if status == "partial" && gaps.is_empty() { return Err("partial worker scope without explicit gaps".into()); }
    Ok((status == "partial", gaps,
        capabilities.get("tokens").and_then(Value::as_str) == Some("complete"),
        capabilities.get("byteSpans").and_then(Value::as_str) == Some("complete")))
}

fn parse_frames(wire: &[u8], job: &ScopeRequest, sources: &[&Loaded], target: &Loaded) -> Result<WorkerFacts, String> {
    let mut cursor = 0usize;
    let mut count = 0;
    let mut tokens = Vec::new();
    let mut result = None;
    while cursor < wire.len() {
        if wire.len() - cursor < 4 { return Err("incomplete worker frame header".into()); }
        let length = u32::from_le_bytes(wire[cursor..cursor+4].try_into().map_err(|_| "frame header")?) as usize;
        cursor += 4;
        if length == 0 || length > FRAME_MAX || wire.len()-cursor < length { return Err("invalid or truncated worker frame length".into()); }
        let frame: Value = serde_json::from_slice(&wire[cursor..cursor+length]).map_err(|e| format!("invalid worker JSON frame: {e}"))?;
        cursor += length;
        if frame.get("version").and_then(Value::as_u64) != Some(1) { return Err("worker frame version mismatch".into()); }
        match take_text(&frame, "/kind")? {
            "fact" if result.is_none() => {
                if count == job.limits.max_records { return Err("worker exceeded record budget".into()); }
                check_scope(&frame, job, target)?;
                if take_text(&frame, "/provider")? != Language::Csharp.provider() { return Err("wrong provider in fact frame".into()); }
                let cap = take_text(&frame, "/capability")?;
                if !matches!(cap, "tokens" | "declarations" | "references" | "calls") { return Err("unknown worker fact capability".into()); }
                if cap == "tokens" {
                    let span = frame.pointer("/span/byteSpan").ok_or("token missing byte span/mapping")?;
                    let mapped = if span.get("known").and_then(Value::as_bool) == Some(true) {
                        if take_text(span, "/encoding")? != "utf8" { return Err("unsupported worker byte encoding".into()); }
                        let start = span.get("start").and_then(Value::as_u64).ok_or("invalid byte start")? as usize;
                        let len = span.get("length").and_then(Value::as_u64).ok_or("invalid byte length")? as usize;
                        let end = start.checked_add(len).ok_or("worker byte span overflow")?;
                        let byte_span = ByteSpan { start, end };
                        if byte_span.start >= byte_span.end || !byte_span.verified(&target.bytes) {
                            return Err("worker token span is unmapped, empty, or not an exact UTF-8 boundary".into());
                        }
                        verify_worker_utf16(&frame, &target.bytes, &byte_span)?;
                        Some(byte_span)
                    } else if span.get("known").and_then(Value::as_bool) == Some(false) {
                        if take_text(span, "/gap")?.is_empty() { return Err("unmapped span omitted reason".into()); }
                        None
                    } else { return Err("invalid byte mapping status".into()); };
                    let declared_text = take_text(&frame, "/value/text")?;
                    let truncated = frame.pointer("/value/textTruncated").and_then(Value::as_bool).ok_or("missing truncation status")?;
                    let category = take_text(&frame, "/value/category")?;
                    if !matches!(category, "identifier" | "keyword" | "literal" | "syntax") { return Err("bad worker token category".into()); }
                    let text = if let Some(r) = &mapped {
                        let actual = std::str::from_utf8(&target.bytes[r.start..r.end]).map_err(|_| "bad token UTF-8 range")?;
                        if (!truncated && actual != declared_text) || (truncated && !actual.starts_with(declared_text)) {
                            return Err("worker token text does not match granted source bytes".into());
                        }
                        actual.to_owned()
                    } else { declared_text.to_owned() };
                    tokens.push(WorkerToken {text, category: category.to_owned(), span: mapped, truncated});
                }
                count += 1;
            }
            "result" if result.is_none() => {
                if length > TERMINAL_MAX { return Err("worker terminal exceeded 64KiB".into()); }
                result = Some(frame);
            }
            _ => return Err("worker emitted repeated terminal, unknown kind or trailing fact".into()),
        }
    }
    let terminal = result.ok_or("C# worker missing terminal result")?;
    let (partial, gaps, tokens_complete, spans_complete) = validate_terminal(&terminal, job, sources, target, count, tokens.len())?;
    if target.source.byte_mapping == "unavailable" && spans_complete { return Err("worker asserted byte span completeness for unmapped source".into()); }
    if spans_complete && tokens.iter().any(|t| t.span.is_none()) { return Err("worker announced complete mapping with unmapped tokens".into()); }
    Ok(WorkerFacts { tokens, partial, gaps, tokens_complete, spans_complete })
}

pub fn run(job: &ScopeRequest, sources: &[&Loaded], target: &Loaded, cancel: &AtomicBool) -> Result<WorkerFacts, String> {
    let worker = job.worker.as_ref().ok_or("C# worker was not explicitly granted")?;
    let request = json!({
        "version": 1, "kind": "analyze", "workspaceEpoch": job.workspace_epoch,
        "committedRevision": job.committed_revision, "workspaceRoot": job.workspace_root,
        "linkedRoots": job.linked_roots, "documentId": target.source.document_id,
        "context": {
            "key": job.context.key, "targetFramework": job.context.target_framework,
            "assemblyName": job.context.assembly_name, "languageVersion": job.context.language_version,
            "referencePack": job.context.reference_pack, "defines": job.context.defines,
            "checked": job.context.checked, "optimize": job.context.optimize,
            "referenceClosureComplete": job.context.reference_closure_complete,
            "sourceClosureComplete": job.context.source_closure_complete
        },
        "sources": sources.iter().map(|src| json!({
            "documentId": src.source.document_id, "logicalPath": src.source.logical_path,
            "physicalPath": src.source.physical_path, "provenance": src.source.provenance.as_str(),
            "bytesBase64": src.source.bytes_base64, "sha256": src.digest,
            "byteMapping": src.source.byte_mapping,
        })).collect::<Vec<_>>(),
        "limits": { "maxRecords": job.limits.max_records, "maxSyntaxNodes": job.limits.max_syntax_nodes,
            "maxFrontiers": job.limits.max_frontiers, "maxDiagnostics": job.limits.max_diagnostics }
    });
    let payload = serde_json::to_vec(&request).map_err(|e| e.to_string())?;
    if payload.is_empty() || payload.len() > FRAME_MAX { return Err("C# framed request exceeds 1MiB".into()); }
    let mut stdin_frame = Vec::with_capacity(payload.len()+4);
    stdin_frame.extend_from_slice(&(payload.len() as u32).to_le_bytes());
    stdin_frame.extend_from_slice(&payload);
    let max_stdout = job.limits.max_worker_bytes.checked_sub(stdin_frame.len()).ok_or("worker wire budget exceeded by request")?;
    if max_stdout < TERMINAL_MAX { return Err("no reserved terminal bytes in worker budget".into()); }
    // No writer/reader threads or naked PID supervision. The platform scope
    // must prove recursive drainage within the same deadline, including on
    // cancellation, failed peers, descendants, and transport overflow.
    let (wire, _stderr) = supervisor::exchange(worker, &stdin_frame, max_stdout,
        job.limits.max_stderr_bytes, job.limits.worker_deadline_ms, cancel)?;
    parse_frames(&wire, job, sources, target)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn admission() -> (ScopeRequest, Loaded) {
        let bytes = b"class C {}";
        let root = if cfg!(windows) { "C:/voda/project" } else { "/w/project" };
        let pack = if cfg!(windows) { "C:/voda/ref/net10.0" } else { "/x/net10.0" };
        let host = if cfg!(windows) { "C:/Windows/System32/where.exe" } else { "/bin/false" };
        let dll = if cfg!(windows) { "C:/voda/worker.dll" } else { "/tmp/worker.dll" };
        let input = json!({
            "version":1, "workspaceEpoch":"epoch-3", "committedRevision":"abc123",
            "workspaceRoot":root, "context":{
                "key":"Csharp|net10.0", "targetFramework":"net10.0",
                "languageVersion":"CSharp14", "assemblyName":"Fixture",
                "referencePack":pack},
            "worker":{"dotnet":host, "assembly":dll},
            "sources":[{"language":"csharp", "documentId":"logical:C",
                "logicalPath":"src/C.cs", "physicalPath":format!("{root}/src/C.cs"),
                "provenance":"normal", "bytesBase64":crate::base64::encode(bytes),
                "sha256":crate::contract::hex_digest(bytes)}]
        });
        let req: ScopeRequest = serde_json::from_value(input).expect("typed request");
        let (req, mut admitted) = req.admit().expect("caller boundary accepts fixture");
        (req, admitted.remove(0))
    }
    fn scope(job: &ScopeRequest, source: &Loaded) -> Value {
        json!({"workspaceEpoch":job.workspace_epoch,"committedRevision":job.committed_revision,
            "contextKey":job.context.key,"documentId":source.source.document_id,
            "sourceSha256":source.digest})
    }
    fn framed(value: &Value) -> Vec<u8> {
        let content = serde_json::to_vec(value).unwrap();
        let mut frame = (content.len() as u32).to_le_bytes().to_vec();
        frame.extend_from_slice(&content);
        frame
    }
    #[test]
    fn missing_terminal_and_partial_frame_are_rejected() {
        let (req, src) = admission();
        assert!(parse_frames(&[],&req,&[&src],&src).unwrap_err().contains("terminal"));
        assert!(parse_frames(&[1, 2, 3],&req,&[&src],&src).unwrap_err().contains("header"));
        let mut bad = (u32::MAX).to_le_bytes().to_vec();
        bad.push(b'{');
        assert!(parse_frames(&bad,&req,&[&src],&src).unwrap_err().contains("frame length"));
    }
    #[test]
    fn wrong_worker_epoch_and_hash_never_commit_frames() {
        let (req, src) = admission();
        let mut fact = json!({"version":1,"kind":"fact","provider":Language::Csharp.provider(),
            "scope":scope(&req,&src),"capability":"declarations"});
        assert!(check_scope(&fact,&req,&src).is_ok());
        fact["scope"]["workspaceEpoch"] = json!("epoch-OLD");
        assert!(parse_frames(&framed(&fact),&req,&[&src],&src).unwrap_err().contains("stale"));
        fact["scope"]["workspaceEpoch"] = json!(req.workspace_epoch);
        fact["scope"]["sourceSha256"] = json!("0".repeat(64));
        assert!(parse_frames(&framed(&fact),&req,&[&src],&src).unwrap_err().contains("stale"));
        let terminal = json!({"version":1,"kind":"result","scope":scope(&req,&src)});
        assert!(validate_terminal(&terminal,&req,&[&src],&src,0,0).is_err());
    }
}
