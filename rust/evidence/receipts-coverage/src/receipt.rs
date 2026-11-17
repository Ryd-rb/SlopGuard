//! Typed v1 test-run detail receipt. The receipt is **not** a TRX parser or a
//! source of authority: its provenance must match the admitted attempt exactly.
//! The pinned C# durable format stores only bounded, code-shaped diagnostics.
use crate::{Check, Problem, Status, digest_matches, field, valid_digest, sha256_hex};
use serde::{Deserialize, Serialize};

pub const FORMAT_VERSION:i32=1;
pub const MAX_RECORD_BYTES:usize=2_048;
pub const MAX_TRX_TESTS:i32=100_000;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum Platform {Passed, Failed, ZeroDiscovered, AllSkipped, Timeout, Cancelled,
    StartupOrBuildFailure, ArtifactFailure, RunnerFailure, Incomplete}
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum Runner {ZeroExit, NonzeroExit, Incomplete, TimedOut, Cancelled, StartupOrBuildFailure}
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum Artifact {Complete, Missing, Oversized, Malformed, Inconsistent, Stale}
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum Execution {Unknown, Passed, Failed, ZeroDiscovered, AllSkipped}
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum Scheduler {Passed, Failed, Cancelled, Other}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Counters {pub total:i32, pub executed:i32, pub passed:i32,
    pub failed:i32, pub skipped:i32, pub other:i32}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TestReceipt {
    pub format_version:i32,
    pub attempt_id:i64, pub revision:i64, pub epoch:String, pub fence_sequence:i64,
    pub git_fingerprint:String, pub root_identity:String, pub target:String,
    pub input_fingerprint:String, pub profile_revision:String, pub lease_id:Option<String>,
    pub build_domain_version:Option<i32>, pub build_sha256:Option<String>,
    pub runner_sha256:Option<String>, pub artifact_sha256:Option<String>,
    pub artifact_bytes:Option<i64>, pub run_id:Option<String>,
    pub platform:Platform, pub runner:Runner, pub artifact:Artifact,
    pub execution:Execution, pub counters:Option<Counters>, pub reason_code:String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AdmittedAttempt {
    pub attempt_id:i64, pub revision:i64, pub epoch:String, pub fence_sequence:i64,
    pub git_branch:String, pub git_commit:String, pub root_identity:String,
    pub target:String, pub input_fingerprint:String, pub profile_revision:String,
    pub stored_lease_id:Option<String>, pub scheduler:Scheduler,
}
fn valid_upper_digest(value:&str)->bool {valid_digest(value) && value==value.to_ascii_uppercase()}
fn optional_digest(value:Option<&str>)->bool {value.is_none_or(valid_upper_digest)}
fn code(s:&str)->bool {s.len()>0 && s.len()<=64 && s.bytes().all(|c|
    c.is_ascii_alphanumeric() || c==b'_' || c==b'-' || c==b'.')}
fn uuid(s:&str)->bool {
    if s.len()!=36 || s=="00000000-0000-0000-0000-000000000000" {return false;}
    s.bytes().enumerate().all(|(i,b)|if [8,13,18,23].contains(&i) {b==b'-'} else {b.is_ascii_hexdigit()})
}
/// C# `GitIdentity`: branch/commit character lengths are UTF-16 lengths.
pub fn git_identity(branch:&str,commit:&str)->String {
    let b=branch.encode_utf16().count(); let c=commit.encode_utf16().count();
    sha256_hex(format!("{b}:{branch}:{c}:{commit}").as_bytes())
}

pub fn validate_payload(r:&TestReceipt)->Check<()> {
    if r.format_version!=FORMAT_VERSION {return Err(Problem::new(Status::Unsupported,"receipt_format_unsupported"));}
    if r.attempt_id<=0 || r.revision<0 || !uuid(&r.epoch) || r.fence_sequence<0 ||
        !valid_upper_digest(&r.git_fingerprint) || !field(&r.root_identity,160) ||
        !field(&r.target,128) || !field(&r.input_fingerprint,128) || !field(&r.profile_revision,128) ||
        r.lease_id.as_deref().is_some_and(|s|s.len()>64) ||
        !optional_digest(r.build_sha256.as_deref()) || !optional_digest(r.runner_sha256.as_deref()) ||
        !optional_digest(r.artifact_sha256.as_deref()) ||
        r.build_domain_version.is_some_and(|v|v!=1) ||
        r.build_domain_version.is_some()!=r.build_sha256.is_some() ||
        r.artifact_bytes.is_some_and(|v| !(0..=crate::MAX_TRX_BYTES as i64).contains(&v)) ||
        r.artifact_sha256.is_some()!=r.artifact_bytes.is_some() ||
        r.run_id.as_deref().is_some_and(|v|!uuid(v)) || !code(&r.reason_code) {
        return Err(Problem::new(Status::Malformed,"receipt_metadata_invalid"));
    }
    if let Some(c)=&r.counters {
        if c.total<0 || c.total>MAX_TRX_TESTS || c.executed<0 || c.passed<0 || c.failed<0 ||
            c.skipped<0 || c.other<0 || c.executed>c.total || c.passed>c.executed ||
            c.failed>c.executed || i64::from(c.passed)+i64::from(c.failed)>i64::from(c.executed) ||
            i64::from(c.skipped)+i64::from(c.other)>i64::from(c.total) {
            return Err(Problem::new(Status::Malformed,"receipt_counters_invalid"));
        }
    }
    if r.artifact==Artifact::Complete && (r.artifact_sha256.is_none() ||
        !r.artifact_bytes.is_some_and(|n|n>0) || r.run_id.is_none() || r.counters.is_none()) {
        return Err(Problem::new(Status::Malformed,"complete_artifact_missing_fields"));
    }
    if r.artifact!=Artifact::Complete && r.counters.is_some() {
        return Err(Problem::new(Status::Malformed,"incomplete_artifact_has_counters"));
    }
    let is_terminal=matches!(r.runner,Runner::ZeroExit|Runner::NonzeroExit);
    let contradicted=match r.platform {
        Platform::Passed => r.execution!=Execution::Passed || r.runner!=Runner::ZeroExit ||
            r.artifact!=Artifact::Complete || !r.counters.as_ref().is_some_and(|c|
                c.executed>0 && c.failed==0 && c.passed==c.executed),
        Platform::Failed => r.execution!=Execution::Failed || !is_terminal ||
            r.artifact!=Artifact::Complete || !r.counters.as_ref().is_some_and(|c|c.failed>0),
        Platform::ZeroDiscovered => r.execution!=Execution::ZeroDiscovered || !is_terminal ||
            r.artifact!=Artifact::Complete || !r.counters.as_ref().is_some_and(|c|c.total==0),
        Platform::AllSkipped => r.execution!=Execution::AllSkipped || !is_terminal ||
            r.artifact!=Artifact::Complete || !r.counters.as_ref().is_some_and(|c|c.total>0 && c.executed==0),
        Platform::Cancelled => r.runner!=Runner::Cancelled,
        Platform::Timeout => r.runner!=Runner::TimedOut,
        Platform::ArtifactFailure => r.artifact==Artifact::Complete,
        Platform::RunnerFailure => r.execution!=Execution::Unknown || !is_terminal || r.artifact!=Artifact::Complete,
        Platform::StartupOrBuildFailure => r.runner!=Runner::StartupOrBuildFailure,
        Platform::Incomplete => r.runner!=Runner::Incomplete || r.execution!=Execution::Unknown,
    };
    if contradicted {return Err(Problem::new(Status::Malformed,"receipt_disposition_inconsistent"));}
    let stored_bytes=[&r.root_identity,&r.target,&r.input_fingerprint,&r.profile_revision,&r.reason_code]
        .iter().map(|v|v.len()).sum::<usize>() +r.lease_id.as_ref().map_or(0,String::len)+
        r.build_sha256.as_ref().map_or(0,String::len)+r.runner_sha256.as_ref().map_or(0,String::len)+
        r.artifact_sha256.as_ref().map_or(0,String::len)+r.git_fingerprint.len()+512;
    if stored_bytes>MAX_RECORD_BYTES {return Err(Problem::new(Status::Malformed,"receipt_budget_exceeded"));}
    Ok(())
}

pub fn validate(r:&TestReceipt,a:&AdmittedAttempt)->Check<()> {
    if r.attempt_id!=a.attempt_id || r.revision!=a.revision || r.epoch!=a.epoch ||
        r.fence_sequence!=a.fence_sequence || !digest_matches(&r.git_fingerprint,&git_identity(&a.git_branch,&a.git_commit)) ||
        r.root_identity!=a.root_identity || r.target!=a.target ||
        r.input_fingerprint!=a.input_fingerprint || r.profile_revision!=a.profile_revision ||
        r.lease_id!=a.stored_lease_id {
        return Err(Problem::new(Status::Mismatch,"receipt_attempt_provenance_mismatch"));
    }
    validate_payload(r)?;
    if match a.scheduler {
        Scheduler::Passed=>r.platform!=Platform::Passed,
        Scheduler::Failed=>r.platform!=Platform::Failed,
        Scheduler::Cancelled=>r.platform!=Platform::Cancelled,
        Scheduler::Other=>matches!(r.platform,Platform::Passed|Platform::Failed|Platform::Cancelled),
    } {return Err(Problem::new(Status::Mismatch,"receipt_scheduler_mismatch"));}
    Ok(())
}

pub fn usable_for_coverage(r:&TestReceipt,a:&AdmittedAttempt)->bool {
    validate(r,a).is_ok() && r.artifact==Artifact::Complete &&
        matches!(r.platform,Platform::Passed|Platform::Failed)
}
