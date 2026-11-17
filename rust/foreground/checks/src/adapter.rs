//! Run the public C# batch adapter inside a scoped worker lease.
use crate::io::MAX_DOCUMENT;
use slophawk_worker_scope::{Limits, find_on_path, run};
use std::{path::Path, process::Command, sync::atomic::AtomicBool, time::Duration};

pub(crate) fn execute(
    assembly: &Path,
    verb: Option<&str>,
    input: &Path,
    scratch: &Path,
) -> Result<Vec<u8>, String> {
    let mut command = Command::new(find_on_path("dotnet")?);
    command.arg("exec").arg(assembly);
    if let Some(verb) = verb {
        command.arg(verb);
    }
    command.arg(input).current_dir(scratch);
    let output = run(&command, &[], Limits {
        stdout: MAX_DOCUMENT,
        stderr: 64 * 1024,
        deadline: Duration::from_secs(30),
    }, &AtomicBool::new(false))?;
    if output.code != 0 {
        let error = String::from_utf8_lossy(&output.stderr);
        return Err(format!("The adapter exited with code {}: {}",
            output.code, error.chars().take(2048).collect::<String>()));
    }
    Ok(output.stdout)
}
