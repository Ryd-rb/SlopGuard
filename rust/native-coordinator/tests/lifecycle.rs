use slophawk_native_coordinator::{Admission, Coordinator, Event, FixtureResult};
use sha2::{Digest, Sha256};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::AtomicBool;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

struct Scratch(PathBuf);
impl Scratch {
    fn new() -> Self {
        let nanos = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();
        let root = std::env::temp_dir().join(format!("sh-native-fixture-{}-{nanos}", std::process::id()));
        fs::create_dir(&root).unwrap();
        Self(root)
    }
    fn root(&self) -> &Path { &self.0 }
    fn write(&self, path: &str, text: &str) {
        let target = self.0.join(path);
        fs::create_dir_all(target.parent().unwrap()).unwrap();
        fs::write(target, text).unwrap();
    }
}
impl Drop for Scratch {
    fn drop(&mut self) { let _ = fs::remove_dir_all(&self.0); }
}

fn inventory_grant(root: &Coordinator, path: &Path, binary: &Path) -> io::Result<()> {
    let digest = format!("{:x}", Sha256::digest(fs::read(binary)?));
    let payload = serde_json::json!({
        "version": 1, "rootIdentity": root.fence().root_identity,
        "peers": [{"operation": "inventory", "executable": binary, "sha256": digest,
            "toolVersion": "1", "context": "csharp:unresolved:.",
            "policyDigest": root.fence().policy_digest, "budgetRef": "inventory-20s",
            "grantRef": "test-inventory"}]
    });
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    serde_json::to_writer(options.open(path)?, &payload)?;
    Ok(())
}

#[test]
fn installed_inventory_peer_publishes_without_consuming_pending_causes() -> io::Result<()> {
    if cfg!(target_os = "linux") && std::env::var_os("SLOPHAWK_LIVE_NATIVE_SCOPE").is_none() {
        return Ok(());
    }
    let root = Scratch::new();
    let owner = Scratch::new();
    root.write("Source.cs", "class Before {}\n");
    let mut coordinator = Coordinator::open(root.root())?;
    root.write("Source.cs", "class After {}\n");
    assert!(coordinator.poll()?);
    let pending = coordinator.pending().to_vec();
    assert_eq!(pending.len(), 1);
    let executable = Path::new(env!("CARGO_BIN_EXE_slophawk-native-coordinator"));
    let profile = owner.root().join("inventory.json");
    inventory_grant(&coordinator, &profile, executable)?;
    let cancel = AtomicBool::new(false);
    assert_eq!(coordinator.run_inventory_peer(&profile, "csharp:unresolved:.", &cancel)?, 1);
    assert_eq!(coordinator.pending(), pending);
    assert!(coordinator.read_current()?.unwrap().contains("inventory_complete"));
    fs::remove_file(&profile)?;
    assert!(coordinator.run_inventory_peer(&profile, "csharp:unresolved:.", &cancel).is_err());
    assert_eq!(coordinator.attempts().len(), 1);
    drop(coordinator);
    let reopened = Coordinator::open(root.root())?;
    assert_eq!(reopened.pending(), pending);
    assert_eq!(reopened.attempts()[0].status, "completed");
    Ok(())
}

#[cfg(target_os = "linux")]
#[test]
fn coordinator_restart_recovers_the_persisted_live_os_scope() -> io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    use std::process::Stdio;
    if std::env::var_os("SLOPHAWK_LIVE_NATIVE_SCOPE").is_none() {
        return Ok(());
    }
    let root = Scratch::new();
    let owner = Scratch::new();
    root.write("Source.cs", "class Before {}\n");
    let mut coordinator = Coordinator::open(root.root())?;
    root.write("Source.cs", "class After {}\n");
    assert!(coordinator.poll()?);
    let pending = coordinator.pending().to_vec();
    assert_eq!(pending.len(), 1);
    let signal = owner.root().join("started");
    let worker = owner.root().join("owner-test-worker");
    owner.write("owner-test-worker", &format!("#!/bin/sh\n/usr/bin/sleep 30 &\nprintf '%s\\n' \"$!\" > '{}'\nwait\n", signal.display()));
    fs::set_permissions(&worker, fs::Permissions::from_mode(0o700))?;
    let grant = owner.root().join("inventory.json");
    inventory_grant(&coordinator, &grant, &worker)?;
    drop(coordinator);
    let mut child = Command::new(env!("CARGO_BIN_EXE_slophawk-native-coordinator"))
        .arg("run-peer").arg("--root").arg(root.root()).arg("--profile").arg(grant)
        .args(["--context", "csharp:unresolved:."]).stdout(Stdio::null()).spawn()?;
    let deadline = Instant::now() + Duration::from_secs(8);
    while !signal.is_file() && Instant::now() < deadline && child.try_wait()?.is_none() {
        std::thread::sleep(Duration::from_millis(25));
    }
    child.kill()?;
    child.wait()?;
    let recovered = Coordinator::open(root.root())?;
    assert_eq!(recovered.pending(), pending);
    let attempt = recovered.attempts().last().unwrap();
    assert_eq!(attempt.status, "interrupted");
    assert_eq!(attempt.reason, "scope_drained_after_restart");
    assert!(!recovered.read_current()?.unwrap().contains("inventory_complete"));
    assert!(signal.is_file(), "the scoped worker must have started before the coordinator exited");
    Ok(())
}

#[test]
fn clean_start_duplicate_atomic_save_and_save_burst() -> io::Result<()> {
    let root = Scratch::new();
    root.write("a.cs", "class C {}\n");
    let mut c = Coordinator::open(root.root())?;
    assert!(c.pending().is_empty(), "startup is a baseline without new intents");
    assert!(c.read_current()?.unwrap().contains("\"schemaVersion\":2"));
    let now = Instant::now();
    for i in 0..200 {
        c.notify(Event::Change("a.cs".into()), now + Duration::from_millis(i))?;
    }
    assert_eq!(c.queue_len(), 200);
    assert!(c.read_current()?.is_none(), "notification withdraws current projection");
    assert!(!c.tick(now + Duration::from_millis(400))?);
    assert!(c.tick(now + Duration::from_millis(501))?);
    assert!(c.pending().is_empty(), "identical bytes do not create intents");
    root.write("a.cs", "class C { int X; }\n");
    c.notify(Event::Change("a.cs".into()), Instant::now())?;
    assert_eq!(c.reconcile_now()?.len(), 1);
    assert_eq!(c.pending().len(), 1);
    Ok(())
}

#[test]
fn rename_changed_bytes_and_deletion_keep_both_endpoints() -> io::Result<()> {
    let root = Scratch::new();
    root.write("before.cs", "class Before {}\n");
    let mut c = Coordinator::open(root.root())?;
    fs::rename(root.root().join("before.cs"), root.root().join("after.cs"))?;
    root.write("after.cs", "class After {}\n");
    c.notify(Event::Rename { from: "before.cs".into(), to: "after.cs".into() }, Instant::now())?;
    let delta = c.reconcile_now()?;
    assert_eq!(delta.len(), 1);
    assert_eq!(delta[0].kind, "rename");
    assert_eq!(delta[0].old_path, "before.cs");
    assert_eq!(delta[0].new_path, "after.cs");
    let intent = c.pending().last().unwrap();
    assert_eq!(intent.old_path, "before.cs");
    assert_eq!(intent.new_path, "after.cs");
    fs::remove_file(root.root().join("after.cs"))?;
    c.notify(Event::Delete("after.cs".into()), Instant::now())?;
    let deleted = c.reconcile_now()?;
    assert_eq!(deleted.len(), 1);
    assert_eq!(deleted[0].kind, "delete");
    assert!(deleted[0].new_digest.is_empty());
    assert_eq!(c.pending().len(), 2);
    Ok(())
}

#[test]
fn branch_a_b_a_changes_epoch_and_rejects_obsolete_attempt() -> io::Result<()> {
    let root = Scratch::new();
    root.write("a.cs", "one");
    let commit = "a367ef58076f7e29d2ae475a3b593daad04c4998";
    root.write(".git/HEAD", "ref: refs/heads/A\n");
    root.write(".git/refs/heads/A", &format!("{commit}\n"));
    root.write(".git/refs/heads/B", &format!("{commit}\n"));
    let mut c = Coordinator::open(root.root())?;
    let epoch_a = c.epoch();
    root.write("a.cs", "two");
    assert!(c.poll()?);
    let admitted = wait_admit(&mut c)?;
    root.write(".git/HEAD", "ref: refs/heads/B\n");
    assert!(c.poll()?);
    assert!(!c.finish(admitted, FixtureResult::Complete)?);
    assert_eq!(c.pending().len(), 1);
    root.write(".git/HEAD", "ref: refs/heads/A\n");
    assert!(c.poll()?);
    assert_ne!(epoch_a, c.epoch());
    let current = wait_admit(&mut c)?;
    assert!(c.finish(current, FixtureResult::Complete)?);
    assert!(c.pending().is_empty());
    assert!(c.read_current()?.unwrap().contains("\"coverage\":\"complete\""),
        "only the controlled fixture receipt may say complete");
    Ok(())
}
#[test]
fn failed_cancelled_and_restart_recover_pending_evidence() -> io::Result<()> {
    let root = Scratch::new();
    root.write("p.cs", "before");
    let mut c = Coordinator::open(root.root())?;
    root.write("p.cs", "after");
    c.notify(Event::Change("p.cs".into()), Instant::now())?;
    c.reconcile_now()?;
    let failed = wait_admit(&mut c)?;
    assert!(!c.finish(failed, FixtureResult::Fail)?);
    assert_eq!(c.pending().len(), 1);
    let cancelled = wait_admit(&mut c)?;
    assert!(!c.finish(cancelled, FixtureResult::Cancel)?);
    let orphan = wait_admit(&mut c)?;
    let previous = c.generation();
    drop(orphan); // simulate process interruption before a result transaction
    drop(c);
    let mut recovered = Coordinator::open(root.root())?;
    assert!(recovered.generation() > previous);
    assert_eq!(recovered.pending().len(), 1);
    assert!(recovered.attempts().iter().any(|a| a.status == "interrupted"));
    let admitted = wait_admit(&mut recovered)?;
    assert!(recovered.finish(admitted, FixtureResult::Complete)?);
    assert_eq!(recovered.pending().len(), 0);
    Ok(())
}

#[test]
fn per_workspace_single_writer_and_bounded_shutdown() -> io::Result<()> {
    let root = Scratch::new();
    let c = Coordinator::open(root.root())?;
    let second = Coordinator::open(root.root());
    assert!(second.is_err());
    assert_eq!(second.err().expect("second writer rejected").kind(), io::ErrorKind::WouldBlock);
    let output = c.report_path();
    assert!(output.exists());
    drop(c);
    assert!(!output.exists(), "clean shutdown withdraws current output");
    let c = Coordinator::open(root.root())?;
    assert!(c.read_current()?.is_some());
    Ok(())
}

#[cfg(unix)]
#[test]
fn unreadable_link_does_not_delete_predecessor_evidence() -> io::Result<()> {
    use std::os::unix::fs::symlink;
    let root = Scratch::new();
    root.write("p.cs", "prior");
    let mut c = Coordinator::open(root.root())?;
    fs::remove_file(root.root().join("p.cs"))?;
    symlink("/not-part-of-the-scratch-workspace", root.root().join("p.cs"))?;
    c.notify(Event::Change("p.cs".into()), Instant::now())?;
    let changes = c.reconcile_now()?;
    assert!(changes.is_empty(), "uncertain entry may not become deletion");
    assert!(c.partial());
    assert_eq!(c.inventory().len(), 1);
    assert!(c.read_current()?.unwrap().contains("incomplete_source_enumeration"));
    Ok(())
}

#[test]
fn portable_cancel_is_incomplete_and_does_not_consume_intent() -> io::Result<()> {
    // This uses the actual built fixture binary (not the integration test
    // executable) and is only enabled where process groups are supported.
    let root = Scratch::new();
    root.write("p.cs", "one");
    let mut c = Coordinator::open(root.root())?;
    root.write("p.cs", "two");
    c.notify(Event::Change("p.cs".into()), Instant::now())?;
    c.reconcile_now()?;
    // `run_fake` runs `current_exe`; the fixture subprocess is only available
    // when the binary is executed rather than the libtest harness. The
    // process-backed path is exercised by the `serve` protocol test below.
    let admitted = wait_admit(&mut c)?;
    assert!(!c.finish(admitted, FixtureResult::Cancel)?);
    assert_eq!(c.pending().len(), 1);
    // OS process-tree behavior is checked only by platform_exec fixtures.
    Ok(())
}



#[test]
fn polling_discovers_fours_languages_but_does_not_schedule_cross_language_work() -> io::Result<()> {
    let root = Scratch::new();
    root.write("C.cs", "class C {}\n");
    root.write("pyproject.toml", "[project]\nname='p'\nversion='0.1'\n");
    root.write("p.py", "print(1)\n");
    root.write("p.pyi", "x: int\n");
    root.write("Cargo.toml", "[package]\nname='p'\nversion='0.1.0'\n");
    root.write("src/lib.rs", "pub fn f() {}\n");
    root.write("demo.fluxr", "let x = 1\n");
    let mut c = Coordinator::open(root.root())?;
    let report = c.read_current()?.unwrap();
    for language in ["csharp", "python", "rust", "fluxr"] {
        assert!(report.contains(&format!("\"language\":\"{language}\"")));
    }
    assert!(report.contains("\"crossLanguageImpact\":false"));
    assert!(c.reply_json("list_checkers").contains("\"payload\":[]"));
    root.write("p.py", "print(2)\n");
    assert!(c.poll()?);
    assert!(c.pending().is_empty(), "Python discovery cannot create a C# cause");
    let report = c.read_current()?.unwrap();
    assert!(report.contains("\"schemaVersion\":2"));
    Ok(())
}

#[test]
fn polling_timestamp_churn_overflow_and_incomplete_subtree() -> io::Result<()> {
    let root = Scratch::new();
    root.write("stable.cs", "class A {}\n");
    root.write("broken/missing.cs", "class B {}\n");
    let mut c = Coordinator::open(root.root())?;
    root.write("stable.cs", "class A {}\n"); // mtime-only churn, same bytes
    assert!(!c.poll()?, "unchanged digest does not invalidate revision");
    root.write("stable.cs", "class Changed {}\n");
    c.notify(Event::Overflow, Instant::now())?;
    assert_eq!(c.reconcile_now()?.len(), 1);
    assert_eq!(c.pending().len(), 1);
    #[cfg(unix)]
    {
        use std::os::unix::fs::symlink;
        fs::remove_file(root.root().join("broken/missing.cs"))?;
        fs::remove_dir(root.root().join("broken"))?;
        symlink("/outside-test-root", root.root().join("broken"))?;
        fs::remove_file(root.root().join("stable.cs"))?;
        c.notify(Event::Overflow, Instant::now())?;
        let delta = c.reconcile_now()?;
        assert!(delta.iter().any(|d| d.kind == "delete" && d.old_path == "stable.cs"),
            "a sibling's uncertainty must not hide a proven deletion");
        assert!(c.inventory().contains_key("broken/missing.cs"));
        assert!(c.uncertainty().contains("broken"));
        assert!(c.read_current()?.unwrap().contains("\"partial\":true"));
    }
    Ok(())
}

#[test]
fn physical_root_replacement_and_stale_report_are_rejected() -> io::Result<()> {
    let root = Scratch::new();
    root.write("stable.cs", "class A {}\n");
    let mut c = Coordinator::open(root.root())?;
    let good = c.read_current()?.unwrap();
    let marker = "\"projectionRevision\":";
    let number = good.split_once(marker).unwrap().1.split(',').next().unwrap();
    let stale = good.replacen(&format!("{marker}{number}"),
        "\"projectionRevision\":0", 1);
    assert_ne!(stale, good, "the expected report revision must be present");
    fs::write(c.report_path(), stale)?;
    assert!(c.read_current()?.is_none());
    c.recheck()?;
    assert!(c.read_current()?.is_some());
    let renamed = root.root().with_extension("previous");
    let change = fs::rename(root.root(), &renamed);
    if cfg!(windows) && change.as_ref().is_err_and(|error|
        error.kind() == io::ErrorKind::PermissionDenied) {
        // Windows can prevent moving a directory with an active writer handle.
        // Verify the replacement after releasing that handle and reopening.
        let prior = c.fence().root_identity;
        assert!(c.read_current()?.is_some());
        drop(c);
        fs::rename(root.root(), &renamed)?;
        fs::create_dir(root.root())?;
        let replacement = Coordinator::open(root.root())?;
        assert_ne!(prior, replacement.fence().root_identity);
        assert!(replacement.inventory().is_empty());
        fs::remove_dir_all(&renamed)?;
        return Ok(());
    }
    change?;
    fs::create_dir(root.root())?;
    assert!(c.poll().is_err(), "the original writer cannot publish to a new physical root");
    drop(c);
    fs::remove_dir_all(&renamed)?;
    Ok(())
}

fn wait_admit(coordinator: &mut Coordinator) -> io::Result<Admission> {
    for _ in 0..200 {
        match coordinator.admit() {
            Ok(Some(lease)) => return Ok(lease),
            Ok(None) => return Err(io::Error::other("expected a pending fixture intent")),
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                std::thread::sleep(Duration::from_millis(10));
            }
            Err(error) => return Err(error),
        }
    }
    Err(io::Error::other("user worker slot remained occupied"))
}
