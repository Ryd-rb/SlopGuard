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
}

#[test]
fn rust_simple_local_renaming_and_near_miss() {
    let a = "fn f() { let variable = 1; variable + 2; }";
    let b = "fn f() { let result = 1; result + 2; }";
    let c = "fn f() { let result = 1; result + 3; }";
    let p = run(request(vec![source("rust","r1",a),source("rust","r2",a),source("rust","r3",b),source("rust","r4",c)]));
    assert!(matching(&p, "exact") >= 1, "{:#?}",p.pairs);
    assert!(matching(&p, "renamed_proven_local") >= 1, "{:#?}",p.pairs);
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
    assert!(matching(&report,"exact") >= 1);
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
    let pair = out.pairs.iter().find(|p| p.classification == "exact").expect("whole-unit C# match");
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
            g.reason.contains("cgroup") || g.reason.contains("drain")));
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
