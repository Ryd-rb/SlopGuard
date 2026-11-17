//! Strict `slophawk.migration-job/1` framing fixtures (not run in recipient VM).
use slophawk_test_control::{peer, *};
use std::io::{Cursor, Read, Write};

struct Duplex { input: Cursor<Vec<u8>>, written: Vec<u8> }
impl Read for Duplex {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> { self.input.read(buf) }
}
impl Write for Duplex {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.written.extend_from_slice(buf); Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> { Ok(()) }
}
fn frame(terminal: &Terminal) -> Vec<u8> {
    let mut output=vec![];
    peer::write_frame(&mut output,terminal).unwrap();
    output
}
fn job() -> Job {
    let h="F".repeat(64);
    Job { identity:JobIdentity {
        protocol:PROTOCOL.into(),operation:"test".into(),job:"test-attempt:12".into(),
        attempt:12,number:1,scope:"scope-twelve".into(),
        fence:Fence {root:"root".into(),context:"context".into(),epoch:"epoch".into(),
            generation:1,sequence:1,revision:1,inputs:h.clone(),tool:h.clone(),
            policy:h.clone(),profile:h.clone(),grant:"grant".into(),budget:"budget".into()},
        plan:Plan {key:h.clone(),target:"unit-target".into(),fingerprint:h,
            causes:vec!["known-cause".into()]}
    }, max_response_bytes:MAX_FRAME as u32 }
}
fn terminal(job: &Job) -> Terminal {
    Terminal {identity:job.identity.clone(),status:TerminalStatus::Deferred,observed:Some(RunDisposition::Unadmitted),
        reason:Some("policy not available".into()),omissions:vec!["unavailable".into()],receipt:None }
}

#[test]
fn little_endian_one_terminal_and_full_identity() {
    let job=job();
    let reply=frame(&terminal(&job));
    assert_eq!(u32::from_le_bytes(reply[..4].try_into().unwrap()) as usize,reply.len()-4);
    let mut duplex=Duplex{input:Cursor::new(reply),written:vec![]};
    let accepted=peer::exchange(&mut duplex,&job).unwrap();
    assert_eq!(accepted.status,TerminalStatus::Deferred);
    assert_eq!(u32::from_le_bytes(duplex.written[..4].try_into().unwrap()) as usize,
        duplex.written.len()-4);
    let decoded:Job=peer::read_frame(&mut Cursor::new(duplex.written)).unwrap();
    assert_eq!(decoded,job);
}
#[test]
fn reject_huge_prefix_before_allocation_and_truncated_utf8_json() {
    let mut oversized=(MAX_FRAME as u32+1).to_le_bytes().to_vec();
    assert!(peer::read_frame::<Terminal>(&mut Cursor::new(&oversized)).is_err());
    oversized=vec![7,0,0,0,b'{',b'}'];
    assert!(peer::read_frame::<Terminal>(&mut Cursor::new(&oversized)).is_err());
    oversized=vec![2,0,0,0,0xff,0xff];
    assert!(peer::read_frame::<Terminal>(&mut Cursor::new(&oversized)).is_err());
    oversized=vec![2,0,0,0,b'{',b'}'];
    assert!(peer::read_frame::<Terminal>(&mut Cursor::new(&oversized)).is_err());
}
#[test]
fn mismatched_job_epoch_budget_or_attempt_never_becomes_evidence() {
    let job=job();
    for dimension in 0..4 {
        let mut bad=terminal(&job);
        match dimension {
            0=>bad.identity.attempt+=1,
            1=>bad.identity.fence.epoch="old-epoch".into(),
            2=>bad.identity.fence.budget="unknown-budget".into(),
            _=>bad.identity.plan.fingerprint="A".repeat(64),
        }
        let reply=frame(&bad);
        let mut duplex=Duplex{input:Cursor::new(reply),written:vec![]};
        assert!(peer::exchange(&mut duplex,&job).is_err());
    }
}
#[test]
fn complete_without_receipt_or_with_omissions_is_not_complete() {
    let job=job();
    let mut bad=terminal(&job);
    bad.status=TerminalStatus::Complete;
    let mut duplex=Duplex{input:Cursor::new(frame(&bad)),written:vec![]};
    assert!(peer::exchange(&mut duplex,&job).is_err());
    bad.reason=Some("x".repeat(513));
    let mut duplex=Duplex{input:Cursor::new(frame(&bad)),written:vec![]};
    assert!(peer::exchange(&mut duplex,&job).is_err());
}
