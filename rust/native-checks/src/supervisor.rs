use crate::contract::Worker;
use slophawk_worker_scope::{Limits, run};
use std::process::Command;
use std::sync::atomic::AtomicBool;
use std::time::Duration;

pub(crate) fn exchange(
    worker: &Worker,
    frame: &[u8],
    stdout_cap: usize,
    stderr_cap: usize,
    deadline_ms: u64,
    cancel: &AtomicBool,
) -> Result<(Vec<u8>, Vec<u8>), String> {
    let mut command = Command::new(&worker.dotnet);
    command.arg(&worker.assembly);
    let output = run(&command, frame, Limits {
        stdout: stdout_cap,
        stderr: stderr_cap,
        deadline: Duration::from_millis(deadline_ms),
    }, cancel)?;
    if output.code != 0 {
        return Err(format!("C# worker exited with code {}; staged facts discarded", output.code));
    }
    Ok((output.stdout, output.stderr))
}
