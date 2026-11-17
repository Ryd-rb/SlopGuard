use serde_json::Value;
use slophawk_public_shell::{
    Result, failure, identity::require_physical, registry::configured_tool,
};
use slophawk_worker_scope::Limits;
use std::{path::Path, process::Command, sync::atomic::AtomicBool, time::Duration};

pub fn projects(root: &Path) -> Result<i32> {
    require_physical(root, true)?;
    let worker = configured_tool("SLOPHAWK_PROJECT_WORKER", root)?;
    let host = configured_tool("SLOPHAWK_NATIVE_DOTNET", root)?;
    let mut command = Command::new(host);
    command
        .arg("exec")
        .arg(worker)
        .args(["--project-graph-v1", "--root"])
        .arg(root)
        .current_dir(root);
    let output = slophawk_worker_scope::run(
        &command,
        &[],
        Limits {
            stdout: 8 * 1024 * 1024,
            stderr: 16 * 1024,
            deadline: Duration::from_secs(55),
        },
        &AtomicBool::new(false),
    )
    .map_err(|error| failure("project_evaluation_failed", error))?;
    if output.code > 1 {
        return Err(failure(
            "project_evaluation_failed",
            String::from_utf8_lossy(&output.stderr),
        ));
    }
    let graph: Value = serde_json::from_slice(&output.stdout)
        .map_err(|error| failure("project_evaluation_failed", error))?;
    if !graph["targets"].is_array()
        || !graph["issues"].is_array()
        || graph["complete"].as_bool() != Some(output.code == 0)
    {
        return Err(failure(
            "project_evaluation_failed",
            "managed project facts have an invalid terminal",
        ));
    }
    println!(
        "{}",
        serde_json::to_string_pretty(&graph)
            .map_err(|e| failure("project_evaluation_failed", e))?
    );
    Ok(output.code as i32)
}

pub fn paths(args: &[String]) -> Result<i32> {
    let verb = args.get(1).map(String::as_str);
    if !matches!(
        (verb, args.len()),
        (Some("build"), 4) | (Some("plan"), 6) | (Some("check" | "review"), 3)
    ) {
        return Err(failure(
            "invalid_arguments",
            "Expected paths build, plan, check or review.",
        ));
    }
    let input = std::path::absolute(&args[2]).map_err(|error| failure("invalid_path", error))?;
    let root = input
        .parent()
        .ok_or_else(|| failure("invalid_path", "Path checker input has no parent."))?;
    let worker = configured_tool("SLOPHAWK_SCALAR_WORKER", root)?;
    let mut routed = args[1..].to_vec();
    routed.push(
        worker
            .into_os_string()
            .into_string()
            .map_err(|_| failure("invalid_path", "Worker path must be Unicode."))?,
    );
    let (output, code) = slophawk_scalar_foreground::run(&routed)
        .map_err(|error| failure("path_check_failed", error))?;
    println!(
        "{}",
        serde_json::to_string_pretty(&output).map_err(|e| failure("path_check_failed", e))?
    );
    Ok(code)
}
