//! Opt-in, host-granted scalar foreground coordinator. The shipped CLI and scalar wire-v1
//! are intentionally unchanged. All user-visible decisions, artifact seals and replay
//! classifications live here; Roslyn and execution remain in the .NET worker.
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use slophawk_scalar_paths::{parse_wire, search};
use std::collections::{HashMap, HashSet};
use std::env;
use std::fs::{self, File};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::AtomicBool;
use slophawk_worker_scope::{Limits, find_on_path};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const PROTOCOL: &str = "slophawk.migration-job/1";
const FORMAT: &str = "slophawk-scalar-run-v1";
const MAX_FRAME: usize = 16 * 1024 * 1024;
const MAX_SOURCE: usize = 16 * 1024 * 1024;
const WORKER_DEADLINE: Duration = Duration::from_secs(120);

type Result<T> = std::result::Result<T, String>;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "PascalCase")]
pub struct Identity {
    protocol: String,
    operation: String,
    job: String,
    root: String,
    context: String,
    epoch: String,
    revision: String,
    input_digest: String,
    tool_digest: String,
    policy_digest: String,
    budget_ref: String,
    grant_ref: String,
}

impl Identity {
    fn same_authority(&self, other: &Self) -> bool {
        self.protocol == other.protocol
            && self.root == other.root
            && self.context == other.context
            && self.epoch == other.epoch
            && self.revision == other.revision
            && self.input_digest == other.input_digest
            && self.tool_digest == other.tool_digest
            && self.policy_digest == other.policy_digest
            && self.budget_ref == other.budget_ref
            && self.grant_ref == other.grant_ref
    }
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
struct Spec {
    #[allow(dead_code)]
    build: Value,
    targets: Vec<Target>,
    rules: String,
    published: String,
    #[serde(default = "default_per_operation")]
    per_operation: usize,
    #[serde(default = "default_work")]
    work_limit: usize,
    #[serde(default = "default_retest")]
    retest_limit: usize,
    #[serde(default = "default_seed")]
    #[allow(dead_code)]
    seed: i32,
}
fn default_per_operation() -> usize { 256 }
fn default_work() -> usize { 128 }
fn default_retest() -> usize { 16 }
fn default_seed() -> i32 { 1 }

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
struct Target {
    operation: String,
    document: String,
    signature: String,
}

#[derive(Clone, Debug)]
struct Source {
    path: PathBuf,
    root: PathBuf,
    scratch: PathBuf,
    spec: Spec,
    digest: String,
    policy_digest: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "PascalCase")]
struct Entry {
    file: String,
    length: u64,
    sha256: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
struct Seal {
    format: String,
    kind: String,
    files: Vec<Entry>,
    identity: Identity,
    mac: String,
}

#[derive(Serialize)]
#[serde(rename_all = "PascalCase")]
struct UnsignedSeal<'a> {
    format: &'a str,
    kind: &'a str,
    files: &'a [Entry],
    identity: &'a Identity,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
struct Terminal {
    identity: Identity,
    status: String,
    reason: Option<String>,
    omissions: Vec<String>,
    value: Option<Value>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "PascalCase")]
struct Expectation {
    operation: String,
    args: Vec<i64>,
    expected: Value,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
struct BuildReceipt {
    compiler_sha256: String,
    runtime_version: String,
    candidate_sha256: String,
    compiler_inputs: Value,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
struct InspectResult {
    compiler_sha256: String,
    candidate_sha256: String,
    runtime_version: String,
    compiler_inputs: Value,
    targets: Vec<InspectedTarget>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
struct InspectedTarget {
    operation: String,
    signature: String,
    bound: bool,
    eligible: bool,
    reason: Option<String>,
    rule_source: Option<String>,
    parameters: Vec<String>,
    facts: String,
    graph: Option<Value>,
    original: Vec<Expectation>,
    ordinary_keys: Vec<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
struct Evaluated {
    index: usize,
    expectation: Option<Expectation>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
struct ReplayResult {
    observed: Vec<Value>,
    ordering_stable: bool,
    replays: usize,
    unknown: Vec<String>,
    candidate_sha256: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
struct PathInputs {
    spec: String,
    spec_sha256: String,
    rules_sha256: String,
    published_sha256: String,
    compiler_sha256: String,
    runtime_version: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
struct PathCase {
    expectation: Expectation,
    origin: String,
    predicted_goal: Option<String>,
    baseline_order: Option<usize>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
struct MethodPlan {
    operation: String,
    signature: String,
    bound: bool,
    eligible: bool,
    reason: Option<String>,
    rule_source: Option<String>,
    work: usize,
    unknown: Vec<String>,
    rejected_by_domain: usize,
    baseline_cases: usize,
    graph_proposals: usize,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
struct PathPlan {
    identity: Identity,
    inputs: PathInputs,
    scratch: String,
    build: String,
    build_sha256: String,
    candidate_sha256: String,
    compiler_inputs: Value,
    mode: String,
    per_operation: usize,
    retest_limit: usize,
    methods: Vec<MethodPlan>,
    cases: Vec<PathCase>,
    blocker: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
struct Violation {
    original_index: usize,
    retest_job: String,
    case: PathCase,
    observed: Value,
    source: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
struct Scope {
    operation: String,
    signature: String,
    bound: bool,
    reason: Option<String>,
    unknown_frontiers: usize,
    planned_cases: usize,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
struct PathReview {
    identity: Identity,
    plan: String,
    plan_sha256: String,
    mode: String,
    outcome: String,
    cases: usize,
    compared: usize,
    unsupported: usize,
    retests: usize,
    unconfirmed: usize,
    work: usize,
    initial_job: String,
    reproduced: Vec<Violation>,
    unstable: usize,
    blocker: Option<String>,
    scope: Vec<Scope>,
    measured_coverage: String,
    unknown: Vec<String>,
}

#[derive(Clone, Debug)]
struct Proposed {
    operation: String,
    args: Vec<i64>,
    goal: String,
}

fn hash(bytes: &[u8]) -> String {
    Sha256::digest(bytes).iter().map(|b| format!("{b:02X}")).collect()
}

fn hmac(bytes: &[u8], secret: &[u8]) -> String {
    let mut key = if secret.len() > 64 {
        Sha256::digest(secret).to_vec()
    } else {
        secret.to_vec()
    };
    key.resize(64, 0);
    let mut inner = [0x36u8; 64];
    let mut outer = [0x5cu8; 64];
    for (i, b) in key.iter().enumerate() {
        inner[i] ^= b;
        outer[i] ^= b;
    }
    let inside = Sha256::new().chain_update(inner).chain_update(bytes).finalize();
    let out = Sha256::new().chain_update(outer).chain_update(inside).finalize();
    out.iter().map(|b| format!("{b:02X}")).collect()
}

fn secure_equals(a: &str, b: &str) -> bool {
    if a.len() != b.len() { return false; }
    a.bytes().zip(b.bytes()).fold(0u8, |diff, (x, y)| diff | (x ^ y)) == 0
}

fn required_env(key: &str) -> Result<String> {
    let value = env::var(key).map_err(|_| format!("Missing independent host authority {key}"))?;
    if value.is_empty() || value.len() > 512 {
        return Err(format!("Invalid host authority {key}"));
    }
    Ok(value)
}

fn seal_key() -> Result<Vec<u8>> {
    let secret = required_env("SLOPHAWK_PATHS_SEAL_KEY")?.into_bytes();
    if secret.len() < 32 { return Err("The host seal key must contain at least 32 bytes".into()); }
    Ok(secret)
}

fn read_file(path: &Path, limit: usize) -> Result<Vec<u8>> {
    let meta = fs::symlink_metadata(path).map_err(|e| format!("{}: {e}", path.display()))?;
    if !meta.file_type().is_file() || meta.len() as u128 > limit as u128 {
        return Err(format!("Not a bounded ordinary file: {}", path.display()));
    }
    let bytes = fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
    if bytes.len() as u64 != meta.len() || bytes.len() > limit {
        return Err(format!("Changed input: {}", path.display()));
    }
    Ok(bytes)
}

fn source_relative(root: &Path, path: &str) -> Result<PathBuf> {
    let relative = Path::new(path);
    if path.is_empty() || relative.is_absolute() || relative.components().any(|part| {
        matches!(part, std::path::Component::ParentDir | std::path::Component::Prefix(_))
    }) { return Err("Spec dependency escapes its root".into()); }
    let joined = root.join(relative);
    let absolute = joined.canonicalize().map_err(|e| e.to_string())?;
    if !absolute.starts_with(root) { return Err("Spec dependency escaped its root".into()); }
    Ok(absolute)
}

impl Source {
    fn open(spec_path: &Path, scratch_path: &Path) -> Result<Self> {
        let path = spec_path.canonicalize().map_err(|e| e.to_string())?;
        let root = path.parent().ok_or("Spec has no parent")?.to_path_buf();
        let scratch = scratch_path.canonicalize().map_err(|e| e.to_string())?;
        if scratch.starts_with(&root) || root.starts_with(&scratch) || root == scratch {
            return Err("Scratch and controlled source must be disjoint directories".into());
        }
        for dir in [&root, &scratch] {
            let meta = fs::symlink_metadata(dir).map_err(|e| e.to_string())?;
            if !meta.file_type().is_dir() { return Err("Source/scratch must be ordinary directories".into()); }
        }
        let bytes = read_file(&path, MAX_SOURCE)?;
        let digest = hash(&bytes);
        let spec: Spec = serde_json::from_slice(&bytes).map_err(|e| e.to_string())?;
        if spec.targets.is_empty() || spec.targets.len() > 32 ||
            spec.per_operation == 0 || spec.per_operation > 512 ||
            spec.work_limit == 0 || spec.work_limit > 8192 ||
            spec.retest_limit == 0 || spec.retest_limit > 32 ||
            spec.build.is_null() || spec.targets.iter().any(|t| t.operation.is_empty() ||
                t.document.is_empty() || t.signature.is_empty()) ||
            spec.targets.iter().map(|t| &t.operation).collect::<HashSet<_>>().len() != spec.targets.len() {
            return Err("Invalid or ambiguous scalar spec".into());
        }
        let rules = hash(&read_file(&source_relative(&root, &spec.rules)?, MAX_SOURCE)?);
        let published = hash(&read_file(&source_relative(&root, &spec.published)?, MAX_SOURCE)?);
        let policy_digest = hash(format!("{rules}:{published}").as_bytes());
        Ok(Self { path, root, scratch, spec, digest, policy_digest })
    }

    fn identity(&self, operation: &str, worker: &Path) -> Result<Identity> {
        let worker = worker.canonicalize().map_err(|e| e.to_string())?;
        if worker.starts_with(&self.root) { return Err("The worker must not be controlled by the source root".into()); }
        let tool_digest = hash(&read_file(&worker, MAX_SOURCE)?);
        let nanos = SystemTime::now().duration_since(UNIX_EPOCH).map_err(|e| e.to_string())?.as_nanos();
        let job = format!("{:032x}", nanos ^ u128::from(std::process::id()));
        Ok(Identity {
            protocol: PROTOCOL.into(), operation: operation.into(), job,
            root: self.root.to_string_lossy().into_owned(),
            context: self.root.to_string_lossy().into_owned(),
            epoch: required_env("SLOPHAWK_PATHS_EPOCH")?,
            revision: required_env("SLOPHAWK_PATHS_REVISION")?,
            input_digest: self.digest.clone(), tool_digest,
            policy_digest: self.policy_digest.clone(),
            budget_ref: required_env("SLOPHAWK_PATHS_BUDGET")?,
            grant_ref: required_env("SLOPHAWK_PATHS_GRANT")?,
        })
    }

    fn inputs(&self, receipt: &BuildReceipt) -> Result<PathInputs> {
        Ok(PathInputs {
            spec: self.path.to_string_lossy().into_owned(),
            spec_sha256: self.digest.clone(),
            rules_sha256: hash(&read_file(&source_relative(&self.root, &self.spec.rules)?, MAX_SOURCE)?),
            published_sha256: hash(&read_file(&source_relative(&self.root, &self.spec.published)?, MAX_SOURCE)?),
            compiler_sha256: receipt.compiler_sha256.clone(),
            runtime_version: receipt.runtime_version.clone(),
        })
    }
}

fn write_json<T: Serialize>(path: &Path, value: &T) -> Result<()> {
    let bytes = serde_json::to_vec_pretty(value).map_err(|e| e.to_string())?;
    if bytes.len() > MAX_FRAME { return Err("Artifact exceeds 16 MiB".into()); }
    let mut file = File::create_new(path).map_err(|e| e.to_string())?;
    file.write_all(&bytes).map_err(|e| e.to_string())?;
    file.sync_all().map_err(|e| e.to_string())
}

fn read_json<T: for<'de> Deserialize<'de>>(path: &Path) -> Result<T> {
    serde_json::from_slice(&read_file(path, MAX_FRAME)?).map_err(|e| e.to_string())
}

fn worker_call(worker: &Path, identity: &Identity, payload: Value) -> Result<Value> {
    let request = serde_json::to_vec(&json!({"Identity": identity, "Payload": payload}))
        .map_err(|e| e.to_string())?;
    if request.len() > MAX_FRAME { return Err("Migration request exceeds 16 MiB".into()); }
    let mut frame = Vec::with_capacity(request.len() + 4);
    frame.extend_from_slice(&(request.len() as u32).to_le_bytes());
    frame.extend_from_slice(&request);
    let mut command = Command::new(find_on_path("dotnet")?);
    command.arg("exec").arg(worker);
    for variable in ["SLOPHAWK_PATHS_GRANT", "SLOPHAWK_PATHS_EPOCH",
        "SLOPHAWK_PATHS_REVISION", "SLOPHAWK_PATHS_BUDGET"] {
        if let Some(value) = std::env::var_os(variable) {
            command.env(variable, value);
        }
    }
    let output = slophawk_worker_scope::run(&command, &frame, Limits {
        stdout: MAX_FRAME + 4,
        stderr: 64 * 1024,
        deadline: WORKER_DEADLINE,
    }, &AtomicBool::new(false))?;
    let (prefix, bytes) = output.stdout.split_first_chunk::<4>()
        .ok_or("Worker omitted its terminal frame header")?;
    if bytes.is_empty() || u32::from_le_bytes(*prefix) as usize != bytes.len() {
        return Err("Worker emitted an incomplete or extra terminal frame".into());
    }
    let terminal: Terminal = serde_json::from_slice(bytes).map_err(|e| e.to_string())?;
    if terminal.identity != *identity || terminal.status != "complete" || output.code != 0 ||
        !terminal.omissions.is_empty() {
        return Err(format!("Invalid/incomplete worker terminal: {}: {}", terminal.status,
            terminal.reason.unwrap_or_else(|| "No complete matching terminal".into())));
    }
    terminal.value.ok_or_else(|| "Worker completed without an output value".into())
}

fn staging(scratch: &Path, kind: &str, job: &str) -> Result<(PathBuf, PathBuf)> {
    let folder = match kind {
        "build" => "path-builds", "plan" | "review" => "path-checks", _ => return Err("Bad stage kind".into()),
    };
    let root = scratch.join(".slophawk").join(folder);
    fs::create_dir_all(&root).map_err(|e| e.to_string())?;
    let path = root.join(format!("{job}.pending"));
    fs::create_dir(&path).map_err(|e| e.to_string())?;
    Ok((path, root.join(job)))
}

fn entries(dir: &Path, kind: &str) -> Result<Vec<Entry>> {
    let names: &[&str] = match kind {
        "build" => &["build.json", "candidate.dll", "candidate.runtimeconfig.json"],
        "plan" => &["candidates.json", "graph.json", "plan.json"],
        "review" => &["observed.json", "review.json", "unstable.json"],
        _ => return Err("Unknown sealed artifact kind".into()),
    };
    let actual = fs::read_dir(dir).map_err(|e| e.to_string())?.count();
    let sealed = dir.join("manifest.json").exists();
    if actual != names.len() + usize::from(sealed) {
        return Err("Missing or extra staged artifact".into());
    }
    names.iter().map(|name| {
        let path = dir.join(name);
        let bytes = read_file(&path, MAX_SOURCE)?;
        Ok(Entry { file: (*name).into(), length: bytes.len() as u64, sha256: hash(&bytes) })
    }).collect()
}

fn publish(staged: &Path, final_dir: &Path, kind: &str, identity: &Identity) -> Result<PathBuf> {
    let files = entries(staged, kind)?;
    let unsigned = UnsignedSeal { format: FORMAT, kind, files: &files, identity };
    let signed = serde_json::to_vec(&unsigned).map_err(|e| e.to_string())?;
    let seal = Seal { format: FORMAT.into(), kind: kind.into(), files, identity: identity.clone(),
        mac: hmac(&signed, &seal_key()?) };
    write_json(&staged.join("manifest.json"), &seal)?;
    fs::rename(staged, final_dir).map_err(|e| e.to_string())?;
    Ok(final_dir.join("manifest.json"))
}

fn read_seal(manifest: &Path, kind: &str) -> Result<(PathBuf, Seal)> {
    if manifest.file_name().and_then(|n| n.to_str()) != Some("manifest.json") {
        return Err("Not a published path manifest".into());
    }
    let dir = manifest.parent().ok_or("Manifest without a parent")?.canonicalize().map_err(|e| e.to_string())?;
    if dir.file_name().and_then(|n| n.to_str()).is_none_or(|name|
        name.len() != 32 || !name.bytes().all(|b| b.is_ascii_hexdigit())) {
        return Err("Unexpected publication directory identity".into());
    }
    let seal: Seal = read_json(manifest)?;
    if seal.format != FORMAT || seal.kind != kind || seal.identity.protocol != PROTOCOL {
        return Err("Wrong path seal format/kind".into());
    }
    let expected = entries(&dir, kind)?;
    if seal.files != expected { return Err("Incomplete, reordered or altered artifact closure".into()); }
    // Entries includes all files except the seal, so explicitly reject unexpected extra paths.
    let count = fs::read_dir(&dir).map_err(|e| e.to_string())?.count();
    if count != expected.len() + 1 { return Err("Unlisted artifact in sealed directory".into()); }
    let unsigned = UnsignedSeal { format: &seal.format, kind: &seal.kind, files: &seal.files,
        identity: &seal.identity };
    let data = serde_json::to_vec(&unsigned).map_err(|e| e.to_string())?;
    if !secure_equals(&seal.mac, &hmac(&data, &seal_key()?)) {
        return Err("Path seal MAC is stale or tampered".into());
    }
    Ok((dir, seal))
}

fn attested_build(manifest: &Path, current: &Identity, scratch: &Path) -> Result<(PathBuf, BuildReceipt)> {
    let (dir, seal) = read_seal(manifest, "build")?;
    if !seal.identity.same_authority(current) || !dir.starts_with(scratch.join(".slophawk/path-builds")) {
        return Err("Build seal is stale or from another workspace".into());
    }
    let receipt: BuildReceipt = read_json(&dir.join("build.json"))?;
    if receipt.candidate_sha256 != hash(&read_file(&dir.join("candidate.dll"), MAX_SOURCE)?) ||
        receipt.compiler_inputs.get("References").and_then(Value::as_array).is_none_or(|refs|
            refs.len() < 2 || refs.len() > 256) {
        return Err("Candidate or reference closure does not match the build receipt".into());
    }
    Ok((dir, receipt))
}

fn inspect(worker: &Path, source: &Source, current: &Identity, candidate: Option<&Path>,
    graph_on: bool) -> Result<InspectResult> {
    let reply = worker_call(worker, current, json!({
        "specPath": source.path, "candidatePath": candidate, "graphOn": graph_on
    }))?;
    let result: InspectResult = serde_json::from_value(reply).map_err(|e| e.to_string())?;
    let digest = match candidate {
        Some(path) => hash(&read_file(path, MAX_SOURCE)?),
        None => hash(&[]),
    };
    if result.candidate_sha256 != digest ||
        result.targets.len() != source.spec.targets.len() {
        return Err("The fact export disagrees with its build or target set".into());
    }
    for (target, expected) in result.targets.iter().zip(&source.spec.targets) {
        if target.operation != expected.operation || target.signature != expected.signature {
            return Err("Worker target replies changed order/identity".into());
        }
    }
    Ok(result)
}

fn candidate_compatible(args: &[i64], types: &[String]) -> bool {
    args.len() == types.len() && args.iter().zip(types).all(|(arg, ty)| {
        (*arg >= i32::MIN as i64 && *arg <= i32::MAX as i64) &&
            (ty != "bool" || *arg == 0 || *arg == 1)
    })
}

fn key(args: &[i64]) -> String {
    args.iter().map(i64::to_string).collect::<Vec<_>>().join(",")
}

/// Byte-for-byte intent of PathCheck.Schedule: prefix of ordinary, then one graph
/// proposal and two baseline cases, stable duplicates and original baseline ranks.
fn schedule(original: &[Expectation], graph: &[(Expectation, String)],
    ordinary: &HashSet<String>, limit: usize) -> Vec<PathCase> {
    let ranks: HashMap<_, _> = original.iter().enumerate().map(|(i, e)| (key(&e.args), i)).collect();
    let mut cases = Vec::new();
    let mut seen = HashSet::new();
    let mut original_index = 0;
    while original_index < original.len() && ordinary.contains(&key(&original[original_index].args)) {
        let case = original[original_index].clone();
        seen.insert(key(&case.args));
        cases.push(PathCase { expectation: case, origin: "ordinary".into(),
            predicted_goal: None, baseline_order: Some(original_index) });
        original_index += 1;
    }
    let mut graph_index = 0;
    while seen.len() < limit && (original_index < original.len() || graph_index < graph.len()) {
        if graph_index < graph.len() {
            let (expectation, goal) = &graph[graph_index];
            graph_index += 1;
            let name = key(&expectation.args);
            if seen.insert(name.clone()) {
                let rank = ranks.get(&name).copied();
                cases.push(PathCase { expectation: expectation.clone(),
                    origin: if rank.is_some() { "graph-shared" } else { "graph-new" }.into(),
                    predicted_goal: Some(goal.clone()), baseline_order: rank });
            }
        }
        let mut added = 0;
        while added < 2 && seen.len() < limit && original_index < original.len() {
            let expectation = original[original_index].clone();
            let index = original_index;
            original_index += 1;
            if !seen.insert(key(&expectation.args)) { continue; }
            cases.push(PathCase { expectation, origin: "baseline".into(),
                predicted_goal: None, baseline_order: Some(index) });
            added += 1;
        }
    }
    cases
}

fn validated_source(plan: &PathPlan, worker: &Path) -> Result<(Source, Identity)> {
    let source = Source::open(Path::new(&plan.inputs.spec), Path::new(&plan.scratch))?;
    let identity = source.identity("inspect", worker)?;
    if plan.inputs.spec_sha256 != source.digest ||
        plan.inputs.rules_sha256 != hash(&read_file(&source_relative(&source.root, &source.spec.rules)?, MAX_SOURCE)?) ||
        plan.inputs.published_sha256 != hash(&read_file(&source_relative(&source.root, &source.spec.published)?, MAX_SOURCE)?) ||
        !identity.same_authority(&plan.identity) ||
        plan.per_operation != source.spec.per_operation || plan.retest_limit != source.spec.retest_limit {
        return Err("Plan source, policy, budgets, tool or authority changed".into());
    }
    Ok((source, identity))
}

fn read_plan(manifest: &Path) -> Result<(PathBuf, PathPlan)> {
    let (directory, _) = read_seal(manifest, "plan")?;
    let plan: PathPlan = read_json(&directory.join("plan.json"))?;
    if plan.blocker.is_some() || plan.cases.is_empty() ||
        !directory.starts_with(Path::new(&plan.scratch).join(".slophawk/path-checks")) {
        return Err(plan.blocker.clone().unwrap_or_else(|| "No runnable sealed path cases".into()));
    }
    if plan.build_sha256 != hash(&read_file(Path::new(&plan.build), MAX_SOURCE)?) ||
        plan.mode != "graph-on" && plan.mode != "graph-off" {
        return Err("Referenced build or path mode changed".into());
    }
    Ok((directory, plan))
}

pub fn build(spec: &Path, scratch: &Path, worker: &Path) -> Result<Value> {
    let source = Source::open(spec, scratch)?;
    let identity = source.identity("build", worker)?;
    let (stage, final_dir) = staging(&source.scratch, "build", &identity.job)?;
    let result = (|| {
        let reply = worker_call(worker, &identity, json!({
            "specPath": source.path, "outputDir": stage
        }))?;
        let claimed: BuildReceipt = serde_json::from_value(reply).map_err(|e| e.to_string())?;
        let receipt: BuildReceipt = read_json(&stage.join("build.json"))?;
        if claimed.compiler_sha256 != receipt.compiler_sha256 ||
            claimed.candidate_sha256 != receipt.candidate_sha256 ||
            claimed.runtime_version != receipt.runtime_version ||
            claimed.compiler_inputs != receipt.compiler_inputs ||
            claimed.candidate_sha256 != hash(&read_file(&stage.join("candidate.dll"), MAX_SOURCE)?) {
            return Err("Emitted candidate and build receipt disagree".into());
        }
        let manifest = publish(&stage, &final_dir, "build", &identity)?;
        Ok(json!({"Build": manifest, "Candidate": final_dir.join("candidate.dll"),
            "CompilerSha256": claimed.compiler_sha256,
            "CandidateSha256": claimed.candidate_sha256}))
    })();
    if result.is_err() { let _ = fs::remove_dir_all(stage); }
    result
}

pub fn plan(spec: &Path, scratch: &Path, build: &Path, mode: &str, worker: &Path) -> Result<Value> {
    if mode != "graph-on" && mode != "graph-off" { return Err("Use graph-on or graph-off".into()); }
    let source = Source::open(spec, scratch)?;
    let identity = source.identity("inspect", worker)?;
    if build == Path::new("-") {
        return structural_plan(&source, &identity, mode, worker);
    }
    let (build_dir, receipt) = attested_build(build, &identity, &source.scratch)?;
    let candidate = build_dir.join("candidate.dll");
    let inspected = inspect(worker, &source, &identity, Some(&candidate), mode == "graph-on")?;
    if inspected.compiler_sha256 != receipt.compiler_sha256 ||
        inspected.compiler_inputs != receipt.compiler_inputs ||
        inspected.runtime_version != receipt.runtime_version ||
        inspected.candidate_sha256 != receipt.candidate_sha256 {
        return Err("Compiler input/reference closure differs from the original emission".into());
    }
    let mut methods = Vec::new();
    let mut cases = Vec::new();
    let mut proposed = Vec::<Proposed>::new();
    let mut modeled = Vec::new();
    let mut graphs = serde_json::Map::new();
    let mut baselines = serde_json::Map::new();
    let mut proposals = serde_json::Map::new();
    for target in &inspected.targets {
        graphs.insert(target.operation.clone(), target.graph.clone().unwrap_or(Value::Null));
        let parsed = parse_wire(&target.facts).map_err(|e| format!("{}: {e}", target.operation))?;
        let searched = search(&parsed);
        let eligible = target.eligible && target.bound;
        let candidates = if eligible && mode == "graph-on" { searched.candidates } else { Vec::new() };
        let candidate_count = candidates.len();
        for candidate in candidates {
            if candidate_compatible(&candidate.args, &target.parameters) {
                proposed.push(Proposed { operation: target.operation.clone(), args: candidate.args,
                    goal: candidate.predicted_goal });
            }
        }
        modeled.push((target.operation.clone(), target.original.clone(),
            target.ordinary_keys.iter().cloned().collect::<HashSet<_>>(), target.parameters.clone()));
        baselines.insert(target.operation.clone(), serde_json::to_value(&target.original).map_err(|e| e.to_string())?);
        methods.push(MethodPlan {
            operation: target.operation.clone(), signature: target.signature.clone(),
            bound: target.bound, eligible, reason: target.reason.clone(),
            rule_source: target.rule_source.clone(),
            work: searched.work, unknown: searched.unknown.into_iter()
                .filter(|reason| mode != "graph-off" || reason != "Scalar search was not requested.")
                .collect(),
            rejected_by_domain: 0, baseline_cases: target.original.len(), graph_proposals: candidate_count,
        });
    }
    let evaluations: Vec<Evaluated> = if proposed.is_empty() { Vec::new() } else {
        let evaluator_id = source.identity("evaluate", worker)?;
        let reply = worker_call(worker, &evaluator_id, json!({
            "specPath": source.path, "requests": proposed.iter().map(|p|
                json!({"Operation":p.operation,"Args":p.args})).collect::<Vec<_>>()
        }))?;
        serde_json::from_value(reply).map_err(|e| e.to_string())?
    };
    if evaluations.len() != proposed.len() { return Err("Evaluator omitted scalar candidates".into()); }
    let mut evaluated: HashMap<String, Vec<(Expectation, String)>> = HashMap::new();
    for (index, (proposal, answer)) in proposed.iter().zip(evaluations).enumerate() {
        if answer.index != index { return Err("Evaluator reordered its replies".into()); }
        if let Some(expectation) = answer.expectation {
            if expectation.operation != proposal.operation || expectation.args != proposal.args {
                return Err("Evaluator replied with a different candidate identity".into());
            }
            evaluated.entry(proposal.operation.clone()).or_default().push((expectation, proposal.goal.clone()));
        }
    }
    for (idx, (operation, baseline, ordinary, parameters)) in modeled.into_iter().enumerate() {
        let graph = evaluated.remove(&operation).unwrap_or_default();
        methods[idx].rejected_by_domain = methods[idx].graph_proposals.saturating_sub(graph.len());
        if methods[idx].eligible {
            for item in schedule(&baseline, &graph, &ordinary, source.spec.per_operation) {
                if !candidate_compatible(&item.expectation.args, &parameters) {
                    return Err("Scheduled candidate escaped parameter domain".into());
                }
                cases.push(item);
            }
        }
        proposals.insert(operation, serde_json::to_value(&graph).map_err(|e| e.to_string())?);
    }
    let blocker = if cases.is_empty() { Some("No bound qualified target with runnable cases".into()) }
        else { None };
    let plan = PathPlan {
        identity: identity.clone(), inputs: source.inputs(&receipt)?,
        scratch: source.scratch.to_string_lossy().into_owned(),
        build: build.to_string_lossy().into_owned(),
        build_sha256: hash(&read_file(build, MAX_SOURCE)?),
        candidate_sha256: receipt.candidate_sha256, compiler_inputs: receipt.compiler_inputs,
        mode: mode.into(), per_operation: source.spec.per_operation,
        retest_limit: source.spec.retest_limit, methods, cases, blocker,
    };
    let (stage, final_dir) = staging(&source.scratch, "plan", &identity.job)?;
    let result = (|| {
        write_json(&stage.join("graph.json"), &json!({"BuildAttested": true, "Graphs": graphs}))?;
        write_json(&stage.join("candidates.json"), &json!({"Baseline": baselines, "Graph": proposals}))?;
        write_json(&stage.join("plan.json"), &plan)?;
        let manifest = publish(&stage, &final_dir, "plan", &identity)?;
        Ok(json!({"Plan": manifest, "Mode": mode, "Cases": plan.cases.len(),
            "Blocker": plan.blocker, "Methods": plan.methods}))
    })();
    if result.is_err() { let _ = fs::remove_dir_all(stage); }
    result
}

fn structural_plan(source: &Source, identity: &Identity, mode: &str, worker: &Path) -> Result<Value> {
    let inspected = inspect(worker, source, identity, None, false)?;
    let blocker = "No controlled build was supplied.";
    let mut graphs = serde_json::Map::new();
    let mut methods = Vec::new();
    for target in inspected.targets {
        graphs.insert(target.operation.clone(), target.graph.unwrap_or(Value::Null));
        let searched = search(&parse_wire(&target.facts).map_err(|error| error.to_string())?);
        methods.push(MethodPlan { operation: target.operation, signature: target.signature, bound: target.bound,
            eligible: false, reason: target.reason.or_else(|| Some(blocker.into())), rule_source: target.rule_source,
            work: searched.work, unknown: searched.unknown, rejected_by_domain: 0, baseline_cases: 0, graph_proposals: 0 });
    }
    let receipt = BuildReceipt { compiler_sha256: inspected.compiler_sha256, runtime_version: inspected.runtime_version,
        candidate_sha256: inspected.candidate_sha256, compiler_inputs: inspected.compiler_inputs };
    let plan = PathPlan { identity: identity.clone(), inputs: source.inputs(&receipt)?,
        scratch: source.scratch.display().to_string(), build: String::new(), build_sha256: String::new(),
        candidate_sha256: String::new(), compiler_inputs: receipt.compiler_inputs, mode: mode.into(),
        per_operation: source.spec.per_operation, retest_limit: source.spec.retest_limit,
        methods, cases: Vec::new(), blocker: Some(blocker.into()) };
    let (stage, destination) = staging(&source.scratch, "plan", &identity.job)?;
    write_json(&stage.join("graph.json"), &json!({"BuildAttested": false, "Graphs": graphs}))?;
    write_json(&stage.join("candidates.json"), &json!({"Baseline": {}, "Graph": {}}))?;
    write_json(&stage.join("plan.json"), &plan)?;
    let manifest = publish(&stage, &destination, "plan", identity)?;
    Ok(json!({"Plan": manifest, "Mode": mode, "Cases": 0, "Blocker": blocker, "Methods": plan.methods}))
}

fn replay(worker: &Path, source: &Source, candidate: &Path, current: &Identity,
    cases: &[Expectation]) -> Result<ReplayResult> {
    let (stage, _unused) = staging(&source.scratch, "review", &current.job)?;
    // Reuse a temporary host-owned attempt dir, never stage a review from an unconfirmed terminal.
    let result = (|| {
        let reply = worker_call(worker, current, json!({
            "specPath": source.path, "candidatePath": candidate,
            "workDir": stage,
            "inputs": cases.iter().map(|item| json!({"Operation":item.operation,"Args":item.args}))
                .collect::<Vec<_>>()
        }))?;
        let observation: ReplayResult = serde_json::from_value(reply).map_err(|e| e.to_string())?;
        if observation.observed.len() != cases.len() || observation.replays != 2 ||
            observation.candidate_sha256 != hash(&read_file(candidate, MAX_SOURCE)?) {
            return Err("Original candidate replay returned incomplete ordered observations".into());
        }
        Ok(observation)
    })();
    let _ = fs::remove_dir_all(stage);
    result
}

fn missing_coverage(scope: &[Scope]) -> bool {
    scope.iter().any(|s| !s.bound || s.planned_cases == 0 || s.unknown_frontiers != 0)
}

pub fn check(plan_manifest: &Path, worker: &Path) -> Result<Value> {
    let (_, plan) = read_plan(plan_manifest)?;
    let (source, current) = validated_source(&plan, worker)?;
    let (build_dir, receipt) = attested_build(Path::new(&plan.build), &current, &source.scratch)?;
    if plan.candidate_sha256 != receipt.candidate_sha256 ||
        plan.compiler_inputs != receipt.compiler_inputs || plan.inputs.compiler_sha256 != receipt.compiler_sha256 ||
        plan.inputs.runtime_version != receipt.runtime_version {
        return Err("The plan does not reference the exact original compilation".into());
    }
    let candidate = build_dir.join("candidate.dll");
    let inspected = inspect(worker, &source, &current, Some(&candidate), plan.mode == "graph-on")?;
    if inspected.compiler_sha256 != receipt.compiler_sha256 ||
        inspected.compiler_inputs != receipt.compiler_inputs {
        return Err("Compiler options, sources or reference closure changed".into());
    }
    let cases = plan.cases.iter().map(|item| item.expectation.clone()).collect::<Vec<_>>();
    let replay_id = source.identity("replay", worker)?;
    let initial = replay(worker, &source, &candidate, &replay_id, &cases)?;
    let mut unknown = initial.unknown;
    let mut reproduced = Vec::new();
    let mut changed = Vec::new();
    let mut retests = 0;
    let mut unsupported = 0;
    let mut compared = 0;
    let mut findings = Vec::new();
    for (i, observed) in initial.observed.iter().enumerate() {
        if observed.is_null() { unsupported += 1; continue; }
        compared += 1;
        if *observed != cases[i].expected { findings.push(i); }
    }
    // Stable round-robin across operations, keeping each operation's original reply order.
    let mut ranks = HashMap::<String, usize>::new();
    let mut priority = findings.iter().map(|&i| {
        let operation = cases[i].operation.clone();
        let rank = ranks.entry(operation).or_insert(0);
        let result = (*rank, i);
        *rank += 1;
        result
    }).collect::<Vec<_>>();
    priority.sort_unstable();
    let mut pending = findings.len();
    if initial.ordering_stable {
        for (_, index) in priority.into_iter().take(plan.retest_limit) {
            let single = replay(worker, &source, &candidate, &source.identity("replay", worker)?,
                std::slice::from_ref(&cases[index]))?;
            retests += 1;
            if single.ordering_stable && single.observed[0] == initial.observed[index] {
                reproduced.push(Violation { original_index: index, retest_job: replay_id.job.clone(),
                    case: plan.cases[index].clone(), observed: single.observed[0].clone(),
                    source: plan.methods.iter().find(|m| m.operation == cases[index].operation)
                        .and_then(|m| m.rule_source.clone()).unwrap_or_default() });
            } else {
                changed.push(json!({"Index": index, "Before": initial.observed[index],
                    "After": single.observed[0]}));
            }
            pending -= 1;
        }
    } else {
        unknown.push("Original adapter changed output order under reverse-order replay".into());
        changed.push(json!({"UnstableBatch": cases.len()}));
    }
    // Before publication, recheck all source and reference bytes, compiler options,
    // candidate, policy and the host epoch after every original-build replay.
    let late = Source::open(&source.path, &source.scratch)?;
    let late_id = late.identity("inspect", worker)?;
    if !late_id.same_authority(&current) { return Err("Authority or policy changed during replay".into()); }
    let final_inspect = inspect(worker, &late, &late_id, Some(&candidate), plan.mode == "graph-on")?;
    if final_inspect.compiler_inputs != receipt.compiler_inputs ||
        final_inspect.candidate_sha256 != receipt.candidate_sha256 ||
        final_inspect.compiler_sha256 != receipt.compiler_sha256 {
        return Err("Compilation closure changed during replay".into());
    }
    let scope = plan.methods.iter().map(|m| Scope {
        operation: m.operation.clone(), signature: m.signature.clone(),
        bound: m.bound, reason: m.reason.clone(), unknown_frontiers: m.unknown.len(),
        planned_cases: cases.iter().filter(|e| e.operation == m.operation).count(),
    }).collect::<Vec<_>>();
    let outcome = if unsupported != 0 || pending != 0 || !changed.is_empty() || missing_coverage(&scope) {
        "incomplete"
    } else if !reproduced.is_empty() { "reproduced-discrepancy" }
    else { "no-discrepancy-in-scope" };
    let review = PathReview {
        identity: current.clone(), plan: plan_manifest.to_string_lossy().into_owned(),
        plan_sha256: hash(&read_file(plan_manifest, MAX_SOURCE)?), mode: plan.mode,
        outcome: outcome.into(), cases: cases.len(), compared, unsupported, retests,
        unconfirmed: pending, work: plan.methods.iter().map(|m| m.work).sum(),
        initial_job: replay_id.job, reproduced, unstable: changed.len(), blocker: None,
        scope, measured_coverage: "unknown: no invocation-scoped branch trace".into(), unknown,
    };
    let (stage, final_dir) = staging(&source.scratch, "review", &current.job)?;
    let result = (|| {
        write_json(&stage.join("review.json"), &review)?;
        write_json(&stage.join("unstable.json"), &changed)?;
        write_json(&stage.join("observed.json"), &initial.observed)?;
        let manifest = publish(&stage, &final_dir, "review", &current)?;
        Ok(json!({"Review": manifest, "Result": review}))
    })();
    if result.is_err() { let _ = fs::remove_dir_all(stage); }
    result
}

pub fn review(review_manifest: &Path, worker: &Path) -> Result<Value> {
    let (dir, review_seal) = read_seal(review_manifest, "review")?;
    let reviewed: PathReview = read_json(&dir.join("review.json"))?;
    if review_seal.identity != reviewed.identity ||
        reviewed.plan_sha256 != hash(&read_file(Path::new(&reviewed.plan), MAX_SOURCE)?) {
        return Err("The review references a changed or unsealed plan".into());
    }
    let (_, plan) = read_plan(Path::new(&reviewed.plan))?;
    if reviewed.mode != plan.mode || reviewed.cases != plan.cases.len() ||
        reviewed.work != plan.methods.iter().map(|m| m.work).sum::<usize>() {
        return Err("Review scope does not match its source plan".into());
    }
    let (source, identity) = validated_source(&plan, worker)?;
    if !identity.same_authority(&reviewed.identity) || !identity.same_authority(&review_seal.identity) {
        return Err("Review authority is stale".into());
    }
    let (build_dir, receipt) = attested_build(Path::new(&plan.build), &identity, &source.scratch)?;
    let inspected = inspect(worker, &source, &identity, Some(&build_dir.join("candidate.dll")), plan.mode == "graph-on")?;
    if receipt.compiler_sha256 != inspected.compiler_sha256 ||
        receipt.candidate_sha256 != inspected.candidate_sha256 ||
        receipt.compiler_inputs != inspected.compiler_inputs ||
        plan.compiler_inputs != inspected.compiler_inputs {
        return Err("Review references a stale compiler/source/reference closure".into());
    }
    Ok(json!({"Review": review_manifest, "Current": true, "Result": reviewed}))
}

/// CLI adaptor for the opt-in native foreground command; the shipped `slophawk paths` argv is unchanged.
pub fn run(args: &[String]) -> Result<(Value, i32)> {
    let path = |i: usize| args.get(i).map(Path::new).ok_or("Missing scalar foreground argument");
    let value = match args.first().map(String::as_str) {
        Some("build") if args.len() == 4 => build(path(1)?, path(2)?, path(3)?)?,
        Some("plan") if args.len() == 6 => plan(path(1)?, path(2)?, path(3)?, &args[4], path(5)?)?,
        Some("check") if args.len() == 3 => check(path(1)?, path(2)?)?,
        Some("review") if args.len() == 3 => review(path(1)?, path(2)?)?,
        _ => return Err("Usage: scalar-paths-foreground build <spec> <scratch> <worker.dll> | plan <spec> <scratch> <build-manifest> <graph-on|graph-off> <worker.dll> | check <plan-manifest> <worker.dll> | review <review-manifest> <worker.dll>".into()),
    };
    let exit = if value.get("Blocker").is_some_and(|v| !v.is_null()) ||
        value.get("Result").and_then(|r| r.get("Outcome")).and_then(Value::as_str)
            .is_some_and(|o| o != "no-discrepancy-in-scope") { 1 } else { 0 };
    Ok((value, exit))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn expectation(n: i64) -> Expectation { Expectation { operation: "a".into(), args: vec![n], expected: json!(n) } }
    #[test]
    fn graph_off_keeps_original_seed_prefix_and_order() {
        let baseline = vec![expectation(0), expectation(1), expectation(2)];
        let ordinary = ["0".into()].into_iter().collect();
        let scheduled = schedule(&baseline, &[], &ordinary, 3);
        assert_eq!(scheduled.iter().map(|c| c.expectation.args[0]).collect::<Vec<_>>(), [0, 1, 2]);
        assert_eq!(scheduled.iter().map(|c| c.origin.as_str()).collect::<Vec<_>>(), ["ordinary", "baseline", "baseline"]);
    }
    #[test]
    fn graph_on_interleaves_one_graph_then_two_baseline() {
        let baseline = (0..5).map(expectation).collect::<Vec<_>>();
        let graph = vec![(expectation(9), "b2:conditional".into()),
            (expectation(1), "b2:fallThrough".into())];
        let scheduled = schedule(&baseline, &graph, &["0".into()].into_iter().collect(), 5);
        assert_eq!(scheduled.iter().map(|c| c.expectation.args[0]).collect::<Vec<_>>(), [0, 9, 1, 2, 3]);
        assert_eq!(scheduled[1].origin, "graph-new");
        assert_eq!(scheduled[1].predicted_goal.as_deref(), Some("b2:conditional"));
    }
    #[test]
    fn hmac_tampering_is_detected_and_has_fixed_length() {
        let a = hmac(b"signed artifact", b"independent host secret with sufficient length");
        let b = hmac(b"signed artifacT", b"independent host secret with sufficient length");
        assert_eq!(a.len(), 64);
        assert!(!secure_equals(&a, &b));
    }
    #[test]
    fn zero_scalar_budgets_are_not_measured_coverage() {
        let wire = "scalar-path-facts\t1\nlimits\t0\t0\nbound\t0\t0\t-\nend\n";
        let result = search(&parse_wire(wire).unwrap());
        assert_eq!(result.work, 0);
        assert!(result.candidates.is_empty());
    }
}
