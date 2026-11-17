//! Separate executable: no changes to the existing SlopHawk entry point.
fn run() -> Result<i32, String> {
    let mut args = std::env::args_os();
    let _executable = args.next();
    if args.next().as_deref() != Some(std::ffi::OsStr::new("shadow")) { return Err("usage: slophawk-native-checks shadow --request /absolute/request.json".into()); }
    if args.next().as_deref() != Some(std::ffi::OsStr::new("--request")) { return Err("expected --request".into()); }
    let name = args.next().ok_or("missing request path")?;
    if args.next().is_some() { return Err("unexpected CLI arguments".into()); }
    slophawk_native_checks::write_shadow_report(std::path::Path::new(&name), std::io::stdout().lock())
}
fn main() {
    match run() {
        Ok(code) => std::process::exit(code),
        Err(message) => { eprintln!("native shadow rejected: {message}"); std::process::exit(2); }
    }
}
