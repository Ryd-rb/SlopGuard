//! One-shot private worker. Not a public argv adapter and not schedulable by
//! repository settings. Parent grants and supervises a Linux process group.
use slophawk_language_execution::protocol::{self, Frame, Job, Status};
use slophawk_language_execution::worker_execute;
use std::env;
use std::io::{stdin, stdout};
use std::path::Path;

fn main() {
    let args = env::args().collect::<Vec<_>>();
    if args.len() != 3 || args[1] != "--migration-job-1" {
        std::process::exit(2);
    }
    let Some(job) = protocol::read_frame::<Job>(&mut stdin().lock()).ok().flatten() else {
        std::process::exit(2);
    };
    if protocol::read_frame::<Job>(&mut stdin().lock()).ok().flatten().is_some() || !job.valid() {
        std::process::exit(2);
    }
    let (payload, status, reasons, omissions) = match worker_execute(&job, Path::new(&args[2])) {
        Ok(payload) => {
            let outcome = payload.get("outcome").and_then(|s| s.as_str());
            let status = match outcome {
                Some("unsupported") => Status::Unsupported,
                Some("stale") => Status::Stale,
                Some("cancelled") => Status::Cancelled,
                Some("refused" | "malformed_output" | "failed" | "timed_out") => Status::Failed,
                _ => Status::Complete, // pytest test failures and Cargo build failures remain observations.
            };
            if matches!(status, Status::Complete) { (Some(payload), status, vec![], vec![]) }
            else { (None, status, vec!["FluxR checked runner did not complete qualified scope".into()], vec!["unqualified_fluxr_evidence".into()]) }
        }
        Err(reason) => {
            let status = if reason.contains("not qualified") || reason.contains("unsupported") {
                Status::Unsupported
            } else if reason.contains("changed") || reason.contains("stale") {
                Status::Stale
            } else if reason.contains("unresolved") || reason.contains("unknown") {
                Status::Partial
            } else { Status::Failed };
            (None, status, vec![reason.chars().take(240).collect()], vec!["no_accepted_observation".into()])
        }
    };
    let mut out = stdout().lock();
    if let Some(payload) = payload {
        if protocol::write_frame(&mut out, &Frame::Evidence {
            format: protocol::PROTOCOL.into(), identity: job.identity.clone(), payload,
        }).is_err() { std::process::exit(2); }
    }
    if protocol::write_frame(&mut out, &Frame::Terminal {
        format: protocol::PROTOCOL.into(), identity: job.identity.clone(),
        status, reasons, omissions,
    }).is_err() { std::process::exit(2); }
}
