//! Owner-configured executable tests run under the resident writer's fence.
use super::{bytes, create_bound, invalid, resident_store::nonce, store_snapshot};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use slophawk_language_execution as worker;
use slophawk_python_project_context as python;
use slophawk_rust_project_context as cargo;
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::{self, ErrorKind};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::{self, JoinHandle};
use worker::protocol::sha256;

const LIMIT: usize = 2 * 1024 * 1024;
const MAX_PROVIDER_INPUTS: usize = 4096;
const MAX_TARGETS: usize = 16;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Profile {
    version: u32,
    root_identity: String,
    worker: worker::Executable,
    budget: worker::Budget,
    providers: Vec<Provider>,
}

#[derive(Deserialize)]
#[serde(tag = "language", rename_all = "snake_case", deny_unknown_fields)]
enum Provider {
    Python {
        interpreter: worker::Executable,
        project_root: String,
        environment_sha256: String,
        pytest_sha256: String,
        plugins_sha256: String,
        effective_config_sha256: String,
    },
    Rust {
        cargo: worker::Executable,
        variant: worker::CargoVariant,
        allow_build: bool,
        allow_build_scripts: bool,
        allow_proc_macros: bool,
    },
    Fluxr {
        checked_worker: worker::Executable,
        environment_sha256: String,
        modules: Vec<FluxrModule>,
        operations: Vec<worker::FluxrOperation>,
        cases: Value,
        limits: Value,
        output_scope: String,
    },
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct FluxrModule {
    module_id: String,
    logical_document_id: String,
    relative_path: String,
}

impl Provider {
    fn name(&self) -> &'static str {
        match self {
            Self::Python { .. } => "python",
            Self::Rust { .. } => "rust",
            Self::Fluxr { .. } => "fluxr",
        }
    }
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Record {
    id: u64,
    revision: i64,
    epoch: String,
    scope_sha256: String,
    state: String,
    receipts: Vec<Value>,
    error: Option<String>,
}

pub(super) struct Scheduler {
    profile: PathBuf,
    ledger: PathBuf,
    spool: PathBuf,
    latest: Option<Record>,
    error: Option<String>,
    enabled: bool,
}

struct Observation {
    scope: String,
    profile_sha256: String,
    profile: Profile,
    branch_commit: String,
    ledger: BTreeMap<String, worker::InputFile>,
}

fn json_bytes(value: &impl Serialize) -> io::Result<Vec<u8>> {
    serde_json::to_vec(value).map_err(io::Error::other)
}

fn inspect(profile: &Path, root: &Path, identity: &str, revision: i64) -> io::Result<Observation> {
    let raw = worker::read_owner_document(profile, root)?;
    let profile: Profile = serde_json::from_slice(&raw).map_err(io::Error::other)?;
    let providers = profile
        .providers
        .iter()
        .map(Provider::name)
        .collect::<BTreeSet<_>>();
    if profile.version != 1
        || profile.root_identity != identity
        || providers.is_empty()
        || providers.len() != profile.providers.len()
        || !profile.budget.valid()
        || !profile.worker.valid()
    {
        return Err(invalid("invalid executable test registration"));
    }
    let (git, scan) = super::observe_workspace(root)?;
    if scan.partial {
        return Err(invalid(
            "executable tests require a complete source inventory",
        ));
    }
    let ledger = scan
        .files
        .into_iter()
        .map(|(path, entry)| {
            let input = worker::InputFile {
                relative_path: path.clone(),
                sha256: entry.digest,
                byte_count: entry.length,
            };
            (path, input)
        })
        .collect::<BTreeMap<_, _>>();
    let profile_sha256 = sha256(&raw);
    let scope = sha256(&json_bytes(&(
        identity,
        revision,
        &git.metadata_identity,
        &git.branch,
        &git.commit,
        &profile_sha256,
        &ledger,
    ))?);
    Ok(Observation {
        scope,
        profile_sha256,
        profile,
        branch_commit: git.commit,
        ledger,
    })
}

impl Scheduler {
    pub(super) fn open(state: &Path, epoch: &str) -> io::Result<Self> {
        let ledger = state.join(format!("native-{epoch}.tests.json"));
        let latest = match bytes(&ledger, LIMIT) {
            Ok(raw) => Some(serde_json::from_slice::<Record>(&raw).map_err(io::Error::other)?),
            Err(error) if error.kind() == ErrorKind::NotFound => None,
            Err(error) => return Err(error),
        };
        let mut schedule = Self {
            profile: state.join("native-tests.json"),
            spool: state.to_owned(),
            ledger,
            latest,
            error: None,
            enabled: false,
        };
        if let Some(record) = schedule
            .latest
            .clone()
            .filter(|record| record.state == "running")
        {
            schedule.save(Record {
                state: "interrupted".into(),
                error: Some("attempt_interrupted_before_receipt".into()),
                ..record
            })?;
        }
        Ok(schedule)
    }

    fn save(&mut self, record: Record) -> io::Result<()> {
        let data = json_bytes(&record)?;
        if data.len() > LIMIT {
            return Err(invalid("test scheduler receipt budget exceeded"));
        }
        store_snapshot(&self.ledger, &data, &nonce()?)?;
        self.latest = Some(record);
        Ok(())
    }

    pub(super) fn start(
        &mut self,
        root: &Path,
        identity: &str,
        epoch: &str,
        revision: i64,
    ) -> Option<Running> {
        self.error = None;
        let result = (|| {
            self.enabled = true;
            let observed = match inspect(&self.profile, root, identity, revision) {
                Ok(observed) => observed,
                Err(error) if error.kind() == ErrorKind::NotFound => {
                    self.enabled = false;
                    return Ok(None);
                }
                Err(error) => return Err(error),
            };
            if self.latest.as_ref().is_some_and(|last| {
                last.scope_sha256 == observed.scope
                    && !matches!(last.state.as_str(), "interrupted" | "superseded")
            }) {
                return Ok(None);
            }
            let id = self
                .latest
                .as_ref()
                .map_or(Some(1), |last| last.id.checked_add(1))
                .ok_or_else(|| invalid("test attempt sequence exhausted"))?;
            let record = Record {
                id,
                revision,
                epoch: epoch.into(),
                scope_sha256: observed.scope.clone(),
                state: "running".into(),
                receipts: Vec::new(),
                error: None,
            };
            self.save(record.clone())?;
            let work = Work {
                root: root.to_owned(),
                profile: self.profile.clone(),
                spool: self.spool.clone(),
                id,
                revision,
                epoch: epoch.into(),
                observed,
            };
            match Running::start(work) {
                Ok(running) => Ok(Some(running)),
                Err(error) => {
                    self.save(Record {
                        state: "blocked".into(),
                        error: Some("worker_thread_unavailable".into()),
                        ..record
                    })?;
                    Err(error)
                }
            }
        })();
        match result {
            Ok(work) => work,
            Err(error) => {
                self.error = Some(error.to_string());
                None
            }
        }
    }

    pub(super) fn finish(
        &mut self,
        root: &Path,
        identity: &str,
        revision: i64,
        finished: Finished,
    ) -> io::Result<()> {
        let Some(record) = self
            .latest
            .as_ref()
            .filter(|record| record.id == finished.id && record.state == "running")
        else {
            return Err(invalid("test attempt no longer owns its fence"));
        };
        let current = inspect(&self.profile, root, identity, revision);
        let (state, receipts, error) = if current
            .as_ref()
            .is_ok_and(|value| value.scope == record.scope_sha256)
        {
            let state = if finished
                .receipts
                .iter()
                .any(|receipt| receipt["outcome"] == "blocked")
            {
                "blocked"
            } else {
                "completed"
            };
            (state, finished.receipts, None)
        } else {
            (
                "superseded",
                Vec::new(),
                Some("source_or_grant_changed_before_receipt".into()),
            )
        };
        self.save(Record {
            state: state.into(),
            receipts,
            error,
            ..record.clone()
        })
    }

    pub(super) fn status(&self) -> Value {
        if !self.enabled {
            return json!({"state":"awaiting_adapter","latest":null,"retryAt":null,"error":null});
        }
        let state = match (&self.error, &self.latest) {
            (Some(_), _) => "blocked_policy",
            (_, Some(record)) => record.state.as_str(),
            _ => "awaiting_adapter",
        };
        json!({"state":state,"latest":self.latest,"retryAt":null,"error":self.error})
    }

    pub(super) fn failed(&mut self, error: io::Error) {
        self.error = Some(error.to_string());
    }
}

pub struct Running {
    id: u64,
    cancel: Arc<AtomicBool>,
    thread: Option<JoinHandle<Vec<Value>>>,
}

pub struct Finished {
    id: u64,
    receipts: Vec<Value>,
}

impl Running {
    fn start(work: Work) -> io::Result<Self> {
        let id = work.id;
        let cancel = Arc::new(AtomicBool::new(false));
        let signal = Arc::clone(&cancel);
        let thread = thread::Builder::new()
            .name("slophawk-executable-tests".into())
            .spawn(move || work.run(&signal))?;
        Ok(Self {
            id,
            cancel,
            thread: Some(thread),
        })
    }

    pub fn completed(&mut self) -> Option<io::Result<Finished>> {
        let thread = self.thread.take_if(|thread| thread.is_finished())?;
        Some(
            thread
                .join()
                .map(|receipts| Finished {
                    id: self.id,
                    receipts,
                })
                .map_err(|_| invalid("executable test worker panicked")),
        )
    }

    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::Release);
    }
}

impl Drop for Running {
    fn drop(&mut self) {
        self.cancel();
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

struct Work {
    root: PathBuf,
    profile: PathBuf,
    spool: PathBuf,
    id: u64,
    revision: i64,
    epoch: String,
    observed: Observation,
}

struct Context<'a> {
    work: &'a Work,
    root: String,
    envelope: PathBuf,
    artifacts: PathBuf,
    attempts: u64,
    signal: &'a AtomicBool,
}

impl Work {
    fn run(&self, signal: &AtomicBool) -> Vec<Value> {
        let mut context = match self.context(signal) {
            Ok(context) => context,
            Err(error) => return vec![json!({"outcome":"blocked","error":error.to_string()})],
        };
        self.observed
            .profile
            .providers
            .iter()
            .map(|provider| match context.run(provider) {
                Ok(receipts) => json!({"provider":provider.name(),"receipts":receipts}),
                Err(error) => json!({"provider":provider.name(),"outcome":"blocked","error":error}),
            })
            .collect()
    }

    fn context<'a>(&'a self, signal: &'a AtomicBool) -> io::Result<Context<'a>> {
        let root = self
            .root
            .to_str()
            .and_then(cargo::platform_path_to_wire)
            .ok_or_else(|| invalid("unsupported executable test workspace path"))?;
        let directory = self.spool.join(format!("test-{}", nonce()?));
        let mut builder = fs::DirBuilder::new();
        #[cfg(unix)]
        let options = {
            use std::os::unix::fs::DirBuilderExt;
            builder.mode(0o700)
        };
        #[cfg(windows)]
        let options = &mut builder;
        #[cfg(not(any(unix, windows)))]
        let options = &builder;
        options.create(&directory)?;
        let artifacts = directory.join("artifacts");
        options.create(&artifacts)?;
        Ok(Context {
            work: self,
            root,
            envelope: directory,
            artifacts,
            attempts: 0,
            signal,
        })
    }
}

#[derive(Clone, Copy, Default)]
struct BuildGrant {
    build: bool,
    scripts: bool,
    macros: bool,
}

impl Context<'_> {
    fn execute(
        &mut self,
        operation: worker::Operation,
        context: &str,
        input: &str,
        source: &str,
        config: &str,
        environment: &str,
        inputs: Vec<worker::InputFile>,
        build: BuildGrant,
    ) -> Result<worker::Verified, String> {
        if self.signal.load(Ordering::Acquire) {
            return Err("test scheduling cancelled".into());
        }
        self.attempts += 1;
        let attempt_id = self
            .work
            .id
            .checked_mul(100)
            .and_then(|id| id.checked_add(self.attempts))
            .ok_or("test worker attempt sequence exhausted")?
            .to_string();
        let tool = operation.executable().clone();
        let job = worker::Job {
            format: worker::PROTOCOL.into(),
            identity: worker::Identity {
                operation: operation.name().into(),
                job_id: format!("{}-{}", operation.name(), self.work.id),
                attempt_id,
                root: self.root.clone(),
                context_sha256: context.into(),
                epoch: self.work.epoch.clone(),
                committed_revision: self
                    .work
                    .revision
                    .try_into()
                    .map_err(|_| "invalid inventory revision")?,
                fence_sequence: self.work.id,
                input_sha256: input.into(),
                source_sha256: source.into(),
                config_sha256: config.into(),
                environment_sha256: environment.into(),
                tool_sha256: tool.sha256.clone(),
                policy_sha256: self.work.observed.profile_sha256.clone(),
                grant_ref: self.work.observed.profile_sha256.clone(),
                budget_ref: "native-executable-tests-1".into(),
            },
            budget: self.work.observed.profile.budget.clone(),
            inputs,
            operation,
        };
        let id = &job.identity;
        let profile = json!({
            "root":id.root,"context_sha256":id.context_sha256,"epoch":id.epoch,
            "committed_revision":id.committed_revision,"fence_sequence":id.fence_sequence,
            "input_sha256":id.input_sha256,"source_sha256":id.source_sha256,
            "input_files_sha256":sha256(&json_bytes(&job.inputs).map_err(|error| error.to_string())?),
            "operation_sha256":sha256(&json_bytes(&job.operation).map_err(|error| error.to_string())?),
            "config_sha256":id.config_sha256,"environment_sha256":id.environment_sha256,
            "policy_sha256":id.policy_sha256,"grant_ref":id.grant_ref,"budget_ref":id.budget_ref,
            "operations":[id.operation],"allow_build":build.build,"allow_build_scripts":build.scripts,
            "allow_proc_macros":build.macros,"tool":tool,"worker":self.work.observed.profile.worker,
            "artifacts_root":self.artifacts.to_str().and_then(cargo::platform_path_to_wire)
                .ok_or("invalid executable test artifact path")?,"budget":job.budget,
        });
        let path = self.envelope.join(format!("{}.json", id.attempt_id));
        create_bound(
            &path,
            &json_bytes(&profile).map_err(|error| error.to_string())?,
        )
        .map_err(|error| error.to_string())?;
        let current = || {
            worker::read_owner_document(&self.work.profile, &self.work.root)
                .is_ok_and(|bytes| sha256(&bytes) == self.work.observed.profile_sha256)
        };
        let result = worker::run_admitted_guarded(&path, job, self.signal, &current)
            .map_err(|error| format!("{error:?}"));
        fs::remove_file(path).map_err(|error| error.to_string())?;
        result
    }

    fn inputs(&self, keep: impl Fn(&str) -> bool) -> Result<Vec<worker::InputFile>, String> {
        let inputs = self
            .work
            .observed
            .ledger
            .values()
            .filter(|item| keep(&item.relative_path))
            .take(MAX_PROVIDER_INPUTS + 1)
            .cloned()
            .collect::<Vec<_>>();
        if inputs.is_empty()
            || inputs.len() > MAX_PROVIDER_INPUTS
            || inputs.iter().any(|input| !input.valid())
            || inputs.iter().map(|input| input.byte_count).sum::<u64>() > 32 * 1024 * 1024
        {
            return Err("invalid or oversized executable test input closure".into());
        }
        Ok(inputs)
    }

    fn read(&self, input: &worker::InputFile) -> Result<Vec<u8>, String> {
        let contents = bytes(
            &self.work.root.join(&input.relative_path),
            input.byte_count as usize,
        )
        .map_err(|error| error.to_string())?;
        if sha256(&contents) != input.sha256 {
            return Err("test input changed during selection".into());
        }
        Ok(contents)
    }

    fn run(&mut self, provider: &Provider) -> Result<Vec<Value>, String> {
        match provider {
            Provider::Python {
                interpreter,
                project_root,
                environment_sha256,
                pytest_sha256,
                plugins_sha256,
                effective_config_sha256,
            } => {
                let prefix = format!("{project_root}/");
                let inputs = self.inputs(|path| {
                    (project_root == "." || path.starts_with(&prefix))
                        && (path.ends_with(".py")
                            || path.ends_with(".pyi")
                            || matches!(
                                Path::new(path).file_name().and_then(|name| name.to_str()),
                                Some(
                                    "pyproject.toml"
                                        | "pytest.ini"
                                        | "setup.cfg"
                                        | "tox.ini"
                                        | "setup.py"
                                )
                            ))
                })?;
                let files = inputs
                    .iter()
                    .map(|file| {
                        Ok(python::SnapshotFile {
                            path: file.relative_path.clone(),
                            bytes: self.read(file)?,
                        })
                    })
                    .collect::<Result<Vec<_>, String>>()?;
                let context = python::analyze(
                    python::WorkspaceScope {
                        workspace_id: self.work.observed.profile.root_identity.clone(),
                        revision: format!("{}-{}", self.work.epoch, self.work.revision),
                        project_roots: vec![project_root.clone()],
                        inventory: python::Inventory::Complete,
                    },
                    files,
                )
                .map_err(|error| format!("{error:?}"))?;
                let manifest = if project_root == "." {
                    "pyproject.toml".into()
                } else {
                    format!("{prefix}pyproject.toml")
                };
                let selection = python::select_tests(
                    &context,
                    &sha256(&[]),
                    &[python::Change {
                        before: None,
                        after: Some(manifest),
                    }],
                )
                .map_err(|error| format!("{error:?}"))?;
                if selection.status != python::SelectionStatus::NeedsAdmittedCollection
                    || !selection
                        .impacted
                        .iter()
                        .any(|scope| scope.root == *project_root)
                {
                    return Err("configured Python project could not be selected".into());
                }
                let outcome = self.execute(
                    worker::Operation::Pytest {
                        interpreter: interpreter.clone(),
                        project_root: project_root.clone(),
                        pytest_sha256: pytest_sha256.clone(),
                        plugins_sha256: plugins_sha256.clone(),
                        effective_config_sha256: Some(effective_config_sha256.clone()),
                    },
                    &selection.selection_sha256,
                    &selection.selection_sha256,
                    &selection.identity.source_sha256,
                    &selection.identity.configuration_sha256,
                    environment_sha256,
                    inputs,
                    BuildGrant::default(),
                )?;
                let receipt = worker::classify_python(&selection, &outcome)
                    .map_err(|error| format!("{error:?}"))?;
                Ok(vec![
                    json!({"target":project_root,"outcome":format!("{:?}",receipt.disposition),
                    "selectionSha256":selection.selection_sha256,"collected":receipt.collected,
                    "passed":receipt.passed,"failed":receipt.failed,"skipped":receipt.skipped,
                    "artifactDirectory":outcome.artifact_dir()}),
                ])
            }
            Provider::Rust {
                cargo: tool,
                variant,
                allow_build,
                allow_build_scripts,
                allow_proc_macros,
            } => self.run_cargo(
                tool,
                variant.clone(),
                BuildGrant {
                    build: *allow_build,
                    scripts: *allow_build_scripts,
                    macros: *allow_proc_macros,
                },
            ),
            Provider::Fluxr {
                checked_worker,
                environment_sha256,
                modules,
                operations,
                cases,
                limits,
                output_scope,
            } => {
                let inputs =
                    self.inputs(|path| modules.iter().any(|module| module.relative_path == path))?;
                let modules = modules
                    .iter()
                    .map(|module| {
                        let file = inputs
                            .iter()
                            .find(|input| input.relative_path == module.relative_path)
                            .ok_or("registered FluxR module absent from the source inventory")?;
                        Ok(worker::FluxrModule {
                            module_id: module.module_id.clone(),
                            logical_document_id: module.logical_document_id.clone(),
                            relative_path: module.relative_path.clone(),
                            sha256: file.sha256.clone(),
                            codec: "canonical_checked".into(),
                        })
                    })
                    .collect::<Result<Vec<_>, String>>()?;
                let input = sha256(
                    &worker::protocol::ordered_json(cases.clone())
                        .map_err(|error| error.to_string())?,
                );
                let source = sha256(&json_bytes(&modules).map_err(|error| error.to_string())?);
                let config = sha256(
                    &json_bytes(&(operations, limits, output_scope))
                        .map_err(|error| error.to_string())?,
                );
                let outcome = self.execute(
                    worker::Operation::FluxrCheck {
                        checked_worker: checked_worker.clone(),
                        modules,
                        operations: operations.clone(),
                        cases: cases.clone(),
                        limits: limits.clone(),
                        output_scope: output_scope.clone(),
                        mode: "check".into(),
                    },
                    &self.work.observed.scope,
                    &input,
                    &source,
                    &config,
                    environment_sha256,
                    inputs,
                    BuildGrant::default(),
                )?;
                Ok(vec![
                    json!({"target":output_scope,"receipt":outcome.payload()}),
                ])
            }
        }
    }

    fn run_cargo(
        &mut self,
        tool: &worker::Executable,
        mut variant: worker::CargoVariant,
        grant: BuildGrant,
    ) -> Result<Vec<Value>, String> {
        let inputs = self.inputs(|path| {
            path.ends_with(".rs")
                || path.ends_with("Cargo.toml")
                || path == "Cargo.lock"
                || path.starts_with(".cargo/")
                || path.starts_with("rust-toolchain")
        })?;
        variant.epoch = self.work.epoch.clone();
        variant.revision = self
            .work
            .revision
            .try_into()
            .map_err(|_| "invalid inventory revision")?;
        variant.git_commit = self.work.observed.branch_commit.clone();
        let binding = worker::cargo_binding(&self.root, tool, &variant)?;
        let context = binding.digest().map_err(|error| error.to_string())?;
        let manifest_path = format!("{}/Cargo.toml", self.root);
        let ledger = sha256(&json_bytes(&inputs).map_err(|error| error.to_string())?);
        let config = self.work.observed.profile_sha256.clone();
        let environment = variant.environment_sha256.clone();
        let metadata = self.execute(
            worker::Operation::CargoMetadata {
                cargo: tool.clone(),
                variant: variant.clone(),
                manifest_path: manifest_path.clone(),
            },
            &context,
            &ledger,
            &ledger,
            &config,
            &environment,
            inputs.clone(),
            BuildGrant::default(),
        )?;
        let snapshot = |file: &worker::InputFile| {
            Ok(cargo::SnapshotFile {
                path: format!("{}/{}", self.root, file.relative_path),
                bytes: self.read(file)?,
                expected_sha256: file.sha256.clone(),
            })
        };
        let manifests = inputs
            .iter()
            .filter(|file| file.relative_path.ends_with("Cargo.toml"))
            .map(snapshot)
            .collect::<Result<Vec<_>, String>>()?;
        let lock = inputs
            .iter()
            .find(|file| file.relative_path == "Cargo.lock")
            .ok_or("Cargo.lock missing")?;
        let graph = worker::classify_cargo_metadata(&metadata, manifests, snapshot(lock)?)?;
        if graph.coverage != cargo::Coverage::EvaluatedCargoGraph {
            return Err("Cargo graph is incomplete".into());
        }
        // The scheduler uses the whole workspace as its first-run frontier.
        // The existing selector computes the bounded conservative package list.
        let selection = cargo::select_tests(
            None,
            &graph,
            &[cargo::Change {
                path: manifest_path.clone(),
                previous_path: None,
            }],
            MAX_TARGETS,
        )
        .map_err(|error| error.to_string())?;
        if !selection.deferred_package_ids.is_empty() {
            return Err("Cargo package selection exceeds the test budget".into());
        }
        let mut receipts = Vec::new();
        for target in selection.selected {
            let job = cargo::CargoTestJob {
                context_sha256: graph.context_sha256.clone(),
                package_id: target.package_id,
                attempt_id: self
                    .work
                    .id
                    .checked_mul(100)
                    .and_then(|id| id.checked_add(self.attempts + 1))
                    .ok_or("Cargo test attempt sequence exhausted")?,
                fence_sequence: self.work.id,
            };
            let package_spec = graph
                .test_package_spec(&job.package_id)
                .map_err(|error| error.to_string())?;
            let capture = self.execute(
                worker::Operation::CargoTest {
                    cargo: tool.clone(),
                    variant: variant.clone(),
                    manifest_path: manifest_path.clone(),
                    package_spec,
                    graph_sha256: graph.context_sha256.clone(),
                },
                &graph.context_sha256,
                &ledger,
                &ledger,
                &config,
                &environment,
                inputs.clone(),
                grant,
            )?;
            let receipt = worker::classify_cargo_test(&capture, &graph, &job)?;
            receipts.push(json!({
                "target":receipt.package_spec,"packageId":receipt.package_id,"contextSha256":receipt.context_sha256,
                "bindingSha256":receipt.binding_sha256,"attemptId":receipt.attempt_id,"fenceSequence":receipt.fence_sequence,
                "targetTriple":receipt.target_triple,"lockSha256":receipt.lock_sha256,
                "outputSha256":receipt.output_sha256,"errorSha256":receipt.error_sha256,
                "artifactDirectory":capture.artifact_dir(),
                "outcome":format!("{:?}",receipt.disposition),"reason":receipt.reason_code,
                "conservativeSelection":selection.conservative_fallback,
                "counters":receipt.counters.map(|value| json!({"total":value.total,"passed":value.passed,
                    "failed":value.failed,"ignored":value.ignored,"measured":value.measured,
                    "filteredOut":value.filtered_out,"harnesses":value.harnesses})),
            }));
        }
        Ok(receipts)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;
    use std::time::{Duration, Instant};

    fn path_text(path: &Path) -> String {
        cargo::platform_path_to_wire(path.to_str().unwrap()).unwrap()
    }

    fn tool(name: &str) -> worker::Executable {
        let path =
            PathBuf::from(std::env::var_os(name).unwrap_or_else(|| panic!("missing {name}")));
        worker::Executable {
            path: path_text(&path),
            sha256: sha256(&fs::read(path).unwrap()),
        }
    }

    fn private_dir(path: &Path) {
        fs::create_dir(path).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(path, fs::Permissions::from_mode(0o700)).unwrap();
        }
    }

    #[test]
    #[ignore = "requires the owner-admitted language, pytest, Cargo and FluxR executables"]
    fn resident_scheduler_runs_registered_providers_and_recovers_receipts() {
        let temp = std::env::var_os("VODA_REAL_WORKER_TEMP")
            .map(PathBuf::from)
            .unwrap_or_else(std::env::temp_dir);
        let fixture = temp.join(format!("voda-resident-tests-{}", nonce().unwrap()));
        let root = fixture.join("workspace");
        let state = fixture.join("state");
        for path in [&fixture, &root, &state] {
            private_dir(path);
        }
        let git_command = |args: &[&str]| {
            let status = Command::new("git")
                .current_dir(&root)
                .args(args)
                .status()
                .unwrap();
            assert!(status.success(), "git {args:?}");
        };
        git_command(&["init", "-q"]);
        git_command(&[
            "-c",
            "user.name=SlopHawk fixture",
            "-c",
            "user.email=fixture@example.invalid",
            "-c",
            "commit.gpgsign=false",
            "commit",
            "--allow-empty",
            "-qm",
            "baseline",
        ]);
        let cargo_fixture = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../project-context/rust/tests/fixtures/workspace");
        for name in [
            "Cargo.toml",
            "Cargo.lock",
            "alpha/Cargo.toml",
            "beta/Cargo.toml",
            "alpha/src/lib.rs",
            "beta/src/lib.rs",
        ] {
            fs::create_dir_all(root.join(name).parent().unwrap()).unwrap();
            fs::copy(cargo_fixture.join(name), root.join(name)).unwrap();
        }
        fs::write(
            root.join("pyproject.toml"),
            b"[project]\nname = 'voda-fixture'\n",
        )
        .unwrap();
        fs::write(
            root.join("test_example.py"),
            b"def test_passes():\n    assert 1 + 1 == 2\n",
        )
        .unwrap();
        fs::write(
            root.join("affine.fluxr"),
            b"fluxr 1\npub run 3 x int factor int offset int int ret add mul x factor offset\n",
        )
        .unwrap();
        let identity = super::super::physical_root(&root).unwrap();
        let state = state.canonicalize().unwrap();
        let root = root.canonicalize().unwrap();
        let cargo_tool = tool("VODA_CARGO_BIN");
        let rustc = tool("VODA_RUSTC_BIN");
        let rustdoc = tool("VODA_RUSTDOC_BIN");
        let version = Command::new(&rustc.path).arg("-Vv").output().unwrap();
        assert!(version.status.success());
        let target = std::str::from_utf8(&version.stdout)
            .unwrap()
            .lines()
            .find_map(|line| line.strip_prefix("host: "))
            .unwrap();
        #[cfg(windows)]
        let msvc = Some(worker::MsvcToolchain {
            linker: tool("VODA_MSVC_LINKER"),
            library_paths: std::env::split_paths(&std::env::var_os("VODA_MSVC_LIB").unwrap())
                .map(|path| path_text(&path))
                .collect(),
        });
        #[cfg(not(windows))]
        let msvc = None;
        let cargo_environment = match &msvc {
            Some(msvc) => {
                worker::cargo_msvc_environment_digest(&cargo_tool, &rustc, &rustdoc, msvc, target)
            }
            None => worker::cargo_environment_digest(&cargo_tool, &rustc, &rustdoc),
        };
        let variant = worker::CargoVariant {
            rustc,
            rustdoc,
            revision: 0,
            epoch: "unset".into(),
            git_commit: "unset".into(),
            target_triple: target.into(),
            rustc_version: "rustc 1.99.1".into(),
            rustc_vv_sha256: sha256(&version.stdout),
            environment_sha256: cargo_environment,
            all_features: false,
            no_default_features: false,
            features: Vec::new(),
            msvc,
        };
        let registration = json!({"version":1,"rootIdentity":identity,
        "worker":tool("VODA_LANGUAGE_WORKER_BIN"),
        "budget":{"deadline_ms":60000,"stdout_bytes":262144,"stderr_bytes":65536,"artifact_bytes":2097152},
        "providers":[
            {"language":"python","interpreter":tool("VODA_PYTHON_BIN"),"project_root":".",
            "environment_sha256":sha256(b"isolated wheel environment"),
            "pytest_sha256":std::env::var("VODA_PYTEST_SHA256").unwrap(),
            "plugins_sha256":std::env::var("VODA_PLUGIN_SHA256").unwrap(),
            "effective_config_sha256":std::env::var("VODA_EFFECTIVE_PYTEST_CONFIG_SHA256").unwrap()},
            {"language":"rust","cargo":cargo_tool,"variant":variant,"allow_build":true,
            "allow_build_scripts":true,"allow_proc_macros":true},
            {"language":"fluxr","checked_worker":tool("VODA_FLUXR_WORKER_BIN"),
            "environment_sha256":sha256(b"pinned FluxR worker"),
            "modules":[{"module_id":"affine","logical_document_id":"affine.fluxr","relative_path":"affine.fluxr"}],
            "operations":[{"operation":"affine","module_id":"affine","export":"run","input_kinds":[],
                "argument_kinds":["i64","i64","i64"],"result_kind":"i64"}],
            "cases":[{"Operation":"affine","Args":[7,3,2],"Expected":23}],
            "limits":null,"output_scope":"tests/checked"}
        ]});
        create_bound(
            &state.join("native-tests.json"),
            &json_bytes(&registration).unwrap(),
        )
        .unwrap();
        let epoch = nonce().unwrap();
        let mut scheduler = Scheduler::open(&state, &epoch).unwrap();
        let mut running = scheduler
            .start(&root, &identity, &epoch, 5)
            .expect("first scope should run");
        assert_eq!(scheduler.status()["latest"]["state"], "running");
        let deadline = Instant::now() + Duration::from_secs(100);
        let finished = loop {
            if let Some(receipt) = running.completed() {
                break receipt.unwrap();
            }
            assert!(Instant::now() < deadline, "scheduler did not finish");
            thread::sleep(Duration::from_millis(25));
        };
        scheduler.finish(&root, &identity, 5, finished).unwrap();
        let first = scheduler.status();
        assert_eq!(first["state"], "completed", "{first:#}");
        assert_eq!(
            first["latest"]["receipts"][0]["receipts"][0]["outcome"], "Passed",
            "{first:#}"
        );
        let cargo = first["latest"]["receipts"][1]["receipts"]
            .as_array()
            .unwrap();
        assert_eq!(cargo.len(), 2, "{first:#}");
        assert!(cargo.iter().any(|receipt| receipt["target"] == "alpha@0.1.0"
            && receipt["outcome"] == "Passed"), "{first:#}");
        assert!(
            cargo.iter().any(|receipt| receipt["target"] == "beta@0.1.0"
                && receipt["outcome"] == "ZeroDiscovered"),
            "{first:#}"
        );
        assert_eq!(
            first["latest"]["receipts"][2]["receipts"][0]["receipt"]["observed_json"],
            "[23]"
        );

        let mut restored = Scheduler::open(&state, &epoch).unwrap();
        assert!(restored.start(&root, &identity, &epoch, 5).is_none());
        assert_eq!(restored.status(), first);
        let mut interrupted = restored.latest.clone().unwrap();
        interrupted.state = "running".into();
        interrupted.receipts.clear();
        restored.save(interrupted).unwrap();
        slophawk_worker_scope::recover(Duration::from_secs(3)).unwrap();
        let restored = Scheduler::open(&state, &epoch).unwrap();
        assert_eq!(restored.latest.unwrap().state, "interrupted");
        // The test-owned output is retained for inspecting raw provider artifacts.
        eprintln!("executable scheduler evidence: {}", fixture.display());
    }
}
