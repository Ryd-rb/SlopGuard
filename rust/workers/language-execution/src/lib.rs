//! Admitted language execution with private `slophawk.migration-job/1`
//! receipt transport. Not a daemon scheduler registration or public CLI change.
//!
//! The owner creates an OS-user-private profile outside the checkout, pins a
//! short-lived language-worker executable and trusted tool digests, and calls
//! `run_admitted`. Repository config/source cannot manufacture a grant. Only a
//! valid terminal, successful worker process and unchanged owner profile may
//! yield a `Verified` capture. Runtime checks are deliberately not performed
//! from passive `project-context` parsing.

pub mod protocol;
mod admission;
mod python;
mod runner;

pub use admission::{Refusal, Verified, discard_staged, read_owner_document, read_owner_document_bounded, run_admitted, run_admitted_guarded, verify_executable};
pub use runner::{cargo_binding, cargo_environment_digest, cargo_msvc_environment_digest};
pub use protocol::{Budget, CargoVariant, Executable, InputFile, FluxrModule, FluxrOperation, Identity, Job, MsvcToolchain, Operation, Status, PROTOCOL};
use slophawk_python_project_context as py;
use slophawk_rust_project_context as cargo;

/// Consume a verified worker's fresh pytest artifacts through the pinned
/// context classifier. No imported JSON artifact can confer admission by itself.
pub fn classify_python(selection: &py::SelectionContext, verified: &Verified)
    -> Result<py::ReceiptResult, Refusal>
{
    let Operation::Pytest { project_root, pytest_sha256, plugins_sha256, effective_config_sha256, .. } = &verified.job().operation else {
        return Err(Refusal::Denied("not a pytest job"));
    };
    let identity = &verified.job().identity;
    if identity.context_sha256 != selection.selection_sha256
        || identity.input_sha256 != selection.selection_sha256
        || identity.source_sha256 != selection.identity.source_sha256
        || identity.config_sha256 != selection.identity.configuration_sha256
    { return Err(Refusal::Stale("Python selection/config/source fence changed")); }
    let evidence: python::Evidence = serde_json::from_value(verified.payload().clone())
        .map_err(|_| Refusal::Failed("worker Python evidence malformed"))?;
    if effective_config_sha256.as_deref() != Some(evidence.effective_config_sha256.as_str())
        || !protocol::is_sha(&evidence.effective_config_sha256) {
        return Err(Refusal::Stale("effective pytest config changed"));
    }
    let binding = py::ReceiptBinding {
        attempt_id: identity.attempt_id.clone(), project_root: project_root.clone(),
        scope_sha256: selection.identity.scope_sha256.clone(),
        source_sha256: selection.identity.source_sha256.clone(),
        configuration_sha256: selection.identity.configuration_sha256.clone(),
        effective_pytest_config_sha256: evidence.effective_config_sha256.clone(),
        selection_sha256: selection.selection_sha256.clone(),
        interpreter_sha256: identity.tool_sha256.clone(),
        environment_sha256: identity.environment_sha256.clone(),
        pytest_sha256: pytest_sha256.clone(),
        plugins_sha256: plugins_sha256.clone(),
        collected_ids_sha256: py::node_ids_digest(&evidence.collection.node_ids),
    };
    let artifact = |wire: python::ArtifactWire| py::Artifact {
        sha256: wire.sha256, byte_count: wire.byte_count,
        modified_ms: wire.modified_ms, output_excerpt: wire.excerpt,
    };
    let collection = evidence.collection;
    let run = match evidence.run {
        Some(run) => {
            let cases = run.cases.into_iter().map(|case| {
                let outcome = match case.outcome.as_str() {
                    "passed" => py::CaseOutcome::Passed,
                    "failed" => py::CaseOutcome::Failed,
                    "skipped" => py::CaseOutcome::Skipped,
                    "xfailed" => py::CaseOutcome::XFailed,
                    "xpassed" => py::CaseOutcome::XPassed,
                    "error" => py::CaseOutcome::Error,
                    _ => return Err(Refusal::Failed("unknown pytest case outcome")),
                };
                Ok(py::PytestCase {
                    node_id: case.node_id, outcome, failure_excerpt: case.failure_excerpt,
                })
            }).collect::<Result<Vec<_>, Refusal>>()?;
            Some(py::Run {
                process: py::Process::Exited(run.exit),
                finished: run.finished, cases, artifact: Some(artifact(run.artifact)),
            })
        }
        None => None,
    };
    let observation = py::Observation {
        binding: binding.clone(),
        collection: Some(py::Collected {
            process: py::Process::Exited(collection.exit),
            finished: collection.finished,
            full_project_scope: collection.full_project_scope,
            filtered: collection.filtered,
            collection_errors: collection.collection_errors,
            node_ids: collection.node_ids,
            artifact: Some(artifact(collection.artifact)),
        }),
        run,
    };
    // Keep clock times supplied by the authenticated parent, not by the plugin.
    let admitted = py::Admission {
        binding, started_ms: verified.started_ms(), finished_ms: verified.finished_ms(),
        owned_directory: true,
    };
    Ok(py::classify_receipt(selection, &admitted, &observation))
}

fn cargo_capture(verified: &Verified, binding: cargo::Binding, argv: Vec<String>,
    test_job: Option<&cargo::CargoTestJob>) -> Result<cargo::WorkerCapture, Refusal>
{
    let captured: runner::Capture = serde_json::from_value(verified.payload().clone())
        .map_err(|_| Refusal::Failed("malformed Cargo stream capture"))?;
    let (attempt_id, fence_sequence, graph_sha256) = match test_job {
        Some(job) => (Some(job.attempt_id), Some(job.fence_sequence), Some(job.context_sha256.clone())),
        None => (None, None, None),
    };
    Ok(cargo::WorkerCapture {
        binding, argv, admitted: true, attempt_id, fence_sequence, graph_sha256,
        exit: cargo::WorkerExit::Exited(captured.exit), truncated: false,
        stdout: captured.stdout, stderr: captured.stderr,
    })
}

/// Root and member manifest snapshots plus Cargo.lock are supplied as exact
/// bytes by the owner and cross-checked by the existing graph interpreter.
pub fn classify_cargo_metadata(verified: &Verified,
    manifests: Vec<cargo::SnapshotFile>, lock: cargo::SnapshotFile)
    -> Result<cargo::CargoGraph, String>
{
    let Operation::CargoMetadata { cargo: tool, variant, manifest_path } = &verified.job().operation else {
        return Err("not a Cargo metadata job".into());
    };
    let binding = runner::cargo_binding(&verified.job().identity.root, tool, variant)?;
    verify_cargo_inputs(verified).map_err(|e| e.to_string())?;
    if verified.job().identity.context_sha256 != binding.digest().map_err(|e| e.to_string())? {
        return Err("Cargo metadata binding digest changed".into());
    }
    let argv = cargo::metadata_argv(&binding, manifest_path).map_err(|e| e.to_string())?;
    for snapshot in manifests.iter().chain(std::iter::once(&lock)) {
        let relative = std::path::Path::new(&snapshot.path).strip_prefix(&verified.job().identity.root)
            .map_err(|_| "Cargo snapshot is outside the admitted root")?;
        let relative = relative.to_str().ok_or("Cargo snapshot path is not UTF-8")?;
        let item = verified.job().inputs.iter().find(|item| item.relative_path == relative)
            .ok_or("Cargo snapshot was not admitted")?;
        if item.sha256 != protocol::sha256(&snapshot.bytes)
            || item.byte_count != snapshot.bytes.len() as u64 {
            return Err("Cargo snapshot digest differs from admission ledger".into());
        }
    }
    let capture = cargo_capture(verified, binding.clone(), argv, None).map_err(|e| format!("{e:?}"))?;
    cargo::interpret_metadata(cargo::MetadataInput {
        binding, manifest_path: manifest_path.clone(), manifests, lock, capture,
    }).map_err(|e| e.to_string())
}

/// Select a resolved workspace package from the previous evaluated graph;
/// Cargo's stable JSON events and libtest summaries are interpreted by the
/// existing Rust receipt classifier (never inferred from a zero exit alone).
pub fn classify_cargo_test(verified: &Verified, graph: &cargo::CargoGraph,
    test: &cargo::CargoTestJob) -> Result<cargo::TestReceipt, String>
{
    let Operation::CargoTest { cargo: tool, variant, manifest_path, package_spec, graph_sha256 } =
        &verified.job().operation else { return Err("not a Cargo test job".into()); };
    let identity = &verified.job().identity;
    verify_cargo_inputs(verified).map_err(|e| e.to_string())?;
    for snapshot in &graph.manifest_digests {
        let relative = std::path::Path::new(&snapshot.path).strip_prefix(&identity.root)
            .map_err(|_| "manifest outside admitted Cargo test root")?;
        if !verified.job().inputs.iter().any(|f|
            f.relative_path == relative.to_string_lossy().as_ref() && f.sha256 == snapshot.sha256) {
            return Err("Cargo test manifest changed since graph interpretation".into());
        }
    }
    if !verified.job().inputs.iter().any(|f| f.relative_path == "Cargo.lock"
        && f.sha256 == graph.lock_sha256) {
        return Err("Cargo test lock changed since graph interpretation".into());
    }
    if graph_sha256 != &graph.context_sha256 || identity.context_sha256 != graph.context_sha256
        || identity.committed_revision != graph.binding.workspace_revision
        || identity.epoch != graph.binding.epoch
        || identity.attempt_id != test.attempt_id.to_string()
        || identity.fence_sequence != test.fence_sequence
        || package_spec != &graph.test_package_spec(&test.package_id).map_err(|e| e.to_string())?
        || runner::cargo_binding(&identity.root, tool, variant)? != graph.binding {
        return Err("Cargo test attempt/graph/variant identity changed".into());
    }
    let argv = cargo::test_argv(&graph.binding, manifest_path, package_spec).map_err(|e| e.to_string())?;
    let capture = cargo_capture(verified, graph.binding.clone(), argv, Some(test))
        .map_err(|e| format!("{e:?}"))?;
    cargo::classify_test_capture(graph, test, &capture).map_err(|e| e.to_string())
}

/// Only callable from the dedicated one-shot worker binary, never as a grant.
#[doc(hidden)]
pub fn worker_execute(job: &Job, directory: &std::path::Path) -> Result<serde_json::Value, String> {
    runner::execute(job, directory)
}

fn verify_cargo_inputs(verified: &Verified) -> Result<(), &'static str> {
    let digest = protocol::sha256(&serde_json::to_vec(&verified.job().inputs)
        .map_err(|_| "Cargo ledger serialization failed")?);
    let id = &verified.job().identity;
    if id.source_sha256 != digest || id.input_sha256 != digest {
        return Err("Cargo source/input digests differ from owned input ledger");
    }
    Ok(())
}
