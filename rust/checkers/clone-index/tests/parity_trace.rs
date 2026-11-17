//! Golden trace captured using the exact pinned C# source, so Rust tests do
//! not require the .NET SDK. Re-capture after independently verifying changes.
use std::process::Command;

#[test]
fn matches_pinned_csharp_full_trace_including_mutations_and_caps() {
    let output = Command::new(env!("CARGO_BIN_EXE_clone-parity"))
        .arg(concat!(env!("CARGO_MANIFEST_DIR"), "/fixtures/parity.tsv"))
        .output()
        .expect("run Rust parity binary");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        std::str::from_utf8(&output.stdout).unwrap(),
        include_str!("../fixtures/parity.expected.tsv")
    );
}
