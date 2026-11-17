use slophawk_public_shell::cli::project;
use slophawk_public_shell::identity::WorkspaceIdentity;
use slophawk_public_shell::mcp;
use slophawk_public_shell::protocol::{Command, DaemonClient, DaemonReply};
use slophawk_public_shell::registry::Registry;
use slophawk_public_shell::transport::NativeTransport;
use slophawk_public_shell::{Result, ShellError, failure};
use std::path::Path;
use std::process::{Command as ProcessCommand, Stdio};
use std::thread;
use std::time::{Duration, Instant};

const HELP: &str = "SlopHawk native compatibility shell (wire 1, report schema 2)\n\
Lifecycle     slophawk-native start|status|results|recheck|stop [--root <checkout>]\n\
MCP           slophawk-native mcp [--root <checkout>]\n\
Foreground    slophawk-native projects|check|check-contracts|check-observe|check-retest|check-review|contracts|probes|paths ...\n\
Requires an owner-pinned legacy C# launcher for foreground families, daemon start and unsupported native platforms.";

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match run(&args) {
        Ok(code) => std::process::exit(code),
        Err(error) => { eprintln!("{error}"); std::process::exit(1); }
    }
}

fn run(args: &[String]) -> Result<i32> {
    if args.is_empty() || matches!(args, [h] if h == "--help" || h == "-h") {
        println!("{HELP}");
        return Ok(0);
    }
    let verb = args[0].as_str();
    if matches!(verb, "check" | "check-contracts" | "check-observe" | "check-retest" |
        "check-review" | "contracts" | "probes" | "paths" | "projects" | "serve") {
        let registry = Registry::load(Some(&std::env::current_dir().map_err(|e| failure("invalid_root", e))?))?;
        return registry.legacy()?.forward(args, None);
    }
    if !matches!(verb, "start" | "status" | "results" | "recheck" | "stop" | "mcp") {
        return Err(failure("invalid_arguments", "Unknown command. Run slophawk-native --help."));
    }
    let root = match args {
        [_] => std::env::current_dir().map_err(|e| failure("invalid_root", e))?,
        [_, option, path] if option == "--root" => Path::new(path).to_path_buf(),
        _ => return Err(failure("invalid_arguments", "Expected [--root <checkout>] only.")),
    };

    // The .NET Unix pipe socket name is not a supported public contract. Use
    // the same verified C# CLI/SDK (including MCP stdio) on unsupported OSes.
    if !cfg!(windows) {
        let registry = Registry::load(Some(&root))?;
        return registry.legacy()?.forward(args, Some(&root));
    }
    let identity = match WorkspaceIdentity::resolve(&root) {
        Ok(identity) => identity,
        Err(error) if error.code == "identity_unsupported" => {
            let registry = Registry::load(Some(&root))?;
            return registry.legacy()?.forward(args, Some(&root));
        }
        Err(error) => return Err(error),
    };
    let client = DaemonClient::new(NativeTransport::new(identity.instance_name.clone()),
        identity.root.to_string_lossy().into_owned());
    match verb {
        "start" => {
            let result = ensure_started(&client, &identity.root)?;
            print_reply(&result)
        }
        "status" => print_reply(&client.command(Command::Status)?),
        "results" => print_reply(&client.command(Command::Results)?),
        "recheck" => print_reply(&client.command(Command::Recheck)?),
        "stop" => print_reply(&client.command(Command::Stop)?),
        "mcp" => {
            let ready = ensure_started(&client, &identity.root)?;
            if !ready.ok { return Err(ShellError::new(ready.code.as_deref().unwrap_or("daemon_unavailable"),
                ready.error.as_deref().unwrap_or("Daemon is not available."))); }
            let stdin = std::io::stdin();
            let stdout = std::io::stdout();
            mcp::serve(&client, stdin.lock(), stdout.lock())?;
            Ok(0)
        }
        _ => unreachable!(),
    }
}

fn ensure_started(client: &DaemonClient<NativeTransport>, root: &Path) -> Result<DaemonReply> {
    match client.command(Command::Status) {
        Ok(reply) if reply.ok => return Ok(reply),
        Ok(reply) => return Ok(reply), // A typed daemon failure must not cause replacement.
        Err(error) if error.code == "daemon_unavailable" => (),
        Err(error) => return Err(error), // Cannot infer absence from a bad handshake.
    }
    let registry = Registry::load(Some(root))?;
    let launcher = registry.daemon_launcher()?;
    // A failed/partial spawn must not replace an incompatible existing daemon.
    let mut process: ProcessCommand = launcher.command(&["serve".into(), "--root".into(),
        root.to_string_lossy().into_owned()], Some(root))?;
    process.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
    let mut child = process.spawn().map_err(|e| failure("failed_launch", e))?;
    let deadline = Instant::now() + Duration::from_secs(8);
    loop {
        match client.command(Command::Status) {
            Ok(reply) => return Ok(reply),
            Err(error) if error.code == "daemon_unavailable" => (),
            Err(error) => return Err(error),
        }
        if let Some(status) = child.try_wait().map_err(|e| failure("failed_launch", e))? {
            return Err(failure("failed_launch", format!("Daemon launcher exited before readiness: {status}")));
        }
        if Instant::now() >= deadline {
            return Err(failure("daemon_timeout", "Daemon failed to become ready within eight seconds."));
        }
        thread::sleep(Duration::from_millis(70));
    }
}

fn print_reply(reply: &DaemonReply) -> Result<i32> {
    let projection = project(reply)?;
    if let Some(text) = projection.stdout { println!("{text}"); }
    if let Some(text) = projection.stderr { eprintln!("{text}"); }
    Ok(projection.exit_code)
}
