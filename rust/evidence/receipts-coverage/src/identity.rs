//! Exact byte framing of `BuildOutputIdentity` domain 1, as published at
//! a367ef58076f7e29d2ae475a3b593daad04c4998. Read-scope provenance is
//! *not* inferred from these hashes; pass only collector-attested observations.
use crate::{logical_path, valid_digest, sha256_hex};
use serde::{Deserialize, Serialize};
use std::cmp::Ordering;
use std::collections::{BTreeMap, HashSet};

const DOMAIN: &str = "SlopHawk/SG-03/BuildOutputIdentity/v1";
pub const DOMAIN_VERSION: i32 = 1;
pub const MAX_PROBLEMS: usize = 24;
const MAX_ENTRIES: usize = 4096;
const SECTIONS: [&str; 11] = ["target", "mapping", "settings", "runner", "adapter",
    "membership", "compile", "project-references", "reference-assemblies", "runtime-assets", "outputs"];

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Member { pub path: String, pub link: Option<String> }
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CompileInput { pub path: String, pub link: Option<String>, pub sha256: Option<String> }
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IdentityFile { pub path: String, pub sha256: Option<String> }
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OutputObservation { Unavailable, ExpectedOnly, ByteVerified }
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeclaredOutput { pub path: String, pub sha256: Option<String>, pub observation: OutputObservation }
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CollectionMode { Unknown, LiveCheckout, ImmutableSnapshot }

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProjectTarget {
    pub id: Option<String>, pub project_path: Option<String>,
    pub framework: Option<String>, pub configuration: Option<String>,
    pub platform: Option<String>, pub runtime_identifier: Option<String>,
    pub sdk_version: Option<String>, pub is_test_project: bool,
    pub evaluation_fingerprint: Option<String>,
    pub compile: Option<Vec<Member>>, pub references: Option<Vec<String>>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BuildInputs {
    pub target: ProjectTarget, pub project_graph_complete: bool,
    pub path_mapping_identity: Option<String>, pub path_mapping_trusted: bool,
    pub collection_mode: CollectionMode, pub settings_sha256: Option<String>,
    pub runner_identity: Option<String>, pub adapter_identity: Option<String>,
    pub ordered_compile_inputs: Option<Vec<CompileInput>>,
    pub reference_assemblies: Option<Vec<IdentityFile>>,
    pub runtime_assets: Option<Vec<IdentityFile>>,
    pub declared_outputs: Option<Vec<DeclaredOutput>>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProblemKind { Incomplete, Mismatch, UntrustedPath, UnsupportedIdentity }
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IdentityProblem { pub kind: ProblemKind, pub field: String }
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Section { pub name: String, pub sha256: String }
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Stamp {
    pub domain_version: i32, pub sha256: String, pub complete: bool,
    pub sections: Vec<Section>, pub problems: Vec<IdentityProblem>,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Reuse { pub reusable: bool, pub problems: Vec<IdentityProblem> }

#[derive(Default)]
struct Problems { list: Vec<IdentityProblem>, complete: bool }
impl Problems {
    fn new() -> Self { Self { list: Vec::new(), complete: true } }
    fn add(&mut self, kind: ProblemKind, name: impl Into<String>) {
        self.complete = false;
        let p = IdentityProblem { kind, field: name.into() };
        if self.list.len() < MAX_PROBLEMS && !self.list.contains(&p) { self.list.push(p); }
    }
}
#[derive(Default)]
struct Wire(Vec<u8>);
impl Wire {
    fn number(&mut self, value: i32) { self.0.extend(value.to_be_bytes()); }
    fn text(&mut self, value: Option<&str>) {
        if let Some(v) = value { self.number(v.len() as i32); self.0.extend(v.as_bytes()); }
        else { self.number(-1); }
    }
    fn bool(&mut self, value: bool) { self.number(i32::from(value)); }
    fn digest(&self) -> String { sha256_hex(&self.0) }
}
fn section(name: &str, p: &mut Problems, body: impl FnOnce(&mut Wire, &mut Problems)) -> Section {
    let mut w = Wire::default(); w.text(Some(DOMAIN)); w.text(Some(name));
    body(&mut w, p);
    Section { name: name.into(), sha256: w.digest() }
}
fn ordinal(a: &str, b: &str) -> Ordering { a.encode_utf16().cmp(b.encode_utf16()) }
fn opt_ordinal(a: &Option<String>, b: &Option<String>) -> Ordering {
    match (a, b) {
        (None, None) => Ordering::Equal, (None, _) => Ordering::Less, (_, None) => Ordering::Greater,
        (Some(a), Some(b)) => ordinal(a, b),
    }
}
fn member_order(a: &(Option<String>, Option<String>), b: &(Option<String>, Option<String>)) -> Ordering {
    opt_ordinal(&a.0, &b.0).then_with(|| opt_ordinal(&a.1, &b.1))
}
fn normalized_text(p: &mut Problems, raw: Option<&str>, name: &str, required: bool) -> Option<String> {
    match raw {
        None => { p.add(ProblemKind::Incomplete, name); None },
        Some("") if required => { p.add(ProblemKind::Incomplete, name); None },
        Some(v) if v.encode_utf16().count() > 4096 || v.chars().any(char::is_control) => {
            p.add(ProblemKind::UnsupportedIdentity, name); None
        }
        Some(v) => Some(v.into()),
    }
}
fn normalized_path(p: &mut Problems, raw: Option<&str>, name: &str) -> Option<String> {
    let text = normalized_text(p, raw, name, true)?;
    match logical_path(&text, usize::MAX) {
        Some(v) => Some(v),
        None => { p.add(ProblemKind::UntrustedPath, name); None }
    }
}
fn optional_path(p: &mut Problems, raw: Option<&str>, name: &str) -> Option<String> {
    raw.and_then(|v| normalized_path(p, Some(v), name))
}
fn normalized_digest(p: &mut Problems, raw: Option<&str>, name: &str) -> Option<String> {
    match raw {
        None | Some("") => { p.add(ProblemKind::Incomplete, name); None },
        Some(v) if !valid_digest(v) => { p.add(ProblemKind::UnsupportedIdentity, name); None },
        Some(v) => Some(v.to_ascii_uppercase()),
    }
}
fn reference_id(p: &mut Problems, id: Option<&str>, name: &str) -> Option<String> {
    if let Some(s) = id { if valid_digest(s) { return Some(s.to_ascii_uppercase()); } }
    let Some((path, tfm)) = id.and_then(|v| v.rsplit_once('|')) else {
        p.add(if id.is_some() { ProblemKind::UnsupportedIdentity } else { ProblemKind::Incomplete }, name);
        return None;
    };
    if path.is_empty() || tfm.is_empty() {
        p.add(ProblemKind::UnsupportedIdentity, name); return None;
    }
    let path = normalized_path(p, Some(path), &format!("{name}.path"))?;
    let tfm = normalized_text(p, Some(tfm), &format!("{name}.tfm"), true)?;
    Some(format!("{path}|{tfm}"))
}
fn count<T>(p: &mut Problems, w: &mut Wire, entries: Option<&[T]>, name: &str) -> bool {
    match entries {
        None => { w.number(-1); p.add(ProblemKind::Incomplete, name); false },
        Some(a) if a.len() > MAX_ENTRIES => {
            w.number(-2); p.add(ProblemKind::UnsupportedIdentity, format!("{name}.budget")); false
        },
        Some(a) => { w.number(a.len() as i32); true }
    }
}
fn write_files(p: &mut Problems, w: &mut Wire, entries: Option<&[IdentityFile]>, name: &str) {
    if !count(p, w, entries, name) { return; }
    let mut files: Vec<_> = entries.unwrap().iter().enumerate().map(|(i,f)| (
        normalized_path(p, Some(&f.path), &format!("{name}[{i}].path")),
        normalized_digest(p, f.sha256.as_deref(), &format!("{name}[{i}].digest"))
    )).collect();
    files.sort_by(|a,b| opt_ordinal(&a.0,&b.0).then_with(||opt_ordinal(&a.1,&b.1)));
    for (path,digest) in &files { w.text(path.as_deref()); w.text(digest.as_deref()); }
    if files.windows(2).any(|v| v[0].0.is_some() && v[0].0 == v[1].0) {
        p.add(ProblemKind::UnsupportedIdentity, format!("{name}.duplicate"));
    }
}

pub fn capture(input: &BuildInputs) -> Stamp {
    let mut p = Problems::new(); let t = &input.target;
    let mut sections = Vec::with_capacity(SECTIONS.len());
    sections.push(section("target", &mut p, |w,p| {
        w.text(reference_id(p, t.id.as_deref(), "target.id").as_deref());
        w.text(normalized_path(p, t.project_path.as_deref(), "target.project").as_deref());
        for (value, name, required) in [
            (t.framework.as_deref(), "target.tfm", true),
            (t.configuration.as_deref(), "target.configuration", true),
            (t.platform.as_deref(), "target.platform", true),
            (t.runtime_identifier.as_deref(), "target.rid", false),
            (t.sdk_version.as_deref(), "target.sdk", true),
        ] { w.text(normalized_text(p, value, name, required).as_deref()); }
        w.bool(t.is_test_project); w.bool(input.project_graph_complete);
        if !input.project_graph_complete { p.add(ProblemKind::Incomplete, "target.graph"); }
        w.text(normalized_digest(p, t.evaluation_fingerprint.as_deref(), "target.evaluation").as_deref());
    }));
    sections.push(section("mapping", &mut p, |w,p| {
        w.text(normalized_text(p, input.path_mapping_identity.as_deref(), "mapping.identity", true).as_deref());
        w.bool(input.path_mapping_trusted);
        w.number(match input.collection_mode { CollectionMode::Unknown=>0, CollectionMode::LiveCheckout=>1, CollectionMode::ImmutableSnapshot=>2 });
        if !input.path_mapping_trusted { p.add(ProblemKind::UntrustedPath, "mapping.trust"); }
        if input.collection_mode == CollectionMode::Unknown { p.add(ProblemKind::Incomplete, "mapping.collection_mode"); }
    }));
    sections.push(section("settings", &mut p, |w,p| w.text(normalized_digest(p, input.settings_sha256.as_deref(), "settings.digest").as_deref())));
    sections.push(section("runner", &mut p, |w,p| w.text(normalized_text(p, input.runner_identity.as_deref(), "runner.identity", true).as_deref())));
    sections.push(section("adapter", &mut p, |w,p| w.text(normalized_text(p, input.adapter_identity.as_deref(), "adapter.identity", true).as_deref())));

    let mut membership: Option<Vec<(Option<String>,Option<String>)>> = None;
    sections.push(section("membership", &mut p, |w,p| {
        if !count(p,w,t.compile.as_deref(),"target.compile") { return; }
        let mut list: Vec<_> = t.compile.as_ref().unwrap().iter().enumerate().map(|(i,m)| (
            normalized_path(p,Some(&m.path),&format!("target.compile[{i}].path")),
            optional_path(p,m.link.as_deref(),&format!("target.compile[{i}].link"))
        )).collect();
        list.sort_by(member_order);
        for (path,link) in &list { w.text(path.as_deref()); w.text(link.as_deref()); }
        if list.windows(2).any(|v| v[0].0.is_some() && v[0] == v[1]) {
            p.add(ProblemKind::UnsupportedIdentity,"target.compile.duplicate");
        }
        membership = Some(list);
    }));
    let mut observed: Option<Vec<(Option<String>,Option<String>)>> = None;
    sections.push(section("compile", &mut p, |w,p| {
        if !count(p,w,input.ordered_compile_inputs.as_deref(),"compile") { return; }
        let mut list = Vec::new();
        for (i,entry) in input.ordered_compile_inputs.as_ref().unwrap().iter().enumerate() {
            let path = normalized_path(p, Some(&entry.path), &format!("compile[{i}].path"));
            let link = optional_path(p, entry.link.as_deref(), &format!("compile[{i}].link"));
            let digest = normalized_digest(p, entry.sha256.as_deref(), &format!("compile[{i}].digest"));
            w.text(path.as_deref()); w.text(link.as_deref()); w.text(digest.as_deref());
            list.push((path,link));
        }
        let mut sorted = list.clone(); sorted.sort_by(member_order);
        if sorted.windows(2).any(|v| v[0].0.is_some() && v[0] == v[1]) {
            p.add(ProblemKind::UnsupportedIdentity,"compile.duplicate");
        }
        observed = Some(sorted);
    }));
    if let (Some(m),Some(o)) = (&membership,&observed) {
        if m != o { p.add(ProblemKind::Mismatch,"compile.membership"); }
    }
    sections.push(section("project-references", &mut p, |w,p| {
        if !count(p,w,t.references.as_deref(),"target.references") { return; }
        let mut ids: Vec<_> = t.references.as_ref().unwrap().iter().enumerate().map(|(i,id)|
            reference_id(p,Some(id),&format!("target.references[{i}]"))).collect();
        ids.sort_by(opt_ordinal); for id in &ids { w.text(id.as_deref()); }
        if ids.windows(2).any(|v| v[0] == v[1]) {
            p.add(ProblemKind::UnsupportedIdentity,"target.references.duplicate");
        }
    }));
    sections.push(section("reference-assemblies", &mut p, |w,p| write_files(p,w,input.reference_assemblies.as_deref(),"references")));
    sections.push(section("runtime-assets", &mut p, |w,p| write_files(p,w,input.runtime_assets.as_deref(),"runtime")));
    sections.push(section("outputs", &mut p, |w,p| {
        if !count(p,w,input.declared_outputs.as_deref(),"outputs") { return; }
        let outputs=input.declared_outputs.as_ref().unwrap();
        if outputs.is_empty() { p.add(ProblemKind::Incomplete,"outputs.empty"); }
        let mut list: Vec<_> = outputs.iter().enumerate().map(|(i,o)| (
            normalized_path(p,Some(&o.path),&format!("outputs[{i}].path")),
            normalized_digest(p,o.sha256.as_deref(),&format!("outputs[{i}].digest")),
            o.observation
        )).collect();
        list.sort_by(|a,b| opt_ordinal(&a.0,&b.0).then_with(||opt_ordinal(&a.1,&b.1)).then_with(||
            (a.2 as i32).cmp(&(b.2 as i32))));
        for (path,digest,observation) in &list {
            w.text(path.as_deref()); w.text(digest.as_deref());
            w.number(match observation { OutputObservation::Unavailable=>0, OutputObservation::ExpectedOnly=>1, OutputObservation::ByteVerified=>2 });
            if *observation != OutputObservation::ByteVerified { p.add(ProblemKind::Incomplete,"outputs.not_byte_verified"); }
        }
        if list.windows(2).any(|v| v[0].0.is_some() && v[0].0 == v[1].0) {
            p.add(ProblemKind::UnsupportedIdentity,"outputs.duplicate");
        }
    }));
    let mut w=Wire::default(); w.text(Some(DOMAIN)); w.number(DOMAIN_VERSION);
    for s in &sections { w.text(Some(&s.name)); w.text(Some(&s.sha256)); }
    Stamp { domain_version: DOMAIN_VERSION, sha256:w.digest(),complete:p.complete,
        sections, problems:p.list }
}

fn valid_stamp(s: &Stamp) -> bool {
    if s.domain_version != DOMAIN_VERSION || s.sha256.len()!=64 ||
        !valid_digest(&s.sha256) || s.sha256 != s.sha256.to_ascii_uppercase() ||
        s.sections.len()!=SECTIONS.len() || s.problems.len()>MAX_PROBLEMS ||
        s.complete == !s.problems.is_empty() { return false; }
    let mut w=Wire::default(); w.text(Some(DOMAIN)); w.number(DOMAIN_VERSION);
    for (section,name) in s.sections.iter().zip(SECTIONS) {
        if section.name != name || !valid_digest(&section.sha256) ||
            section.sha256 != section.sha256.to_ascii_uppercase() { return false; }
        w.text(Some(name)); w.text(Some(&section.sha256));
    }
    w.digest() == s.sha256
}
pub fn compare(previous: &Stamp, current: &Stamp) -> Reuse {
    let mut p=Problems::new(); let old_valid=valid_stamp(previous); let new_valid=valid_stamp(current);
    if !old_valid || !new_valid { p.add(ProblemKind::UnsupportedIdentity,"stamp.domain_or_structure"); }
    for (name,s) in [("previous",previous),("current",current)] {
        if !s.complete {
            p.add(ProblemKind::Incomplete,name);
            for issue in &s.problems { p.add(issue.kind,issue.field.clone()); }
        }
    }
    if old_valid && new_valid && previous.sha256 != current.sha256 {
        let a: BTreeMap<_,_>=previous.sections.iter().map(|s|(&s.name,&s.sha256)).collect();
        let b: BTreeMap<_,_>=current.sections.iter().map(|s|(&s.name,&s.sha256)).collect();
        let names: HashSet<_>=a.keys().chain(b.keys()).copied().collect();
        let mut names: Vec<_>=names.into_iter().collect(); names.sort_by(|a,b|ordinal(a,b));
        for name in names { if a.get(name) != b.get(name) { p.add(ProblemKind::Mismatch,name.as_str()); } }
        if p.complete { p.add(ProblemKind::Mismatch,"fingerprint"); }
    }
    Reuse { reusable:p.complete, problems:p.list }
}

/// The build must be paired with independently observed, complete runtime closure;
/// an exact hash alone never attests that all runtime dependencies were discovered.
pub fn closure_checked(stamp: &Stamp, input: &BuildInputs,
    required_runtime: &[String], observed_runtime: &[String]) -> Reuse {
    let mut p=Problems::new();
    if !stamp.complete || !valid_stamp(stamp) || capture(input).sha256 != stamp.sha256 {
        p.add(ProblemKind::Incomplete,"build.identity");
    }
    if required_runtime.len()>MAX_ENTRIES || observed_runtime.len()>MAX_ENTRIES {
        p.add(ProblemKind::UnsupportedIdentity,"runtime.observation_budget");
    } else {
        let required: HashSet<_>=required_runtime.iter().map(|s|logical_path(s,4096)).collect();
        if required.contains(&None) || required.len()!=required_runtime.len() {
            p.add(ProblemKind::UntrustedPath,"runtime.required");
        }
        // A caller cannot silently claim a complete closure that differs from the
        // runtime-asset declarations hashed into this exact BuildOutputIdentity stamp.
        let declared: HashSet<_> = input.runtime_assets.as_ref().map(|files|
            files.iter().map(|f|logical_path(&f.path,4096)).collect()).unwrap_or_default();
        if input.runtime_assets.is_none() || required!=declared {
            p.add(ProblemKind::Incomplete,"runtime.declared_closure");
        }
        for path in observed_runtime {
            if !required.contains(&logical_path(path,4096)) {
                p.add(ProblemKind::Incomplete,"runtime.undeclared");
            }
        }
    }
    Reuse { reusable:p.complete, problems:p.list }
}
