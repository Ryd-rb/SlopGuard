use slophawk_public_shell::cli::project;
use slophawk_public_shell::identity::WorkspaceIdentity;
use slophawk_public_shell::protocol::DaemonReply;
use slophawk_public_shell::{Result, failure};
use std::path::Path;
mod migration;
mod resident;
mod tools;
mod detached;

const HELP: &str = "SlopHawk (wire 1, report schema 2)\n\
Lifecycle     slophawk start|status|results|recheck|stop [--root <checkout>]\n\
Checkers      slophawk list-checkers [--root <checkout>] | checker-plan --request <request.json>\n\
MCP           slophawk mcp [--root <checkout>]\n\
Foreground    slophawk projects|check|check-contracts|check-observe|check-retest|check-review|contracts|probes|paths ...\n\
Source checks slophawk shadow --request <request.json>\n\
C# operations use owner-pinned Roslyn and VSTest tools.";

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if matches!(args.as_slice(), [verb] if verb == "--slophawk-migration-job-v1") {
        if let Err(error) = migration::run_stdio() {
            eprintln!("Native state transition refused: {error}");
            std::process::exit(2);
        }
        return;
    }
    match run(&args) {
        Ok(code) => std::process::exit(code),
        Err(error) => { eprintln!("{error}"); std::process::exit(1); }
    }
}

fn run(args: &[String]) -> Result<i32> {
    match args {
        [flag, root, state, profile] if flag == "--native-resident-v1" =>
            return resident::serve(Path::new(root), Path::new(state), profile).map(|()| 0)
                .map_err(|error| failure("native_daemon_failed", error)),
        [flag, root, state, profile, verb] if flag == "--native-workspace-v1" =>
            return resident::cli(Path::new(root), Path::new(state), profile, verb),
        [flag, option, root] if flag == "--import-schema-v5" && option == "--root" => {
            let workspace = WorkspaceIdentity::resolve(Path::new(root))?;
            return migration::native_start::import_legacy(&workspace, std::io::stdin().lock())
                .map(|()| 0).map_err(|error| failure("import_failed", error));
        }
        _ => (),
    }
    if args.is_empty() || matches!(args, [h] if h == "--help" || h == "-h") {
        println!("{HELP}");
        return Ok(0);
    }
    let verb = args[0].as_str();
    if verb == "checker-plan" {
        return match args {
            [_, flag, path] if flag == "--request" => {
                let plan = slophawk_public_shell::checker_plan::read(Path::new(path))?;
                println!("{plan}");
                Ok(0)
            }
            _ => Err(failure("invalid_arguments", "Expected checker-plan --request <request.json>.")),
        };
    }
    if verb == "shadow" {
        return match args {
            [_, flag, path] if flag == "--request" =>
                slophawk_native_checks::write_shadow_report(Path::new(path), std::io::stdout().lock())
                    .map_err(|error| failure("shadow_failed", error)),
            _ => Err(failure("invalid_arguments", "Expected shadow --request <request.json>.")),
        };
    }
    if matches!(verb, "check" | "check-observe" | "check-retest" | "check-review") {
        return slophawk_foreground_checks::run(args)
            .map(i32::from).map_err(|error| failure("foreground_check_failed", error));
    }
    if matches!(verb, "contracts" | "probes" | "check-contracts") {
        return slophawk_contracts_probes::command::run(args).map(i32::from)
            .map_err(|error| failure("foreground_check_failed", error));
    }
    if verb == "paths" { return tools::paths(args); }
    if verb == "projects" { return tools::projects(&workspace_argument(args)?); }
    if !matches!(verb, "start" | "status" | "results" | "recheck" | "stop" | "list-checkers" | "mcp" | "serve") {
        return Err(failure("invalid_arguments", "Unknown command. Run slophawk --help."));
    }
    let workspace = WorkspaceIdentity::resolve(&workspace_argument(args)?)?;
    let (state, profile) = migration::native_start::open(&workspace, matches!(verb, "start" | "mcp" | "serve"))
        .map_err(|error| failure(if error.kind() == std::io::ErrorKind::NotFound { "daemon_unavailable" }
            else { "invalid_state" }, error))?;
    if verb == "serve" {
        resident::serve(&workspace.root, &state, &profile).map(|()| 0)
            .map_err(|error| failure("native_daemon_failed", error))
    } else {
        resident::cli(&workspace.root, &state, &profile, verb)
    }
}

fn workspace_argument(args: &[String]) -> Result<std::path::PathBuf> {
    match args {
        [_] => std::env::current_dir().map_err(|e| failure("invalid_root", e)),
        [_, option, path] if option == "--root" => std::path::absolute(path).map_err(|e| failure("invalid_root", e)),
        _ => Err(failure("invalid_arguments", "Expected [--root <checkout>] only.")),
    }
}

fn print_reply(reply: &DaemonReply) -> Result<i32> {
    let projection = project(reply)?;
    if let Some(text) = projection.stdout { println!("{text}"); }
    if let Some(text) = projection.stderr { eprintln!("{text}"); }
    Ok(projection.exit_code)
}
