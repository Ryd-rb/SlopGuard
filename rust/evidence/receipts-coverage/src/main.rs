//! Public-argv, explicitly unadmitted inspection adapter. The trusted library
//! `capture_coverage` remains the only producer of `PhysicalSnapshot`.
use serde::{Serialize,de::DeserializeOwned};
use slophawk_receipts_coverage::{cobertura,identity,receipt};
use std::{env,fs::File,io::{Read,Write},path::Path};
const MAX_JSON_BYTES:usize=256*1024;
const MAX_OUTPUT_BYTES:usize=4*1024*1024;

fn limited(path:&str,limit:usize)->Result<Vec<u8>,String> {
    let mut f=File::open(Path::new(path)).map_err(|_|"input_unavailable")?;
    let mut result=Vec::new();
    f.take(limit as u64+1).read_to_end(&mut result).map_err(|_|"input_read_failed")?;
    if result.len()>limit {return Err("input_budget_exceeded".into());}
    Ok(result)
}
fn json<T:DeserializeOwned>(path:&str)->Result<T,String> {
    serde_json::from_slice(&limited(path,MAX_JSON_BYTES)?).map_err(|_|"invalid_or_unknown_json_schema".into())
}
fn output<T:Serialize>(value:&T)->Result<(),String> {
    let v=serde_json::to_vec(value).map_err(|_|"output_serialization_failed")?;
    if v.len()>MAX_OUTPUT_BYTES {return Err("output_budget_exceeded".into());}
    std::io::stdout().write_all(&v).map_err(|_|"output_write_failed".to_string())?;
    std::io::stdout().write_all(b"\n").map_err(|_|"output_write_failed".to_string())
}
#[derive(Serialize)]
struct Inspection<'a>{authority:&'static str,accepted_for_verification:bool,result:&'a cobertura::ImportResult}
fn run(args:&[String])->Result<(),String> {
    match args {
        [cmd,source] if cmd=="build-identity"=>{
            let input:identity::BuildInputs=json(source)?;
            output(&identity::capture(&input))
        }
        [cmd,prior,next] if cmd=="compare-build"=>{
            let prior:identity::Stamp=json(prior)?;
            let next:identity::Stamp=json(next)?;
            output(&identity::compare(&prior,&next))
        }
        [cmd,details,admitted] if cmd=="validate-test"=>{
            let details:receipt::TestReceipt=json(details)?;
            let admitted:receipt::AdmittedAttempt=json(admitted)?;
            output(&receipt::validate(&details,&admitted).map(|_|"validated"))
        }
        [cmd,artifact,receipt_json,expected_json] if cmd=="inspect-cobertura"=>{
            let bytes=limited(artifact,slophawk_receipts_coverage::MAX_COVERAGE_BYTES)?;
            let r:cobertura::CoverageReceipt=json(receipt_json)?;
            let e:cobertura::ExpectedRun=json(expected_json)?;
            let result=cobertura::inspect_unattested(&bytes,&r,&e);
            output(&Inspection {authority:"unattested_argv_inspection",accepted_for_verification:false,result:&result})
        }
        _=>Err("usage: receipts-coverage build-identity INPUT.json | compare-build BEFORE.json AFTER.json | validate-test RECEIPT.json ATTEMPT.json | inspect-cobertura XML RECEIPT.json EXPECTED.json".into()),
    }
}
fn main(){
    let args:Vec<_>=env::args().skip(1).collect();
    if let Err(code)=run(&args) { eprintln!("receipts-coverage: {code}");std::process::exit(2); }
}
