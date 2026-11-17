//! Cobertura run-level importer, based on the pinned C# `CoberturaCoverageImporter`.
//! XML cannot declare aliases, source/build/tool identities or read authority.
use crate::{capture::PhysicalSnapshot, digest_matches, field, logical_path,
    valid_digest, Problem, Status, MAX_COVERAGE_BYTES};
use quick_xml::events::{BytesStart, Event};
use quick_xml::Reader;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::io::Cursor;

pub const MAX_ALIASES: usize=4096;
pub const MAX_ELEMENTS: usize=200_000;
pub const MAX_CLASSES: usize=10_000;
pub const MAX_LINES: usize=20_000;
pub const MAX_DEPTH: usize=12;
const MAX_COUNT: i32=1_000_000_000;
const MAX_LINE: i32=1_000_000;
const MAX_AGE_MS: i64=86_400_000;
const SKEW_MS: i64=120_000;

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Alias {
    pub collector_path: String, pub target: String, pub source_path: String, pub source_sha256: String,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all="snake_case")]
pub enum RunOutcome { Passed, Failed, Skipped, ZeroDiscovered, Unadmitted, Incomplete }
impl RunOutcome { pub fn completed(self) -> bool { matches!(self,Self::Passed|Self::Failed) } }
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExpectedRun {
    pub root_identity: String, pub epoch: String, pub attempt_id: i64,
    pub target: String, pub build_identity: String, pub binary_identity: String,
    pub collector: String, pub collector_version: String, pub source_identity: String,
    pub started_ms: i64, pub now_ms: i64, pub outcome: RunOutcome,
    pub aliases: Vec<Alias>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CoverageReceipt {
    pub root_identity: String, pub epoch: String, pub attempt_id: i64,
    pub target: String, pub build_identity: String, pub binary_identity: String,
    pub collector: String, pub collector_version: String, pub source_identity: String,
    pub collected_ms: i64, pub artifact_sha256: String, pub aliases: Vec<Alias>,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all="snake_case")]
pub enum ImportState { Imported, Missing, Malformed, Expired, Mismatched, UnmappedScope, Unsupported }
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all="snake_case")]
pub enum LineState { Unknown, Uncovered, Observed }
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Line { pub target:String, pub source_path:String, pub source_sha256:String, pub number:i32, pub hits:i32 }
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BranchAggregate { pub target:String, pub source_path:String, pub line:i32, pub covered:i32, pub valid:i32 }
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UnknownScope { pub collector_path:String, pub reason:String }
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RunTotals {
    pub epoch:String, pub attempt_id:i64, pub target:String, pub build_identity:String,
    pub binary_identity:String, pub collector:String, pub collector_version:String,
    pub source_identity:String, pub collected_ms:i64, pub artifact_sha256:String,
    pub outcome:RunOutcome, pub lines_covered:i32, pub lines_valid:i32,
    pub branches_covered:i32, pub branches_valid:i32,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ImportResult {
    pub state:ImportState, pub reason:String, pub totals:Option<RunTotals>,
    pub lines:Vec<Line>, pub branch_aggregates:Vec<BranchAggregate>, pub unknown_scope:Vec<UnknownScope>,
}
impl ImportResult {
    pub fn fail(state:ImportState, reason:&'static str) -> Self {
        Self{state,reason:reason.into(),totals:None,lines:vec![],branch_aggregates:vec![],unknown_scope:vec![]}
    }
    pub fn line_state(&self,target:&str,source_path:&str,line:i32,source_digest:&str) -> LineState {
        match self.lines.iter().find(|v|v.target==target && v.source_path==source_path &&
            v.number==line && digest_matches(&v.source_sha256,source_digest)) {
            None=>LineState::Unknown, Some(v) if v.hits==0=>LineState::Uncovered,
            Some(_)=>LineState::Observed,
        }
    }
}
fn alias_path(path:&str) -> bool {
    field(path,1024) && !path.contains('\\') && !path.split('/').any(|p|p=="."||p=="..")
}
fn aliases_valid(aliases:&[Alias]) -> bool {
    if aliases.len()>MAX_ALIASES { return false; }
    let mut unique=HashSet::new(); let mut sources=HashMap::new();
    for a in aliases {
        if !alias_path(&a.collector_path) || !field(&a.target,256) ||
            logical_path(&a.source_path,1024).as_deref()!=Some(a.source_path.as_str()) ||
            !valid_digest(&a.source_sha256) || !unique.insert(a.clone()) { return false; }
        let key=(&a.target,&a.source_path);
        if sources.insert(key,&a.source_sha256).is_some_and(|prior|prior != &a.source_sha256) {return false;}
    }
    true
}
fn context_valid(e:&ExpectedRun) -> bool {
    e.attempt_id>0 && e.epoch!="00000000-0000-0000-0000-000000000000" &&
        [&e.root_identity,&e.epoch,&e.target,&e.build_identity,&e.binary_identity,
        &e.collector,&e.collector_version,&e.source_identity].iter().all(|v|field(v,256)) &&
        e.started_ms<=e.now_ms && aliases_valid(&e.aliases)
}
fn receipt_valid(r:&CoverageReceipt) -> bool {
    r.attempt_id>0 && r.epoch!="00000000-0000-0000-0000-000000000000" &&
        [&r.root_identity,&r.epoch,&r.target,&r.build_identity,&r.binary_identity,
        &r.collector,&r.collector_version,&r.source_identity].iter().all(|v|field(v,256)) &&
        valid_digest(&r.artifact_sha256) && aliases_valid(&r.aliases)
}
fn aliases_match(a:&[Alias],b:&[Alias]) -> bool {
    a.len()==b.len() && a.iter().collect::<HashSet<_>>()==b.iter().collect::<HashSet<_>>()
}

/// Only a physical, lease-bound capture can enter the accepted import path.
/// `ExpectedRun` and `ReadScopeFacts` must have come from a supervisor that has
/// already admitted the execution; neither of these serializable types conveys
/// authority merely because it appears in repository text.
pub fn import(snapshot:Option<&PhysicalSnapshot>,receipt:Option<&CoverageReceipt>,expected:&ExpectedRun) -> ImportResult {
    let (Some(snapshot),Some(receipt))=(snapshot,receipt) else {return ImportResult::fail(ImportState::Missing,"artifact_or_receipt_missing");};
    if !snapshot.facts.complete() || snapshot.facts.root_identity!=expected.root_identity ||
        snapshot.facts.epoch!=expected.epoch || snapshot.facts.attempt_id!=expected.attempt_id ||
        snapshot.facts.target!=expected.target || snapshot.facts.build_identity!=expected.build_identity ||
        snapshot.facts.started_ms!=expected.started_ms || snapshot.facts.now_ms!=expected.now_ms ||
        receipt.collected_ms>snapshot.facts.finished_ms.saturating_add(SKEW_MS) {
        return ImportResult::fail(ImportState::Mismatched,"read_scope_identity_mismatch");
    }
    inspect_bytes(snapshot.bytes(),snapshot.digest(),receipt,expected)
}

/// Unattested parity/diagnostic reader. Its output MUST NOT be fed to the verifier:
/// it checks receipt coherence and XML syntax, but it cannot authenticate the read.
pub fn inspect_unattested(bytes:&[u8],receipt:&CoverageReceipt,expected:&ExpectedRun) -> ImportResult {
    let digest=crate::sha256_hex(bytes);
    inspect_bytes(bytes,&digest,receipt,expected)
}
fn inspect_bytes(bytes:&[u8],digest:&str,receipt:&CoverageReceipt,expected:&ExpectedRun) -> ImportResult {
    if bytes.is_empty() || bytes.len()>MAX_COVERAGE_BYTES {return ImportResult::fail(ImportState::Malformed,"artifact_size_invalid");}
    if !context_valid(expected) || !receipt_valid(receipt) {return ImportResult::fail(ImportState::Malformed,"receipt_or_context_invalid");}
    if receipt.epoch!=expected.epoch || receipt.root_identity!=expected.root_identity ||
        receipt.attempt_id!=expected.attempt_id || receipt.target!=expected.target ||
        receipt.build_identity!=expected.build_identity || receipt.binary_identity!=expected.binary_identity ||
        receipt.collector!=expected.collector || receipt.collector_version!=expected.collector_version ||
        receipt.source_identity!=expected.source_identity {
        return ImportResult::fail(ImportState::Mismatched,"receipt_identity_mismatch");
    }
    if receipt.collected_ms<expected.started_ms || receipt.collected_ms>expected.now_ms.saturating_add(SKEW_MS) ||
        expected.now_ms.saturating_sub(receipt.collected_ms)>MAX_AGE_MS {
        return ImportResult::fail(ImportState::Expired,"receipt_outside_current_run");
    }
    if !aliases_match(&expected.aliases,&receipt.aliases) {
        return ImportResult::fail(ImportState::Mismatched,"source_alias_or_digest_mismatch");
    }
    if !digest_matches(&receipt.artifact_sha256,digest) {
        return ImportResult::fail(ImportState::Mismatched,"artifact_digest_mismatch");
    }
    if !expected.outcome.completed() {return ImportResult::fail(ImportState::Missing,"test_run_not_completed");}
    match parse(bytes,receipt,expected) {
        Ok(result)=>result,
        Err(p)=>ImportResult::fail(ImportState::Malformed,if p.code=="xml_syntax" {"invalid_cobertura_xml"}
            else {"invalid_cobertura_structure_or_counter"}),
    }
}

fn fail(code:&'static str)->Problem {Problem::new(Status::Malformed,code)}
fn checked_counter(raw:Option<&str>, limit:i32)->Result<i32,Problem> {
    let v=raw.ok_or_else(||fail("counter_missing"))?;
    if v.is_empty() || v.len()>10 || !v.bytes().all(|c|c.is_ascii_digit()) {return Err(fail("counter_invalid"));}
    let n=v.parse::<i32>().map_err(|_|fail("counter_invalid"))?;
    if n>limit {return Err(fail("counter_invalid"));} Ok(n)
}
fn percentage(raw:&str)->Result<(),Problem> {
    if raw.len()<2 || raw.len()>32 || !raw.ends_with('%') {return Err(fail("percentage_invalid"));}
    decimal_0_100(&raw[..raw.len()-1]).map(|_|())
}
fn decimal_0_100(raw:&str)->Result<f64,Problem> {
    if raw.is_empty() || !raw.bytes().all(|b|b.is_ascii_digit() || b==b'.') || raw.bytes().filter(|b|*b==b'.').count()>1 {
        return Err(fail("percentage_invalid"));
    }
    let n: f64=raw.parse().map_err(|_|fail("percentage_invalid"))?;
    if !(0.0..=100.0).contains(&n) {return Err(fail("percentage_invalid"));} Ok(n)
}
fn branch_counts(raw:&str)->Result<(i32,i32),Problem> {
    if raw.len()>80 || !raw.ends_with(')') {return Err(fail("branch_invalid"));}
    let (percent,counts)=raw.split_once("% (").ok_or_else(||fail("branch_invalid"))?;
    decimal_0_100(percent)?;
    let (covered,valid)=counts[..counts.len()-1].split_once('/').ok_or_else(||fail("branch_invalid"))?;
    let covered=checked_counter(Some(covered),MAX_COUNT)?;
    let valid=checked_counter(Some(valid),MAX_COUNT)?;
    if covered>valid {return Err(fail("branch_invalid"));} Ok((covered,valid))
}
fn permitted(parent:Option<&str>,name:&str)->bool {
    matches!((parent,name),
        (None,"coverage") | (Some("coverage"),"sources"|"packages") |
        (Some("sources"),"source") | (Some("packages"),"package") |
        (Some("package"),"classes") | (Some("classes"),"class") |
        (Some("class"),"methods"|"lines") | (Some("methods"),"method") |
        (Some("method"),"lines") | (Some("lines"),"line") |
        (Some("line"),"conditions") | (Some("conditions"),"condition"))
}
fn attributes(start:&BytesStart<'_>,reader:&Reader<Cursor<&[u8]>>)->Result<HashMap<String,String>,Problem> {
    let mut values=HashMap::new();
    for (idx,a) in start.attributes().with_checks(true).enumerate() {
        if idx>=32 {return Err(fail("attribute_budget"));}
        let a=a.map_err(|_|fail("attribute_duplicate_or_invalid"))?;
        let key=std::str::from_utf8(a.key.as_ref()).map_err(|_|fail("xml_syntax"))?;
        if key.contains(':') || key=="xmlns" {return Err(fail("xml_namespace_unsupported"));}
        let value=a.decode_and_unescape_value(reader.decoder()).map_err(|_|fail("xml_syntax"))?.into_owned();
        if key.len()>128 || value.chars().count()>4096 || values.insert(key.into(),value).is_some() {
            return Err(fail("attribute_budget"));
        }
    }
    Ok(values)
}
fn attr<'a>(map:&'a HashMap<String,String>,key:&str)->Option<&'a str> {map.get(key).map(String::as_str)}

fn parse(bytes:&[u8],receipt:&CoverageReceipt,expected:&ExpectedRun)->Result<ImportResult,Problem> {
    let mut aliases:HashMap<&str,Vec<&Alias>>=HashMap::new();
    for a in &expected.aliases {aliases.entry(&a.collector_path).or_default().push(a);}
    let mut result=ImportResult {state:ImportState::Imported,reason:"attributed".into(),
        totals:None,lines:vec![],branch_aggregates:vec![],unknown_scope:vec![]};
    let mut unknown=BTreeMap::<String,String>::new();
    let mut class_keys=HashSet::<(String,String)>::new();
    let mut line_keys=HashSet::<(String,i32)>::new();
    let mut measured_keys=HashSet::<(String,String,i32)>::new();
    let mut stack=Vec::<String>::new();
    let(mut elements,mut classes,mut line_nodes)=(0,0,0);
    let(mut packages_seen,mut sources_seen)=(false,false);
    let(mut class_alias,mut mapped):(Option<String>,Option<Alias>)=(None,None);
    let mut reader=Reader::from_reader(Cursor::new(bytes));
    reader.config_mut().check_end_names=true;
    let mut scratch=Vec::new();
    loop {
        let event=reader.read_event_into(&mut scratch).map_err(|_|fail("xml_syntax"))?;
        match event {
            Event::Eof=>break,
            Event::Start(ref e)|Event::Empty(ref e)=>{
                let empty=matches!(&event,Event::Empty(_));
                let name=std::str::from_utf8(e.name().as_ref()).map_err(|_|fail("xml_syntax"))?.to_owned();
                if name.contains(':') || stack.len()>=MAX_DEPTH || !permitted(stack.last().map(String::as_str),&name) {
                    return Err(fail("xml_structure"));
                }
                elements+=1; if elements>MAX_ELEMENTS {return Err(fail("element_budget"));}
                let a=attributes(e,&reader)?;
                if name=="coverage" {
                    if result.totals.is_some() {return Err(fail("duplicate_root"));}
                    let lc=checked_counter(attr(&a,"lines-covered"),MAX_COUNT)?;
                    let lv=checked_counter(attr(&a,"lines-valid"),MAX_COUNT)?;
                    let bc=checked_counter(attr(&a,"branches-covered"),MAX_COUNT)?;
                    let bv=checked_counter(attr(&a,"branches-valid"),MAX_COUNT)?;
                    if lc>lv || bc>bv {return Err(fail("root_counter_inconsistent"));}
                    result.totals=Some(RunTotals { epoch:receipt.epoch.clone(),attempt_id:receipt.attempt_id,
                        target:expected.target.clone(),build_identity:receipt.build_identity.clone(),
                        binary_identity:receipt.binary_identity.clone(),collector:receipt.collector.clone(),
                        collector_version:receipt.collector_version.clone(),source_identity:receipt.source_identity.clone(),
                        collected_ms:receipt.collected_ms,artifact_sha256:receipt.artifact_sha256.clone(),
                        outcome:expected.outcome,lines_covered:lc,lines_valid:lv,branches_covered:bc,branches_valid:bv });
                }
                if name=="packages" { if packages_seen {return Err(fail("duplicate_packages"));} packages_seen=true; }
                if name=="sources" { if sources_seen {return Err(fail("duplicate_sources"));} sources_seen=true; }
                if name=="package" && !attr(&a,"name").is_some_and(|v|field(v,1024)) {return Err(fail("package_invalid"));}
                if name=="class" {
                    classes+=1; if classes>MAX_CLASSES {return Err(fail("class_budget"));}
                    let class=attr(&a,"name").filter(|v|field(v,1024)).ok_or_else(||fail("class_name_invalid"))?;
                    let alias=attr(&a,"filename").filter(|v|alias_path(v)).ok_or_else(||fail("class_filename_invalid"))?;
                    if !class_keys.insert((alias.into(),class.into())) {return Err(fail("class_duplicate"));}
                    class_alias=Some(alias.into()); mapped=None;
                    match aliases.get(alias) {
                        None=>{unknown.insert(alias.into(),"unmapped_alias".into());},
                        Some(v) if v.len()!=1=>{unknown.insert(alias.into(),"ambiguous_alias".into());},
                        Some(v) if v[0].target!=expected.target=>{unknown.insert(alias.into(),"other_target".into());},
                        Some(v)=>mapped=Some(v[0].clone()),
                    }
                }
                if name=="method" && (!attr(&a,"name").is_some_and(|v|field(v,1024)) ||
                    !attr(&a,"signature").is_some_and(|v|field(v,1024))) {return Err(fail("method_invalid"));}
                if name=="line" {
                    line_nodes+=1; if line_nodes>MAX_LINES {return Err(fail("line_budget"));}
                    let number=checked_counter(attr(&a,"number"),MAX_LINE)?;
                    let hits=checked_counter(attr(&a,"hits"),MAX_COUNT)?;
                    if number==0 {return Err(fail("line_zero"));}
                    let branch=attr(&a,"branch"); let condition=attr(&a,"condition-coverage");
                    if branch.is_some_and(|v|v!="true"&&v!="false") || branch==Some("false") && condition.is_some() {
                        return Err(fail("branch_flag_invalid"));
                    }
                    let counts=condition.map(branch_counts).transpose()?;
                    if stack.len()==6 && stack[4]=="class" && stack[5]=="lines" {
                        let alias=class_alias.as_ref().ok_or_else(||fail("line_without_class"))?;
                        if !line_keys.insert((alias.clone(),number)) {return Err(fail("duplicate_line"));}
                        if let Some(m)=&mapped {
                            if !measured_keys.insert((m.target.clone(),m.source_path.clone(),number)) {
                                return Err(fail("duplicate_mapped_line"));
                            }
                            result.lines.push(Line{target:m.target.clone(),source_path:m.source_path.clone(),
                                source_sha256:m.source_sha256.clone(),number,hits});
                            if let Some((covered,valid))=counts {
                                result.branch_aggregates.push(BranchAggregate{target:m.target.clone(),
                                    source_path:m.source_path.clone(),line:number,covered,valid});
                            }
                        }
                    }
                }
                if name=="condition" {
                    checked_counter(attr(&a,"number"),MAX_LINE)?;
                    if !attr(&a,"type").is_some_and(|v|field(v,256)) {return Err(fail("condition_type_invalid"));}
                    percentage(attr(&a,"coverage").ok_or_else(||fail("condition_percentage_missing"))?)?;
                }
                stack.push(name.clone());
                if empty {
                    stack.pop(); if name=="class" {class_alias=None; mapped=None;}
                }
            }
            Event::End(ref e)=>{
                let end=std::str::from_utf8(e.name().as_ref()).map_err(|_|fail("xml_syntax"))?.to_owned();
                if stack.pop().as_deref()!=Some(end.as_str()) {return Err(fail("end_mismatch"));}
                if end=="class" {class_alias=None; mapped=None;}
            }
            Event::Text(ref e)=>{
                if stack.last().map(String::as_str)!=Some("source") &&
                    !std::str::from_utf8(e.as_ref()).map_err(|_|fail("xml_syntax"))?.trim().is_empty() {
                    return Err(fail("unexpected_text"));
                }
            }
            Event::CData(ref e)=>{
                if stack.last().map(String::as_str)!=Some("source") &&
                    !std::str::from_utf8(e.as_ref()).map_err(|_|fail("xml_syntax"))?.trim().is_empty() {
                    return Err(fail("unexpected_cdata"));
                }
            }
            Event::DocType(_)|Event::GeneralRef(_)=>return Err(fail("xml_dtd_or_entity_prohibited")),
            Event::Decl(_)|Event::Comment(_)|Event::PI(_)=>{},
        }
        scratch.clear();
    }
    if result.totals.is_none() || !packages_seen || !stack.is_empty() {return Err(fail("xml_incomplete"));}
    result.unknown_scope=unknown.into_iter().map(|(collector_path,reason)|UnknownScope{collector_path,reason}).collect();
    if !result.unknown_scope.is_empty() {result.state=ImportState::UnmappedScope; result.reason="partial_unmapped_scope".into();}
    Ok(result)
}

/// Capability wrapper: only the physical, internally validated import path can
/// mint one. Unattested `inspect_unattested` results cannot be projected.
#[derive(Clone, Debug)]
pub struct AcceptedCoverage { result:ImportResult }
impl AcceptedCoverage {
    pub fn result(&self)->&ImportResult {&self.result}
}
pub fn import_accepted(snapshot:&PhysicalSnapshot,receipt:&CoverageReceipt,expected:&ExpectedRun)
    ->Result<AcceptedCoverage,ImportResult> {
    let result=import(Some(snapshot),Some(receipt),expected);
    if matches!(result.state,ImportState::Imported|ImportState::UnmappedScope) {
        Ok(AcceptedCoverage{result})
    } else {Err(result)}
}
