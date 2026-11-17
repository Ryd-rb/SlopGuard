//! Public executable for supervised jobs. One framed request -> one framed
//! terminal. Standard output contains protocol bytes only. No worker launched.
use serde::{Deserialize,Serialize};
use serde_json::{json,Value};
use slophawk_fact_index::{Batch,Error,FactIndex,Fence,QueryLimits,Settings,FRAME_MAX_BYTES,OUTPUT_MAX_BYTES};
use std::io::{self,Read,Write};

const PROTOCOL: &str = "slophawk.migration-job/1";
#[derive(Debug,Clone,Serialize,Deserialize)]
#[serde(deny_unknown_fields)]
struct JobIdentity {
    operation: String, job_id: String, root_id: String, context_key: String,
    epoch: u64, committed_revision: u64, input_digest: String,
    tool_digest: String, policy_digest: String, budget_ref: String, grant_ref: String,
}
#[derive(Deserialize)]
#[serde(tag="action",rename_all="snake_case",deny_unknown_fields)]
enum Request {
    Ingest { batch: Batch },
    Query { fence: Fence, #[serde(default)] limits: Option<QueryLimits> },
    FactsPage { fence: Fence, document_id: String, from_ordinal: u32, max_records: u32 },
    State,
}
impl Request {
    fn name(&self)->&'static str { match self {Self::Ingest{..}=>"ingest",Self::Query{..}=>"query",Self::FactsPage{..}=>"facts_page",Self::State=>"state"} }
    fn fence(&self)->Option<&Fence> {match self {Self::Ingest{batch}=>Some(&batch.fence),Self::Query{fence,..}|Self::FactsPage{fence,..}=>Some(fence),Self::State=>None}}
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Job { protocol: String, identity: JobIdentity, request: Request }
#[derive(Clone,Copy,Serialize)]
#[serde(rename_all="snake_case")]
#[allow(dead_code)]
enum Status { Complete, Partial, Unsupported, Deferred, Cancelled, Stale, Failed }
#[derive(Serialize)]
struct Terminal {
    protocol: &'static str, identity: JobIdentity, status: Status,
    reason: Option<String>, omissions: Vec<String>, result: Option<Value>,
}
fn limited(s:&str)->String { s.chars().take(512).collect() }
fn terminal(identity:JobIdentity,status:Status,reason:Option<String>,omissions:Vec<String>,result:Option<Value>)->Terminal {
    Terminal { protocol:PROTOCOL,identity,status,reason:reason.map(|s|limited(&s)),omissions:omissions.into_iter().take(32).map(|s|limited(&s)).collect(),result }
}
fn read_frame_from<R: Read>(input:&mut R)->Result<Vec<u8>,String> {
    let mut header=[0u8;4];
    input.read_exact(&mut header).map_err(|e|format!("length header: {e}"))?;
    let len=u32::from_le_bytes(header) as usize;
    if len==0 || len>FRAME_MAX_BYTES {return Err("invalid frame byte length".into());}
    let mut body=vec![0u8;len];
    input.read_exact(&mut body).map_err(|e|format!("truncated frame: {e}"))?;
    Ok(body)
}
fn read_frame()->Result<Vec<u8>,String> { read_frame_from(&mut io::stdin().lock()) }
fn write_frame_to<W: Write>(reply:&Terminal,out:&mut W)->Result<(),String> {
    let payload=serde_json::to_vec(reply).map_err(|e|e.to_string())?;
    if payload.len()>OUTPUT_MAX_BYTES { return Err("terminal exceeds frame limit".into()); }
    out.write_all(&(payload.len() as u32).to_le_bytes()).map_err(|e|e.to_string())?;
    out.write_all(&payload).map_err(|e|e.to_string())?;
    out.flush().map_err(|e|e.to_string())?;
    Ok(())
}
fn write_frame(reply:&Terminal)->Result<(),String> { write_frame_to(reply,&mut io::stdout().lock()) }
fn valid_digest(s:&str)->bool {s.len()==64&&s.bytes().all(|b|b.is_ascii_hexdigit())}
fn same_fence(identity:&JobIdentity,f:&Fence)->bool {
    identity.root_id==f.root_id && identity.epoch==f.epoch && identity.committed_revision==f.committed_revision
    && identity.input_digest==f.input_digest && identity.tool_digest==f.tool_digest && identity.policy_digest==f.policy_digest
}
fn handle(index:&mut FactIndex,job:Job,root:&str)->Terminal {
    let Job {protocol,identity,request}=job;
    if protocol!=PROTOCOL || identity.operation!=request.name() || identity.root_id!=root
        || identity.job_id.trim().is_empty() || identity.job_id.len()>256
        || identity.context_key!="*" // workspace-wide v1; no implicit cross-context claim
        || identity.budget_ref.trim().is_empty() || identity.budget_ref.len()>256
        || identity.grant_ref.trim().is_empty() || identity.grant_ref.len()>256
        || !valid_digest(&identity.input_digest) || !valid_digest(&identity.tool_digest)
        || !valid_digest(&identity.policy_digest)
        || request.fence().is_some_and(|f| !same_fence(&identity,f))
    {
        return terminal(identity,Status::Unsupported,Some("unknown protocol, identity mismatch or unsupported context scope".into()),vec!["workspace scope only; grant references never confer execution authority".into()],None);
    }
    let outcome: Result<(Status,Value,Vec<String>),Error> = (|| {
        match request {
            Request::Ingest{batch} => {
                let receipt=index.apply(batch)?;
                let omissions=receipt.retention_frontier.clone().into_iter().collect::<Vec<_>>();
                Ok((if omissions.is_empty(){Status::Complete}else{Status::Partial},json!(receipt),omissions))
            }
            Request::Query{fence,limits} => {
                let result=index.query(&fence,limits.unwrap_or_default())?;
                let omissions=result.gaps.iter().take(32).map(|g|format!("{:?}:{}",g.reason,g.scope_key)).collect::<Vec<_>>();
                Ok((if result.gaps.is_empty(){Status::Complete}else{Status::Partial},json!(result),omissions))
            }
            Request::FactsPage{fence,document_id,from_ordinal,max_records} => {
                let facts=index.facts_page(&fence,&document_id,from_ordinal,max_records)?;
                let next_ordinal = from_ordinal.saturating_add(facts.len() as u32);
                Ok((Status::Complete,json!({"facts":facts,"next_ordinal":next_ordinal}),vec![]))
            }
            Request::State => {
                let state=index.state()?;
                if !same_fence(&identity,&state) {return Err(Error::Stale("state read fence mismatch".into()));}
                Ok((Status::Complete,json!(state),vec![]))
            }
        }
    })();
    match outcome {
        Ok((status,result,omissions))=> terminal(identity,status,None,omissions,Some(result)),
        Err(e)=>{
            let status=match e {Error::Stale(_)=>Status::Stale,Error::Budget(_)=>Status::Deferred,
                Error::UnsupportedFormat(_)=>Status::Unsupported, _=>Status::Failed};
            terminal(identity,status,Some(e.to_string()),vec![],None)
        }
    }
}
fn run()->Result<(),String> {
    let args=std::env::args().collect::<Vec<_>>();
    if args.len()!=3 {return Err("usage: fact-index <sqlite-db-path> <physical-root-id>".into());}
    let input=read_frame()?;
    // A malformed request has no trusted identity to echo; emit no terminal.
    // Callers discard all staged output when the terminal is absent.
    let job:Job=serde_json::from_slice(&input).map_err(|e|format!("malformed frame: {e}"))?;
    let identity=job.identity.clone();
    let settings=Settings::default();
    let reply=match FactIndex::open(&args[1],&args[2],settings) {
        Ok(mut index)=>handle(&mut index,job,&args[2]),
        Err(e)=>terminal(identity,match e{Error::UnsupportedFormat(_)=>Status::Unsupported,Error::Stale(_)=>Status::Stale,Error::Budget(_)=>Status::Deferred,_=>Status::Failed},Some(e.to_string()),vec![],None),
    };
    // The large result is discarded rather than streaming an incomplete output.
    // Terminal errors always remain bounded and protocol-conformant.
    if serde_json::to_vec(&reply).map_err(|e|e.to_string())?.len()>OUTPUT_MAX_BYTES {
        write_frame(&terminal(reply.identity,Status::Deferred,Some("serialized result exceeds output frame".into()),
            vec!["output-size frontier".into()],None))
    } else { write_frame(&reply) }
}
fn main() { if let Err(e)=run() { eprintln!("{}",limited(&e));std::process::exit(2); } }

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    fn identity() -> JobIdentity {
        JobIdentity {operation:"state".into(),job_id:"job-1".into(),root_id:"root".into(),
            context_key:"*".into(),epoch:0,committed_revision:0,
            input_digest:"0".repeat(64),tool_digest:"0".repeat(64),policy_digest:"0".repeat(64),
            budget_ref:"budget-ref".into(),grant_ref:"grant-ref".into()}
    }
    #[test]
    fn frame_length_is_bounded_before_allocation_and_truncation_is_rejected() {
        let mut too_large = Cursor::new(((FRAME_MAX_BYTES as u32)+1).to_le_bytes().to_vec());
        assert!(read_frame_from(&mut too_large).unwrap_err().contains("byte length"));
        let mut zero = Cursor::new(0u32.to_le_bytes().to_vec());
        assert!(read_frame_from(&mut zero).is_err());
        let mut truncated = Cursor::new([8u32.to_le_bytes().as_slice(),b"abc"].concat());
        assert!(read_frame_from(&mut truncated).unwrap_err().contains("truncated"));
    }
    #[test]
    fn terminal_echoes_full_identity_and_uses_little_endian_u32_framing() {
        let original=identity();
        let reply=terminal(original.clone(),Status::Partial,Some("work truncated".into()),vec!["one".into()],None);
        let mut output=Vec::new();
        write_frame_to(&reply,&mut output).unwrap();
        assert_eq!(u32::from_le_bytes(output[..4].try_into().unwrap()) as usize,output.len()-4);
        let decoded:Value=serde_json::from_slice(&output[4..]).unwrap();
        assert_eq!(decoded["protocol"],PROTOCOL);
        assert_eq!(decoded["identity"]["job_id"],original.job_id);
        assert_eq!(decoded["identity"]["budget_ref"],original.budget_ref);
        assert_eq!(decoded["identity"]["grant_ref"],original.grant_ref);
        assert_eq!(decoded["status"],"partial");
        assert_eq!(decoded["omissions"][0],"one");
    }
    #[test]
    fn missing_or_mismatched_job_identity_is_not_authority() {
        let path=std::env::temp_dir().join(format!("sg-mig-09-protocol-{}-{}.db",std::process::id(),line!()));
        let mut index=FactIndex::open(&path,"root",Settings::default()).unwrap();
        let mut wrong=identity();wrong.context_key="other-context".into();
        let terminal=handle(&mut index,Job{protocol:PROTOCOL.into(),identity:wrong,request:Request::State},"root");
        assert!(matches!(terminal.status,Status::Unsupported));
        let mut wrong=identity();wrong.committed_revision=4;
        let terminal=handle(&mut index,Job{protocol:PROTOCOL.into(),identity:wrong,request:Request::State},"root");
        assert!(matches!(terminal.status,Status::Stale));
        drop(index);
        let _=std::fs::remove_file(&path);
        let _=std::fs::remove_file(format!("{}-wal",path.display()));
        let _=std::fs::remove_file(format!("{}-shm",path.display()));
    }
}
