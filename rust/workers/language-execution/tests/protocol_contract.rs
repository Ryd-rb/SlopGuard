//! Source-only contract tests. Owner runs these after dependency/lock setup.
use slophawk_language_execution::protocol::{self, Frame, Identity, ReplyFailure, Status};
use std::io::Cursor;

fn identity() -> Identity {
    let h = "a".repeat(64);
    Identity {
        operation: "pytest".into(), job_id: "job".into(), attempt_id: "1".into(),
        root: "/tmp/a".into(), context_sha256: h.clone(), epoch: "epoch".into(),
        committed_revision: 8, fence_sequence: 0, input_sha256: h.clone(), source_sha256: h.clone(),
        config_sha256: h.clone(), environment_sha256: h.clone(), tool_sha256: h.clone(),
        policy_sha256: h, grant_ref: "grant".into(), budget_ref: "budget".into(),
    }
}
fn terminal(id: Identity, status: Status) -> Frame {
    Frame::Terminal {
        format: protocol::PROTOCOL.into(), identity: id, status,
        reasons: vec![], omissions: vec![],
    }
}
fn evidence(id: Identity) -> Frame {
    Frame::Evidence {
        format: protocol::PROTOCOL.into(), identity: id,
        payload: serde_json::json!({"reported":true}),
    }
}

#[test]
fn little_endian_frame_size_and_strict_round_trip() {
    let a = identity();
    let mut bytes = Vec::new();
    protocol::write_frame(&mut bytes, &evidence(a.clone())).unwrap();
    let len = u32::from_le_bytes(bytes[0..4].try_into().unwrap()) as usize;
    assert_eq!(len + 4, bytes.len());
    let mut reader = Cursor::new(bytes);
    let frame = protocol::read_frame::<Frame>(&mut reader).unwrap().unwrap();
    assert_eq!(frame.identity(), &a);
    assert!(protocol::read_frame::<Frame>(&mut reader).unwrap().is_none());
}

#[test]
fn oversized_frames_are_rejected_before_allocation() {
    let mut bytes = Cursor::new(u32::MAX.to_le_bytes().to_vec());
    assert!(protocol::read_frame::<Frame>(&mut bytes).is_err());
    let mut bytes = Cursor::new(vec![0, 0, 0, 0]);
    assert!(protocol::read_frame::<Frame>(&mut bytes).is_err());
    let mut bytes = Cursor::new(vec![12, 0, 0, 0, b'{', b'}']);
    assert!(protocol::read_frame::<Frame>(&mut bytes).is_err());
}

#[test]
fn matching_complete_terminal_is_required_to_release_evidence() {
    let a = identity();
    let b = Identity { epoch: "later".into(), ..a.clone() };
    assert!(protocol::verify_terminal(&[evidence(a.clone())], &a).is_err());
    assert!(protocol::verify_terminal(&[terminal(a.clone(), Status::Complete)], &a).is_err());
    assert!(protocol::verify_terminal(&[evidence(a.clone()), terminal(b, Status::Complete)], &a).is_err());
    assert!(protocol::verify_terminal(&[
        evidence(a.clone()), terminal(a.clone(), Status::Complete), terminal(a.clone(), Status::Complete)
    ], &a).is_err());
    assert_eq!(protocol::verify_terminal(&[
        evidence(a.clone()), terminal(a.clone(), Status::Complete)
    ], &a).unwrap()["reported"], true);
}

#[test]
fn explicit_noncomplete_terminal_preserves_reason_without_publishing() {
    let a = identity();
    let mut t = terminal(a.clone(), Status::Stale);
    if let Frame::Terminal { reasons, omissions, .. } = &mut t {
        reasons.push("revoked_profile".into());
        omissions.push("no_run_receipt".into());
    }
    let failure = protocol::verify_terminal(&[evidence(a.clone()), t], &a).unwrap_err();
    assert_eq!(failure, ReplyFailure::NonComplete {
        status: Status::Stale, reasons: vec!["revoked_profile".into()],
        omissions: vec!["no_run_receipt".into()],
    });
}

#[test]
fn cargo_argv_and_controlled_environment_bind_exact_target_and_tool() {
    use slophawk_language_execution::{Executable, cargo_environment_digest};
    use slophawk_rust_project_context::{Binding, Features, metadata_argv, test_argv};
    let cargo = Executable { path: "/opt/approved/bin/cargo".into(), sha256: "a".repeat(64) };
    let rustc = Executable { path: "/opt/approved/bin/rustc".into(), sha256: "b".repeat(64) };
    let environment = cargo_environment_digest(&cargo, &rustc);
    assert_ne!(environment, cargo_environment_digest(&cargo, &Executable {
        sha256: "c".repeat(64), ..rustc.clone()
    }));
    let binding = Binding {
        workspace_root: "/tmp/pinned-cargo".into(), workspace_revision: 5,
        epoch: "fixture-epoch".into(), git_commit: "1".repeat(40),
        target_triple: "x86_64-unknown-linux-gnu".into(),
        rustc_version: "rustc 1.99.1".into(), rustc_vv_sha256: "2".repeat(64),
        cargo_executable_sha256: cargo.sha256.clone(), environment_sha256: environment,
        features: Features { selected: vec!["alpha/with-beta".into()], ..Default::default() },
    };
    let meta = metadata_argv(&binding, "/tmp/pinned-cargo/Cargo.toml").unwrap();
    let test = test_argv(&binding, "/tmp/pinned-cargo/Cargo.toml", "alpha@0.1.0").unwrap();
    for args in [&meta, &test] {
        assert!(args.contains(&"--locked".to_string()));
        assert!(args.contains(&"--offline".to_string()));
        assert!(args.contains(&"alpha/with-beta".to_string()));
    }
    assert!(meta.contains(&"--filter-platform".to_string()));
    assert!(test.contains(&"--target".to_string()));
    assert!(test.contains(&"--message-format=json".to_string()));
}
