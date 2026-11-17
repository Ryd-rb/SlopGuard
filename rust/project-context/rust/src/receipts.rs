//! Bounded stable Cargo JSON build events + libtest text summaries.
//! No claim of per-test identifiers, exhaustive discovery, or code coverage.

use crate::{
    CargoGraph, Error, MAX_LOG_BYTES, MAX_TESTS, WorkerCapture, WorkerExit, sha256_hex, test_argv,
};
use serde_json::Value;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CargoTestJob {
    pub context_sha256: String,
    pub package_id: String,
    pub attempt_id: u64,
    pub fence_sequence: u64,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Counters {
    /// Sum of `running N tests` over all observed libtest executables.
    pub total: u64,
    pub passed: u64,
    pub failed: u64,
    pub ignored: u64,
    pub measured: u64,
    pub filtered_out: u64,
    pub harnesses: u64,
}

impl Counters {
    pub fn executed(&self) -> u64 {
        self.passed + self.failed
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Disposition {
    Passed,
    Failed,
    AllSkipped,
    ZeroDiscovered,
    FilteredOut,
    BuildFailure,
    RunnerFailure,
    Cancelled,
    TimedOut,
    Incomplete,
    Malformed,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TestReceipt {
    pub format_version: u32,
    pub context_sha256: String,
    pub binding_sha256: String,
    pub workspace_revision: u64,
    pub epoch: String,
    pub attempt_id: u64,
    pub fence_sequence: u64,
    pub package_id: String,
    pub package_spec: String,
    pub target_triple: String,
    pub manifest_sha256: Vec<crate::ManifestDigest>,
    pub lock_sha256: String,
    pub output_sha256: Option<String>,
    pub error_sha256: Option<String>,
    pub output_bytes: usize,
    pub error_bytes: usize,
    pub worker_exit: WorkerExit,
    pub disposition: Disposition,
    pub reason_code: &'static str,
    /// Some statuses have *no* valid counts. No counts are invented for them.
    pub counters: Option<Counters>,
    /// A valid selected package receipt is NOT whole-workspace coverage.
    pub package_scope_only: bool,
}

impl TestReceipt {
    pub fn is_current(&self, graph: &CargoGraph) -> bool {
        self.context_sha256 == graph.context_sha256
            && self.workspace_revision == graph.binding.workspace_revision
            && self.epoch == graph.binding.epoch
            && self.lock_sha256 == graph.lock_sha256
            && self.manifest_sha256 == graph.manifest_digests
            && self.binding_sha256 == graph.binding.digest().unwrap_or_default()
    }
}

pub fn classify_test_capture(
    graph: &CargoGraph,
    job: &CargoTestJob,
    capture: &WorkerCapture,
) -> Result<TestReceipt, Error> {
    if job.attempt_id == 0 || job.context_sha256 != graph.context_sha256 {
        return Err(Error::Stale(
            "test job references another graph or an invalid attempt",
        ));
    }
    let package_spec = graph.test_package_spec(&job.package_id)?;
    let argv = test_argv(&graph.binding, &graph.root_manifest, &package_spec)?;
    capture.check(&graph.binding, &argv)?;
    if capture.attempt_id != Some(job.attempt_id)
        || capture.fence_sequence != Some(job.fence_sequence)
        || capture.graph_sha256.as_deref() != Some(graph.context_sha256.as_str())
    {
        return Err(Error::Stale(
            "test output is from another attempt, fence or graph",
        ));
    }
    let over_cap = capture.stdout.len() > MAX_LOG_BYTES || capture.stderr.len() > MAX_LOG_BYTES;
    let mut receipt = TestReceipt {
        format_version: crate::SCHEMA_VERSION,
        context_sha256: graph.context_sha256.clone(),
        binding_sha256: graph.binding.digest()?,
        workspace_revision: graph.binding.workspace_revision,
        epoch: graph.binding.epoch.clone(),
        attempt_id: job.attempt_id,
        fence_sequence: job.fence_sequence,
        package_id: job.package_id.clone(),
        package_spec,
        target_triple: graph.binding.target_triple.clone(),
        manifest_sha256: graph.manifest_digests.clone(),
        lock_sha256: graph.lock_sha256.clone(),
        output_sha256: None,
        error_sha256: None,
        output_bytes: capture.stdout.len(),
        error_bytes: capture.stderr.len(),
        worker_exit: capture.exit,
        disposition: Disposition::Incomplete,
        reason_code: "output_incomplete",
        counters: None,
        package_scope_only: true,
    };
    if over_cap || capture.truncated {
        return Ok(receipt);
    }
    receipt.output_sha256 = Some(sha256_hex(&capture.stdout));
    receipt.error_sha256 = Some(sha256_hex(&capture.stderr));
    match capture.exit {
        WorkerExit::TimedOut => {
            receipt.disposition = Disposition::TimedOut;
            receipt.reason_code = "worker_timeout";
            return Ok(receipt);
        }
        WorkerExit::Cancelled => {
            receipt.disposition = Disposition::Cancelled;
            receipt.reason_code = "worker_cancelled";
            return Ok(receipt);
        }
        WorkerExit::Terminated => {
            receipt.disposition = Disposition::Incomplete;
            receipt.reason_code = "worker_terminated";
            return Ok(receipt);
        }
        WorkerExit::Exited(_) => {}
    }
    let transcript = match parse_test_transcript(&capture.stdout) {
        Ok(report) => report,
        Err(()) => {
            receipt.disposition = Disposition::Malformed;
            receipt.reason_code = "malformed_test_transcript";
            return Ok(receipt);
        }
    };
    let exit_code = match capture.exit {
        WorkerExit::Exited(code) => code,
        _ => unreachable!("terminal worker states returned before parsing"),
    };
    if transcript.build_finished == Some(false) {
        if exit_code != 0 && transcript.counters.is_none() {
            receipt.disposition = Disposition::BuildFailure;
            receipt.reason_code = "cargo_build_failed";
        } else {
            receipt.disposition = Disposition::Malformed;
            receipt.reason_code = "contradictory_build_status";
        }
        return Ok(receipt);
    }
    if transcript.build_finished != Some(true) {
        receipt.disposition = Disposition::RunnerFailure;
        receipt.reason_code = "missing_cargo_build_receipt";
        return Ok(receipt);
    }
    let Some(counts) = transcript.counters else {
        receipt.disposition = Disposition::RunnerFailure;
        receipt.reason_code = if exit_code == 0 {
            "missing_test_summary"
        } else {
            "runner_without_summary"
        };
        return Ok(receipt);
    };
    if counts.failed > 0 && exit_code != 0 {
        receipt.disposition = Disposition::Failed;
        receipt.reason_code = "test_failures_observed";
    } else if counts.failed > 0 || exit_code != 0 || transcript.failed_summary {
        receipt.disposition = Disposition::Malformed;
        receipt.reason_code = "exit_and_summary_disagree";
        return Ok(receipt);
    } else if counts.measured > 0 {
        receipt.disposition = Disposition::Incomplete;
        receipt.reason_code = "measured_cases_not_qualified";
        return Ok(receipt);
    } else if counts.executed() > 0 {
        receipt.disposition = Disposition::Passed;
        receipt.reason_code = "selected_tests_passed";
    } else if counts.ignored > 0 {
        receipt.disposition = Disposition::AllSkipped;
        receipt.reason_code = "selected_tests_ignored";
    } else if counts.filtered_out > 0 {
        receipt.disposition = Disposition::FilteredOut;
        receipt.reason_code = "all_tests_filtered_out";
    } else {
        receipt.disposition = Disposition::ZeroDiscovered;
        receipt.reason_code = "zero_tests_discovered";
    }
    receipt.counters = Some(counts);
    Ok(receipt)
}

struct Transcript {
    build_finished: Option<bool>,
    counters: Option<Counters>,
    failed_summary: bool,
}

fn parse_test_transcript(bytes: &[u8]) -> Result<Transcript, ()> {
    let text = std::str::from_utf8(bytes).map_err(|_| ())?;
    let mut result = Transcript {
        build_finished: None,
        counters: None,
        failed_summary: false,
    };
    let mut running = None;
    for line in text.lines() {
        let line = line.trim_end_matches('\r');
        if line.starts_with('{') {
            let event: Value = serde_json::from_str(line).map_err(|_| ())?;
            let obj = event.as_object().ok_or(())?;
            let kind = obj.get("reason").and_then(Value::as_str).ok_or(())?;
            if kind == "build-finished" {
                let success = obj.get("success").and_then(Value::as_bool).ok_or(())?;
                if result.build_finished.replace(success).is_some() {
                    return Err(());
                }
            }
            // Other compiler-artifact and compiler-message records are retained
            // only in the bounded output digest. No diagnostics become tests.
        } else if let Some(text) = line.strip_prefix("running ") {
            if running.is_some() {
                return Err(());
            }
            let num = text
                .strip_suffix(" tests")
                .or_else(|| text.strip_suffix(" test"))
                .ok_or(())?;
            running = Some(num.parse::<u64>().map_err(|_| ())?);
        } else if let Some(line) = line.strip_prefix("test result: ") {
            let expected = running.take().ok_or(())?;
            let (failed, summary) = parse_summary(line)?;
            if summary.total != expected {
                return Err(());
            }
            let current = result.counters.get_or_insert_with(Counters::default);
            current.total = current.total.checked_add(summary.total).ok_or(())?;
            current.passed = current.passed.checked_add(summary.passed).ok_or(())?;
            current.failed = current.failed.checked_add(summary.failed).ok_or(())?;
            current.ignored = current.ignored.checked_add(summary.ignored).ok_or(())?;
            current.measured = current.measured.checked_add(summary.measured).ok_or(())?;
            current.filtered_out = current
                .filtered_out
                .checked_add(summary.filtered_out)
                .ok_or(())?;
            current.harnesses = current.harnesses.checked_add(1).ok_or(())?;
            if current.total > MAX_TESTS
                || current.filtered_out > MAX_TESTS
                || current.harnesses > 256
            {
                return Err(());
            }
            result.failed_summary |= failed;
        }
    }
    if running.is_some() {
        return Err(());
    }
    Ok(result)
}

fn parse_summary(line: &str) -> Result<(bool, Counters), ()> {
    let (failed, rest) = if let Some(rest) = line.strip_prefix("ok. ") {
        (false, rest)
    } else if let Some(rest) = line.strip_prefix("FAILED. ") {
        (true, rest)
    } else {
        return Err(());
    };
    let fields = rest.split("; ").collect::<Vec<_>>();
    if fields.len() != 6 || !fields[5].starts_with("finished in ") {
        return Err(());
    }
    let parse = |idx: usize, suffix: &str| -> Result<u64, ()> {
        fields[idx]
            .strip_suffix(suffix)
            .ok_or(())?
            .parse::<u64>()
            .map_err(|_| ())
    };
    let passed = parse(0, " passed")?;
    let failed_count = parse(1, " failed")?;
    let ignored = parse(2, " ignored")?;
    let measured = parse(3, " measured")?;
    let filtered_out = parse(4, " filtered out")?;
    let total = passed
        .checked_add(failed_count)
        .and_then(|n| n.checked_add(ignored))
        .and_then(|n| n.checked_add(measured))
        .ok_or(())?;
    if failed != (failed_count > 0) || total > MAX_TESTS || filtered_out > MAX_TESTS {
        return Err(());
    }
    Ok((
        failed,
        Counters {
            total,
            passed,
            failed: failed_count,
            ignored,
            measured,
            filtered_out,
            harnesses: 1,
        },
    ))
}
