//! These tests execute the fixture binary; kept separate from portable unit
//! tests so owner can run them only in an authorized Linux job/cgroup fixture.
#![cfg(target_os = "linux")]
use std::fs;
use std::io::{self, Write};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

#[test]
fn fixture_cli_envelopes_and_clean_shutdown() -> io::Result<()> {
    let nanos = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();
    let root = std::env::temp_dir().join(format!("sh-native-platform-{}-{nanos}", std::process::id()));
    fs::create_dir(&root)?;
    fs::write(root.join("p.cs"), "a")?;
    let mut child = Command::new(env!("CARGO_BIN_EXE_slophawk-native-coordinator"))
        .args(["serve", "--root", root.to_str().unwrap()])
        .stdin(Stdio::piped()).stdout(Stdio::piped()).spawn()?;
    let mut writer = child.stdin.take().unwrap();
    for _ in 0..100 {
        if root.join(".slophawk-native-coordinator/results.json").exists() { break; }
        std::thread::sleep(Duration::from_millis(10));
    }
    fs::write(root.join("p.cs"), "b")?;
    writer.write_all(b"hello\nstatus\nevent change p.cs\nrecheck\nrun cancel\nresults\nstop\n")?;
    drop(writer);
    let output = child.wait_with_output()?;
    assert!(output.status.success());
    let lines = String::from_utf8(output.stdout).unwrap();
    assert!(lines.contains("\"wireVersion\":1"));
    assert!(lines.contains("\"reportSchemaVersion\":2"));
    assert!(lines.contains("\"stopping\":true"));
    assert!(lines.contains("\"outcome\":\"cancelled\""));
    assert!(!root.join(".slophawk-native-coordinator/results.json").exists());
    assert!(!root.join(".slophawk/results.json").exists());
    fs::remove_dir_all(&root)?;
    Ok(())
}
