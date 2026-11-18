//! Scheduled use of the pinned C# project evaluator, build and VSTest peer.
use serde::Deserialize;
use serde_json::{Value, json};
use slophawk_language_execution::{self as worker, protocol::sha256};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::AtomicBool;
use std::time::Duration;

const FRAME_MAX: usize = 2 * 1024 * 1024;
const REQUEST_MAX: usize = 256 * 1024;
const OUTPUT_MAX: usize = 8 * 1024 * 1024 + 512;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Target {
    project_path: String,
    framework: String,
    results_root: PathBuf,
    closure: Value,
    runner: worker::Executable,
}

pub(super) struct Schedule<'a> {
    pub root: &'a Path,
    pub root_identity: &'a str,
    pub revision: i64,
    pub epoch: &'a str,
    pub fence: u64,
    pub scope: &'a str,
    pub branch: &'a str,
    pub commit: &'a str,
    pub execution_profile: &'a Path,
    pub deadline: Duration,
    pub cancelled: &'a AtomicBool,
    pub current: &'a dyn Fn() -> bool,
}

fn pin(root: &Path, executable: &worker::Executable) -> Result<(), String> {
    let path = Path::new(&executable.path)
        .canonicalize()
        .map_err(|error| error.to_string())?;
    if path.starts_with(root) {
        return Err("C# execution tools must be external to the checkout".into());
    }
    worker::verify_executable(executable).map_err(|error| format!("{error:?}"))
}

struct Peer<'a> {
    schedule: Schedule<'a>,
    dotnet: &'a worker::Executable,
    assembly: &'a worker::Executable,
    grant: String,
    epoch: String,
    root: String,
}

pub(super) fn run(
    schedule: Schedule<'_>,
    dotnet: &worker::Executable,
    assembly: &worker::Executable,
    targets: &[Target],
) -> Result<Vec<Value>, String> {
    if targets.is_empty() || targets.len() > 16 {
        return Err("invalid C# target budget".into());
    }
    let epoch = schedule.epoch;
    if epoch.len() != 32 || !epoch.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err("invalid C# epoch".into());
    }
    let peer = Peer {
        root: schedule
            .root
            .to_str()
            .and_then(slophawk_rust_project_context::platform_path_to_wire)
            .ok_or("invalid C# workspace path")?,
        grant: sha256(
            &worker::read_owner_document(schedule.execution_profile, schedule.root)
                .map_err(|error| error.to_string())?,
        )
        .to_ascii_uppercase(),
        epoch: format!(
            "{}-{}-{}-{}-{}",
            &epoch[..8],
            &epoch[8..12],
            &epoch[12..16],
            &epoch[16..20],
            &epoch[20..]
        ),
        schedule,
        dotnet,
        assembly,
    };
    let graph = peer
        .exchange(
            "evaluate",
            "workspace",
            json!({"root":peer.root,"dotnetPath":dotnet.path,"timeoutSeconds":peer.timeout()}),
            0,
            None,
        )?
        .remove(0);
    let graph = graph.as_object().ok_or("C# evaluation graph is absent")?;
    let evaluated = graph
        .get("targets")
        .and_then(Value::as_array)
        .ok_or("C# evaluation targets absent")?;
    if graph.get("complete") != Some(&Value::Bool(true)) {
        return Err("C# project evaluation is incomplete".into());
    }
    let mut selected = std::collections::BTreeSet::new();
    targets
        .iter()
        .enumerate()
        .map(|(index, target)| {
            let mut matching = evaluated.iter().filter(|entry| {
                entry["projectPath"] == target.project_path
                    && entry["framework"] == target.framework
                    && entry["isTestProject"] == true
            });
            let item = matching
                .next()
                .ok_or("configured C# target was not evaluated")?;
            let id = item["id"]
                .as_str()
                .ok_or("C# evaluation omitted target identity")?;
            if matching.next().is_some() || !selected.insert(id) {
                return Err("configured C# target is ambiguous".into());
            }
            peer.test(id, target, index as u64)
        })
        .collect()
}

impl Peer<'_> {
    fn timeout(&self) -> u64 {
        self.schedule.deadline.as_secs().max(1)
    }

    fn test(&self, id: &str, target: &Target, index: u64) -> Result<Value, String> {
        pin(self.schedule.root, &target.runner)?;
        let input = sha256(format!("{}\0{id}", self.schedule.scope).as_bytes());
        let work = json!({
            "root":self.root,"dotnetPath":self.dotnet.path,
            "selectedTargetId":id,"resultsRoot":target.results_root,"closure":target.closure,
            "selectedInputFingerprint":input,"causeIds":[input],
            "gitBranch":self.schedule.branch,"gitCommit":self.schedule.commit,
            "runnerPath":target.runner.path,"timeoutSeconds":self.timeout()
        });
        let build = self.exchange("build", id, work.clone(), 0, None)?.remove(0);
        let assembly = build["invokedTargets"]
            .as_array()
            .and_then(|targets| targets.last())
            .and_then(|target| target["targetAssembly"].as_str())
            .ok_or("C# build assembly is absent")?;
        let options = build["outputs"]
            .as_array()
            .ok_or("C# build outputs are absent")?;
        let mut matches = options
            .iter()
            .filter(|output| output["physicalPath"] == assembly && output["status"] == 0);
        let assembly_digest = matches
            .next()
            .and_then(|entry| entry["sha256"].as_str())
            .filter(|digest| {
                digest.len() == 64 && digest.bytes().all(|byte| byte.is_ascii_hexdigit())
            })
            .ok_or("C# build assembly digest is absent")?;
        if matches.next().is_some() || build["selectedTargetId"] != id {
            return Err("C# build receipt target or output was ambiguous".into());
        }
        let attempt = self
            .schedule
            .fence
            .checked_mul(100)
            .and_then(|value| value.checked_add(index + 50))
            .filter(|value| *value <= i64::MAX as u64)
            .ok_or("C# attempt sequence exhausted")?;
        let facts = self.exchange(
            "test",
            id,
            work,
            attempt,
            Some((assembly_digest, &target.runner.sha256)),
        )?;
        pin(self.schedule.root, &target.runner)?;
        let fact = &facts[1];
        let receipt = &fact["receipt"];
        let disposition = match fact["disposition"].as_u64() {
            Some(0) => "Passed",
            Some(1) => "Failed",
            Some(2) => "ZeroDiscovered",
            Some(3) => "AllSkipped",
            _ => return Err("C# test peer emitted an unqualified disposition".into()),
        };
        if receipt["attemptId"] != attempt
            || receipt["revision"] != self.schedule.revision
            || receipt["epoch"] != self.epoch
            || receipt["fenceSequence"] != self.schedule.fence
            || receipt["rootIdentity"] != self.schedule.root_identity
            || receipt["target"] != id
            || receipt["inputFingerprint"] != input
            || receipt["profileRevision"] != self.grant
            || receipt["platform"] != fact["disposition"]
            || !receipt["runnerSha256"]
                .as_str()
                .is_some_and(|digest| digest.eq_ignore_ascii_case(&target.runner.sha256))
        {
            return Err("C# test receipt does not bind the scheduled attempt".into());
        }
        Ok(
            json!({"target":id,"projectPath":target.project_path,"framework":target.framework,
            "outcome":disposition,"receipt":receipt}),
        )
    }

    fn exchange(
        &self,
        operation: &str,
        context: &str,
        payload: Value,
        attempt: u64,
        execution: Option<(&str, &str)>,
    ) -> Result<Vec<Value>, String> {
        let schedule = &self.schedule;
        let current = || {
            (schedule.current)()
                && worker::read_owner_document(schedule.execution_profile, schedule.root)
                    .is_ok_and(|bytes| sha256(&bytes).eq_ignore_ascii_case(&self.grant))
        };
        pin(schedule.root, self.dotnet)?;
        pin(schedule.root, self.assembly)?;
        let payload_sha256 =
            sha256(&serde_json::to_vec(&payload).map_err(|error| error.to_string())?);
        let (executable, runner) = execution.unzip();
        let identity = json!({
            "jobId":format!("csharp-{}-{operation}-{attempt}",schedule.fence),"root":self.root,
            "rootIdentity":schedule.root_identity,"context":context,"epoch":self.epoch,
            "committedRevision":schedule.revision,"fenceSequence":schedule.fence,
            "inputSha256":payload_sha256,"toolSha256":self.dotnet.sha256,
            "policySha256":self.grant,"grantRevision":self.grant,"budgetReference":"native-executable-tests-1",
            "attemptId":attempt,"attemptNumber":i32::from(attempt != 0),
            "executableSha256":executable,"runnerSha256":runner
        });
        let request = serde_json::to_vec(&json!({"schema":worker::PROTOCOL,"kind":"request",
            "operation":operation,"identity":identity,"payload":payload}))
        .map_err(|error| error.to_string())?;
        if request.len() > REQUEST_MAX {
            return Err("C# request exceeds the worker frame budget".into());
        }
        let mut input = (request.len() as u32).to_le_bytes().to_vec();
        input.extend(request);
        let mut command = Command::new(&self.dotnet.path);
        command.arg(&self.assembly.path).current_dir(schedule.root);
        let output = slophawk_worker_scope::run_guarded(
            &command,
            &input,
            slophawk_worker_scope::Limits {
                stdout: OUTPUT_MAX,
                stderr: 4096,
                deadline: schedule.deadline,
            },
            schedule.cancelled,
            &current,
        )?;
        if output.code != 0 || !current() {
            return Err("C# worker exit or grant changed".into());
        }
        pin(schedule.root, self.dotnet)?;
        pin(schedule.root, self.assembly)?;
        let reply = reply(&output.stdout, &identity, operation)?;
        Ok(reply)
    }
}

fn reply(mut bytes: &[u8], identity: &Value, operation: &str) -> Result<Vec<Value>, String> {
    let expected: &[&str] = match operation {
        "evaluate" => &["evaluated_graph"],
        "build" => &["verified_build"],
        "test" => &["verified_build", "admitted_vstest"],
        _ => return Err("unexpected C# operation".into()),
    };
    let mut facts = Vec::new();
    while !bytes.is_empty() {
        let (header, rest) = bytes.split_at_checked(4).ok_or("short C# reply header")?;
        let length = u32::from_le_bytes(header.try_into().unwrap()) as usize;
        if !(1..=FRAME_MAX).contains(&length) {
            return Err("invalid C# reply frame size".into());
        }
        let (frame, rest) = rest
            .split_at_checked(length)
            .ok_or("truncated C# reply frame")?;
        bytes = rest;
        let frame: Value = serde_json::from_slice(frame).map_err(|error| error.to_string())?;
        if frame["schema"] != worker::PROTOCOL
            || frame["operation"] != operation
            || frame["identity"] != *identity
        {
            return Err("C# reply identity does not match the request".into());
        }
        if frame["kind"] == "terminal" {
            if frame["status"] != "complete" {
                let reason = frame["reason"]
                    .as_str()
                    .filter(|text| worker::protocol::code(text))
                    .unwrap_or("unclassified");
                if let Some(omissions) = frame["omissions"].as_array() {
                    let codes = omissions
                        .iter()
                        .filter_map(Value::as_str)
                        .filter(|code| worker::protocol::code(code))
                        .take(8)
                        .collect::<Vec<_>>();
                    if !codes.is_empty() {
                        return Err(format!(
                            "C# worker refused {operation} ({reason}, {})",
                            codes.join(", ")
                        ));
                    }
                }
                return Err(format!(
                    "C# worker refused the requested operation ({reason})"
                ));
            }
            if bytes.is_empty()
                && facts.len() == expected.len()
                && frame["omissions"].as_array().is_some_and(Vec::is_empty)
            {
                return Ok(facts);
            }
            return Err("C# reply had missing or extra facts".into());
        }
        if frame["kind"] != "fact"
            || expected.get(facts.len()).copied() != frame["payload"]["kind"].as_str()
            || !frame["payload"]["data"].is_object()
        {
            return Err("unexpected C# worker fact".into());
        }
        facts.push(frame["payload"]["data"].clone());
    }
    Err("C# reply did not contain a terminal".into())
}
