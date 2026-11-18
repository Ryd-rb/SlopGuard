//! Executor for the *already admitted* private worker process. No arbitrary argv,
//! shell fragments, dependency installation, network or implicit Python plugins.
use crate::admission::read_bounded;
use crate::protocol::{self, Budget, CargoVariant, Executable, FluxrOperation, Job, MsvcToolchain, Operation};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use slophawk_rust_project_context as cargo_context;
use slophawk_fluxr_checked_context as fluxr_context;
use std::fs::{self, File};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::AtomicBool;
use std::thread;
use std::time::Duration;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Capture {
    pub exit: i32,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
}

fn child_capture(mut command: Command, budget: &Budget) -> Result<Capture, String> {
    let mut child = command.stdout(Stdio::piped()).stderr(Stdio::piped())
        .stdin(Stdio::null()).spawn().map_err(|_| "approved child could not start".to_string())?;
    let stdout = child.stdout.take().ok_or("missing child stdout")?;
    let stderr = child.stderr.take().ok_or("missing child stderr")?;
    let cap_out = budget.stdout_bytes;
    let cap_err = budget.stderr_bytes;
    // The parent process-group deadline kills these children as well. Both
    // pipes are drained independently, enforcing caps while data is consumed.
    let out = thread::spawn(move || read_bounded(stdout, cap_out));
    let err = thread::spawn(move || read_bounded(stderr, cap_err));
    let status = child.wait().map_err(|_| "approved child could not be reaped")?;
    let stdout = out.join().map_err(|_| "child stdout reader failed")?
        .map_err(|_| "child stdout overflow")?;
    let stderr = err.join().map_err(|_| "child stderr reader failed")?
        .map_err(|_| "child stderr overflow")?;
    Ok(Capture { exit: status.code().unwrap_or(-1), stdout, stderr })
}

pub(crate) fn controlled_env(command: &mut Command) -> Result<(), String> {
    command.env_clear().env("LC_ALL", "C").env("LANG", "C");
    #[cfg(windows)]
    command.env("SystemRoot", windows_directory()?);
    Ok(())
}

#[cfg(windows)]
fn windows_directory() -> Result<PathBuf, String> {
    use std::ffi::OsString;
    use std::os::windows::ffi::OsStringExt;
    use windows_sys::Win32::System::SystemInformation::GetSystemWindowsDirectoryW;
    let mut text = [0u16; 32768];
    let length = unsafe { GetSystemWindowsDirectoryW(text.as_mut_ptr(), text.len() as u32) } as usize;
    if length == 0 || length >= text.len() {
        return Err("Windows system directory could not be determined".into());
    }
    Ok(OsString::from_wide(&text[..length]).into())
}

/// Exact controlled Cargo environment recipe. Per-attempt scratch paths are
/// represented by fixed tokens; no inherited variables or network are used.
pub fn cargo_environment_digest(cargo: &Executable, rustc: &Executable, rustdoc: &Executable) -> String {
    let representation = cargo_environment(cargo, rustc, rustdoc);
    protocol::sha256(&protocol::ordered_json(representation).expect("static cargo environment schema"))
}

fn cargo_environment(cargo: &Executable, rustc: &Executable, rustdoc: &Executable) -> Value {
    serde_json::json!({
        "schema": "slophawk/cargo-controlled-environment/2",
        "cargo": cargo,
        "rustc": rustc,
        "rustdoc": rustdoc,
        "LC_ALL": "C", "LANG": "C", "PATH": "/usr/bin:/bin",
        "RUSTC": &rustc.path, "RUSTDOC": &rustdoc.path, "CARGO_HOME": "$ATTEMPT/cargo-home",
        "CARGO_TARGET_DIR": "$ATTEMPT/target", "CARGO_NET_OFFLINE": "true",
        "CARGO_INCREMENTAL": "0", "stdin": "null",
    })
}

pub fn cargo_msvc_environment_digest(cargo: &Executable, rustc: &Executable, rustdoc: &Executable,
    msvc: &MsvcToolchain, target: &str) -> String {
    let mut representation = cargo_environment(cargo, rustc, rustdoc);
    representation["schema"] = "slophawk/cargo-controlled-environment/3-msvc".into();
    representation["PATH"] = "$SYSTEMROOT/System32".into();
    representation["SystemRoot"] = "$OS_SYSTEM_WINDOWS_DIRECTORY".into();
    representation["TEMP"] = "$ATTEMPT/temp".into();
    representation["TMP"] = "$ATTEMPT/temp".into();
    representation["msvc"] = serde_json::to_value(msvc).expect("MSVC toolchain schema");
    representation["linker_variable"] = msvc_linker_variable(target).into();
    protocol::sha256(&protocol::ordered_json(representation).expect("static cargo environment schema"))
}

fn msvc_linker_variable(target: &str) -> String {
    format!("CARGO_TARGET_{}_LINKER", target.replace('-', "_").to_ascii_uppercase())
}

pub fn cargo_binding(root: &str, tool: &Executable, variant: &CargoVariant) -> Result<cargo_context::Binding, String> {
    let binding = cargo_context::Binding {
        workspace_root: root.to_owned(), workspace_revision: variant.revision,
        epoch: variant.epoch.clone(), git_commit: variant.git_commit.clone(),
        target_triple: variant.target_triple.clone(), rustc_version: variant.rustc_version.clone(),
        rustc_vv_sha256: variant.rustc_vv_sha256.clone(),
        cargo_executable_sha256: tool.sha256.clone(),
        environment_sha256: variant.environment_sha256.clone(),
        features: cargo_context::Features {
            all: variant.all_features, no_default: variant.no_default_features,
            selected: variant.features.clone(),
        },
    };
    binding.validate().map_err(|e| e.to_string())?;
    Ok(binding)
}

fn run_cargo(job: &Job, directory: &Path, cargo: &Executable, variant: &CargoVariant,
    manifest_path: &str, package: Option<&str>) -> Result<Value, String> {
    let binding = cargo_binding(&job.identity.root, cargo, variant)?;
    let ledger_digest = protocol::sha256(&serde_json::to_vec(&job.inputs)
        .map_err(|_| "Cargo input ledger encoding failed")?);
    if job.identity.source_sha256 != ledger_digest || job.identity.input_sha256 != ledger_digest {
        return Err("Cargo input ledger differs from source and input binding".into());
    }
    let environment = match &variant.msvc {
        None if !cfg!(windows) => cargo_environment_digest(cargo, &variant.rustc, &variant.rustdoc),
        Some(msvc) if cfg!(windows) && msvc.linker.valid()
            && matches!(variant.target_triple.as_str(), "x86_64-pc-windows-msvc" | "aarch64-pc-windows-msvc")
            && (1..=8).contains(&msvc.library_paths.len())
            && msvc.library_paths.iter().all(|path|
                cargo_context::valid_absolute_path(path) && !path.contains(';')) =>
            cargo_msvc_environment_digest(cargo, &variant.rustc, &variant.rustdoc, msvc, &variant.target_triple),
        _ => return Err("Cargo toolchain does not match the supported platform".into()),
    };
    if !variant.rustc.valid() || !variant.rustdoc.valid() || variant.environment_sha256 != environment {
        return Err("Cargo controlled environment differs from admitted identity".into());
    }
    let mut probe = Command::new(&variant.rustc.path);
    controlled_env(&mut probe)?;
    probe.arg("-Vv").current_dir(&job.identity.root);
    let report = child_capture(probe, &job.budget)?;
    if report.exit != 0 || protocol::sha256(&report.stdout) != variant.rustc_vv_sha256 {
        return Err("rustc -Vv identity differs from admitted variant".into());
    }
    if job.identity.committed_revision != variant.revision || job.identity.epoch != variant.epoch
        || job.identity.environment_sha256 != variant.environment_sha256 {
        return Err("Cargo variant differs from admitted workspace fence".into());
    }
    let argv = if let Some(spec) = package {
        cargo_context::test_argv(&binding, manifest_path, spec)
    } else { cargo_context::metadata_argv(&binding, manifest_path) }
        .map_err(|e| e.to_string())?;
    // Cargo reads configuration from ancestor .cargo directories. An unseen
    // config could change tools, target, feature behavior or source locations.
    let root = Path::new(&job.identity.root);
    for ancestor in root.ancestors() {
        for name in [".cargo/config", ".cargo/config.toml"] {
            let candidate = ancestor.join(name);
            if let Ok(meta) = fs::symlink_metadata(&candidate) {
                let admitted = ancestor == root && meta.file_type().is_file()
                    && job.inputs.iter().any(|f| f.relative_path == name);
                if !admitted { return Err("unadmitted Cargo configuration influences tool invocation".into()); }
            }
        }
    }
    let mut command = Command::new(&cargo.path);
    controlled_env(&mut command)?;
    command.args(&argv).current_dir(&job.identity.root)
        .env("RUSTC", &variant.rustc.path)
        .env("RUSTDOC", &variant.rustdoc.path)
        .env("CARGO_NET_OFFLINE", "true")
        .env("CARGO_HOME", directory.join("cargo-home"))
        .env("CARGO_TARGET_DIR", directory.join("target"))
        .env("CARGO_INCREMENTAL", "0");
    #[cfg(not(windows))]
    command.env("PATH", "/usr/bin:/bin");
    #[cfg(windows)]
    {
        let msvc = variant.msvc.as_ref().ok_or("MSVC toolchain not admitted")?;
        let scratch = directory.join("temp");
        fs::create_dir(&scratch).map_err(|_| "MSVC scratch allocation failed")?;
        command.env("PATH", windows_directory()?.join("System32"))
            .env("TEMP", &scratch)
            .env("TMP", &scratch)
            .env("LIB", std::env::join_paths(&msvc.library_paths).map_err(|_| "invalid MSVC library paths")?)
            .env(msvc_linker_variable(&variant.target_triple), &msvc.linker.path);
    }
    fs::create_dir(directory.join("cargo-home")).map_err(|_| "Cargo scratch allocation failed")?;
    let capture = child_capture(command, &job.budget)?;
    if package.is_none() && (capture.stdout.len() > cargo_context::MAX_METADATA_BYTES
        || capture.stderr.len() > cargo_context::MAX_LOG_BYTES) {
        return Err("metadata capture exceeded classifier bounds".into());
    }
    if package.is_some() {
        if capture.stdout.len().saturating_add(capture.stderr.len()) > job.budget.artifact_bytes {
            return Err("Cargo test artifacts exceeded the attempt budget".into());
        }
        for (name, contents) in [("cargo-stdout", &capture.stdout), ("cargo-stderr", &capture.stderr)] {
            use std::io::Write;
            fs::OpenOptions::new().write(true).create_new(true).open(directory.join(name))
                .and_then(|mut file| file.write_all(contents))
                .map_err(|_| "Cargo test artifact could not be recorded")?;
        }
    }
    serde_json::to_value(capture).map_err(|_| "Cargo capture encoding failed".into())
}

fn read_declared_source(root: &str, relative: &str, sha: &str) -> Result<PathBuf, String> {
    if relative.is_empty() || relative.len() > 1024 || relative.starts_with('/')
        || relative.contains('\\') || relative.contains(':')
        || relative.split('/').any(|part| part.is_empty() || part == "." || part == "..")
        || !protocol::is_sha(sha) { return Err("invalid declared FluxR source path or digest".into()); }
    let path = Path::new(root).join(relative);
    if !crate::admission::physical(&path).map_err(|_| "declared source missing")? {
        return Err("FluxR source path is not canonical inside checkout".into());
    }
    let meta = fs::symlink_metadata(&path).map_err(|_| "source unreadable")?;
    if !meta.file_type().is_file() || meta.len() > 1024 * 1024 {
        return Err("FluxR source is not a bounded regular file".into());
    }
    let contents = read_bounded(File::open(&path).map_err(|_| "source unreadable")?, 1024 * 1024)
        .map_err(|_| "FluxR source exceeds limit")?;
    if protocol::sha256(&contents) != sha { return Err("FluxR source changed".into()); }
    Ok(path)
}

fn scalar(s: &str) -> Result<fluxr_context::ScalarKind, String> {
    match s { "i64" => Ok(fluxr_context::ScalarKind::I64),
        "u64" => Ok(fluxr_context::ScalarKind::U64),
        "bool" => Ok(fluxr_context::ScalarKind::Bool),
        _ => Err("unsupported FluxR scalar type".into()) }
}
fn bound_operation(spec: &FluxrOperation) -> Result<fluxr_context::BoundOperation, String> {
    Ok(fluxr_context::BoundOperation {
        operation: spec.operation.clone(), module_id: spec.module_id.clone(), export: spec.export.clone(),
        input_kinds: spec.input_kinds.iter().map(|s| scalar(s)).collect::<Result<_, _>>()?,
        argument_kinds: spec.argument_kinds.iter().map(|s| scalar(s)).collect::<Result<_, _>>()?,
        result_kind: scalar(&spec.result_kind)?,
    })
}

fn run_fluxr(job: &Job, checked_worker: &Executable,
    modules: &[protocol::FluxrModule], operations: &[FluxrOperation], cases: &Value,
    limits: &Value, output_scope: &str, mode: &str) -> Result<Value, String> {
    use fluxr_context::{ExecutionMode, SourceCodec};
    if modules.is_empty() || modules.len() > fluxr_context::MAX_MODULES || operations.is_empty()
        || operations.len() > fluxr_context::MAX_OPERATIONS { return Err("FluxR scope limit".into()); }
    let declared_digest = protocol::sha256(&serde_json::to_vec(modules)
        .map_err(|_| "FluxR module binding encoding failed")?);
    if job.identity.source_sha256 != declared_digest {
        return Err("FluxR module source binding changed".into());
    }
    let mut declared = Vec::new();
    for module in modules {
        if module.codec != "canonical_checked" {
            return Err("FluxR compact/native codec not qualified".into());
        }
        let path = read_declared_source(&job.identity.root, &module.relative_path, &module.sha256)?;
        declared.push(fluxr_context::DeclaredModule {
            module_id: module.module_id.clone(), logical_document_id: module.logical_document_id.clone(),
            source_path: path, source_sha256: module.sha256.clone(), codec: SourceCodec::CanonicalChecked,
        });
    }
    let original_source = declared.iter().map(|m| m.source_sha256.clone()).collect::<Vec<_>>();
    let request = fluxr_context::Request {
        modules: declared,
        operations: operations.iter().map(bound_operation).collect::<Result<_, _>>()?,
        output_scope: output_scope.into(),
        execution: ExecutionMode::PortableChecked,
        limits: if limits.is_null() { Default::default() }
            else { serde_json::from_value(limits.clone()).map_err(|_| "FluxR limits are malformed")? },
    };
    if mode != "check" && mode != "observe" && mode != "replay" { return Err("unknown FluxR mode".into()); }
    let bytes = protocol::ordered_json(cases.clone()).map_err(|_| "FluxR cases are malformed")?;
    if protocol::sha256(&bytes) != job.identity.input_sha256 {
        return Err("FluxR ordered case digest changed".into());
    }
    if bytes.len() > fluxr_context::MAX_JOB_BYTES { return Err("FluxR case budget exceeded".into()); }
    let profile = fluxr_context::WorkerProfile {
        executable: PathBuf::from(&checked_worker.path),
        executable_sha256: checked_worker.sha256.clone(),
        rustc_version: fluxr_context::RUST_TOOLCHAIN.to_owned(),
        timeout: Duration::from_secs(30),
    };
    // The caller's outer worker group enforces the whole operation (source reads,
    // compiler checks, two replay runs), not merely FluxR's direct child deadline.
    let cancelled = AtomicBool::new(false);
    let receipt = match mode {
        "check" => serde_json::to_value(fluxr_context::check(&request, &bytes, &profile, &cancelled)),
        "observe" => serde_json::to_value(fluxr_context::observe(&request, &bytes, &profile, &cancelled)),
        _ => serde_json::to_value(fluxr_context::replay_ordered(&request, &bytes, &profile, &cancelled)),
    }.map_err(|_| "FluxR receipt serialization failed")?;
    for (module, original) in request.modules.iter().zip(&original_source) {
        read_declared_source(&job.identity.root,
            &modules.iter().find(|m| m.module_id == module.module_id).ok_or("FluxR source binding missing")?.relative_path,
            original)?;
    }
    Ok(receipt)
}

/// The worker is a narrowly typed executor. The trusted coordinator, not these
/// repository-controlled fields, authenticates the grant and accepted tool hash.
pub fn execute(job: &Job, directory: &Path) -> Result<Value, String> {
    if !job.valid() || !directory.is_absolute() { return Err("invalid migration job".into()); }
    match &job.operation {
        Operation::Pytest { interpreter, project_root, effective_config_sha256, .. } =>
            crate::python::run_python(job, directory, interpreter, project_root, effective_config_sha256.as_deref()),
        Operation::CargoMetadata { cargo, variant, manifest_path } =>
            run_cargo(job, directory, cargo, variant, manifest_path, None),
        Operation::CargoTest { cargo, variant, manifest_path, package_spec, graph_sha256 } => {
            if !protocol::is_sha(graph_sha256) || graph_sha256 != &job.identity.context_sha256 {
                return Err("Cargo test graph identity mismatch".into());
            }
            run_cargo(job, directory, cargo, variant, manifest_path, Some(package_spec))
        }
        Operation::FluxrCheck { checked_worker, modules, operations, cases, limits, output_scope, mode } =>
            run_fluxr(job, checked_worker, modules, operations, cases, limits, output_scope, mode),
    }
}

pub fn invoke_python(interpreter: &Executable, cwd: &Path, args: &[String],
    budget: &Budget) -> Result<Capture, String> {
    let mut command = Command::new(&interpreter.path);
    controlled_env(&mut command)?;
    command.current_dir(cwd).args(args)
        .env("PYTEST_DISABLE_PLUGIN_AUTOLOAD", "1")
        .env("PYTHONNOUSERSITE", "1")
        .env("PYTHONDONTWRITEBYTECODE", "1")
        .env("PYTHONHASHSEED", "0");
    child_capture(command, budget)
}

pub fn python_args(path: &Path, collect: bool) -> Vec<String> {
    let mut args = vec!["-I", "-B", "-m", "pytest", "-p", "pytest_jsonreport",
        "-p", "no:cacheprovider", "--json-report",
        "--disable-warnings", "--color=no", "--tb=short", "-q"]
        .into_iter().map(str::to_owned).collect::<Vec<_>>();
    args.push(format!("--json-report-file={}", path.display()));
    if collect { args.push("--collect-only".into()); }
    args
}

pub fn probe_python(interpreter: &Executable, root: &Path, budget: &Budget) -> Result<(), String> {
    // Introspection uses only stdlib distribution metadata; no SlopHawk Python app
    // or plugin is installed or imported. The selected repo is not on PYTHONPATH.
    let script = "import sys, importlib.metadata as m; print(sys.implementation.name, sys.version_info.major, sys.version_info.minor, m.version('pytest'), m.version('pytest-json-report'))";
    let capture = invoke_python(interpreter, root, &["-I".into(), "-c".into(), script.into()], budget)?;
    if capture.exit != 0 { return Err("Python distribution probe failed".into()); }
    let tokens = std::str::from_utf8(&capture.stdout).map_err(|_| "invalid Python probe encoding")?
        .trim().split_whitespace().collect::<Vec<_>>();
    if tokens.len() != 5 || tokens[0] != "cpython" || tokens[1] != "3"
        || tokens[2].parse::<u32>().map_err(|_| "invalid CPython version")? < 10
        || tokens[3] != "9.0.2" || tokens[4] != "1.5.0" {
        return Err("requires CPython >=3.10, pytest 9.0.2 and pytest-json-report 1.5.0".into());
    }
    Ok(())
}

pub fn artifact(path: &Path, budget: &Budget, excerpt: &str) -> Result<(Vec<u8>, crate::python::ArtifactWire), String> {
    let meta = fs::symlink_metadata(path).map_err(|_| "pytest report is missing")?;
    if !meta.file_type().is_file() || meta.len() == 0 || meta.len() as usize > budget.artifact_bytes {
        return Err("pytest artifact absent or exceeds budget".into());
    }
    let data = read_bounded(File::open(path).map_err(|_| "pytest report unreadable")?, budget.artifact_bytes)
        .map_err(|_| "pytest report exceeds budget")?;
    let modified_ms = meta.modified().map_err(|_| "pytest report mtime missing")?
        .duration_since(std::time::UNIX_EPOCH).map_err(|_| "pytest report mtime invalid")?.as_millis() as u64;
    let info = crate::python::ArtifactWire {
        sha256: protocol::sha256(&data), byte_count: data.len() as u64, modified_ms,
        excerpt: excerpt.chars().filter(|c| !c.is_control() || matches!(c, '\n' | '\r' | '\t'))
            .take(1024).collect(),
    };
    Ok((data, info))
}
