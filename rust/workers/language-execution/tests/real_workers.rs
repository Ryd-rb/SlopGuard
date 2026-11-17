//! Explicitly ignored, Linux-only real worker fixtures. The owner prepares
//! pinned tools/offline Python wheel environment and invokes with --ignored.
//! No fixture is executed by source-only task delivery.
#![cfg(target_os = "linux")]
use slophawk_language_execution::{
    Budget, CargoVariant, Executable, FluxrModule, FluxrOperation, Identity, InputFile,
    Job, Operation, PROTOCOL, Refusal, Status, classify_cargo_metadata,
    classify_cargo_test, classify_python, run_admitted, cargo_environment_digest,
};
use slophawk_language_execution::protocol;
use slophawk_python_project_context as python;
use slophawk_rust_project_context as rust;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{atomic::{AtomicBool, AtomicU64, Ordering}, Arc};
use std::thread;
use std::time::Duration;

static NEXT: AtomicU64 = AtomicU64::new(0);
struct Scratch { path: PathBuf, root: PathBuf, owner: PathBuf, artifacts: PathBuf }
impl Scratch {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!("voda-language-{}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
        fs::create_dir(&path).unwrap();
        let root = path.join("workspace");
        let owner = path.join("owner");
        let artifacts = path.join("artifacts");
        for dir in [&root, &owner, &artifacts] {
            fs::create_dir(dir).unwrap();
            fs::set_permissions(dir, fs::Permissions::from_mode(0o700)).unwrap();
        }
        Self { path, root, owner, artifacts }
    }
    fn write(&self, relative: &str, bytes: &[u8]) {
        let path = self.root.join(relative);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, bytes).unwrap();
    }
    fn ledger(&self, names: &[&str]) -> Vec<InputFile> {
        names.iter().map(|name| {
            let bytes = fs::read(self.root.join(name)).unwrap();
            InputFile {
                relative_path: (*name).into(), sha256: protocol::sha256(&bytes),
                byte_count: bytes.len() as u64,
            }
        }).collect()
    }
    fn permit(&self, job: &Job, worker: &Executable) -> PathBuf {
        let name = format!("{}-{}.json", job.identity.job_id, job.identity.attempt_id);
        let file = self.owner.join(name);
        let profile = serde_json::json!({
            "root": &job.identity.root,
            "context_sha256": &job.identity.context_sha256,
            "epoch": &job.identity.epoch,
            "committed_revision": job.identity.committed_revision,
            "fence_sequence": job.identity.fence_sequence,
            "input_sha256": &job.identity.input_sha256,
            "source_sha256": &job.identity.source_sha256,
            "input_files_sha256": protocol::sha256(&serde_json::to_vec(&job.inputs).unwrap()),
            "operation_sha256": protocol::sha256(&serde_json::to_vec(&job.operation).unwrap()),
            "config_sha256": &job.identity.config_sha256,
            "environment_sha256": &job.identity.environment_sha256,
            "policy_sha256": &job.identity.policy_sha256,
            "grant_ref": &job.identity.grant_ref,
            "budget_ref": &job.identity.budget_ref,
            "operations": [job.operation.name()],
            "allow_build": true,
            "allow_build_scripts": true,
            "allow_proc_macros": true,
            "tool": job.operation.executable(),
            "worker": worker,
            "artifacts_root": self.artifacts.to_str().unwrap(),
            "budget": &job.budget,
        });
        let mut writer = OpenOptions::new().write(true).create_new(true).mode(0o600).open(&file).unwrap();
        writer.write_all(&serde_json::to_vec(&profile).unwrap()).unwrap();
        file
    }
}
impl Drop for Scratch { fn drop(&mut self) { let _ = fs::remove_dir_all(&self.path); } }

fn env_needed(name: &str) -> String {
    std::env::var(name).unwrap_or_else(|_| panic!("owner must provide {name} for ignored fixture"))
}
fn executable(path: &str) -> Executable {
    let contents = fs::read(path).unwrap();
    Executable { path: path.into(), sha256: protocol::sha256(&contents) }
}
fn migration_worker() -> Executable {
    executable(env!("CARGO_BIN_EXE_slophawk-language-worker"))
}
fn identity(scratch: &Scratch, operation: &str, tool: &Executable, context: &str, input: &str,
    source: &str, config: &str, env_sha: &str, attempt: &str) -> Identity {
    Identity {
        operation: operation.into(), job_id: format!("{operation}-{attempt}"),
        attempt_id: attempt.into(), root: scratch.root.to_str().unwrap().into(),
        context_sha256: context.into(), epoch: "fixture-epoch".into(),
        committed_revision: 5, fence_sequence: 0, input_sha256: input.into(), source_sha256: source.into(),
        config_sha256: config.into(), environment_sha256: env_sha.into(),
        tool_sha256: tool.sha256.clone(), policy_sha256: protocol::sha256(b"owner-only-fixture-policy"),
        grant_ref: "owner-grant".into(), budget_ref: "bounded-job".into(),
    }
}
fn budget(ms: u64) -> Budget {
    Budget { deadline_ms: ms, stdout_bytes: 256 * 1024,
        stderr_bytes: 64 * 1024, artifact_bytes: 2 * 1024 * 1024 }
}
fn run(scratch: &Scratch, job: Job) -> Result<slophawk_language_execution::Verified, Refusal> {
    let path = scratch.permit(&job, &migration_worker());
    run_admitted(&path, job, &AtomicBool::new(false))
}
fn py_scope(scratch: &Scratch, test: &str) -> (python::SelectionContext, Vec<InputFile>) {
    scratch.write("pyproject.toml", b"[project]\nname = 'voda-fixture'\n");
    let filename = format!("tests/{test}");
    let source = fs::read(Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/python").join(test)).unwrap();
    scratch.write(&filename, &source);
    let files = vec![
        python::SnapshotFile { path: "pyproject.toml".into(), bytes: fs::read(scratch.root.join("pyproject.toml")).unwrap() },
        python::SnapshotFile { path: filename.clone(), bytes: source },
    ];
    let context = python::analyze(python::WorkspaceScope {
        workspace_id: "voda-fixture".into(), revision: "fixture-epoch-rev5".into(),
        project_roots: vec![".".into()], inventory: python::Inventory::Complete,
    }, files).unwrap();
    let selection = python::select_tests(&context, &"0".repeat(64), &[
        python::Change { before: Some(filename.clone()), after: Some(filename.clone()) }
    ]).unwrap();
    (selection, scratch.ledger(&["pyproject.toml", &filename]))
}
fn py_job(scratch: &Scratch, selection: &python::SelectionContext,
    inputs: Vec<InputFile>, attempt: &str) -> Job {
    let interpreter = executable(&env_needed("VODA_PYTHON_BIN"));
    let root = &selection.identity;
    Job {
        format: PROTOCOL.into(),
        identity: identity(scratch, "pytest", &interpreter, &selection.selection_sha256,
            &selection.selection_sha256, &root.source_sha256, &root.configuration_sha256,
            &protocol::sha256(b"owner-verified-venv-environment"), attempt),
        budget: budget(90_000), inputs,
        operation: Operation::Pytest {
            interpreter, project_root: ".".into(),
            pytest_sha256: env_needed("VODA_PYTEST_SHA256"),
            plugins_sha256: env_needed("VODA_PLUGIN_SHA256"),
            effective_config_sha256: Some(env_needed("VODA_EFFECTIVE_PYTEST_CONFIG_SHA256")),
        },
    }
}

#[test]
#[ignore = "requires owner-admitted Python venv with exact offline pytest/json-report wheels"]
fn real_pytest_collection_and_call_receipt() {
    let scratch = Scratch::new();
    let (selection, inputs) = py_scope(&scratch, "test_pass.py");
    let receipt = run(&scratch, py_job(&scratch, &selection, inputs, "12")).unwrap();
    let result = classify_python(&selection, &receipt).unwrap();
    assert_eq!(result.disposition, python::ReceiptDisposition::Passed, "{result:?}");
    assert_eq!((result.collected, result.passed), (1, 1));
}

#[test]
#[ignore = "requires owner-admitted Python venv; expected failing/skip/xfail/xpass/setup/teardown cases"]
fn real_pytest_nearby_negative_stage_cases() {
    let scratch = Scratch::new();
    let (selection, inputs) = py_scope(&scratch, "test_nearby.py");
    let receipt = run(&scratch, py_job(&scratch, &selection, inputs, "13")).unwrap();
    let result = classify_python(&selection, &receipt).unwrap();
    assert_eq!(result.disposition, python::ReceiptDisposition::Failed, "{result:?}");
    assert_eq!(result.collected, 6);
    assert!(result.failed >= 3 && result.skipped >= 2);
}

fn copy_cargo_fixture(scratch: &Scratch) -> Vec<&'static str> {
    let source = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../project-context/rust/tests/fixtures/workspace");
    let names = ["Cargo.toml", "Cargo.lock", "alpha/Cargo.toml", "beta/Cargo.toml",
        "alpha/src/lib.rs", "beta/src/lib.rs"];
    for relative in names { scratch.write(relative, &fs::read(source.join(relative)).unwrap()); }
    names.to_vec()
}
fn snap(path: PathBuf) -> rust::SnapshotFile {
    let bytes = fs::read(&path).unwrap();
    rust::SnapshotFile { expected_sha256: protocol::sha256(&bytes),
        path: path.to_str().unwrap().into(), bytes }
}

#[test]
#[ignore = "requires owner-approved Linux Rust 1.99.1 toolchain and offline Cargo execution"]
fn real_locked_cargo_two_package_graph_and_test_receipt() {
    let scratch = Scratch::new();
    let files = copy_cargo_fixture(&scratch);
    let ledger = scratch.ledger(&files);
    let tool = executable(&env_needed("VODA_CARGO_BIN"));
    let rustc = env_needed("VODA_RUSTC_BIN");
    let output = Command::new(&rustc).arg("-Vv").output().unwrap();
    assert!(output.status.success());
    let version = String::from_utf8(output.stdout.clone()).unwrap();
    assert!(version.contains("rustc 1.99.1"));
    let target = version.lines().find_map(|l| l.strip_prefix("host: ")).unwrap();
    let variant = CargoVariant {
        rustc: executable(&rustc),
        revision: 5, epoch: "fixture-epoch".into(),
        git_commit: "0123456789012345678901234567890123456789".into(),
        target_triple: target.into(), rustc_version: "rustc 1.99.1".into(),
        rustc_vv_sha256: protocol::sha256(&output.stdout),
        environment_sha256: cargo_environment_digest(&tool, &executable(&rustc)),
        all_features: false, no_default_features: false,
        features: vec!["alpha/with-beta".into()],
    };
    let binding = rust::Binding {
        workspace_root: scratch.root.to_str().unwrap().into(), workspace_revision: variant.revision,
        epoch: variant.epoch.clone(), git_commit: variant.git_commit.clone(),
        target_triple: variant.target_triple.clone(), rustc_version: variant.rustc_version.clone(),
        rustc_vv_sha256: variant.rustc_vv_sha256.clone(),
        cargo_executable_sha256: tool.sha256.clone(), environment_sha256: variant.environment_sha256.clone(),
        features: rust::Features { selected: variant.features.clone(), ..Default::default() },
    };
    let manifest = scratch.root.join("Cargo.toml").to_str().unwrap().to_string();
    let context = binding.digest().unwrap();
    let ledger_digest = protocol::sha256(&serde_json::to_vec(&ledger).unwrap());
    let metadata_job = Job {
        format: PROTOCOL.into(),
        identity: identity(&scratch, "cargo_metadata", &tool, &context,
            &ledger_digest, &ledger_digest,
            &protocol::sha256(b"cargo-config"), &variant.environment_sha256, "21"),
        budget: budget(120_000), inputs: ledger.clone(),
        operation: Operation::CargoMetadata { cargo: tool.clone(), variant: variant.clone(), manifest_path: manifest.clone() },
    };
    let receipt = run(&scratch, metadata_job).unwrap();
    let graph = classify_cargo_metadata(&receipt,
        ["Cargo.toml", "alpha/Cargo.toml", "beta/Cargo.toml"].map(|p| snap(scratch.root.join(p))).to_vec(),
        snap(scratch.root.join("Cargo.lock"))).unwrap();
    assert_eq!(graph.workspace_members.len(), 2);
    assert_eq!(graph.coverage, rust::Coverage::EvaluatedCargoGraph);
    let alpha = graph.workspace_members.iter().find(|id| graph.packages[*id].name == "alpha").unwrap().clone();
    let spec = graph.test_package_spec(&alpha).unwrap();
    let test = rust::CargoTestJob { context_sha256: graph.context_sha256.clone(),
        package_id: alpha, attempt_id: 22, fence_sequence: 7 };
    let test_job = Job {
        format: PROTOCOL.into(),
        identity: Identity { fence_sequence: test.fence_sequence,
            ..identity(&scratch, "cargo_test", &tool, &graph.context_sha256,
            &ledger_digest, &ledger_digest,
            &protocol::sha256(b"cargo-config"), &variant.environment_sha256, "22") },
        budget: budget(120_000), inputs: ledger,
        operation: Operation::CargoTest { cargo: tool, variant, manifest_path: manifest,
            package_spec: spec, graph_sha256: graph.context_sha256.clone() },
    };
    let capture = run(&scratch, test_job).unwrap();
    let classified = classify_cargo_test(&capture, &graph, &test).unwrap();
    assert_eq!(classified.disposition, rust::Disposition::Passed, "{classified:?}");
    assert!(classified.counters.unwrap().passed >= 1);
}

#[test]
#[ignore = "requires exact pinned FluxR checked worker and Linux process-group admission"]
fn real_fluxr_checked_and_unqualified_codec() {
    let scratch = Scratch::new();
    let source = b"fluxr 1\npub run 3 x int factor int offset int int ret add mul x factor offset\n";
    scratch.write("src/affine.fluxr", source);
    let tool = executable(&env_needed("VODA_FLUXR_WORKER_BIN"));
    let cases = serde_json::json!([{"Operation":"affine", "Args":[7,3,2], "Expected":23}]);
    let module = FluxrModule { module_id: "affine".into(), logical_document_id: "src/affine.fluxr".into(),
        relative_path: "src/affine.fluxr".into(), sha256: protocol::sha256(source),
        codec: "canonical_checked".into() };
    let operation = FluxrOperation { operation: "affine".into(), module_id: "affine".into(),
        export: "run".into(), input_kinds: vec![],
        argument_kinds: vec!["i64".into(), "i64".into(), "i64".into()],
        result_kind: "i64".into() };
    let job = Job {
        format: PROTOCOL.into(),
        identity: identity(&scratch, "fluxr_check", &tool, &protocol::sha256(b"checked-module-context"),
            &protocol::sha256(&serde_json::to_vec(&cases).unwrap()),
            &protocol::sha256(&serde_json::to_vec(&vec![module.clone()]).unwrap()),
            &protocol::sha256(b"checked-config"), &protocol::sha256(b"fluxr-env"), "31"),
        budget: budget(90_000), inputs: scratch.ledger(&["src/affine.fluxr"]),
        operation: Operation::FluxrCheck { checked_worker: tool, modules: vec![module],
            operations: vec![operation], cases, limits: serde_json::Value::Null,
            output_scope: "tests/checked".into(), mode: "check".into() },
    };
    let good = run(&scratch, job.clone()).unwrap();
    assert_eq!(good.payload()["outcome"], "passed");
    assert_eq!(good.payload()["observed_json"], "[23]");
    let mut bad = job;
    bad.identity.attempt_id = "32".into();
    bad.identity.job_id = "fluxr-check-32".into();
    if let Operation::FluxrCheck { modules, .. } = &mut bad.operation {
        modules[0].codec = "compact".into();
    }
    let denial = run(&scratch, bad).unwrap_err();
    assert!(matches!(denial, Refusal::WorkerStatus { status: Status::Unsupported, .. }));
}

#[test]
#[ignore = "executes a bounded scripted negative worker, not the product"]
fn missing_terminal_and_source_change_cannot_publish() {
    let scratch = Scratch::new();
    scratch.write("input.txt", b"original");
    let tool = executable("/bin/true");
    let outer = scratch.owner.join("missing-terminal");
    fs::write(&outer, b"#!/bin/sh\nexit 0\n").unwrap();
    fs::set_permissions(&outer, fs::Permissions::from_mode(0o700)).unwrap();
    let fake_worker = executable(outer.to_str().unwrap());
    let job = Job {
        format: PROTOCOL.into(),
        identity: identity(&scratch, "pytest", &tool, &protocol::sha256(b"context"),
            &protocol::sha256(b"input"), &protocol::sha256(b"source"),
            &protocol::sha256(b"config"), &protocol::sha256(b"environment"), "41"),
        budget: budget(2_000), inputs: scratch.ledger(&["input.txt"]),
        operation: Operation::Pytest { interpreter: tool, project_root: ".".into(),
            pytest_sha256: protocol::sha256(b"pytest"), plugins_sha256: protocol::sha256(b"plugin"),
            effective_config_sha256: None },
    };
    let profile = scratch.permit(&job, &fake_worker);
    assert!(matches!(run_admitted(&profile, job.clone(), &AtomicBool::new(false)), Err(Refusal::Failed(_))));
    assert!(!scratch.artifacts.join("pytest-41-41").exists());
    let mut stale = job;
    stale.identity.attempt_id = "42".into();
    stale.identity.job_id = "pytest-42".into();
    let profile = scratch.permit(&stale, &fake_worker);
    scratch.write("input.txt", b"changed");
    assert!(matches!(run_admitted(&profile, stale, &AtomicBool::new(false)), Err(Refusal::Stale(_))));
}

#[test]
#[ignore = "admitted POSIX scripted worker for revocation and cancellation; owner runs"]
fn owner_revocation_and_cancellation_fence_a_running_attempt() {
    let scratch = Scratch::new();
    scratch.write("input.txt", b"original");
    let tool = executable("/bin/true");
    let script = scratch.owner.join("endless-worker");
    // The POSIX shell loop is a bounded negative fixture: the parent must kill
    // the process group and never publish evidence from an absent terminal.
    fs::write(&script, b"#!/bin/sh\nwhile :; do :; done\n").unwrap();
    fs::set_permissions(&script, fs::Permissions::from_mode(0o700)).unwrap();
    let worker = executable(script.to_str().unwrap());
    let job = |attempt: &str| Job {
        format: PROTOCOL.into(),
        identity: identity(&scratch, "pytest", &tool, &protocol::sha256(b"context"),
            &protocol::sha256(b"input"), &protocol::sha256(b"source"),
            &protocol::sha256(b"config"), &protocol::sha256(b"environment"), attempt),
        budget: budget(2_000), inputs: scratch.ledger(&["input.txt"]),
        operation: Operation::Pytest { interpreter: tool.clone(), project_root: ".".into(),
            pytest_sha256: protocol::sha256(b"pytest"), plugins_sha256: protocol::sha256(b"plugin"),
            effective_config_sha256: None },
    };
    let first = job("51");
    let profile = scratch.permit(&first, &worker);
    let signal = Arc::new(AtomicBool::new(false));
    let signal_setter = Arc::clone(&signal);
    thread::scope(|scope| {
        scope.spawn(move || {
            thread::sleep(Duration::from_millis(120));
            signal_setter.store(true, Ordering::Release);
        });
        let result = run_admitted(&profile, first, &signal);
        assert!(matches!(result, Err(Refusal::WorkerStatus { status: Status::Cancelled, .. })));
    });
    let second = job("52");
    let profile = scratch.permit(&second, &worker);
    thread::scope(|scope| {
        let profile_for_revoke = profile.clone();
        scope.spawn(move || {
            thread::sleep(Duration::from_millis(120));
            fs::remove_file(profile_for_revoke).unwrap();
        });
        let result = run_admitted(&profile, second, &AtomicBool::new(false));
        assert!(matches!(result, Err(Refusal::Stale(_))));
    });
}
