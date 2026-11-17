//! Opt-in process fixture. Runs repository code only when the caller explicitly
//! authorizes a controlled test worker with `--ignored`.

use slophawk_rust_project_context::{
    Binding, CargoTestJob, Disposition, Features, MetadataInput, SnapshotFile, WorkerCapture,
    WorkerExit, classify_test_capture, interpret_metadata, metadata_argv, sha256_hex, test_argv,
};
use std::{
    env, fs,
    path::{Path, PathBuf},
    process::{Command, Output},
};

fn copy_fixture(to: &Path) {
    let from = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/workspace");
    for rel in [
        "Cargo.toml",
        "Cargo.lock",
        "alpha/Cargo.toml",
        "beta/Cargo.toml",
        "alpha/src/lib.rs",
        "beta/src/lib.rs",
    ] {
        let target = to.join(rel);
        fs::create_dir_all(target.parent().unwrap()).unwrap();
        fs::copy(from.join(rel), target).unwrap();
    }
}

fn snap(path: PathBuf) -> SnapshotFile {
    let bytes = fs::read(&path).unwrap();
    SnapshotFile {
        path: path.to_str().unwrap().into(),
        expected_sha256: sha256_hex(&bytes),
        bytes,
    }
}

fn command(cargo: &str, argv: &[String], root: &Path, scratch: &Path) -> Output {
    Command::new(cargo)
        .args(argv)
        .current_dir(root)
        .env("CARGO_HOME", scratch.join("cargo-home"))
        .env("CARGO_TARGET_DIR", scratch.join("target"))
        .env("CARGO_NET_OFFLINE", "true")
        .output()
        .unwrap()
}

fn capture(binding: Binding, argv: Vec<String>, output: Output) -> WorkerCapture {
    WorkerCapture {
        binding,
        argv,
        admitted: true,
        attempt_id: None,
        fence_sequence: None,
        graph_sha256: None,
        exit: WorkerExit::Exited(output.status.code().unwrap_or(-1)),
        truncated: false,
        stdout: output.stdout,
        stderr: output.stderr,
    }
}

/// Requires a Rust 1.99.1 host, native linker, and an *admitted* worker scope.
/// It uses no network and no crates.io dependencies in the fixture workspace.
/// The test is ignored by normal `cargo test` intentionally.
#[test]
#[ignore = "execute only in an admitted Rust 1.99.1 worker; builds and test code run"]
fn offline_locked_two_package_workspace_and_test_receipt() {
    let scratch = env::temp_dir().join(format!("slophawk-varo8-live-{}", std::process::id()));
    assert!(!scratch.exists(), "choose an unused scratch path");
    fs::create_dir_all(&scratch).unwrap();
    let root = scratch.join("workspace");
    fs::create_dir_all(&root).unwrap();
    copy_fixture(&root);
    let root = fs::canonicalize(root).unwrap();
    let root_text = root.to_str().unwrap().to_owned();
    let manifest = root.join("Cargo.toml");
    let cargo = env::var("CARGO").unwrap_or_else(|_| "cargo".into());
    let rustc = Command::new("rustc").arg("-Vv").output().unwrap();
    assert!(rustc.status.success());
    let version = String::from_utf8(rustc.stdout.clone()).unwrap();
    assert!(version.contains("rustc 1.99.1"), "Rust 1.99.1 is required");
    let cargo_file = fs::canonicalize(&cargo).unwrap();
    let binding = Binding {
        workspace_root: root_text,
        workspace_revision: 5,
        epoch: "offline_fixture_epoch".into(),
        git_commit: "0123456789012345678901234567890123456789".into(),
        target_triple: version
            .lines()
            .find_map(|line| line.strip_prefix("host: "))
            .unwrap()
            .into(),
        rustc_version: "rustc 1.99.1".into(),
        rustc_vv_sha256: sha256_hex(&rustc.stdout),
        cargo_executable_sha256: sha256_hex(&fs::read(cargo_file).unwrap()),
        environment_sha256: sha256_hex(b"fixture-controlled-cargo-home-target-dir"),
        features: Features {
            selected: vec!["alpha/with-beta".into()],
            ..Features::default()
        },
    };
    let metadata_args = metadata_argv(&binding, manifest.to_str().unwrap()).unwrap();
    let metadata_output = command(&cargo, &metadata_args, &root, &scratch);
    assert!(
        metadata_output.status.success(),
        "locked offline Cargo metadata failed"
    );
    let graph = interpret_metadata(MetadataInput {
        binding: binding.clone(),
        manifest_path: manifest.to_str().unwrap().into(),
        manifests: vec![
            snap(root.join("Cargo.toml")),
            snap(root.join("alpha/Cargo.toml")),
            snap(root.join("beta/Cargo.toml")),
        ],
        lock: snap(root.join("Cargo.lock")),
        capture: capture(binding.clone(), metadata_args, metadata_output),
    })
    .unwrap();
    assert_eq!(graph.workspace_members.len(), 2);
    let alpha = graph
        .workspace_members
        .iter()
        .find(|id| graph.packages[*id].name == "alpha")
        .unwrap()
        .clone();
    assert!(
        graph
            .resolved_edges
            .iter()
            .any(|e| e.from == alpha && e.optional == Some(true))
    );
    let package_spec = graph.test_package_spec(&alpha).unwrap();
    let test_args = test_argv(&binding, manifest.to_str().unwrap(), &package_spec).unwrap();
    let test_output = command(&cargo, &test_args, &root, &scratch);
    assert!(
        test_output.status.success(),
        "locked offline Cargo fixture tests failed"
    );
    let mut test_capture = capture(binding, test_args, test_output);
    test_capture.attempt_id = Some(15);
    test_capture.fence_sequence = Some(7);
    test_capture.graph_sha256 = Some(graph.context_sha256.clone());
    let job = CargoTestJob {
        context_sha256: graph.context_sha256.clone(),
        package_id: alpha,
        attempt_id: 15,
        fence_sequence: 7,
    };
    let receipt = classify_test_capture(&graph, &job, &test_capture).unwrap();
    assert_eq!(receipt.disposition, Disposition::Passed);
    assert!(receipt.counters.unwrap().passed >= 1);
    assert!(receipt.is_current(&graph));
    fs::remove_dir_all(scratch).unwrap();
}
