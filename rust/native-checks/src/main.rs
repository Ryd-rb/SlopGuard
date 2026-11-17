//! Separate executable: no changes to the existing SlopHawk entry point.
use slophawk_native_checks::{bounded_report, shadow};
use slophawk_native_checks::contract::Status;
use slophawk_native_checks::contract::{MAX_REQUEST_BYTES, ScopeRequest};
use std::fs::File;
use std::io::{self, Read, Write};
use std::sync::atomic::AtomicBool;
use std::time::Instant;

fn run() -> Result<i32, String> {
    let mut args = std::env::args_os();
    let _executable = args.next();
    if args.next().as_deref() != Some(std::ffi::OsStr::new("shadow")) { return Err("usage: slophawk-native-checks shadow --request /absolute/request.json".into()); }
    if args.next().as_deref() != Some(std::ffi::OsStr::new("--request")) { return Err("expected --request".into()); }
    let name = args.next().ok_or("missing request path")?;
    if args.next().is_some() { return Err("unexpected CLI arguments".into()); }
    let started = Instant::now();
    let file = File::open(&name).map_err(|e| format!("cannot open explicit request: {e}"))?;
    if file.metadata().map_err(|e| e.to_string())?.len() > MAX_REQUEST_BYTES { return Err("request file exceeds 4MiB".into()); }
    let mut raw = Vec::new();
    file.take(MAX_REQUEST_BYTES+1).read_to_end(&mut raw).map_err(|e| e.to_string())?;
    if raw.len() as u64 > MAX_REQUEST_BYTES { return Err("request exceeded 4MiB while reading".into()); }
    let req: ScopeRequest = serde_json::from_slice(&raw).map_err(|e| format!("invalid typed request: {e}"))?;
    let mut report = shadow(req, &AtomicBool::new(false))?;
    report.elapsed_ms = started.elapsed().as_millis();
    let failed = report.status == Status::Failed;
    let output = bounded_report(report)?;
    io::stdout().lock().write_all(&output).map_err(|e| format!("could not emit report: {e}"))?;
    io::stdout().lock().write_all(b"\n").map_err(|e| e.to_string())?;
    Ok(if failed { 1 } else { 0 })
}
fn main() {
    match run() {
        Ok(code) => std::process::exit(code),
        Err(message) => { eprintln!("native shadow rejected: {message}"); std::process::exit(2); }
    }
}
