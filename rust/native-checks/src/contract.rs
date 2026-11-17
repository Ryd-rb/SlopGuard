//! Caller-owned, source-bounded shadow input and explicit rule/scope output.
//! This schema is private to this opt-in CLI; it does not change daemon/MCP contracts.
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::path::{Component, Path};
use sha2::{Digest, Sha256};

pub const MAX_REQUEST_BYTES: u64 = 4 * 1024 * 1024;
pub const MAX_SOURCE_BYTES: usize = 1024 * 1024;
pub const MAX_TOTAL_BYTES: usize = 2 * 1024 * 1024;
pub const MAX_SOURCES: usize = 24;
pub const EXACT: &str = "sg10.token-clones.v1/exact";
pub const RENAMED: &str = "sg10.token-clones.v1/renamed-proven-local";
pub const NEAR: &str = "sg10.token-clones.v1/near";

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum Language { Csharp, Python, Rust, Fluxr }
impl Language {
    pub fn provider(self) -> &'static str {
        match self {
            Self::Csharp => "slophawk.csharp.roslyn.experimental",
            Self::Python => slophawk_python_syntax_provider::PROVIDER_ID,
            Self::Rust => slophawk_rust_source_provider::PROVIDER_ID,
            Self::Fluxr => slophawk_fluxr_provider::PROVIDER_ID,
        }
    }
    pub fn extension(self) -> &'static str {
        match self { Self::Csharp => ".cs", Self::Python => ".py", Self::Rust => ".rs", Self::Fluxr => ".fluxr" }
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ScopeRequest {
    pub version: u32,
    pub workspace_epoch: String,
    pub committed_revision: String,
    pub workspace_root: String,
    #[serde(default)] pub linked_roots: Vec<String>,
    pub context: Context,
    pub sources: Vec<Source>,
    #[serde(default)] pub worker: Option<Worker>,
    #[serde(default)] pub limits: Limits,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Context {
    pub key: String,
    #[serde(default)] pub target_framework: Option<String>,
    #[serde(default)] pub assembly_name: Option<String>,
    #[serde(default)] pub language_version: Option<String>,
    #[serde(default)] pub reference_pack: Option<String>,
    #[serde(default)] pub defines: Vec<String>,
    #[serde(default)] pub checked: bool,
    #[serde(default)] pub optimize: bool,
    #[serde(default)] pub reference_closure_complete: bool,
    #[serde(default)] pub source_closure_complete: bool,
    #[serde(default = "python_minor")] pub python_minor: u8,
    #[serde(default = "rust_edition")] pub rust_edition: String,
    #[serde(default)] pub fluxr_module: Option<String>,
}
fn python_minor() -> u8 { 12 }
fn rust_edition() -> String { "2024".into() }

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Source {
    pub language: Language,
    pub document_id: String,
    pub logical_path: String,
    pub physical_path: String,
    pub provenance: Provenance,
    pub bytes_base64: String,
    pub sha256: String,
    #[serde(default = "exact_utf8")] pub byte_mapping: String,
}
fn exact_utf8() -> String { "exactUtf8".into() }

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Provenance { Normal, Linked, Generated }
impl Provenance { pub fn as_str(self) -> &'static str { match self { Self::Normal => "normal", Self::Linked => "linked", Self::Generated => "generated" } } }

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Worker { pub dotnet: String, pub assembly: String }

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", default, deny_unknown_fields)]
pub struct Limits {
    pub max_records: usize,
    pub max_syntax_nodes: usize,
    pub max_frontiers: usize,
    pub max_diagnostics: usize,
    pub max_worker_bytes: usize,
    pub max_stderr_bytes: usize,
    pub worker_deadline_ms: u64,
    pub max_report_bytes: usize,
    pub min_clone_tokens: usize,
    pub max_tokens_per_block: usize,
    pub max_fingerprint_postings: usize,
    pub max_candidate_pairs: usize,
    pub max_output_pairs: usize,
    pub max_compared_tokens: i64,
}
impl Default for Limits {
    fn default() -> Self {
        Self {
            max_records: 1200, max_syntax_nodes: 80_000, max_frontiers: 32, max_diagnostics: 24,
            max_worker_bytes: 8 * 1024 * 1024, max_stderr_bytes: 64 * 1024,
            worker_deadline_ms: 10_000, max_report_bytes: 1024 * 1024,
            min_clone_tokens: 4, max_tokens_per_block: 4096, max_fingerprint_postings: 64,
            max_candidate_pairs: 20_000, max_output_pairs: 10_000, max_compared_tokens: 2_000_000,
        }
    }
}
impl Limits {
    pub fn validate(self) -> Result<(), String> {
        if !(2..=2400).contains(&self.max_records) || !(1..=100_000).contains(&self.max_syntax_nodes)
            || !(3..=32).contains(&self.max_frontiers) || !(1..=32).contains(&self.max_diagnostics)
            || !(4096..=8*1024*1024).contains(&self.max_worker_bytes)
            || !(1024..=64*1024).contains(&self.max_stderr_bytes)
            || !(100..=60_000).contains(&self.worker_deadline_ms)
            || !(4096..=4*1024*1024).contains(&self.max_report_bytes)
            || !(1..=4096).contains(&self.min_clone_tokens)
            || !(self.min_clone_tokens..=4096).contains(&self.max_tokens_per_block)
            || !(2..=1024).contains(&self.max_fingerprint_postings)
            || !(1..=100_000).contains(&self.max_candidate_pairs)
            || !(1..=100_000).contains(&self.max_output_pairs)
            || !(1..=10_000_000).contains(&self.max_compared_tokens) {
            return Err("requested report, worker or clone budgets exceed admission limits".into());
        }
        Ok(())
    }
}

#[derive(Debug, Clone)]
pub struct Loaded { pub source: Source, pub bytes: Vec<u8>, pub digest: String }

pub fn hex_digest(bytes: &[u8]) -> String { format!("{:x}", Sha256::digest(bytes)) }
fn identity(value: &str, max: usize, what: &str) -> Result<(), String> {
    if value.trim().is_empty() || value.len() > max || value.chars().any(char::is_control) {
        return Err(format!("invalid {what}: blank, control character or oversized"));
    }
    Ok(())
}
fn plain_absolute(path: &str) -> Result<(), String> {
    let p = Path::new(path);
    if !p.is_absolute() || path.len() > 1024 || p.components().any(|c| matches!(c, Component::ParentDir | Component::CurDir)) {
        return Err("paths must be absolute and free of dot/dot-dot components".into());
    }
    Ok(())
}
fn within(root: &str, path: &str) -> bool {
    // Lexical identities only; no source or grants are opened through these paths.
    Path::new(path).starts_with(Path::new(root))
}

impl ScopeRequest {
    pub fn admit(self) -> Result<(Self, Vec<Loaded>), String> {
        if self.version != 1 { return Err("unsupported shadow request version".into()); }
        identity(&self.workspace_epoch, 128, "workspace epoch")?;
        identity(&self.committed_revision, 160, "committed revision")?;
        identity(&self.context.key, 256, "context key")?;
        plain_absolute(&self.workspace_root)?;
        self.limits.validate()?;
        if self.linked_roots.len() > 16 { return Err("linked grant count exceeds 16".into()); }
        for root in &self.linked_roots { plain_absolute(root)?; }
        if self.sources.is_empty() || self.sources.len() > MAX_SOURCES {
            return Err("request must enumerate 1 to 24 sources".into());
        }
        let mut seen = HashSet::new();
        let mut physical = HashSet::new();
        let mut total = 0usize;
        let mut loaded = Vec::with_capacity(self.sources.len());
        for src in &self.sources {
            identity(&src.document_id, 160, "document ID")?;
            identity(&src.logical_path, 512, "logical path")?;
            if Path::new(&src.logical_path).is_absolute() || src.logical_path.split(['/', '\\']).any(|p| p.is_empty() || p == "." || p == "..") {
                return Err("logical source path must be relative without dot segments".into());
            }
            let extension_ok = match src.language {
                Language::Python => src.logical_path.ends_with(".py") || src.logical_path.ends_with(".pyi"),
                other => src.logical_path.ends_with(other.extension()),
            };
            if !extension_ok { return Err("language and source extension mismatch".into()); }
            plain_absolute(&src.physical_path)?;
            if !src.physical_path.ends_with(src.language.extension()) && !(src.language == Language::Python && src.physical_path.ends_with(".pyi")) {
                return Err("language and physical source extension mismatch".into());
            }
            let authorized = within(&self.workspace_root, &src.physical_path)
                || (src.provenance != Provenance::Normal && self.linked_roots.iter().any(|root| within(root, &src.physical_path)));
            if !authorized || (src.provenance == Provenance::Normal && !within(&self.workspace_root, &src.physical_path)) {
                return Err(format!("source {} is outside its provenance grant", src.document_id));
            }
            if !seen.insert(&src.document_id) || !physical.insert(&src.physical_path) {
                return Err("duplicate document ID or physical path".into());
            }
            if !matches!(src.byte_mapping.as_str(), "exactUtf8" | "unavailable") {
                return Err("unknown byte mapping declaration".into());
            }
            if src.bytes_base64.len() > 4*((MAX_SOURCE_BYTES+2)/3) {
                return Err("encoded source byte cap exceeded".into());
            }
            let bytes = crate::base64::decode(&src.bytes_base64)
                .map_err(|_| "invalid base64 source bytes")?;
            let local_max = match src.language { Language::Csharp => 512*1024, Language::Fluxr => 256*1024, Language::Python => 512*1024, Language::Rust => MAX_SOURCE_BYTES };
            if bytes.is_empty() || bytes.len() > local_max { return Err("per-source byte cap exceeded".into()); }
            total = total.checked_add(bytes.len()).ok_or("total source length overflow")?;
            if total > MAX_TOTAL_BYTES { return Err("combined source byte cap exceeded".into()); }
            let digest = hex_digest(&bytes);
            if src.sha256.len() != 64 || !src.sha256.eq_ignore_ascii_case(&digest) {
                return Err("declared source SHA-256 did not match supplied bytes".into());
            }
            loaded.push(Loaded { source: src.clone(), bytes, digest });
        }
        let csharp_count = loaded.iter().filter(|s| s.source.language == Language::Csharp).count();
        if csharp_count > 0 {
            let csharp_bytes = loaded.iter().filter(|s| s.source.language == Language::Csharp).map(|s| s.bytes.len()).sum::<usize>();
            if csharp_bytes > 768*1024 { return Err("C# worker source closure exceeds 768KiB".into()); }
            if self.worker.is_none() { return Err("C# sources require an explicitly configured worker".into()); }
            if self.context.target_framework.as_deref() != Some("net10.0") || self.context.language_version.as_deref() != Some("CSharp14") {
                return Err("C# worker supports only evaluated net10.0/CSharp14".into());
            }
            identity(self.context.assembly_name.as_deref().unwrap_or(""), 128, "assembly name")?;
            if self.context.defines.len() > 64 ||
                self.context.defines.iter().any(|d| d.len() > 128 ||
                    !d.as_bytes().first().is_some_and(|c| c.is_ascii_alphabetic() || *c == b'_') ||
                    !d.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'_')) ||
                self.context.defines.iter().collect::<HashSet<_>>().len() != self.context.defines.len() {
                // Intentionally narrower than C#'s Unicode identifier grammar; caller
                // may use approved ASCII symbols or have an evaluator reject the job.
                return Err("invalid or duplicate C# preprocessor defines".into());
            }
            if let Some(worker) = &self.worker { plain_absolute(&worker.dotnet)?; plain_absolute(&worker.assembly)?; }
            plain_absolute(self.context.reference_pack.as_deref().unwrap_or(""))?;
        }
        Ok((self, loaded))
    }
}

#[derive(Clone, Copy, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Status { Complete, Partial, Unsupported, Failed }

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ByteSpan { pub start: usize, pub end: usize }
impl ByteSpan {
    pub fn verified(&self, bytes: &[u8]) -> bool {
        self.start <= self.end && self.end <= bytes.len()
            && std::str::from_utf8(&bytes[..self.start]).is_ok()
            && std::str::from_utf8(&bytes[self.start..self.end]).is_ok()
    }
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Gap { pub reason: String, pub document_id: String, #[serde(skip_serializing_if="Option::is_none")] pub span: Option<ByteSpan> }

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Rule { pub id: String, pub status: Status, pub scope: String, pub reason: String }

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DocumentReport {
    pub document_id: String, pub provider: String, pub language: Language,
    pub context_key: String, pub source_sha256: String,
    pub status: Status, pub rules: Vec<Rule>, pub gaps: Vec<Gap>,
    pub blocks_indexed: usize, pub tokens_indexed: usize,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Pair {
    pub pair_id: String, pub language: Language, pub classification: String,
    pub left_document: String, pub right_document: String,
    pub left_block: String, pub right_block: String,
    pub left_utf16: [i32; 2], pub right_utf16: [i32; 2],
    /// Half-open coordinates in the exact SHA-256-bound original source bytes.
    pub left_bytes: [usize; 2], pub right_bytes: [usize; 2],
    pub token_count: usize,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Report {
    pub schema: &'static str, pub workspace_epoch: String, pub committed_revision: String,
    pub context_key: String, pub status: Status, pub documents: Vec<DocumentReport>,
    pub pairs: Vec<Pair>, pub gaps: Vec<Gap>, pub limits: Limits,
    pub worker_invocations: usize, pub elapsed_ms: u128,
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn containment_is_component_bounded() {
        assert!(within("/w/project", "/w/project/src/a.cs"));
        assert!(!within("/w/project", "/w/project-not/a.cs"));
        assert!(plain_absolute("/w/../src").is_err());
    }
    #[test]
    fn utf8_span_requires_boundaries() {
        let bytes = "é".as_bytes();
        assert!(!ByteSpan {start: 1, end: 2}.verified(bytes));
        assert!(ByteSpan {start: 0, end: 2}.verified(bytes));
    }
}
