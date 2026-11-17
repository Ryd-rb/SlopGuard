//! Standalone source-port command surface. The shipped C# CLI remains unchanged.
use slophawk_managed_methods::{CheckError, ErrorKind};
use std::{env, fs::File, io::Read, process};

fn run() -> Result<String, CheckError> {
    // Reject non-Unicode argv instead of panicking. The CLI does not interpret
    // metadata-provided paths or execute untrusted assemblies.
    let args = env::args_os().skip(1).map(|a| a.into_string().map_err(|_| CheckError {
        kind: ErrorKind::Unsupported, message: "command arguments must be Unicode".into(),
    })).collect::<Result<Vec<_>, _>>()?;
    match args.as_slice() {
        [probes, methods, candidate, reference] if probes == "probes" && methods == "methods" => {
            let candidate = read(candidate)?;
            let reference = read(reference)?;
            let diffs = slophawk_managed_methods::compare(&candidate, &reference)?;
            slophawk_managed_methods::differences_json(&diffs)
        }
        [probes, literals, file, operations @ ..]
            if probes == "probes" && literals == "literals-names" && !operations.is_empty() => {
            let bytes = read(file)?;
            let entries = slophawk_managed_methods::integer_literals_for_names(&bytes, operations)?;
            slophawk_managed_methods::literals_json(&entries)
        }
        [probes, literals, file, operations @ ..]
            if probes == "probes" && literals == "literals-operations" && !operations.is_empty() => {
            let mut parsed = Vec::new();
            if operations.len() > 4096 { return Err(bad_arguments()); }
            for item in operations {
                let (name, arity) = item.rsplit_once(':').ok_or_else(bad_arguments)?;
                if name.is_empty() || arity.is_empty() || !arity.bytes().all(|b| b.is_ascii_digit()) {
                    return Err(bad_arguments());
                }
                let arity: usize = arity.parse().map_err(|_| bad_arguments())?;
                if arity > i32::MAX as usize { return Err(bad_arguments()); }
                parsed.push((name.to_owned(), arity));
            }
            let bytes = read(file)?;
            let entries = slophawk_managed_methods::integer_literals_for_operations(&bytes, &parsed)?;
            slophawk_managed_methods::literals_json(&entries)
        }
        _ => Err(bad_arguments()),
    }
}
fn bad_arguments() -> CheckError {
    CheckError { kind: ErrorKind::Unsupported, message:
        "usage: managed-methods probes methods <candidate.dll> <reference.dll> | probes literals-names <assembly.dll> <name>... | probes literals-operations <assembly.dll> <name:arity>...".into() }
}
fn read(path: &str) -> Result<Vec<u8>, CheckError> {
    // Size checks precede any input-file buffer allocation. Never reflect,
    // invoke, or map input assembly as executable code.
    let file = File::open(path).map_err(|e| CheckError {
        kind: ErrorKind::Io, message: format!("input open: {e}"),
    })?;
    let metadata = file.metadata().map_err(|e| CheckError {
        kind: ErrorKind::Io, message: format!("input metadata: {e}"),
    })?;
    if metadata.len() > slophawk_managed_methods::MAX_ASSEMBLY_BYTES as u64 {
        return Err(CheckError { kind: ErrorKind::Limit, message: "input file exceeds size cap".into() });
    }
    let mut bounded = file.take(slophawk_managed_methods::MAX_ASSEMBLY_BYTES as u64 + 1);
    let mut contents = Vec::new();
    contents.try_reserve_exact(metadata.len() as usize)
        .map_err(|_| CheckError { kind: ErrorKind::Limit, message: "input buffer allocation failed".into() })?;
    bounded.read_to_end(&mut contents).map_err(|e| CheckError {
        kind: ErrorKind::Io, message: format!("input read: {e}"),
    })?;
    if contents.len() > slophawk_managed_methods::MAX_ASSEMBLY_BYTES {
        return Err(CheckError { kind: ErrorKind::Limit, message: "input grew beyond size cap".into() });
    }
    Ok(contents)
}
fn main() {
    match run() {
        Ok(json) => println!("{json}"),
        Err(e) => {
            eprintln!("managed-methods {}: {}", e.kind.as_str(), e.message);
            process::exit(if e.kind == ErrorKind::Unsupported { 2 } else { 1 });
        }
    }
}
