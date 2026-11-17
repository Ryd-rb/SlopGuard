//! A checked-only FluxR 1 module/test boundary. It does not change the passive
//! provider, launch a compiler, use native execution, or publish daemon results.
//!
//! The caller declares every source file, module, operation, compiler/toolchain
//! and stdout-only scope. A separately built, digest-pinned worker runs only the
//! portable, fuel-bounded VM. Its process is supervised and its result rechecked
//! against the original ordered inputs. No returned failure is a clean check.

use fluxr::structured::{Invocation, Limits, SourceDocument, Type, Value, Workspace};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use slophawk_fluxr_provider as provider;
use slophawk_probe_replay as replay;
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::{Duration, Instant};

pub const RECEIPT_FORMAT: &str = "slophawk-fluxr-checked-replay-v1";
pub const WORKER_FORMAT: &str = "slophawk-fluxr-checked-worker-v1";
pub const RUST_TOOLCHAIN: &str = "1.99.1";
pub const MAX_MODULES: usize = 16;
pub const MAX_OPERATIONS: usize = 64;
pub const MAX_CASES: usize = 256;
pub const MAX_ARGS: usize = 64;
pub const MAX_TOTAL_SOURCE_BYTES: usize = 1024 * 1024;
pub const MAX_JOB_BYTES: usize = 2 * 1024 * 1024;
pub const MAX_WORKER_OUTPUT_BYTES: usize = 128 * 1024;
pub const MAX_STDERR_BYTES: usize = 4096;
const MAX_DIAGNOSTICS: usize = 8;
const MAX_FINDINGS: usize = 16;
const MAX_TEXT: usize = 512;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceCodec {
    CanonicalChecked,
    Compact,
    NativeHost,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExecutionMode {
    PortableChecked,
    Native,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ScalarKind {
    I64,
    U64,
    Bool,
}

impl ScalarKind {
    fn ty(self) -> Type {
        match self {
            Self::I64 => Type::I64,
            Self::U64 => Type::U64,
            Self::Bool => Type::Bool,
        }
    }

    fn map_integer(self, value: i64) -> Option<Value> {
        match self {
            Self::I64 => Some(Value::I64(value)),
            Self::U64 => u64::try_from(value).ok().map(Value::U64),
            Self::Bool => match value {
                0 => Some(Value::Bool(false)),
                1 => Some(Value::Bool(true)),
                _ => None,
            },
        }
    }

    fn result_json(self, value: Value) -> Option<serde_json::Value> {
        match (self, value) {
            (Self::I64, Value::I64(n)) => Some(serde_json::Value::from(n)),
            (Self::U64, Value::U64(n)) => Some(serde_json::Value::from(n)),
            (Self::Bool, Value::Bool(b)) => Some(serde_json::Value::from(b)),
            _ => None,
        }
    }

    fn valid_json(self, value: &serde_json::Value) -> bool {
        match self {
            Self::I64 => value.as_i64().is_some(),
            Self::U64 => value.as_u64().is_some(),
            Self::Bool => value.as_bool().is_some(),
        }
    }
}

/// Source paths are explicit and read only. The digest is of the original bytes;
/// no resolution, implicit module search, Unicode normalization or codec guess.
#[derive(Clone, Debug)]
pub struct DeclaredModule {
    pub module_id: String,
    pub logical_document_id: String,
    pub source_path: PathBuf,
    pub source_sha256: String,
    pub codec: SourceCodec,
}

/// Probe args are *ordered*: all module inputs first, then export parameters.
/// Each Boolean uses 0/1, unsigned integers use 0..=i64::MAX, signed integers
/// preserve the complete i64 domain. Other values are individually unsupported.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BoundOperation {
    pub operation: String,
    pub module_id: String,
    pub export: String,
    pub input_kinds: Vec<ScalarKind>,
    pub argument_kinds: Vec<ScalarKind>,
    pub result_kind: ScalarKind,
}

#[derive(Clone, Debug)]
pub struct Request {
    pub modules: Vec<DeclaredModule>,
    pub operations: Vec<BoundOperation>,
    pub output_scope: String,
    pub execution: ExecutionMode,
    pub limits: Limits,
}

/// The coordinator supplies the exact, previously built worker executable and
/// digest. Pin it to a Rust 1.99.1 build of this crate; the digest is checked on
/// both sides of every supervised run. No shell, build or native-host code runs.
#[derive(Clone, Debug)]
pub struct WorkerProfile {
    pub executable: PathBuf,
    pub executable_sha256: String,
    pub rustc_version: String,
    /// Direct child execution deadline, excluding source admission, hashing and
    /// caller-side checked compilation. This is not an end-to-end job deadline.
    pub timeout: Duration,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Outcome {
    Passed,
    Findings,
    Unsupported,
    Refused,
    Stale,
    Failed,
    MalformedOutput,
    TimedOut,
    Cancelled,
}

#[derive(Clone, Debug, Serialize)]
pub struct ModuleEvidence {
    pub module_id: String,
    pub logical_document_id: String,
    pub source_sha256: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct CheckFinding {
    pub index: usize,
    pub operation: String,
    pub args: Vec<i64>,
    pub candidate: String,
    pub reference: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct Receipt {
    pub format: &'static str,
    pub outcome: Outcome,
    /// Observation-only receipts do not assert an independent expected value.
    pub observe_only: bool,
    pub output_scope: String,
    pub compiler_commit: &'static str,
    pub compiler_tree: &'static str,
    pub rust_toolchain: &'static str,
    pub worker_sha256: String,
    /// Digest of the exact caller-supplied case/expectation bytes.
    pub probe_request_sha256: Option<String>,
    /// Digest of canonical ordered operation and integer input pairs.
    pub ordered_inputs_sha256: Option<String>,
    /// Digest of the exact bounded job sent to the supervised worker.
    pub job_sha256: Option<String>,
    pub execution: ExecutionMode,
    pub modules: Vec<ModuleEvidence>,
    pub cases: usize,
    pub compared: usize,
    pub unsupported: usize,
    pub findings_total: usize,
    pub findings: Vec<CheckFinding>,
    pub diagnostics: Vec<String>,
    /// Present only on fully verified, ordered adapter output. JSON array of
    /// scalar values and explicit nulls for unsupported input mappings.
    pub observed_json: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
pub struct ReplayOperation {
    pub operation: String,
    pub compared: usize,
    pub unsupported: usize,
    pub one_sided: usize,
    pub different: usize,
    pub first_difference_index: Option<usize>,
}

#[derive(Clone, Debug, Serialize)]
pub struct ReplayReceipt {
    pub format: &'static str,
    pub outcome: Outcome,
    pub first: Receipt,
    pub second: Receipt,
    pub operations: Vec<ReplayOperation>,
    pub diagnostics: Vec<String>,
}

impl Receipt {
    fn new(request: &Request, profile: &WorkerProfile) -> Self {
        Self {
            format: RECEIPT_FORMAT,
            outcome: Outcome::Refused,
            observe_only: false,
            output_scope: bounded(&request.output_scope),
            compiler_commit: provider::PARSER_COMMIT,
            compiler_tree: provider::PARSER_TREE,
            rust_toolchain: RUST_TOOLCHAIN,
            worker_sha256: bounded(&profile.executable_sha256),
            probe_request_sha256: None,
            ordered_inputs_sha256: None,
            job_sha256: None,
            execution: request.execution,
            modules: request
                .modules
                .iter()
                .take(MAX_MODULES)
                .map(|item| ModuleEvidence {
                    module_id: bounded(&item.module_id),
                    logical_document_id: bounded(&item.logical_document_id),
                    source_sha256: bounded(&item.source_sha256),
                })
                .collect(),
            cases: 0,
            compared: 0,
            unsupported: 0,
            findings_total: 0,
            findings: Vec::new(),
            diagnostics: Vec::new(),
            observed_json: None,
        }
    }

    fn error(&mut self, kind: Outcome, message: impl AsRef<str>) {
        self.outcome = kind;
        if self.diagnostics.len() < MAX_DIAGNOSTICS {
            self.diagnostics.push(bounded(message.as_ref()));
        }
    }
}

fn bounded(text: &str) -> String {
    text.chars().take(MAX_TEXT).collect()
}

fn sha256(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn valid_digest(digest: &str) -> bool {
    digest.len() == 64
        && digest
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

fn valid_label(label: &str) -> bool {
    !label.is_empty()
        && label.len() <= 128
        && label
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'/' | b':'))
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct WireModule {
    module_id: String,
    logical_document_id: String,
    source_sha256: String,
    source: String,
    codec: SourceCodec,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct WireInput {
    operation: String,
    args: Vec<i64>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct WireJob {
    format: String,
    compiler_commit: String,
    compiler_tree: String,
    rust_toolchain: String,
    execution: ExecutionMode,
    output_scope: String,
    modules: Vec<WireModule>,
    operations: Vec<BoundOperation>,
    inputs: Vec<WireInput>,
    limits: Limits,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum WireResult {
    Value { value: serde_json::Value },
    Unsupported { reason: String },
    RuntimeError { message: String },
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct WireCase {
    operation: String,
    args: Vec<i64>,
    result: WireResult,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct WireReply {
    format: String,
    job_sha256: String,
    output_scope: String,
    cases: Vec<WireCase>,
}

fn checked_limits(limits: Limits) -> bool {
    limits.instructions <= 1_000_000 && limits.cells <= 262_144 && limits.frames <= 256
}

/// Check a fully declared plan. This function executes no code, reads no
/// implicit paths, and retains no facts from failed checked compilation.
fn validate_plan(plan: &WireJob) -> Result<(), String> {
    if plan.format != WORKER_FORMAT
        || plan.compiler_commit != provider::PARSER_COMMIT
        || plan.compiler_tree != provider::PARSER_TREE
        || plan.rust_toolchain != RUST_TOOLCHAIN
    {
        return Err("worker/compiler toolchain identity differs from the pinned contract".into());
    }
    if plan.execution != ExecutionMode::PortableChecked {
        return Err("native execution is not qualified".into());
    }
    if !valid_label(&plan.output_scope)
        || plan.modules.is_empty()
        || plan.modules.len() > MAX_MODULES
        || plan.operations.is_empty()
        || plan.operations.len() > MAX_OPERATIONS
        || plan.inputs.is_empty()
        || plan.inputs.len() > MAX_CASES
        || !checked_limits(plan.limits)
    {
        return Err("plan scope, counts or execution budgets are invalid".into());
    }
    let mut bytes = 0usize;
    let mut modules = BTreeMap::new();
    for item in &plan.modules {
        if !valid_label(&item.module_id)
            || !item.logical_document_id.ends_with(".fluxr")
            || item.logical_document_id.len() > 128
            || item.logical_document_id.chars().any(char::is_control)
            || item.codec != SourceCodec::CanonicalChecked
            || !valid_digest(&item.source_sha256)
        {
            return Err("unsupported codec or invalid declared source identity".into());
        }
        bytes = bytes.saturating_add(item.source.len());
        if bytes > MAX_TOTAL_SOURCE_BYTES || item.source.len() > provider::DEFAULT_MAX_SOURCE_BYTES
        {
            return Err("declared source budget exceeded".into());
        }
        if sha256(item.source.as_bytes()) != item.source_sha256 {
            return Err("stale source bytes differ from declared source SHA-256".into());
        }
        let facts = provider::analyze(provider::Request {
            logical_document_id: &item.logical_document_id,
            declared_module_context: &item.module_id,
            source_bytes: item.source.as_bytes(),
            format: provider::SourceFormat::CanonicalChecked,
            limits: provider::Limits::default(),
        })
        .map_err(|failure| format!("checked source {:?}: {}", failure.kind, failure.message))?;
        if !facts.native_items.is_empty() {
            return Err("explicit native items are unsupported by checked execution".into());
        }
        let source = SourceDocument::parse(&item.source).map_err(|e| bounded(&e.to_string()))?;
        let compiled = source.compile().map_err(|e| bounded(&e.to_string()))?;
        if !compiled
            .host_requirements()
            .map_err(|e| bounded(&e.to_string()))?
            .is_empty()
        {
            return Err("host services are unsupported by checked execution".into());
        }
        if modules.insert(item.module_id.as_str(), compiled).is_some() {
            return Err("duplicate module identity".into());
        }
    }
    let mut names = BTreeSet::new();
    for op in &plan.operations {
        if !valid_label(&op.operation)
            || !valid_label(&op.module_id)
            || !valid_label(&op.export)
            || op.input_kinds.len() + op.argument_kinds.len() > MAX_ARGS
            || !names.insert(op.operation.as_str())
        {
            return Err("invalid or duplicate declared operation".into());
        }
        let module = modules
            .get(op.module_id.as_str())
            .ok_or("operation references an undeclared module")?;
        let inputs = module.input_bindings();
        let (args, result) = module
            .export_signatures()
            .get(&op.export)
            .ok_or("operation references an unexported function")?;
        if inputs.len() != op.input_kinds.len()
            || inputs
                .iter()
                .zip(&op.input_kinds)
                .any(|(found, declared)| found.ty != declared.ty())
            || args.len() != op.argument_kinds.len()
            || args
                .iter()
                .zip(&op.argument_kinds)
                .any(|(found, declared)| *found != declared.ty())
            || result != &op.result_kind.ty()
        {
            return Err("operation scalar mapping disagrees with checked export signature".into());
        }
        if module.export_uses_host(&op.export) {
            return Err("checked export uses an unqualified host service".into());
        }
    }
    for input in &plan.inputs {
        if !valid_label(&input.operation) || input.args.len() > MAX_ARGS {
            return Err("invalid ordered probe input".into());
        }
    }
    Ok(())
}

fn input_values(op: &BoundOperation, args: &[i64]) -> Option<(Vec<Value>, Vec<Value>)> {
    if args.len() != op.input_kinds.len() + op.argument_kinds.len() {
        return None;
    }
    let split = op.input_kinds.len();
    let inputs = op
        .input_kinds
        .iter()
        .zip(&args[..split])
        .map(|(kind, value)| kind.map_integer(*value))
        .collect::<Option<Vec<_>>>()?;
    let params = op
        .argument_kinds
        .iter()
        .zip(&args[split..])
        .map(|(kind, value)| kind.map_integer(*value))
        .collect::<Option<Vec<_>>>()?;
    Some((inputs, params))
}

fn supported_case<'a>(job: &'a WireJob, input: &WireInput) -> Option<&'a BoundOperation> {
    let op = job
        .operations
        .iter()
        .find(|op| op.operation == input.operation)?;
    input_values(op, &input.args)?;
    Some(op)
}

fn execute_job(job: &WireJob) -> Result<Vec<WireCase>, String> {
    validate_plan(job)?;
    let mut compiled = BTreeMap::new();
    for module in &job.modules {
        let document =
            SourceDocument::parse(&module.source).map_err(|e| bounded(&e.to_string()))?;
        compiled.insert(
            module.module_id.as_str(),
            document.compile().map_err(|e| bounded(&e.to_string()))?,
        );
    }
    let mut results = Vec::with_capacity(job.inputs.len());
    for input in &job.inputs {
        let result = if let Some(op) = supported_case(job, input) {
            let (inputs, arguments) = input_values(op, &input.args)
                .ok_or("validated scalar mapping unexpectedly changed")?;
            let module = compiled
                .get(op.module_id.as_str())
                .ok_or("validated module unexpectedly missing")?;
            let outcome = module.run_portable_with(
                Invocation {
                    export: &op.export,
                    inputs: &inputs,
                    arguments: &arguments,
                    limits: job.limits,
                },
                &mut Workspace::default(),
            );
            match outcome {
                Ok(value) => match op.result_kind.result_json(value) {
                    Some(value) => WireResult::Value { value },
                    None => WireResult::RuntimeError {
                        message: "checked result does not match declared scalar type".into(),
                    },
                },
                Err(error) => WireResult::RuntimeError {
                    message: bounded(&error.to_string()),
                },
            }
        } else {
            WireResult::Unsupported {
                reason: "operation or declared scalar mapping is unsupported".into(),
            }
        };
        results.push(WireCase {
            operation: input.operation.clone(),
            args: input.args.clone(),
            result,
        });
    }
    Ok(results)
}

/// The dedicated worker's only protocol. Parse one bounded JSON request and
/// answer with bound results; no filesystem reads, commands or native services.
/// The parent hashes the exact request bytes, validates every response field,
/// and discards results from nonzero exits or malformed frames.
pub fn execute_wire(request_bytes: &[u8]) -> Result<Vec<u8>, String> {
    if request_bytes.len() > MAX_JOB_BYTES {
        return Err("worker request byte budget exceeded".into());
    }
    let job: WireJob = serde_json::from_slice(request_bytes)
        .map_err(|error| format!("invalid worker request: {}", bounded(&error.to_string())))?;
    let cases = execute_job(&job)?;
    let reply = WireReply {
        format: WORKER_FORMAT.into(),
        job_sha256: sha256(request_bytes),
        output_scope: job.output_scope,
        cases,
    };
    let data = serde_json::to_vec(&reply).map_err(|e| bounded(&e.to_string()))?;
    if data.len() > MAX_WORKER_OUTPUT_BYTES {
        return Err("worker response byte budget exceeded".into());
    }
    Ok(data)
}

fn read_bounded(mut reader: impl Read, max: usize) -> std::io::Result<Vec<u8>> {
    let mut bytes = Vec::new();
    reader
        .by_ref()
        .take(max as u64 + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() > max {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "file exceeded bounded read limit",
        ));
    }
    Ok(bytes)
}

fn read_snapshot(module: &DeclaredModule) -> Result<WireModule, String> {
    if !module.source_path.is_absolute() {
        return Err("declared module source path must be absolute".into());
    }
    let metadata = fs::symlink_metadata(&module.source_path)
        .map_err(|error| format!("source metadata failed: {}", bounded(&error.to_string())))?;
    if !metadata.file_type().is_file() || metadata.len() > provider::DEFAULT_MAX_SOURCE_BYTES as u64
    {
        return Err("source must be a regular bounded file, not a symlink".into());
    }
    let bytes = fs::File::open(&module.source_path)
        .and_then(|file| read_bounded(file, provider::DEFAULT_MAX_SOURCE_BYTES))
        .map_err(|error| format!("source read failed: {}", bounded(&error.to_string())))?;
    if !valid_digest(&module.source_sha256) || sha256(&bytes) != module.source_sha256 {
        return Err("stale source digest: declared file content changed".into());
    }
    let source = String::from_utf8(bytes).map_err(|_| "canonical checked source is not UTF-8")?;
    Ok(WireModule {
        module_id: module.module_id.clone(),
        logical_document_id: module.logical_document_id.clone(),
        source_sha256: module.source_sha256.clone(),
        source,
        codec: module.codec,
    })
}

fn read_plan(request: &Request, inputs: Vec<replay::ProbeInput>) -> Result<WireJob, String> {
    let mut modules = Vec::with_capacity(request.modules.len().min(MAX_MODULES));
    if request.modules.len() > MAX_MODULES {
        return Err("too many declared modules".into());
    }
    for module in &request.modules {
        modules.push(read_snapshot(module)?);
    }
    let job = WireJob {
        format: WORKER_FORMAT.into(),
        compiler_commit: provider::PARSER_COMMIT.into(),
        compiler_tree: provider::PARSER_TREE.into(),
        rust_toolchain: RUST_TOOLCHAIN.into(),
        execution: request.execution,
        output_scope: request.output_scope.clone(),
        modules,
        operations: request.operations.clone(),
        inputs: inputs
            .into_iter()
            .map(|x| WireInput {
                operation: x.operation,
                args: x.args,
            })
            .collect(),
        limits: request.limits,
    };
    validate_plan(&job)?;
    Ok(job)
}

fn sources_unchanged(request: &Request) -> Result<(), String> {
    for module in &request.modules {
        read_snapshot(module)?;
    }
    Ok(())
}

#[derive(Debug)]
enum ReplyFailure {
    Malformed(String),
    Runtime(String),
}

fn validate_reply(
    job: &WireJob,
    bytes: &[u8],
    request_digest: &str,
) -> Result<Vec<serde_json::Value>, ReplyFailure> {
    if bytes.len() > MAX_WORKER_OUTPUT_BYTES {
        return Err(ReplyFailure::Malformed(
            "adapter output exceeded budget".into(),
        ));
    }
    let reply: WireReply = serde_json::from_slice(bytes).map_err(|error| {
        ReplyFailure::Malformed(format!(
            "malformed adapter output: {}",
            bounded(&error.to_string())
        ))
    })?;
    if reply.format != WORKER_FORMAT
        || reply.job_sha256 != request_digest
        || reply.output_scope != job.output_scope
        || reply.cases.len() != job.inputs.len()
    {
        return Err(ReplyFailure::Malformed(
            "adapter identity, output scope or ordered result count differs".into(),
        ));
    }
    let mut values = Vec::with_capacity(reply.cases.len());
    for (index, (actual, expected)) in reply.cases.iter().zip(&job.inputs).enumerate() {
        if actual.operation != expected.operation || actual.args != expected.args {
            return Err(ReplyFailure::Malformed(format!(
                "adapter result order or input differs at index {index}"
            )));
        }
        let supported = supported_case(job, expected);
        match (&actual.result, supported) {
            (WireResult::Value { value }, Some(op)) if op.result_kind.valid_json(value) => {
                values.push(value.clone());
            }
            (WireResult::Unsupported { reason }, None)
                if !reason.is_empty() && reason.len() <= MAX_TEXT =>
            {
                values.push(serde_json::Value::Null);
            }
            (WireResult::RuntimeError { message }, Some(_))
                if !message.is_empty() && message.len() <= MAX_TEXT =>
            {
                return Err(ReplyFailure::Runtime(format!(
                    "checked runtime error at input {index}: {}",
                    bounded(message)
                )));
            }
            _ => {
                return Err(ReplyFailure::Malformed(format!(
                    "malformed or dishonest adapter classification at input {index}"
                )));
            }
        }
    }
    Ok(values)
}

fn read_limited(mut reader: impl Read, max: usize) -> (Vec<u8>, bool) {
    let mut buffer = [0u8; 4096];
    let mut received = Vec::new();
    let mut exceeded = false;
    loop {
        match reader.read(&mut buffer) {
            Ok(0) => break,
            Ok(count) if received.len().saturating_add(count) <= max => {
                received.extend_from_slice(&buffer[..count]);
            }
            Ok(_) => exceeded = true, // Drain rather than deadlock on a full pipe.
            Err(_) => {
                exceeded = true;
                break;
            }
        }
    }
    (received, exceeded)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum SupervisionFailure {
    TimedOut,
    Cancelled,
    Failed,
}

fn execute_supervised(
    profile: &WorkerProfile,
    request_bytes: &[u8],
    cancelled: &AtomicBool,
) -> Result<Vec<u8>, (SupervisionFailure, String)> {
    let failure = |kind, message: &str| (kind, bounded(message));
    if cancelled.load(Ordering::Acquire) {
        return Err(failure(
            SupervisionFailure::Cancelled,
            "cancelled before worker start",
        ));
    }
    if profile.rustc_version != RUST_TOOLCHAIN
        || !valid_digest(&profile.executable_sha256)
        || !profile.executable.is_absolute()
        || profile.timeout.is_zero()
        || profile.timeout > Duration::from_secs(60)
    {
        return Err(failure(
            SupervisionFailure::Failed,
            "worker profile or Rust toolchain is not pinned",
        ));
    }
    let verify = || {
        let metadata = fs::symlink_metadata(&profile.executable).map_err(|e| e.to_string())?;
        if !metadata.file_type().is_file() || metadata.len() > 64 * 1024 * 1024 {
            return Err("worker must be a bounded regular executable".to_owned());
        }
        let bytes = fs::File::open(&profile.executable)
            .and_then(|file| read_bounded(file, 64 * 1024 * 1024))
            .map_err(|e| e.to_string())?;
        if sha256(&bytes) != profile.executable_sha256 {
            return Err("worker binary digest does not match declared build".to_owned());
        }
        Ok::<(), String>(())
    };
    verify().map_err(|e| (SupervisionFailure::Failed, bounded(&e)))?;
    if cancelled.load(Ordering::Acquire) {
        return Err(failure(
            SupervisionFailure::Cancelled,
            "cancelled before worker spawn",
        ));
    }
    let mut child = Command::new(&profile.executable)
        .env_clear()
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| (SupervisionFailure::Failed, bounded(&e.to_string())))?;
    let mut stdin = child.stdin.take().expect("piped worker stdin");
    let stdout = child.stdout.take().expect("piped worker stdout");
    let stderr = child.stderr.take().expect("piped worker stderr");
    let bytes = request_bytes.to_vec();
    let writer = thread::spawn(move || stdin.write_all(&bytes));
    let out_reader = thread::spawn(move || read_limited(stdout, MAX_WORKER_OUTPUT_BYTES));
    let err_reader = thread::spawn(move || read_limited(stderr, MAX_STDERR_BYTES));
    let start = Instant::now();
    let mut failed = None;
    let status = loop {
        if cancelled.load(Ordering::Acquire) {
            failed = Some(SupervisionFailure::Cancelled);
            break None;
        }
        if start.elapsed() >= profile.timeout {
            failed = Some(SupervisionFailure::TimedOut);
            break None;
        }
        match child.try_wait() {
            Ok(Some(status)) => break Some(status),
            Ok(None) => thread::sleep(Duration::from_millis(5)),
            Err(_) => {
                failed = Some(SupervisionFailure::Failed);
                break None;
            }
        }
    };
    if status.is_none() {
        let _ = child.kill();
        let _ = child.wait();
    }
    let write_ok = writer.join().is_ok_and(|r| r.is_ok());
    let (output, output_exceeded) = out_reader.join().unwrap_or((Vec::new(), true));
    let (stderr, stderr_exceeded) = err_reader.join().unwrap_or((Vec::new(), true));
    let diagnostic = bounded(&String::from_utf8_lossy(&stderr));
    if let Some(kind) = failed {
        let reason = match kind {
            SupervisionFailure::TimedOut => "worker exceeded the declared execution deadline",
            SupervisionFailure::Cancelled => "worker execution was cancelled",
            SupervisionFailure::Failed => "worker process supervision failed",
        };
        return Err((
            kind,
            if diagnostic.is_empty() {
                reason.into()
            } else {
                format!("{reason}: {diagnostic}")
            },
        ));
    }
    if !write_ok || output_exceeded || stderr_exceeded || !status.is_some_and(|s| s.success()) {
        return Err((
            SupervisionFailure::Failed,
            format!("worker failed or exceeded output budget: {diagnostic}"),
        ));
    }
    verify().map_err(|e| (SupervisionFailure::Failed, bounded(&e)))?;
    Ok(output)
}

#[derive(Clone, Copy)]
enum CaseMode {
    Check,
    Observe,
}

fn run(
    request: &Request,
    cases_bytes: &[u8],
    profile: &WorkerProfile,
    cancelled: &AtomicBool,
    mode: CaseMode,
) -> Receipt {
    let mut receipt = Receipt::new(request, profile);
    receipt.observe_only = matches!(mode, CaseMode::Observe);
    if cases_bytes.len() > replay::MAX_SOURCE_BYTES {
        receipt.error(Outcome::Refused, "probe input byte budget exceeded");
        return receipt;
    }
    let inputs = match mode {
        CaseMode::Check => replay::inputs_for_check_source(cases_bytes),
        CaseMode::Observe => replay::parse_inputs(cases_bytes),
    };
    let inputs = match inputs {
        Ok(value) => value,
        Err(error) => {
            receipt.error(Outcome::Refused, error.to_string());
            return receipt;
        }
    };
    receipt.cases = inputs.len();
    receipt.probe_request_sha256 = Some(sha256(cases_bytes));
    let job = match read_plan(request, inputs) {
        Ok(value) => value,
        Err(error) => {
            let outcome = if error.contains("stale source") {
                Outcome::Stale
            } else if error.contains("unsupported") || error.contains("native") {
                Outcome::Unsupported
            } else {
                Outcome::Refused
            };
            receipt.error(outcome, error);
            return receipt;
        }
    };
    let ordered_bytes = match serde_json::to_vec(&job.inputs) {
        Ok(bytes) => bytes,
        Err(error) => {
            receipt.error(Outcome::Refused, error.to_string());
            return receipt;
        }
    };
    receipt.ordered_inputs_sha256 = Some(sha256(&ordered_bytes));
    let request_bytes = match serde_json::to_vec(&job) {
        Ok(bytes) if bytes.len() <= MAX_JOB_BYTES => bytes,
        _ => {
            receipt.error(
                Outcome::Refused,
                "worker job exceeds serialized byte budget",
            );
            return receipt;
        }
    };
    receipt.job_sha256 = Some(sha256(&request_bytes));
    // Check again immediately before and after the child: a changed disk file
    // cannot satisfy the snapshot even when the worker used old request bytes.
    if let Err(error) = sources_unchanged(request) {
        receipt.error(Outcome::Stale, error);
        return receipt;
    }
    let output = match execute_supervised(profile, &request_bytes, cancelled) {
        Ok(output) => output,
        Err((kind, detail)) => {
            let outcome = match kind {
                SupervisionFailure::TimedOut => Outcome::TimedOut,
                SupervisionFailure::Cancelled => Outcome::Cancelled,
                SupervisionFailure::Failed => Outcome::Failed,
            };
            receipt.error(outcome, detail);
            return receipt;
        }
    };
    if let Err(error) = sources_unchanged(request) {
        receipt.error(Outcome::Stale, error);
        return receipt;
    }
    if cancelled.load(Ordering::Acquire) {
        receipt.error(
            Outcome::Cancelled,
            "cancelled before checked result admission",
        );
        return receipt;
    }
    let observed = match validate_reply(&job, &output, &sha256(&request_bytes)) {
        Ok(value) => value,
        Err(ReplyFailure::Malformed(error)) => {
            receipt.error(Outcome::MalformedOutput, error);
            return receipt;
        }
        Err(ReplyFailure::Runtime(error)) => {
            receipt.error(Outcome::Failed, error);
            return receipt;
        }
    };
    let observed_bytes = match serde_json::to_vec(&observed) {
        Ok(value) => value,
        Err(error) => {
            receipt.error(Outcome::MalformedOutput, error.to_string());
            return receipt;
        }
    };
    if observed_bytes.len() > MAX_WORKER_OUTPUT_BYTES {
        receipt.error(
            Outcome::MalformedOutput,
            "normalized observations exceed output budget",
        );
        return receipt;
    }
    if let CaseMode::Check = mode {
        match replay::check_sources(cases_bytes, &observed_bytes) {
            Ok(summary) => {
                receipt.compared = summary.compared;
                receipt.unsupported = summary.unsupported;
                receipt.findings_total = summary.findings.len();
                for difference in summary.findings.into_iter().take(MAX_FINDINGS) {
                    receipt.findings.push(CheckFinding {
                        index: difference.index,
                        operation: bounded(&difference.input.operation),
                        args: difference.input.args.into_iter().take(MAX_ARGS).collect(),
                        candidate: bounded(difference.candidate.raw_json()),
                        reference: bounded(difference.reference.raw_json()),
                    });
                }
                if receipt.findings_total > MAX_FINDINGS {
                    receipt.error(
                        Outcome::Findings,
                        "receipt retains only the first 16 differences; findings_total is exact",
                    );
                }
            }
            Err(error) => {
                receipt.error(Outcome::Refused, error.to_string());
                return receipt;
            }
        }
    } else {
        receipt.unsupported = observed.iter().filter(|x| x.is_null()).count();
        receipt.compared = observed.len() - receipt.unsupported;
    }
    receipt.observed_json = String::from_utf8(observed_bytes).ok();
    receipt.outcome = if !receipt.findings.is_empty() {
        Outcome::Findings
    } else if receipt.unsupported != 0 {
        Outcome::Unsupported
    } else {
        Outcome::Passed
    };
    receipt
}

/// Check external independent expectations with the shared lossless replay
/// rules. A checked VM result and an expected result must never be conflated.
pub fn check(
    request: &Request,
    expectations_json: &[u8],
    profile: &WorkerProfile,
    cancelled: &AtomicBool,
) -> Receipt {
    run(
        request,
        expectations_json,
        profile,
        cancelled,
        CaseMode::Check,
    )
}

/// Capture ordered scalar observations without calling them passing assertions.
pub fn observe(
    request: &Request,
    inputs_json: &[u8],
    profile: &WorkerProfile,
    cancelled: &AtomicBool,
) -> Receipt {
    run(request, inputs_json, profile, cancelled, CaseMode::Observe)
}

/// Execute the *same* ordered input scope twice in fresh worker processes.
/// Exact source/digest, worker, budget and output-scope binding is revalidated
/// separately on both passes. The shared replay library compares observations.
pub fn replay_ordered(
    request: &Request,
    inputs_json: &[u8],
    profile: &WorkerProfile,
    cancelled: &AtomicBool,
) -> ReplayReceipt {
    let first = observe(request, inputs_json, profile, cancelled);
    let second = if matches!(first.outcome, Outcome::Passed | Outcome::Unsupported) {
        observe(request, inputs_json, profile, cancelled)
    } else {
        let mut skipped = Receipt::new(request, profile);
        skipped.observe_only = true;
        skipped.error(
            Outcome::Refused,
            "second run skipped because first run was not admitted",
        );
        skipped
    };
    let stopped_before_second = !matches!(first.outcome, Outcome::Passed | Outcome::Unsupported);
    let mut result = ReplayReceipt {
        format: RECEIPT_FORMAT,
        outcome: if stopped_before_second {
            first.outcome
        } else {
            second.outcome
        },
        first,
        second,
        operations: Vec::new(),
        diagnostics: Vec::new(),
    };
    let (Some(left), Some(right)) = (
        result.first.observed_json.as_ref(),
        result.second.observed_json.as_ref(),
    ) else {
        return result;
    };
    if result.first.job_sha256 != result.second.job_sha256
        || result.first.ordered_inputs_sha256 != result.second.ordered_inputs_sha256
    {
        result.outcome = Outcome::Stale;
        result
            .diagnostics
            .push("replay job or ordered-input identity changed".into());
        return result;
    }
    match replay::compare_sources(inputs_json, left.as_bytes(), right.as_bytes()) {
        Ok(groups) => {
            let mut differing = false;
            let mut unsupported = false;
            for group in groups {
                differing |= group.different != 0 || group.one_sided != 0;
                unsupported |= group.unsupported != 0;
                result.operations.push(ReplayOperation {
                    operation: bounded(&group.operation),
                    compared: group.compared,
                    unsupported: group.unsupported,
                    one_sided: group.one_sided,
                    different: group.different,
                    first_difference_index: group.first_difference.map(|x| x.index),
                });
            }
            result.outcome = if differing {
                Outcome::Findings
            } else if unsupported {
                Outcome::Unsupported
            } else {
                Outcome::Passed
            };
        }
        Err(error) => {
            result.outcome = Outcome::Refused;
            result.diagnostics.push(bounded(&error.to_string()));
        }
    }
    result
}

/// Passive code paths can call this without opening or running a worker. They
/// still cannot claim checked execution or receipt coverage from source facts.
pub fn passive_checked_facts(
    logical_document_id: &str,
    declared_module_id: &str,
    source: &[u8],
) -> Result<provider::FactBatch, provider::AnalysisFailure> {
    provider::analyze(provider::Request {
        logical_document_id,
        declared_module_context: declared_module_id,
        source_bytes: source,
        format: provider::SourceFormat::CanonicalChecked,
        limits: provider::Limits::default(),
    })
}

/// Utility for a coordinator that previously admitted and built an exact worker.
/// This hashes bytes only; it is not an executable attestation or version proof.
pub fn sha256_file(path: &Path) -> std::io::Result<String> {
    let mut file = fs::File::open(path)?;
    let mut digest = Sha256::new();
    let mut buffer = [0u8; 8192];
    loop {
        let count = file.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        digest.update(&buffer[..count]);
    }
    Ok(format!("{:x}", digest.finalize()))
}

#[cfg(test)]
mod unit_tests {
    use super::*;

    #[test]
    fn bounded_read_accepts_limit_and_stops_at_first_excess_byte() {
        let mut reader = std::io::Cursor::new(vec![1u8; 1024]);
        let error = read_bounded(&mut reader, 16).unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::InvalidData);
        assert_eq!(reader.position(), 17);
        assert_eq!(read_bounded(&[2u8; 16][..], 16).unwrap(), vec![2u8; 16]);
        assert!(read_bounded(&[][..], 0).unwrap().is_empty());
    }

    #[test]
    fn bool_and_unsigned_mapping_refuse_unknown_integers() {
        assert_eq!(ScalarKind::Bool.map_integer(0), Some(Value::Bool(false)));
        assert_eq!(ScalarKind::Bool.map_integer(1), Some(Value::Bool(true)));
        assert_eq!(ScalarKind::Bool.map_integer(2), None);
        assert_eq!(ScalarKind::U64.map_integer(-1), None);
        assert_eq!(
            ScalarKind::I64.map_integer(i64::MIN),
            Some(Value::I64(i64::MIN))
        );
    }

    #[test]
    fn malformed_output_refuses_reordering_and_forged_unsupported() {
        let source = "fluxr 1\npub run 1 x int int ret add x 1\n";
        let job = WireJob {
            format: WORKER_FORMAT.into(),
            compiler_commit: provider::PARSER_COMMIT.into(),
            compiler_tree: provider::PARSER_TREE.into(),
            rust_toolchain: RUST_TOOLCHAIN.into(),
            output_scope: "tests/probes".into(),
            execution: ExecutionMode::PortableChecked,
            modules: vec![WireModule {
                module_id: "math".into(),
                logical_document_id: "math.fluxr".into(),
                source_sha256: sha256(source.as_bytes()),
                source: source.into(),
                codec: SourceCodec::CanonicalChecked,
            }],
            operations: vec![BoundOperation {
                operation: "plus_one".into(),
                module_id: "math".into(),
                export: "run".into(),
                input_kinds: vec![],
                argument_kinds: vec![ScalarKind::I64],
                result_kind: ScalarKind::I64,
            }],
            inputs: vec![WireInput {
                operation: "plus_one".into(),
                args: vec![4],
            }],
            limits: Limits::default(),
        };
        let request = serde_json::to_vec(&job).unwrap();
        let reply = execute_wire(&request).unwrap();
        assert_eq!(
            validate_reply(&job, &reply, &sha256(&request)).unwrap(),
            [serde_json::json!(5)]
        );
        let mut bad: WireReply = serde_json::from_slice(&reply).unwrap();
        bad.cases[0].args = vec![5];
        assert!(
            validate_reply(&job, &serde_json::to_vec(&bad).unwrap(), &sha256(&request)).is_err()
        );
        bad.cases[0].args = vec![4];
        bad.cases[0].result = WireResult::Unsupported {
            reason: "forged".into(),
        };
        assert!(
            validate_reply(&job, &serde_json::to_vec(&bad).unwrap(), &sha256(&request)).is_err()
        );
        assert!(validate_reply(&job, b"not JSON", &sha256(&request)).is_err());
    }
}
