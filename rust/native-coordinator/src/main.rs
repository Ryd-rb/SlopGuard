//! Explicit native `serve-ipc` (private current-user socket) and `serve`
//! (fixture stdin) entry points. Neither replaces the shipped C# daemon or
//! serves MCP; stdout stays protocol-only in the fixture mode.
use slophawk_native_coordinator::{failure, quote, Coordinator, Event, FakeMode};
use std::io::{self, Read};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc};
use std::time::{Duration, Instant};

fn main() {
    if let Err(error) = execute() {
        eprintln!("native coordinator pilot: {error}");
        std::process::exit(1);
    }
}
fn execute() -> io::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if matches!(args.as_slice(), [verb] if verb == "--inventory-peer-v1") {
        return slophawk_native_coordinator::inventory_worker(
            &std::env::current_dir()?, io::stdin().lock(), io::stdout().lock(),
        );
    }
    if let [verb, root_flag, root, profile_flag, profile, context_flag, context] = args.as_slice() {
        if verb == "run-peer" && root_flag == "--root" && profile_flag == "--profile" && context_flag == "--context" {
            let mut coordinator = Coordinator::open(root)?;
            let count = coordinator.run_inventory_peer(
                std::path::Path::new(profile), context, &AtomicBool::new(false),
            )?;
            println!("{{\"verifiedSources\":{count}}}");
            return Ok(());
        }
    }
    if args.first().is_some_and(|v| v == "fake-worker") {
        let mode = args.get(1).map_or("", String::as_str);
        let input = args.get(2).map_or("", String::as_str);
        std::process::exit(slophawk_native_coordinator::fake_worker(mode, input)?);
    }
    if args.first().is_some_and(|v| v == "fake-grandchild") {
        slophawk_native_coordinator::fake_grandchild();
        return Ok(());
    }
    if args.len() != 3 || args[1] != "--root" ||
       (args[0] != "serve" && args[0] != "serve-ipc") {
        eprintln!("usage: slophawk-native-coordinator <serve|serve-ipc> --root <existing workspace>");
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "explicit physical root required"));
    }
    let mut coordinator = Coordinator::open(&args[2])?;
    if args[0] == "serve-ipc" {
        return slophawk_native_coordinator::ipc::serve(&mut coordinator);
    }
    let clock = coordinator.sequence_clock();
    let (tx, rx) = mpsc::sync_channel::<String>(128);
    let overflow = Arc::new(AtomicBool::new(false));
    let producer_overflow = Arc::clone(&overflow);
    std::thread::spawn(move || {
        let mut input = io::stdin().lock();
        loop {
            // `BufRead::lines` accumulates an unbounded hostile line before
            // its length is checked. Consume at most 4096 bytes per command
            // while draining any oversized line without growing the buffer.
            match bounded_line(&mut input, 4096) {
                Ok(Some(Some(line))) => {
                    if line.starts_with("event ") || line.starts_with("git ") {
                        // A notice races a running fake worker before dequeue.
                        clock.fetch_add(1, Ordering::AcqRel);
                    }
                    if tx.try_send(line).is_err() {
                        producer_overflow.store(true, Ordering::Release);
                    }
                }
                Ok(Some(None)) => producer_overflow.store(true, Ordering::Release),
                Ok(None) => break,
                Err(_) => { producer_overflow.store(true, Ordering::Release); break; }
            }
        }
    });
    let mut last_poll = Instant::now();
    loop {
        if overflow.swap(false, Ordering::AcqRel) {
            coordinator.notify(Event::Overflow, Instant::now())?;
        }
        match rx.recv_timeout(Duration::from_millis(50)) {
            Ok(command) => {
                let answer = handle(&mut coordinator, &command)?;
                println!("{answer}");
                if command == "stop" { break; }
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }
        if let Err(error) = coordinator.tick(Instant::now()) {
            println!("{}", failure("reconciliation_failed", &error.to_string()));
        }
        if last_poll.elapsed() >= Duration::from_millis(750) {
            if let Err(error) = coordinator.poll() {
                println!("{}", failure("poll_failed", &error.to_string()));
                return Err(error);
            }
            last_poll = Instant::now();
        }
    }
    Ok(())
}
/// `None` is EOF, `Some(None)` an invalid or oversized command.
/// Read incrementally so hostile stdin never causes an unbounded allocation.
fn bounded_line<R: Read>(input: &mut R, limit: usize) -> io::Result<Option<Option<String>>> {
    let mut bytes = Vec::with_capacity(limit);
    let mut oversized = false;
    let mut received = false;
    loop {
        let mut byte = [0u8; 1];
        if input.read(&mut byte)? == 0 {
            if !received { return Ok(None); }
            break;
        }
        received = true;
        if byte[0] == b'\n' { break; }
        if bytes.len() < limit { bytes.push(byte[0]); }
        else { oversized = true; }
    }
    if oversized { return Ok(Some(None)); }
    Ok(Some(String::from_utf8(bytes).ok().map(|line|
        line.trim_end_matches('\r').to_owned())))
}

fn handle(coordinator: &mut Coordinator, command: &str) -> io::Result<String> {
    let fields: Vec<_> = command.split_whitespace().collect();
    let op = fields.first().copied().unwrap_or("");
    let answer = match (op, fields.as_slice()) {
        ("hello" | "status" | "results" | "list_checkers" | "get_findings" | "workspace_status", [_]) => coordinator.reply_json(op),
        ("event", [_, "add", path]) => { coordinator.notify(Event::Add((*path).into()), Instant::now())?; ack("queued") }
        ("event", [_, "change", path]) => { coordinator.notify(Event::Change((*path).into()), Instant::now())?; ack("queued") }
        ("event", [_, "delete", path]) => { coordinator.notify(Event::Delete((*path).into()), Instant::now())?; ack("queued") }
        ("event", [_, "rename", from, to]) => {
            coordinator.notify(Event::Rename { from: (*from).into(), to: (*to).into() }, Instant::now())?;
            ack("queued")
        }
        ("event", [_, "overflow"]) => { coordinator.notify(Event::Overflow, Instant::now())?; ack("queued") }
        ("git", [_, branch, commit]) => {
            coordinator.notify(Event::Git { branch: (*branch).into(), commit: (*commit).into() }, Instant::now())?;
            ack("queued")
        }
        ("recheck" | "run_checks", [_]) => {
            let changes = coordinator.recheck()?;
            format!("{{\"ok\":true,\"payload\":{{\"revision\":{},\"deltas\":{}}},\"error\":null}}",
                coordinator.revision(), changes.len())
        }
        ("run", [_, mode]) => {
            let mode = match *mode {
                "complete" => FakeMode::Complete, "fail" => FakeMode::Fail, "cancel" => FakeMode::Cancel,
                _ => return Ok(failure("unknown_fixture_mode", "Specify complete, fail, or cancel.")),
            };
            match coordinator.run_fake(mode) {
                Ok(Some((accepted, outcome))) => format!("{{\"ok\":true,\"payload\":{{\"accepted\":{},\"outcome\":{},\"coverage\":{},\"reason\":{},\"workerBytes\":{},\"durationMicros\":{}}},\"error\":null}}",
                    accepted, quote(outcome.status), quote(outcome.coverage), quote(outcome.reason), outcome.bytes, outcome.duration.as_micros()),
                Ok(None) => ack("no_pending_work"),
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => failure("waiting_worker", &error.to_string()),
                Err(error) => failure("worker_failed", &error.to_string()),
            }
        }
        ("stop", [_]) => "{\"ok\":true,\"payload\":{\"stopping\":true},\"error\":null}".into(),
        _ => failure("unknown_command", "Unknown or malformed fixture command."),
    };
    Ok(answer)
}
fn ack(value: &str) -> String {
    format!("{{\"ok\":true,\"payload\":{{\"state\":{}}},\"error\":null}}", quote(value))
}
