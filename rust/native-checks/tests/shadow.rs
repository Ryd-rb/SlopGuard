use serde_json::{Value, json};
use slophawk_native_checks::{bounded_report, shadow};
use slophawk_native_checks::base64;
use slophawk_native_checks::contract::{ScopeRequest, Status, hex_digest};
use std::fs;
use std::process::Command;
use std::sync::atomic::AtomicBool;
#[cfg(target_os = "linux")]
use std::sync::atomic::Ordering;
#[cfg(target_os = "linux")]
use std::sync::Arc;
#[cfg(target_os = "linux")]
use std::time::{Duration, Instant};

fn root() -> &'static str {
    if cfg!(windows) { "C:/voda/project" } else { "/w/project" }
}
fn outside_root() -> &'static str {
    if cfg!(windows) { "C:/outside" } else { "/outside" }
}
fn reference_pack() -> &'static str {
    if cfg!(windows) { "C:/voda/ref/net10.0" } else { "/tmp/reference-pack/net10.0" }
}
fn source_bytes(lang: &str, id: &str, bytes: &[u8]) -> Value {
    let ext = match lang { "python" => "py", "rust" => "rs", "fluxr" => "fluxr", _ => "cs" };
    let relative = format!("src/{id}.{ext}");
    json!({ "language":lang, "documentId":id, "logicalPath":relative,
        "physicalPath":format!("{}/src/{id}.{ext}",root()), "provenance":"normal",
        "bytesBase64":base64::encode(bytes),
        "sha256":hex_digest(bytes), "byteMapping":"exactUtf8" })
}
fn source(lang: &str, id: &str, content: &str) -> Value {
    source_bytes(lang, id, content.as_bytes())
}
fn request(sources: Vec<Value>) -> Value {
    json!({ "version":1, "workspaceEpoch":"epoch-2", "committedRevision":"rev-123",
        "workspaceRoot":root(), "linkedRoots":[],
        "context": { "key":"fixture|target", "pythonMinor":12, "rustEdition":"2024",
            "fluxrModule":"fixture@checked-v1", "targetFramework":"net10.0",
            "assemblyName":"Fixture", "languageVersion":"CSharp14",
            "referencePack":reference_pack(), "referenceClosureComplete":true,
            "sourceClosureComplete":true },
        "sources":sources,
        "limits":{"minCloneTokens":4, "maxTokensPerBlock":4096} })
}
fn run(value: Value) -> slophawk_native_checks::contract::Report {
    let req: ScopeRequest = serde_json::from_value(value).unwrap();
    shadow(req, &AtomicBool::new(false)).unwrap()
}
fn matching(report: &slophawk_native_checks::contract::Report, class: &str) -> usize {
    report.pairs.iter().filter(|p| p.classification == class).count()
}

#[test]
fn python_exact_renamed_and_near_miss_are_separate() {
    let exact = "def total():\n    running_total = 1\n    return running_total + 2\n";
    let renamed = "def total():\n    answer_value = 1\n    return answer_value + 2\n";
    let near = "def total():\n    answer_value = 1\n    return answer_value + 3\n";
    let p = run(request(vec![source("python","a",exact),source("python","b",exact),source("python","c",renamed),source("python","d",near)]));
    assert!(matching(&p, "exact") >= 1, "{:#?}",p.pairs);
    assert!(matching(&p, "renamed_proven_local") >= 1, "{:#?}",p.pairs);
    assert!(!p.pairs.iter().any(|pair| pair.left_document == "d" || pair.right_document == "d"));
    assert_eq!(p.worker_invocations, 0);
    assert!(p.documents.iter().all(|d| d.rules.iter().any(|r| r.id.ends_with("/near") && r.status==Status::Unsupported)));
    assert!(p.pairs.iter().all(|pair| pair.pair_id.starts_with("sg10.python-layout-clones.v2:")));
    assert!(p.documents.iter().all(|d| d.rules.iter().any(|r|
        r.id == slophawk_native_checks::contract::PYTHON_EXACT && r.status == Status::Complete)));
}

#[test]
fn python_layout_rejects_outdented_returns_in_exact_and_renamed_candidates() {
    let a = "def f(flag):\n    value = 0\n    if flag:\n        value = 1\n        return value\n";
    let b = "def f(flag):\n    value = 0\n    if flag:\n        value = 1\n    return value\n";
    let renamed = "def f(flag):\n    result = 0\n    if flag:\n        result = 1\n    return result\n";
    for other in [b, renamed] {
        let report = run(request(vec![source("python", "a", a), source("python", "b", other)]));
        assert!(report.pairs.is_empty(), "{:#?}", report.pairs);
        assert_eq!(report.worker_invocations, 0);
        assert!(report.documents.iter().all(|d| d.rules.iter().any(|r|
            r.id == slophawk_native_checks::contract::PYTHON_RENAMED && r.status == Status::Complete)));
    }
}

#[test]
fn python_layout_rejects_changed_logical_newlines_without_changed_nesting() {
    let a = "def f():\n    value = 1\n    + 2\n    return value\n";
    let b = "def f():\n    value = 1 + 2\n    return value\n";
    let report = run(request(vec![source("python", "a", a), source("python", "b", b)]));
    assert!(report.pairs.is_empty(), "{:#?}", report.pairs);
}

#[test]
fn python_layout_keeps_formatting_clones_and_original_byte_spans() {
    let a = "def f():\n    value = (1 + 2)\n    return value\n";
    let b = b"\xef\xbb\xbfdef f():\r\n\t# comment\r\n\tvalue = (\r\n\t    1 + 2\r\n\t)\r\n\r\n\treturn value";
    let report = run(request(vec![source("python", "a", a), source_bytes("python", "b", b)]));
    assert_eq!(matching(&report, "exact"), 1, "{:#?}", report.pairs);
    let pair = &report.pairs[0];
    assert_eq!(pair.left_bytes, [0, a.trim_end().len()]);
    assert_eq!(pair.right_bytes, [3, b.len()]);
}

#[test]
fn python_layout_keeps_query_caps_visible_after_rejecting_a_candidate() {
    let a = "def f(flag):\n    value = 0\n    if flag:\n        value = 1\n        return value\n";
    let b = "def f(flag):\n    value = 0\n    if flag:\n        value = 1\n    return value\n";
    let mut input = request(vec![source("python", "a", a), source("python", "b", b), source("python", "c", a)]);
    input["limits"]["maxOutputPairs"] = json!(1);
    let report = run(input);
    // The lexical a/b candidate consumes the cap before layout rejects it.
    // The valid a/c pair is deliberately not recovered by an unbounded retry.
    assert!(report.pairs.is_empty(), "{:#?}", report.pairs);
    assert_eq!(report.status, Status::Partial);
    assert!(report.gaps.iter().any(|g| g.reason.contains("OutputPairLimit")));
}

#[test]
fn python_layout_failure_rules_use_the_new_identity() {
    let report = run(request(vec![source("python", "broken", "def broken(")]));
    assert_eq!(report.status, Status::Failed);
    assert!(report.pairs.is_empty());
    assert!(report.documents[0].rules.iter().any(|r|
        r.id == slophawk_native_checks::contract::PYTHON_EXACT && r.status == Status::Unsupported));
    assert!(!report.documents[0].rules.iter().any(|r| r.id == slophawk_native_checks::contract::EXACT));
}

#[test]
fn rust_simple_local_renaming_and_near_miss() {
    let a = "fn f() { let variable = 1; variable + 2; }";
    let b = "fn f() { let result = 1; result + 2; }";
    let c = "fn f() { let result = 1; result + 3; }";
    let p = run(request(vec![source("rust","r1",a),source("rust","r2",a),source("rust","r3",b),source("rust","r4",c)]));
    assert!(matching(&p, "exact") >= 1, "{:#?}",p.pairs);
    assert!(matching(&p, "renamed_proven_local") >= 1, "{:#?}",p.pairs);
    assert!(p.pairs.iter().all(|pair| pair.pair_id.starts_with("sg10.token-clones.v1:")));
    assert!(!p.pairs.iter().any(|pair| pair.left_document == "r4" || pair.right_document == "r4"));
}

#[test]
fn fluxr_native_free_checked_unit_exact_and_near_miss() {
    let a = "fluxr 1\npub run 3 x int factor int offset int int ret add mul x factor offset\n";
    let b = "fluxr 1\npub run 3 x int factor int offset int int ret add mul x factor factor\n";
    let p = run(request(vec![source("fluxr","f1",a),source("fluxr","f2",a),source("fluxr","f3",b)]));
    assert_eq!(matching(&p, "exact"), 1, "{:#?}",p.pairs);
    assert_eq!(matching(&p, "renamed_proven_local"), 0);
    assert!(p.documents.iter().all(|d| d.gaps.iter().any(|g| g.reason.starts_with("fluxr:missing_declaration_range"))));
    assert!(p.documents.iter().all(|d| d.status == Status::Partial));
}

#[test]
fn fluxr_native_items_are_opaque_and_preclude_document_matching() {
    // Pinned FluxR checked provider accepts this native item but does not parse it as Rust.
    let mixed = "fluxr 1\nrust 2\nβ\npub run 0 int ret 1\n";
    let report = run(request(vec![source("fluxr","f1",mixed),source("fluxr","f2",mixed)]));
    assert!(report.pairs.is_empty());
    assert!(report.documents.iter().all(|d| d.status == Status::Partial));
    assert!(report.documents.iter().all(|d| d.gaps.iter().any(|g|
        g.reason.contains("opaque_native_item") && g.span.is_some())));
    assert!(report.documents.iter().all(|d| d.rules.iter().any(|r|
        r.id.ends_with("/exact") && r.status == Status::Unsupported)));
}

#[test]
fn rust_macro_frontier_never_qualifies_a_clean_block() {
    let macro_body = "fn f() { let a = 1; println!(\"{}\", a); }";
    let report = run(request(vec![source("rust","r1",macro_body),source("rust","r2",macro_body)]));
    assert!(report.pairs.is_empty());
    assert!(report.documents.iter().all(|d| d.status == Status::Partial));
    assert!(report.documents.iter().any(|d| d.gaps.iter().any(|g| g.reason.contains("MacroExpansion"))));
}

#[test]
fn untrusted_byte_mapping_cannot_produce_pairs() {
    let mut a = source("python","a","def a():\n    return 1\n");
    let mut b = source("python","b","def a():\n    return 1\n");
    a["byteMapping"] = json!("unavailable");
    b["byteMapping"] = json!("unavailable");
    let p = run(request(vec![a,b]));
    assert!(p.pairs.is_empty());
    assert!(p.documents.iter().all(|d| d.status == Status::Partial && d.blocks_indexed == 0));
    assert!(p.documents.iter().all(|d| d.rules.iter().any(|r|
        r.id == slophawk_native_checks::contract::PYTHON_EXACT && r.status == Status::Unsupported)));
}

#[test]
fn source_hash_and_linked_grants_checked_before_providers() {
    let mut bad = source("rust","s","fn a() {} ");
    bad["sha256"] = json!("0".repeat(64));
    let parsed: ScopeRequest = serde_json::from_value(request(vec![bad])).unwrap();
    assert!(shadow(parsed,&AtomicBool::new(false)).unwrap_err().contains("SHA-256"));
    let mut linked = source("python","a","x=1\n");
    linked["provenance"] = json!("linked");
    linked["physicalPath"] = json!(format!("{}/a.py", outside_root()));
    let parsed: ScopeRequest = serde_json::from_value(request(vec![linked.clone()])).unwrap();
    assert!(shadow(parsed,&AtomicBool::new(false)).unwrap_err().contains("grant"));
    let mut permitted = request(vec![linked]);
    permitted["linkedRoots"] = json!([outside_root()]);
    assert!(shadow(serde_json::from_value(permitted).unwrap(),&AtomicBool::new(false)).is_ok());
}

#[test]
fn worker_crash_results_in_explicit_failure_and_no_pairs() {
    #[cfg(target_os = "linux")] {
        if std::env::var_os("VODA_TEST_SYSTEMD_SCOPE").is_none() { return; }
        let mut input = request(vec![source("csharp","c","class A {}")]);
        input["worker"] = json!({"dotnet":"/bin/false","assembly":"/tmp/worker.dll"});
        let p = run(input);
        assert_eq!(p.worker_invocations,1);
        assert!(p.pairs.is_empty());
        assert_eq!(p.documents[0].status,Status::Failed);
    }
}

#[test]
fn worker_deadline_is_enforced() {
    #[cfg(target_os = "linux")] {
        if std::env::var_os("VODA_TEST_SYSTEMD_SCOPE").is_none() { return; }
        let name = std::env::temp_dir().join(format!("voda-sleep-worker-{}-{}.sh",std::process::id(),"deadline"));
        fs::write(&name, "sleep 20\n").unwrap();
        let mut input = request(vec![source("csharp","c","class A {}")]);
        input["worker"] = json!({"dotnet":"/bin/sh","assembly":name.to_str().unwrap()});
        input["limits"] = json!({"workerDeadlineMs":100});
        let p = run(input);
        fs::remove_file(name).unwrap();
        assert!(p.pairs.is_empty());
        assert_eq!(p.status, Status::Failed);
        assert!(p.documents[0].gaps.iter().any(|g|g.reason.contains("deadline") ||
            g.reason.contains("cgroup") || g.reason.contains("worker")));
    }
}

#[test]
fn deterministic_permutation_and_non_csharp_never_starts_worker() {
    let a = source("python","a","def f():\n    k = 1\n    return k + 2\n");
    let b = source("python","b","def f():\n    k = 1\n    return k + 2\n");
    let mut left = request(vec![a.clone(),b.clone()]);
    let mut right = request(vec![b,a]);
    left["worker"] = json!({"dotnet":"/never/spawn","assembly":"/never/spawn.dll"});
    right["worker"] = left["worker"].clone();
    let l = run(left); let r = run(right);
    assert_eq!(l.worker_invocations,0);
    assert_eq!(r.worker_invocations,0);
    let mut lv = serde_json::to_value(l).unwrap();
    let mut rv = serde_json::to_value(r).unwrap();
    lv.as_object_mut().unwrap().remove("elapsedMs");
    rv.as_object_mut().unwrap().remove("elapsedMs");
    assert_eq!(lv,rv);
}

#[test]
fn standalone_cli_runs_with_typed_source_request() {
    let path = std::env::temp_dir().join(format!("varo-cli-{}.json",std::process::id()));
    fs::write(&path, serde_json::to_vec(&request(vec![source("python","a","def a():\n    return 1\n")])).unwrap()).unwrap();
    let process = Command::new(env!("CARGO_BIN_EXE_slophawk-native-checks"))
        .args(["shadow", "--request"]).arg(&path).output().unwrap();
    fs::remove_file(path).unwrap();
    assert!(process.status.success(), "{}", String::from_utf8_lossy(&process.stderr));
    let output: Value = serde_json::from_slice(&process.stdout).unwrap();
    assert_eq!(output["schema"],"slophawk.native-shadow/1");
    assert_eq!(output["workerInvocations"],0);
    assert_eq!(output["documents"].as_array().unwrap().len(),1);
}

#[test]
fn real_csharp_worker_source_unit_parity_when_explicitly_admitted() {
    // Set all three paths to a built, verified .NET worker, dotnet host and net10.0 reference pack.
    // The pinned integration environment sets them; local Rust-only workflows may opt out.
    let (Ok(host),Ok(dll),Ok(pack)) = (std::env::var("VARO_DOTNET"),std::env::var("VARO_WORKER_DLL"),std::env::var("VARO_REFERENCE_PACK")) else {
        assert!(std::env::var("VARO_REQUIRE_REAL_WORKER").is_err(), "real C# worker requested but not configured");
        return;
    };
    let source_a = "partial class A { static int F(int x) => x + 1; }";
    let source_b = "partial class A { static int F(int x) => x + 2; }";
    let mut input = request(vec![source("csharp","a",source_a),source("csharp","b",source_a),source("csharp","c",source_b)]);
    input["worker"] = json!({"dotnet":host,"assembly":dll});
    input["context"]["referencePack"] = json!(pack);
    let report = run(input);
    assert_eq!(report.worker_invocations, 3);
    assert!(matching(&report,"exact") >= 1, "{:?}", report.documents);
    let pair = report.pairs.iter().find(|p| p.classification == "exact").unwrap();
    assert_eq!(pair.left_bytes, [0, source_a.len()]);
    assert_eq!(pair.right_bytes, [0, source_a.len()]);
    assert_eq!(pair.left_utf16, [0, source_a.encode_utf16().count() as i32]);
    assert_eq!(matching(&report,"renamed_proven_local"),0);
    assert!(!report.pairs.iter().any(|pair| pair.left_document=="c"||pair.right_document=="c"));
    assert!(report.documents.iter().all(|d| d.status != Status::Failed));
}

#[test]
fn bom_unicode_crlf_source_bytes_stay_distinct_from_utf16_coordinates() {
    // The original three BOM bytes are part of the recorded whole-unit byte span.
    // UTF-16 offsets exclude the BOM and count the supplementary rune as two units.
    let text = "class Café { string Smile = \"🙂\";\r\n void Go(){} }";
    let mut source_with_bom = vec![0xef, 0xbb, 0xbf];
    source_with_bom.extend_from_slice(text.as_bytes());
    let p = run(request(vec![source_bytes("python", "a", b"def f():\n    return 1\n"),
        source_bytes("python", "b", b"def f():\n    return 1\n")]));
    assert!(p.pairs.iter().any(|pair| pair.left_bytes[1] <= b"def f():\n    return 1\n".len()));
    // A genuine Roslyn parity test using the BOM, CRLF and both coordinate
    // systems is intentionally conditional on a built, scoped real worker.
    let (Ok(host), Ok(dll), Ok(pack)) = (std::env::var("VARO_DOTNET"),
        std::env::var("VARO_WORKER_DLL"), std::env::var("VARO_REFERENCE_PACK")) else {
        assert!(std::env::var("VARO_REQUIRE_REAL_WORKER").is_err(), "real-worker parity inputs not configured");
        return;
    };
    let mut input = request(vec![source_bytes("csharp", "c1", &source_with_bom),
        source_bytes("csharp", "c2", &source_with_bom)]);
    input["worker"] = json!({"dotnet":host,"assembly":dll});
    input["context"]["referencePack"] = json!(pack);
    let out = run(input);
    let pair = out.pairs.iter().find(|p| p.classification == "exact")
        .unwrap_or_else(|| panic!("whole-unit C# match missing: {:?}", out.documents));
    assert_eq!(pair.left_bytes, [0, source_with_bom.len()]);
    assert_eq!(pair.right_bytes, [0, source_with_bom.len()]);
    assert_eq!(pair.left_utf16, [0, text.encode_utf16().count() as i32]);
    assert_eq!(pair.right_utf16, [0, text.encode_utf16().count() as i32]);
}

#[test]
fn a_failed_peer_discards_all_staged_pairs() {
    #[cfg(target_os = "linux")]
    {
        let mut input = request(vec![source("python","a","def f():\n    return 1\n"),
            source("python","b","def f():\n    return 1\n"),
            source("csharp","bad","class C{}")]);
        input["worker"] = json!({"dotnet":"/definitely/unavailable/dotnet","assembly":"/unavailable/worker.dll"});
        let report = run(input);
        assert_eq!(report.status, Status::Failed);
        assert!(report.pairs.is_empty());
        assert!(report.documents.iter().filter(|d| d.language == slophawk_native_checks::contract::Language::Python)
            .all(|d| d.blocks_indexed == 0 && d.gaps.iter().any(|g| g.reason.contains("peer_failed"))));
        if std::env::var_os("VODA_TEST_SYSTEMD_SCOPE").is_some() {
            slophawk_worker_scope::recover(Duration::from_secs(3)).unwrap();
        }
    }
}

#[test]
fn linux_scoped_descendant_and_refused_stdin_cannot_hold_the_job_forever() {
    #[cfg(target_os = "linux")]
    {
        if std::env::var_os("VODA_TEST_SYSTEMD_SCOPE").is_none() { return; }
        let path = std::env::temp_dir().join(format!("voda-descendant-{}.sh", std::process::id()));
        // Deliberately never reads its request and leaves an inherited output
        // descriptor with a child. Both paths must terminate through the unit.
        fs::write(&path, "sleep 20 &\nsleep 20\n").unwrap();
        let mut input = request(vec![source("csharp","c","class A{}")]);
        input["worker"] = json!({"dotnet":"/bin/sh","assembly":path.to_str().unwrap()});
        input["limits"] = json!({"workerDeadlineMs":250});
        let started = Instant::now();
        let report = run(input);
        let _ = fs::remove_file(path);
        assert!(started.elapsed() < Duration::from_secs(8), "bounded drain/kill required");
        assert_eq!(report.status, Status::Failed);
        assert!(report.pairs.is_empty());
        assert!(report.documents[0].gaps.iter().any(|g| g.reason.contains("deadline") ||
            g.reason.contains("cgroup") || g.reason.contains("drain")), "{:?}", report.documents);
    }
}

#[test]
fn cancellation_before_and_during_worker_admission_revokes_publication() {
    let cancelled = AtomicBool::new(true);
    let parsed: ScopeRequest = serde_json::from_value(request(vec![source("python","a","x=1\n")])).unwrap();
    assert!(shadow(parsed, &cancelled).unwrap_err().contains("cancelled"));
    #[cfg(target_os = "linux")]
    {
        if std::env::var_os("VODA_TEST_SYSTEMD_SCOPE").is_none() { return; }
        let path = std::env::temp_dir().join(format!("voda-cancel-{}.sh", std::process::id()));
        fs::write(&path,"sleep 20\n").unwrap();
        let mut input = request(vec![source("csharp","c","class A{}")]);
        input["worker"] = json!({"dotnet":"/bin/sh","assembly":path.to_str().unwrap()});
        let token = Arc::new(AtomicBool::new(false));
        let trigger = token.clone();
        let task = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(50)); trigger.store(true, Ordering::SeqCst);
        });
        let started = Instant::now();
        let output = shadow(serde_json::from_value(input).unwrap(), &token);
        task.join().unwrap();
        let _ = fs::remove_file(path);
        assert!(started.elapsed() < Duration::from_secs(8));
        match output {
            Ok(report) => { assert!(report.pairs.is_empty()); assert_eq!(report.status, Status::Failed); }
            Err(error) => assert!(error.contains("cancelled") || error.contains("deadline")),
        }
    }
}

#[test]
fn report_size_cap_never_silently_elides_scope_gaps() {
    let p = run(request(vec![source("python","a","def a():\n    return 1\n")]));
    let bytes = bounded_report(p).unwrap();
    assert!(bytes.len() < 1024*1024);
}

#[test]
fn python_empty_token_sources_preserve_other_language_and_python_pairs() {
    let cases: &[(&str, &[u8])] = &[
        ("py", b" \r\n"),
        ("py", b"\t\n \n"),
        ("py", b"\xef\xbb\xbf"),
        ("py", b"\xef\xbb\xbf \r\n"),
        ("py", b"# package marker\n"),
        ("pyi", b" \r\n"),
    ];
    for (extension, content) in cases {
        let mut empty = source_bytes("python", "empty", content);
        empty["logicalPath"] = json!(format!("src/empty.{extension}"));
        empty["physicalPath"] = json!(format!("{}/src/empty.{extension}", root()));
        let mut sources = vec![
            empty,
            source("python", "p1", "def f():\n    return 1\n"),
            source("python", "p2", "def f():\n    return 1\n"),
            source("rust", "r1", "fn f() { 1 + 2; }"),
            source("rust", "r2", "fn f() { 1 + 2; }"),
        ];
        let report = run(request(sources.clone()));
        // Independently specified: one same-language exact pair for each pair
        // of identical functions, and no cross-language or empty-source pair.
        assert_eq!(report.status, Status::Partial);
        assert_eq!(report.pairs.len(), 2, "{content:?}: {:?}", report.documents);
        assert!(report.pairs.iter().all(|pair| pair.classification == "exact"));
        assert!(report.pairs.iter().any(|pair|
            pair.left_document == "p1" && pair.right_document == "p2"));
        assert!(report.pairs.iter().any(|pair|
            pair.left_document == "r1" && pair.right_document == "r2"));
        let empty_report = report.documents.iter().find(|d| d.document_id == "empty").unwrap();
        assert_eq!(empty_report.status, Status::Complete);
        assert_eq!((empty_report.blocks_indexed, empty_report.tokens_indexed), (0, 0));
        assert!(empty_report.gaps.is_empty());
        assert!(empty_report.rules.iter().any(|r|
            r.id.ends_with("/exact") && r.status == Status::Complete));
        assert!(empty_report.rules.iter().filter(|r| !r.id.ends_with("/exact"))
            .all(|r| r.status == Status::Unsupported));
        assert!(!report.gaps.iter().any(|g| g.reason.contains("peer_failed")));
        assert_eq!(report.worker_invocations, 0);
        sources.reverse();
        let reversed = run(request(sources));
        assert_eq!(serde_json::to_value(&report.pairs).unwrap(),
            serde_json::to_value(&reversed.pairs).unwrap());
    }
}

#[test]
fn python_empty_only_is_an_explicit_empty_exact_scope() {
    let report = run(request(vec![source("python", "empty", " \r\n")]));
    assert_eq!(report.status, Status::Partial); // Renamed and near remain unsupported.
    assert!(report.pairs.is_empty());
    assert!(report.gaps.is_empty());
    let document = &report.documents[0];
    assert_eq!(document.status, Status::Complete);
    assert_eq!((document.blocks_indexed, document.tokens_indexed), (0, 0));
    assert!(document.rules.iter().any(|r| r.id.ends_with("/exact")
        && r.status == Status::Complete && r.reason.contains("no substantive clone tokens")));
}

#[test]
fn python_empty_token_handling_does_not_hide_failed_or_unsupported_inputs() {
    let cases: &[(&[u8], u8, Status)] = &[
        (b"def broken(:\n", 12, Status::Failed),
        (b"\xff", 12, Status::Failed),
        (b"# coding: latin-1\n", 12, Status::Unsupported),
        (b" \n", 6, Status::Unsupported),
    ];
    for (content, minor, expected) in cases {
        let mut input = request(vec![
            source_bytes("python", "subject", content),
            source("rust", "r1", "fn f() { 1 + 2; }"),
            source("rust", "r2", "fn f() { 1 + 2; }"),
        ]);
        input["context"]["pythonMinor"] = json!(minor);
        let report = run(input);
        let subject = report.documents.iter().find(|d| d.document_id == "subject").unwrap();
        assert_eq!(subject.status, *expected);
        assert!(!subject.gaps.is_empty());
        if *expected == Status::Failed {
            assert_eq!(report.status, Status::Failed);
            assert!(report.pairs.is_empty());
        } else {
            assert_eq!(report.status, Status::Partial);
            assert_eq!(report.pairs.len(), 1);
        }
    }
}

#[test]
fn python_empty_token_handling_preserves_resource_failure() {
    // At least 66,000 substantive tokens exceed the provider's 65,536-token
    // quota, while 88,000 source bytes remain below the admission byte cap.
    let over_budget = "x=1\n".repeat(22_000);
    let report = run(request(vec![
        source("python", "over_budget", &over_budget),
        source("rust", "r1", "fn f() { 1 + 2; }"),
        source("rust", "r2", "fn f() { 1 + 2; }"),
    ]));
    assert_eq!(report.status, Status::Failed);
    assert!(report.pairs.is_empty());
    assert!(report.gaps.iter().any(|g| g.reason.contains("token quota")));
}

#[test]
fn python_empty_token_handling_preserves_byte_admission_and_mapping() {
    let zero = request(vec![source_bytes("python", "zero", b"")]);
    let parsed: ScopeRequest = serde_json::from_value(zero).unwrap();
    assert!(shadow(parsed, &AtomicBool::new(false)).unwrap_err().contains("per-source byte cap"));

    let mut unmapped = source("python", "empty", " \r\n");
    unmapped["byteMapping"] = json!("unavailable");
    let report = run(request(vec![unmapped]));
    assert_eq!(report.documents[0].status, Status::Partial);
    assert!(report.documents[0].gaps.iter().any(|g| g.reason.contains("caller_declared_unmapped")));
    assert!(report.documents[0].rules.iter().all(|r| r.status == Status::Unsupported));
}

#[test]
fn python_string_literal_remains_a_substantive_token() {
    let report = run(request(vec![source("python", "docstring", "\"documentation\"\n")]));
    assert_eq!(report.documents[0].tokens_indexed, 1);
    assert_eq!(report.documents[0].blocks_indexed, 1);
    assert!(report.pairs.is_empty()); // One token is below the admitted minimum.
}
