use fluxr::structured::Limits;
use slophawk_fluxr_checked_context::{
    BoundOperation, DeclaredModule, ExecutionMode, Outcome, RUST_TOOLCHAIN, Request, ScalarKind,
    SourceCodec, WorkerProfile, check, observe, replay_ordered, sha256_file,
};
use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Duration;

static NEXT: AtomicU64 = AtomicU64::new(0);
const AFFINE: &str = concat!(
    "fluxr 1\n",
    "pub run 3 x int factor int offset int int ret add mul x factor offset\n"
);
const BOOLEAN: &str = concat!(
    "fluxr 1\n",
    "pub run 3 allowed bool blocked bool score int bool ",
    "ret and and allowed not blocked or le 10 score eq score -1\n"
);
const UNSIGNED: &str = concat!(
    "fluxr 1\n",
    "pub run 3 x uint factor uint offset uint uint ret add mul x factor offset\n"
);
const INPUT_ADJUST: &str = concat!(
    "fluxr 1\ninputs 2 enabled bool base int\n",
    "pub run 1 delta int int do 0 1\n",
    "  if input enabled 1 ret add input base delta 1 ret sub input base delta\n"
);

struct Sandbox(PathBuf);

impl Sandbox {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "slophawk-fluxr-checked-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).unwrap();
        Self(path)
    }

    fn module(&self, module_id: &str, source: &str) -> DeclaredModule {
        let path = self.0.join(format!("{module_id}.fluxr"));
        fs::write(&path, source).unwrap();
        DeclaredModule {
            module_id: module_id.into(),
            logical_document_id: format!("src/{module_id}.fluxr"),
            source_path: path.clone(),
            source_sha256: sha256_file(&path).unwrap(),
            codec: SourceCodec::CanonicalChecked,
        }
    }
}

impl Drop for Sandbox {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn operation(
    name: &str,
    module: &str,
    inputs: &[ScalarKind],
    args: &[ScalarKind],
    result: ScalarKind,
) -> BoundOperation {
    BoundOperation {
        operation: name.into(),
        module_id: module.into(),
        export: "run".into(),
        input_kinds: inputs.into(),
        argument_kinds: args.into(),
        result_kind: result,
    }
}

fn fixture_request(sandbox: &Sandbox) -> Request {
    use ScalarKind::{Bool, I64, U64};
    Request {
        modules: vec![
            sandbox.module("affine", AFFINE),
            sandbox.module("boolean", BOOLEAN),
            sandbox.module("unsigned", UNSIGNED),
            sandbox.module("input_adjust", INPUT_ADJUST),
        ],
        operations: vec![
            operation("affine", "affine", &[], &[I64, I64, I64], I64),
            operation("boolean", "boolean", &[], &[Bool, Bool, I64], Bool),
            operation("unsigned", "unsigned", &[], &[U64, U64, U64], U64),
            operation("input_adjust", "input_adjust", &[Bool, I64], &[I64], I64),
        ],
        output_scope: "tests/checked".into(),
        execution: ExecutionMode::PortableChecked,
        limits: Limits::default(),
    }
}

fn profile() -> WorkerProfile {
    let executable = PathBuf::from(env!("CARGO_BIN_EXE_slophawk-fluxr-checked-worker"));
    WorkerProfile {
        executable_sha256: sha256_file(&executable).unwrap(),
        executable,
        rustc_version: RUST_TOOLCHAIN.into(),
        timeout: Duration::from_secs(10),
    }
}

#[test]
fn admitted_positive_covers_boolean_signed_unsigned_and_declared_host_inputs() {
    let sandbox = Sandbox::new();
    let request = fixture_request(&sandbox);
    let cases = br#"[
      {"Operation":"affine","Args":[7,3,2],"Expected":23},
      {"Operation":"boolean","Args":[1,0,10],"Expected":true},
      {"Operation":"unsigned","Args":[7,3,2],"Expected":23},
      {"Operation":"input_adjust","Args":[1,10,3],"Expected":13},
      {"Operation":"input_adjust","Args":[0,10,3],"Expected":7}
    ]"#;
    let receipt = check(&request, cases, &profile(), &AtomicBool::new(false));
    assert_eq!(
        receipt.outcome,
        Outcome::Passed,
        "{:?}",
        receipt.diagnostics
    );
    assert!(!receipt.observe_only);
    assert_eq!(
        (receipt.cases, receipt.compared, receipt.unsupported),
        (5, 5, 0)
    );
    assert_eq!(receipt.observed_json.as_deref(), Some("[23,true,23,13,7]"));
}

#[test]
fn independent_wrong_expectation_is_a_finding_not_a_pass() {
    let sandbox = Sandbox::new();
    let request = fixture_request(&sandbox);
    let cases = br#"[{"Operation":"affine","Args":[7,3,2],"Expected":24}]"#;
    let receipt = check(&request, cases, &profile(), &AtomicBool::new(false));
    assert_eq!(
        receipt.outcome,
        Outcome::Findings,
        "{:?}",
        receipt.diagnostics
    );
    assert_eq!(receipt.findings_total, 1);
    assert_eq!(
        (
            receipt.findings[0].candidate.as_str(),
            receipt.findings[0].reference.as_str()
        ),
        ("23", "24")
    );
}

#[test]
fn ordered_replay_is_deterministic_and_groups_by_first_operation() {
    let sandbox = Sandbox::new();
    let request = fixture_request(&sandbox);
    let inputs = br#"[
      {"Operation":"affine","Args":[7,3,2]},
      {"Operation":"boolean","Args":[0,0,5]},
      {"Operation":"affine","Args":[-7,2,-1]},
      {"Operation":"input_adjust","Args":[0,10,3]}
    ]"#;
    let receipt = replay_ordered(&request, inputs, &profile(), &AtomicBool::new(false));
    assert_eq!(
        receipt.outcome,
        Outcome::Passed,
        "{:?}",
        receipt.diagnostics
    );
    assert_eq!(receipt.first.observed_json, receipt.second.observed_json);
    assert_eq!(receipt.first.job_sha256, receipt.second.job_sha256);
    assert_eq!(
        receipt.first.ordered_inputs_sha256,
        receipt.second.ordered_inputs_sha256
    );
    assert_eq!(
        receipt
            .operations
            .iter()
            .map(|x| x.operation.as_str())
            .collect::<Vec<_>>(),
        ["affine", "boolean", "input_adjust"]
    );
    assert!(
        receipt
            .operations
            .iter()
            .all(|x| x.different == 0 && x.one_sided == 0)
    );
}

#[test]
fn unknown_operation_and_out_of_domain_integer_are_explicitly_unsupported() {
    let sandbox = Sandbox::new();
    let request = fixture_request(&sandbox);
    let inputs = br#"[
      {"Operation":"boolean","Args":[2,0,1]},
      {"Operation":"unsigned","Args":[-1,1,2]},
      {"Operation":"unbound","Args":[1]},
      {"Operation":"affine","Args":[1,2,3]}
    ]"#;
    let receipt = observe(&request, inputs, &profile(), &AtomicBool::new(false));
    assert_eq!(
        receipt.outcome,
        Outcome::Unsupported,
        "{:?}",
        receipt.diagnostics
    );
    assert!(receipt.observe_only);
    assert_eq!((receipt.compared, receipt.unsupported), (1, 3));
    assert_eq!(receipt.observed_json.as_deref(), Some("[null,null,null,5]"));
}

#[test]
fn stale_source_or_mismatched_compiler_binding_never_runs() {
    let sandbox = Sandbox::new();
    let mut request = fixture_request(&sandbox);
    fs::write(
        &request.modules[0].source_path,
        "fluxr 1\npub run 0 int ret 0\n",
    )
    .unwrap();
    let cases = br#"[{"Operation":"affine","Args":[7,3,2],"Expected":23}]"#;
    let result = check(&request, cases, &profile(), &AtomicBool::new(false));
    assert_eq!(result.outcome, Outcome::Stale);
    assert!(result.observed_json.is_none());
    request.modules[0].source_sha256 = sha256_file(&request.modules[0].source_path).unwrap();
    let result = check(&request, cases, &profile(), &AtomicBool::new(false));
    assert_eq!(result.outcome, Outcome::Refused); // interface no longer matches declared mapping
}

#[test]
fn native_items_alternate_codec_and_native_execution_are_separately_refused() {
    let sandbox = Sandbox::new();
    let mut request = fixture_request(&sandbox);
    let cases = br#"[{"Operation":"affine","Args":[7,3,2],"Expected":23}]"#;
    request.modules[0].codec = SourceCodec::Compact;
    assert_eq!(
        check(&request, cases, &profile(), &AtomicBool::new(false)).outcome,
        Outcome::Unsupported
    );
    request.modules[0].codec = SourceCodec::NativeHost;
    assert_eq!(
        check(&request, cases, &profile(), &AtomicBool::new(false)).outcome,
        Outcome::Unsupported
    );
    request.modules[0].codec = SourceCodec::CanonicalChecked;
    request.execution = ExecutionMode::Native;
    assert_eq!(
        check(&request, cases, &profile(), &AtomicBool::new(false)).outcome,
        Outcome::Unsupported
    );
    request.execution = ExecutionMode::PortableChecked;
    let native = concat!(
        "fluxr 1\nrust 2\nβ\n",
        "pub run 3 x int factor int offset int int ret add mul x factor offset\n"
    );
    fs::write(&request.modules[0].source_path, native).unwrap();
    request.modules[0].source_sha256 = sha256_file(&request.modules[0].source_path).unwrap();
    assert_eq!(
        check(&request, cases, &profile(), &AtomicBool::new(false)).outcome,
        Outcome::Unsupported
    );
}

#[test]
fn failed_source_check_and_runtime_trap_do_not_satisfy_expectations() {
    let sandbox = Sandbox::new();
    let mut request = fixture_request(&sandbox);
    let cases = br#"[{"Operation":"affine","Args":[7,3,2],"Expected":23}]"#;
    fs::write(
        &request.modules[0].source_path,
        "fluxr 1\npub run 3 x int factor int offset int int ret missing\n",
    )
    .unwrap();
    request.modules[0].source_sha256 = sha256_file(&request.modules[0].source_path).unwrap();
    let failed = check(&request, cases, &profile(), &AtomicBool::new(false));
    assert_eq!(failed.outcome, Outcome::Refused);
    assert!(
        failed
            .diagnostics
            .iter()
            .any(|x| x.contains("checked source"))
    );
    let trap = "fluxr 1\npub run 3 x int factor int offset int int ret div x 0\n";
    fs::write(&request.modules[0].source_path, trap).unwrap();
    request.modules[0].source_sha256 = sha256_file(&request.modules[0].source_path).unwrap();
    let failed = check(&request, cases, &profile(), &AtomicBool::new(false));
    assert_eq!(failed.outcome, Outcome::Failed, "{:?}", failed.diagnostics);
    assert!(
        failed
            .diagnostics
            .iter()
            .any(|x| x.contains("runtime error"))
    );
}

#[cfg(unix)]
fn scripted_profile(sandbox: &Sandbox, contents: &str, timeout: Duration) -> WorkerProfile {
    use std::os::unix::fs::PermissionsExt;
    let path = sandbox.0.join("fake-worker");
    fs::write(&path, contents).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).unwrap();
    WorkerProfile {
        executable_sha256: sha256_file(&path).unwrap(),
        executable: path,
        rustc_version: RUST_TOOLCHAIN.into(),
        timeout,
    }
}

#[cfg(unix)]
#[test]
fn malformed_adapter_output_is_not_an_observation() {
    let sandbox = Sandbox::new();
    let request = fixture_request(&sandbox);
    let profile = scripted_profile(
        &sandbox,
        "#!/bin/sh\nprintf 'not-json'\n",
        Duration::from_secs(2),
    );
    let cases = br#"[{"Operation":"affine","Args":[1,2,3],"Expected":5}]"#;
    let result = check(&request, cases, &profile, &AtomicBool::new(false));
    assert_eq!(
        result.outcome,
        Outcome::MalformedOutput,
        "{:?}",
        result.diagnostics
    );
    assert!(result.observed_json.is_none());
}

#[cfg(unix)]
#[test]
fn timeout_and_cancellation_are_bounded_non_passing_receipts() {
    let sandbox = Sandbox::new();
    let request = fixture_request(&sandbox);
    // POSIX sh builtins only: this script does not launch descendants.
    let profile = scripted_profile(
        &sandbox,
        "#!/bin/sh\nwhile :; do :; done\n",
        Duration::from_millis(30),
    );
    let cases = br#"[{"Operation":"affine","Args":[1,2,3],"Expected":5}]"#;
    let timed_out = check(&request, cases, &profile, &AtomicBool::new(false));
    assert_eq!(
        timed_out.outcome,
        Outcome::TimedOut,
        "{:?}",
        timed_out.diagnostics
    );
    let cancelled = check(&request, cases, &profile, &AtomicBool::new(true));
    assert_eq!(cancelled.outcome, Outcome::Cancelled);
    assert!(timed_out.observed_json.is_none() && cancelled.observed_json.is_none());
}
