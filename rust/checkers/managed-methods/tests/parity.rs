//! Source-authored differential tests. On the integration host, compile the
//! C# fixture twice, then the pinned-source C# reference harness. This test
//! compares byte-for-byte default System.Text.Json output, not a synthesized
//! golden JSON claim from an unverified previous proposal.
use slophawk_managed_methods::{ErrorKind, compare, differences_json,
    integer_literals_for_names, integer_literals_for_operations, literals_json};
use std::{env, ffi::OsString, fs, process::Command};

struct Harness {
    candidate: OsString,
    reference: OsString,
    oracle_dll: OsString,
    dotnet: OsString,
}
fn harness() -> Option<Harness> {
    let left = env::var_os("SLOPHAWK_PARITY_CANDIDATE");
    let right = env::var_os("SLOPHAWK_PARITY_REFERENCE");
    let oracle = env::var_os("SLOPHAWK_PARITY_CSHARP_DLL");
    if left.is_none() && right.is_none() && oracle.is_none() {
        eprintln!("SKIP source-built differential tests: set all three SLOPHAWK_PARITY_* paths");
        return None;
    }
    Some(Harness {
        candidate: left.expect("set SLOPHAWK_PARITY_CANDIDATE"),
        reference: right.expect("set SLOPHAWK_PARITY_REFERENCE"),
        oracle_dll: oracle.expect("set SLOPHAWK_PARITY_CSHARP_DLL"),
        dotnet: env::var_os("SLOPHAWK_PARITY_DOTNET").unwrap_or_else(|| "dotnet".into()),
    })
}
fn oracle(h: &Harness, command: &str, file: &OsString, rest: &[&str]) -> String {
    let output = Command::new(&h.dotnet).arg(&h.oracle_dll).arg(command).arg(file)
        .args(rest).output().expect("run source-built C# oracle");
    assert!(output.status.success(), "C# reference failure: {}",
        String::from_utf8_lossy(&output.stderr));
    String::from_utf8(output.stdout).expect("C# reference UTF-8 JSON")
}
fn compare_oracle(h: &Harness) -> String {
    let output = Command::new(&h.dotnet).arg(&h.oracle_dll).arg("compare")
        .arg(&h.candidate).arg(&h.reference).output().expect("run C# comparator");
    assert!(output.status.success(), "C# reference failure: {}",
        String::from_utf8_lossy(&output.stderr));
    String::from_utf8(output.stdout).expect("C# reference UTF-8 JSON")
}
#[test]
fn comparison_matches_actual_pinned_csharp_json() {
    let Some(h) = harness() else { return; };
    let candidate = fs::read(&h.candidate).expect("candidate fixture DLL");
    let reference = fs::read(&h.reference).expect("reference fixture DLL");
    let differences = compare(&candidate, &reference).unwrap();
    let actual = differences_json(&differences).unwrap();
    assert_eq!(actual, compare_oracle(&h).trim_end());
    assert_eq!(differences_json(&compare(&candidate, &candidate).unwrap()).unwrap(), "[]");
    // Explicitly assert the normalized *IL operand*, not merely JSON validity.
    // These are the quotation bytes produced by default System.Text.Json.
    let expected = "ldstr \"\\u003C\\u003E\\u0026\\u0027\\u002B\\u0060\\u0022\\\\\\n\\r\\t\\b\\f\\u0001\\u00E9\\uD83D\\uDE00\\u2028 C\"";
    let string_diff = differences.iter().find(|d| d.name == "StringOperand")
        .expect("changed StringOperand method must be present");
    assert!(string_diff.candidate.iter().any(|il| il == expected),
        "unexpected exact default System.Text.Json string operand: {:?}", string_diff.candidate);
    assert!(differences.iter().any(|d| d.name == "Offset" && d.method.contains("(Int32):Int32")));
    assert!(differences.iter().any(|d| d.name == "OffsetLocal") ||
        differences.iter().any(|d| d.name.starts_with("<OffsetLocal>")));
    assert!(differences.iter().any(|d| d.name == "Branches"));
    assert!(differences.iter().any(|d| d.name == "CatchAndFinally"));
}
#[test]
fn literal_modes_match_pinned_csharp_reference() {
    let Some(h) = harness() else { return; };
    let candidate = fs::read(&h.candidate).expect("candidate fixture DLL");
    let names = ["Offset", "Signed", "OffsetLocal", "Lambda", "NestedMethod", "MinimumLong"];
    let wanted = names.map(|name| name.to_owned());
    let actual = literals_json(&integer_literals_for_names(&candidate, &wanted).unwrap()).unwrap();
    assert_eq!(actual, oracle(&h, "names", &h.candidate, &names).trim_end());

    let operations = [("Offset", 1), ("NotPresent", 1), ("Signed", 0), ("NestedMethod", 1)];
    let wanted = operations.map(|(name, arity)| (name.to_owned(), arity));
    let actual = literals_json(&integer_literals_for_operations(&candidate, &wanted).unwrap()).unwrap();
    assert_eq!(actual, oracle(&h, "operations", &h.candidate,
        &["Offset:1", "NotPresent:1", "Signed:0", "NestedMethod:1"]).trim_end());
}
#[test]
fn malformed_real_assembly_is_an_error_not_a_diff() {
    let Some(h) = harness() else { return; };
    let candidate = fs::read(h.candidate).expect("candidate fixture DLL");
    let reference = fs::read(h.reference).expect("reference fixture DLL");
    let mut mutated = candidate.clone();
    let start = mutated.windows(4).position(|w| w == b"BSJB").expect("metadata signature");
    mutated[start] ^= 0xff;
    assert_eq!(compare(&mutated, &reference).unwrap_err().kind, ErrorKind::Malformed);
    let short = &candidate[..candidate.len() / 2];
    assert!(compare(short, &reference).is_err());
}
