//! SG-MIG-09: a derived, revision-fenced SQLite index of provider facts and
//! preclassified whole-block SG-10 tokens. No source discovery or tool execution.
//!
//! A file belongs to exactly one physical root. A logical document ID already
//! includes its evaluated context (as required by the published clone contract);
//! the context and physical source identity are additionally stored verbatim.
//! Never use this file as the daemon's authoritative source inventory.

use rusqlite::{params, Connection, OptionalExtension, Row, Transaction, TransactionBehavior};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::cmp::Ordering;
use std::collections::{HashMap, HashSet};
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

pub const SCHEMA_VERSION: i64 = 1;
pub const APPLICATION_ID: i64 = 0x5346_4939; // "SFI9"
pub const RULE_VERSION: &str = "sg10.token-clones.v1";
pub const FRAME_MAX_BYTES: usize = 4 * 1024 * 1024;
pub const OUTPUT_MAX_BYTES: usize = 4 * 1024 * 1024;
const PAGE_SIZE: u64 = 4096;
const POSTING_PAGE: i64 = 64;
const MAX_TEXT_BYTES: usize = 4096;

#[derive(Debug)]
pub enum Error {
    Sqlite(rusqlite::Error),
    Json(serde_json::Error),
    Io(std::io::Error),
    Invalid(String),
    UnsupportedFormat(String),
    Corrupt(String),
    Stale(String),
    Budget(String),
}
impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Sqlite(e) => write!(f, "sqlite: {e}"),
            Self::Json(e) => write!(f, "json: {e}"),
            Self::Io(e) => write!(f, "I/O: {e}"),
            Self::Invalid(e) => write!(f, "invalid input: {e}"),
            Self::UnsupportedFormat(e) => write!(f, "unsupported format: {e}"),
            Self::Corrupt(e) => write!(f, "corrupt index: {e}"),
            Self::Stale(e) => write!(f, "stale revision: {e}"),
            Self::Budget(e) => write!(f, "budget exceeded: {e}"),
        }
    }
}
impl std::error::Error for Error {}
impl From<rusqlite::Error> for Error { fn from(e: rusqlite::Error) -> Self { Self::Sqlite(e) } }
impl From<serde_json::Error> for Error { fn from(e: serde_json::Error) -> Self { Self::Json(e) } }
impl From<std::io::Error> for Error { fn from(e: std::io::Error) -> Self { Self::Io(e) } }
type Result<T> = std::result::Result<T, Error>;

/// Capacities are not measurements of RSS. They partition the proposed 32 MiB
/// ceiling: SQLite cache 8 MiB, decoded tokens/strings 8 MiB, candidate page
/// 1 MiB, statements 1 MiB, input queue 4 MiB, output queue 4 MiB, reserve
/// 6 MiB. The caller must measure complete process-tree memory at integration.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Settings {
    pub minimum_tokens: u32,
    pub maximum_tokens_per_block: u32,
    pub maximum_fingerprint_postings: u32,
    pub maximum_token_bytes: u32,
    pub maximum_document_blocks: u32,
    pub maximum_document_facts: u32,
    pub maximum_disk_bytes: u64,
}
impl Default for Settings {
    fn default() -> Self {
        Self { minimum_tokens: 40, maximum_tokens_per_block: 4096,
            maximum_fingerprint_postings: 64, maximum_token_bytes: 256,
            maximum_document_blocks: 4096, maximum_document_facts: 20_000,
            maximum_disk_bytes: 256 * 1024 * 1024 }
    }
}
impl Settings {
    fn validate(self) -> Result<()> {
        if self.minimum_tokens == 0 || self.maximum_tokens_per_block < self.minimum_tokens
            || self.maximum_tokens_per_block > 4096 || !(2..=64).contains(&self.maximum_fingerprint_postings)
            || !(1..=512).contains(&self.maximum_token_bytes)
            || !(1..=8192).contains(&self.maximum_document_blocks)
            || !(1..=20_000).contains(&self.maximum_document_facts)
            || !(16 * 1024 * 1024..=2 * 1024 * 1024 * 1024).contains(&self.maximum_disk_bytes)
        { return Err(Error::Invalid("invalid storage settings".into())); }
        Ok(())
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Fence {
    pub root_id: String,
    pub epoch: u64,
    pub committed_revision: u64,
    pub input_digest: String,
    pub tool_digest: String,
    pub policy_digest: String,
}
impl Fence {
    fn validate(&self) -> Result<()> {
        id(&self.root_id, "root_id")?;
        for (name, value) in [("input_digest", &self.input_digest), ("tool_digest", &self.tool_digest), ("policy_digest", &self.policy_digest)] {
            digest(value, name)?;
        }
        i64_bound(self.epoch, "epoch")?;
        i64_bound(self.committed_revision, "revision")?;
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Span {
    pub start_offset: i32,
    pub end_offset: i32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub start_line: Option<i32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub start_column: Option<i32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub end_line: Option<i32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub end_column: Option<i32>,
}
impl Span {
    pub fn offsets(start_offset: i32, end_offset: i32) -> Self {
        Self { start_offset, end_offset, start_line: None, start_column: None, end_line: None, end_column: None }
    }
    fn validate(&self) -> Result<()> {
        if self.start_offset < 0 || self.end_offset < self.start_offset { return Err(Error::Invalid("invalid half-open span".into())); }
        if [self.start_line, self.start_column, self.end_line, self.end_column].into_iter().flatten().any(|v| v < 0) {
            return Err(Error::Invalid("negative line/column".into()));
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum TokenKind {
    #[serde(alias = "other")] Other,
    #[serde(alias = "local_identifier")] LocalIdentifier,
    #[serde(alias = "bound_identifier")] BoundIdentifier,
    #[serde(alias = "literal")] Literal,
}
impl TokenKind {
    fn ordinal(self) -> i32 { match self { Self::Other => 0, Self::LocalIdentifier => 1, Self::BoundIdentifier => 2, Self::Literal => 3 } }
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Token { pub kind: TokenKind, pub text: String }
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Block { pub block_id: String, pub span: Span, pub tokens: Vec<Token> }

/// `payload` is a bounded, opaque provider record; its schema and coordinate
/// unit are retained rather than interpreted as a compiler claim.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Fact {
    pub kind: String,
    pub fact_id: String,
    #[serde(default)] pub symbol_id: Option<String>,
    #[serde(default)] pub owner_id: Option<String>,
    #[serde(default)] pub span: Option<Span>,
    pub payload: Value,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Document {
    pub document_id: String,
    pub context_key: String,
    pub source_id: String,
    pub physical_path: String,
    pub logical_path: String,
    pub provider_id: String,
    pub provider_schema: String,
    pub coordinate_unit: String,
    pub version: String,
    pub source_digest: String,
    pub is_generated: bool,
    pub is_excluded: bool,
    #[serde(default)] pub facts: Vec<Fact>,
    #[serde(default)] pub blocks: Vec<Block>,
}
impl Document {
    fn validate(&self, settings: Settings) -> Result<()> {
        for (n, v) in [("document_id", &self.document_id), ("context_key", &self.context_key),
            ("source_id", &self.source_id), ("physical_path", &self.physical_path),
            ("logical_path", &self.logical_path), ("provider_id", &self.provider_id),
            ("provider_schema", &self.provider_schema), ("version", &self.version)] {
            id(v, n)?;
        }
        id(&self.source_digest, "source_digest")?; // Published Roslyn InputFingerprint is opaque, not necessarily hex.
        if self.coordinate_unit != "utf16" && self.coordinate_unit != "byte" && self.coordinate_unit != "provider" {
            return Err(Error::Invalid("unknown coordinate unit".into()));
        }
        if self.facts.len() > settings.maximum_document_facts as usize || self.blocks.len() > settings.maximum_document_blocks as usize {
            return Err(Error::Budget("document record count".into()));
        }
        let mut keys = HashSet::new();
        let mut fact_bytes = 0usize;
        let mut token_bytes = 0usize;
        for fact in &self.facts {
            id(&fact.kind, "fact.kind")?;
            id(&fact.fact_id, "fact.fact_id")?;
            if let Some(s) = &fact.symbol_id { id(s, "fact.symbol_id")?; }
            if let Some(s) = &fact.owner_id { id(s, "fact.owner_id")?; }
            if let Some(s) = &fact.span { s.validate()?; }
            fact_bytes = fact_bytes.checked_add(serde_json::to_vec(&fact.payload)?.len()).ok_or_else(|| Error::Budget("fact byte overflow".into()))?;
            if fact_bytes > FRAME_MAX_BYTES { return Err(Error::Budget("document facts exceed frame allowance".into())); }
        }
        for block in &self.blocks {
            id(&block.block_id, "block_id")?;
            if !keys.insert(&block.block_id) { return Err(Error::Invalid("duplicate block ID".into())); }
            block.span.validate()?;
            if block.span.end_offset == block.span.start_offset { return Err(Error::Invalid("clone block requires nonempty span".into())); }
            if block.tokens.len() > 65_536 { return Err(Error::Budget("oversized block input count".into())); }
            for token in &block.tokens {
                if token.text.is_empty() || token.text.len() > settings.maximum_token_bytes as usize {
                    return Err(Error::Budget("invalid or oversized token".into()));
                }
                token_bytes = token_bytes.saturating_add(3).saturating_add(token.text.len());
                if token_bytes.saturating_add(fact_bytes) > FRAME_MAX_BYTES {
                    return Err(Error::Budget("document decoded token/fact allowance".into()));
                }
            }
        }
        Ok(())
    }

    /// Consume an unmodified JSON serialization of published
    /// `RoslynDocumentFacts` schema-v1 (PascalCase property names).
    /// The source IDs, compiler versions, frontier, raw records and UTF-16
    /// span coordinates survive exactly; clone blocks require a separate
    /// preclassified input and are intentionally not inferred here.
    pub fn from_roslyn_json(frame: &Value) -> Result<Self> {
        let obj = frame.as_object().ok_or_else(|| Error::Invalid("Roslyn facts must be an object".into()))?;
        let prop = |k: &str| -> Result<&Value> { obj.get(k).ok_or_else(|| Error::Invalid(format!("missing Roslyn {k}"))) };
        if prop("SchemaVersion")?.as_i64() != Some(1) { return Err(Error::UnsupportedFormat("Roslyn facts schema (expected 1)".into())); }
        let identity = prop("Identity")?.as_object().ok_or_else(|| Error::Invalid("Roslyn Identity missing".into()))?;
        let field = |k: &str| -> Result<String> {
            identity.get(k).and_then(Value::as_str).map(str::to_owned)
                .ok_or_else(|| Error::Invalid(format!("Roslyn Identity.{k}")))
        };
        let mut facts = Vec::new();
        for (collection, kind) in [("Declarations", "declaration"), ("References", "reference"), ("Calls", "call"), ("Frontier", "frontier")] {
            let entries = prop(collection)?.as_array().ok_or_else(|| Error::Invalid(format!("Roslyn {collection} not an array")))?;
            if facts.len() + entries.len() > 20_000 { return Err(Error::Budget("Roslyn record count".into())); }
            for (i, value) in entries.iter().enumerate() {
                let row = value.as_object().ok_or_else(|| Error::Invalid("Roslyn record must be object".into()))?;
                let span = match row.get("Span") { Some(Value::Null) | None => None, Some(s) => Some(roslyn_span(s)?), };
                facts.push(Fact { kind: kind.into(), fact_id: format!("{kind}:{i}"),
                    symbol_id: row.get("SymbolId").and_then(Value::as_str).map(str::to_owned),
                    owner_id: row.get("OwnerId").or_else(|| row.get("CallerId")).and_then(Value::as_str).map(str::to_owned),
                    span, payload: value.clone() });
            }
        }
        let provider_schema = format!("roslyn-facts/1;common={};csharp={};assembly={}",
            prop("RoslynCommonAssemblyVersion")?.as_str().ok_or_else(|| Error::Invalid("RoslynCommonAssemblyVersion".into()))?,
            prop("RoslynCSharpAssemblyVersion")?.as_str().ok_or_else(|| Error::Invalid("RoslynCSharpAssemblyVersion".into()))?,
            prop("CompilationAssembly")?.as_str().ok_or_else(|| Error::Invalid("CompilationAssembly".into()))?);
        let context_key = field("ContextKey")?;
        let document_key = field("DocumentKey")?;
        // Physical linked files can occur under several evaluated contexts.
        // Length framing is unambiguous and preserves both original components.
        let document_id = format!("{}:{}{}:{}",context_key.encode_utf16().count(),context_key,
            document_key.encode_utf16().count(),document_key);
        Ok(Self { document_id, context_key,
            source_id: document_key, physical_path: field("PhysicalPath")?, logical_path: field("LogicalPath")?,
            provider_id: "roslyn.csharp".into(), provider_schema, coordinate_unit: "utf16".into(),
            version: field("Revision")?, source_digest: field("InputFingerprint")?,
            is_generated: false, is_excluded: false, facts, blocks: Vec::new() })
    }
}
fn roslyn_span(v: &Value) -> Result<Span> {
    let v = v.as_object().ok_or_else(|| Error::Invalid("Roslyn span".into()))?;
    let int = |s: &str| -> Result<i32> { v.get(s).and_then(Value::as_i64).and_then(|n| i32::try_from(n).ok()).ok_or_else(|| Error::Invalid(format!("Roslyn span {s}"))) };
    let start = int("Start")?;
    let len = int("Length")?;
    let end_offset = start.checked_add(len).ok_or_else(|| Error::Invalid("span overflow".into()))?;
    let span = Span { start_offset: start, end_offset, start_line: Some(int("StartLine")?), start_column: Some(int("StartColumn")?), end_line: Some(int("EndLine")?), end_column: Some(int("EndColumn")?) };
    span.validate()?;
    Ok(span)
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
pub enum Mutation {
    Replace { document: Document },
    CaptureRoslyn { facts: Value },
    Delete { document_id: String },
    /// Preserves existing facts/blocks; updates all posting sort keys in one transaction.
    Rename { document_id: String, new_document_id: String,
        #[serde(default)] new_source_id: Option<String>, #[serde(default)] new_version: Option<String>,
        #[serde(default)] new_logical_path: Option<String>, #[serde(default)] new_physical_path: Option<String>,
        #[serde(default)] new_context_key: Option<String> },
    /// Leaves an unverified document marker while withdrawing all derived claims.
    Invalidate { document_id: String },
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Batch { pub expected_revision: u64, pub fence: Fence, pub operations: Vec<Mutation> }
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct QueryLimits {
    pub maximum_candidate_pairs: u32,
    pub maximum_output_pairs: u32,
    pub maximum_compared_tokens: u64,
    pub maximum_scanned_blocks: u32,
    pub maximum_gaps: u32,
    pub maximum_output_bytes: u32,
}
impl Default for QueryLimits {
    fn default() -> Self {
        Self { maximum_candidate_pairs: 20_000, maximum_output_pairs: 10_000,
            maximum_compared_tokens: 2_000_000, maximum_scanned_blocks: 200_000,
            maximum_gaps: 512, maximum_output_bytes: 2 * 1024 * 1024 }
    }
}
impl QueryLimits {
    fn validate(self) -> Result<()> {
        if self.maximum_candidate_pairs == 0 || self.maximum_candidate_pairs > 100_000
            || self.maximum_output_pairs == 0 || self.maximum_output_pairs > 10_000
            || self.maximum_compared_tokens == 0 || self.maximum_compared_tokens > 100_000_000
            || self.maximum_scanned_blocks == 0 || self.maximum_scanned_blocks > 500_000
            || self.maximum_gaps == 0 || self.maximum_gaps > 1024
            || self.maximum_output_bytes < 1024 || self.maximum_output_bytes > 2 * 1024 * 1024 {
            return Err(Error::Invalid("invalid query limits".into()));
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Location {
    pub document_id: String,
    pub document_version: String,
    pub block_id: String,
    pub span: Span,
    pub token_count: u32,
    pub context_key: String,
    pub source_id: String,
    pub provider_id: String,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
pub enum MatchClass { Exact, RenamedLocalIdentifiers }
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Pair {
    pub pair_id: String, pub match_class: MatchClass, pub normalization_rule_version: String,
    pub left: Location, pub right: Location, pub exact_matched_tokens: u32,
    pub normalized_matched_tokens: u32, pub score: f64,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
pub enum GapReason { OversizedBlock, BoilerplatePostingLimit, CandidatePairLimit, ComparedTokenLimit, OutputPairLimit, ScannedBlockLimit, GapLimit, OutputBytesLimit }
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Gap { pub reason: GapReason, pub scope_key: String, pub affected_count: u64 }
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct Statistics {
    pub document_count: u64, pub indexed_block_count: u64, pub generated_documents: u64,
    pub excluded_documents: u64, pub below_minimum_blocks: u64, pub oversized_blocks: u64,
    pub candidate_pairs_examined: u64, pub compared_tokens_charged: u64,
    pub scanned_blocks: u64,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct QueryResult {
    pub fence: Fence, pub normalization_rule_version: String, pub pairs: Vec<Pair>,
    pub statistics: Statistics, pub gaps: Vec<Gap>, pub index_settings: Settings, pub applied_limits: QueryLimits,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CommitReceipt { pub fence: Fence, pub operations: u32, pub retention_frontier: Option<String> }

fn id(v: &str, n: &str) -> Result<()> {
    if v.trim().is_empty() || v.len() > MAX_TEXT_BYTES || v.contains('\0') { return Err(Error::Invalid(format!("{n} must be nonblank, <=4096 bytes, no NUL"))); }
    Ok(())
}
fn digest(v: &str, n: &str) -> Result<()> {
    if v.len() != 64 || !v.bytes().all(|c| c.is_ascii_hexdigit()) { return Err(Error::Invalid(format!("{n} must be a 64-digit hex digest"))); }
    Ok(())
}
fn i64_bound(v: u64, n: &str) -> Result<i64> { i64::try_from(v).map_err(|_| Error::Invalid(format!("{n} overflow"))) }
fn ordinal_key(v: &str) -> Vec<u8> { v.encode_utf16().flat_map(u16::to_be_bytes).collect() }
fn ordinal_cmp(a: &str, b: &str) -> Ordering { a.encode_utf16().cmp(b.encode_utf16()) }
fn block_id(doc: &str, block: &str) -> String {
    format!("{}:{}{}:{}", doc.encode_utf16().count(), doc, block.encode_utf16().count(), block)
}
fn pair_id(ld: &str, lb: &str, rd: &str, rb: &str) -> String {
    format!("{RULE_VERSION}:{}/{}", block_id(ld, lb), block_id(rd, rb))
}
fn hex(v: &[u8]) -> String { const HEX: &[u8;16] = b"0123456789ABCDEF"; let mut s=String::with_capacity(v.len()*2); for &b in v {s.push(HEX[(b>>4) as usize] as char); s.push(HEX[(b&15) as usize] as char);} s }
fn normalize(raw: &[Token]) -> Vec<Token> {
    let mut locals = HashMap::<&str, usize>::new();
    raw.iter().map(|t| if t.kind != TokenKind::LocalIdentifier { t.clone() } else {
        let next = locals.len();
        let i = *locals.entry(t.text.as_str()).or_insert(next);
        Token { kind: t.kind, text: i.to_string() }
    }).collect()
}
fn fingerprint(tokens: &[Token]) -> [u8;32] {
    let mut sha = Sha256::new();
    for t in tokens {
        sha.update(t.kind.ordinal().to_le_bytes());
        sha.update((t.text.len() as i32).to_le_bytes());
        sha.update(t.text.as_bytes());
    }
    sha.finalize().into()
}
fn encode_tokens(tokens: &[Token]) -> Vec<u8> {
    let mut out = Vec::with_capacity(tokens.len() * 8);
    for t in tokens {
        out.push(t.kind.ordinal() as u8);
        out.extend_from_slice(&(t.text.len() as u16).to_le_bytes());
        out.extend_from_slice(t.text.as_bytes());
    }
    out
}
fn decode_tokens(bytes: &[u8], count: u32, max_token_bytes: u32) -> Result<Vec<Token>> {
    if bytes.len() > (count as usize) * (max_token_bytes as usize + 3) { return Err(Error::Corrupt("oversized token stream".into())); }
    let mut tokens = Vec::with_capacity(count as usize);
    let mut pos = 0usize;
    while pos < bytes.len() {
        if tokens.len() >= count as usize || pos + 3 > bytes.len() { return Err(Error::Corrupt("token framing".into())); }
        let kind = match bytes[pos] { 0 => TokenKind::Other, 1 => TokenKind::LocalIdentifier, 2 => TokenKind::BoundIdentifier, 3 => TokenKind::Literal, _ => return Err(Error::Corrupt("unknown token kind".into())) };
        let n = u16::from_le_bytes([bytes[pos + 1], bytes[pos + 2]]) as usize;
        pos += 3;
        if n == 0 || n > max_token_bytes as usize || n > bytes.len() - pos { return Err(Error::Corrupt("token length".into())); }
        let text = std::str::from_utf8(&bytes[pos..pos + n]).map_err(|_| Error::Corrupt("token UTF-8".into()))?.to_owned();
        pos += n;
        tokens.push(Token { kind, text });
    }
    if tokens.len() != count as usize { return Err(Error::Corrupt("token count".into())); }
    Ok(tokens)
}

/// One connection, one writer. Callers serialize access to the handle. Other
/// processes can read SQLite snapshots but cannot acquire publication rights.
pub struct FactIndex { connection: Connection, path: PathBuf, settings: Settings }

const SCHEMA: &str = r#"
CREATE TABLE format(id INTEGER PRIMARY KEY CHECK(id=1), settings TEXT NOT NULL, schema_digest TEXT NOT NULL);
CREATE TABLE workspace(id INTEGER PRIMARY KEY CHECK(id=1), root_id TEXT NOT NULL, epoch INTEGER NOT NULL,
    revision INTEGER NOT NULL, input_digest TEXT NOT NULL, tool_digest TEXT NOT NULL, policy_digest TEXT NOT NULL);
CREATE TABLE documents(document_id TEXT PRIMARY KEY, document_sort BLOB NOT NULL UNIQUE, context_key TEXT NOT NULL,
    source_id TEXT NOT NULL, physical_path TEXT NOT NULL, logical_path TEXT NOT NULL,
    provider_id TEXT NOT NULL, provider_schema TEXT NOT NULL, coordinate_unit TEXT NOT NULL,
    version TEXT NOT NULL, source_digest TEXT NOT NULL, generated INTEGER NOT NULL, excluded INTEGER NOT NULL,
    valid INTEGER NOT NULL CHECK(valid IN(0,1)));
CREATE TABLE facts(document_id TEXT NOT NULL REFERENCES documents(document_id) ON DELETE CASCADE ON UPDATE CASCADE,
    ordinal INTEGER NOT NULL, kind TEXT NOT NULL, fact_id TEXT NOT NULL, symbol_id TEXT, owner_id TEXT,
    span_start INTEGER, span_end INTEGER, span_json TEXT, payload_json TEXT NOT NULL,
    PRIMARY KEY(document_id,ordinal));
CREATE INDEX facts_by_symbol ON facts(symbol_id, document_id);
CREATE INDEX facts_by_kind ON facts(kind, document_id);
CREATE TABLE blocks(document_id TEXT NOT NULL REFERENCES documents(document_id) ON DELETE CASCADE ON UPDATE CASCADE,
    block_id TEXT NOT NULL, document_sort BLOB NOT NULL, block_sort BLOB NOT NULL, start_offset INTEGER NOT NULL,
    end_offset INTEGER NOT NULL, span_json TEXT NOT NULL, token_count INTEGER NOT NULL, raw BLOB, normalized BLOB,
    exact_fp BLOB, normalized_fp BLOB, indexed INTEGER NOT NULL CHECK(indexed IN(0,1)),
    PRIMARY KEY(document_id,block_id));
CREATE INDEX blocks_in_order ON blocks(document_sort,block_sort);
CREATE TABLE postings(pass INTEGER NOT NULL CHECK(pass IN(0,1)), fingerprint BLOB NOT NULL,
    document_id TEXT NOT NULL, block_id TEXT NOT NULL, document_sort BLOB NOT NULL, block_sort BLOB NOT NULL,
    PRIMARY KEY(pass,document_id,block_id), FOREIGN KEY(document_id,block_id)
      REFERENCES blocks(document_id,block_id) ON DELETE CASCADE ON UPDATE CASCADE);
CREATE INDEX postings_by_fingerprint ON postings(pass,fingerprint,document_sort,block_sort);
CREATE TABLE bucket_counts(pass INTEGER NOT NULL, fingerprint BLOB NOT NULL, posting_count INTEGER NOT NULL CHECK(posting_count>=1),
    PRIMARY KEY(pass,fingerprint));
CREATE TRIGGER postings_inserted AFTER INSERT ON postings BEGIN
    INSERT INTO bucket_counts(pass,fingerprint,posting_count) VALUES(NEW.pass,NEW.fingerprint,1)
    ON CONFLICT(pass,fingerprint) DO UPDATE SET posting_count=posting_count+1;
END;
CREATE TRIGGER postings_deleted AFTER DELETE ON postings BEGIN
    -- The CHECK(posting_count>=1) must never observe a transient zero.
    DELETE FROM bucket_counts WHERE pass=OLD.pass AND fingerprint=OLD.fingerprint AND posting_count=1;
    UPDATE bucket_counts SET posting_count=posting_count-1 WHERE pass=OLD.pass AND fingerprint=OLD.fingerprint AND posting_count>1;
END;
"#;

/// Signature includes actual schema SQL, not just object names, so an
/// incompatible altered index/trigger with a familiar name is not accepted.
fn schema_signature(connection: &Connection) -> Result<String> {
    let mut stmt=connection.prepare("SELECT type,name,sql FROM sqlite_master
        WHERE type IN ('table','index','trigger') AND name NOT LIKE 'sqlite_autoindex_%'
        ORDER BY name LIMIT 32")?;
    let mut rows=stmt.query([])?;
    let mut sha=Sha256::new();
    let mut bytes=0usize;
    while let Some(row)=rows.next()? {
        for col in 0..3 {
            let value: String=row.get(col)?;
            bytes=bytes.checked_add(value.len()).ok_or_else(||Error::Corrupt("schema byte overflow".into()))?;
            if bytes>128*1024 { return Err(Error::Corrupt("schema SQL exceeds layout limit".into())); }
            sha.update((value.len() as u32).to_le_bytes());
            sha.update(value.as_bytes());
        }
    }
    Ok(hex(&sha.finalize()))
}

impl FactIndex {
    /// New cache files are initialized once. An existing unknown, incompatible or
    /// damaged SQLite file is NEVER replaced, migrated or silently treated as empty.
    pub fn open(path: impl AsRef<Path>, root_id: &str, settings: Settings) -> Result<Self> {
        id(root_id, "root_id")?;
        settings.validate()?;
        let path = path.as_ref();
        let first_open = !path.exists();
        if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
            fs::create_dir_all(parent)?;
        }
        let connection = Connection::open(path)?;
        connection.busy_timeout(Duration::from_millis(250))?;
        connection.execute_batch("PRAGMA foreign_keys=ON; PRAGMA trusted_schema=OFF; PRAGMA mmap_size=0; PRAGMA temp_store=FILE; PRAGMA cache_size=-8192; PRAGMA busy_timeout=250; PRAGMA synchronous=FULL; PRAGMA journal_size_limit=8388608; PRAGMA wal_autocheckpoint=64;")?;
        if first_open {
            connection.execute_batch("PRAGMA page_size=4096; PRAGMA journal_mode=WAL;")?;
            connection.execute_batch("BEGIN IMMEDIATE;")?;
            let initialized = (|| -> Result<()> {
                connection.execute_batch(SCHEMA)?;
                let schema_digest = schema_signature(&connection)?;
                connection.execute("INSERT INTO format(id,settings,schema_digest) VALUES(1,?1,?2)",
                    params![serde_json::to_string(&settings)?,schema_digest])?;
                let zero = "0".repeat(64);
                connection.execute("INSERT INTO workspace(id,root_id,epoch,revision,input_digest,tool_digest,policy_digest) VALUES(1,?1,0,0,?2,?2,?2)", params![root_id,zero])?;
                connection.pragma_update(None, "application_id", APPLICATION_ID)?;
                connection.pragma_update(None, "user_version", SCHEMA_VERSION)?;
                Ok(())
            })();
            match initialized {
                Ok(()) => connection.execute_batch("COMMIT;")?,
                Err(e) => { let _ = connection.execute_batch("ROLLBACK;"); return Err(e); }
            }
        }
        let application_id: i64 = connection.pragma_query_value(None,"application_id",|r|r.get(0))?;
        let user_version: i64 = connection.pragma_query_value(None,"user_version",|r|r.get(0))?;
        if application_id != APPLICATION_ID || user_version != SCHEMA_VERSION {
            return Err(Error::UnsupportedFormat(format!("application_id={application_id}, user_version={user_version}; expected {APPLICATION_ID}/{SCHEMA_VERSION}")));
        }
        let mode: String = connection.pragma_query_value(None,"journal_mode",|r|r.get(0))?;
        if !mode.eq_ignore_ascii_case("wal") {return Err(Error::UnsupportedFormat("index requires local SQLite WAL".into()));}
        let integrity: String = connection.query_row("PRAGMA quick_check(1)",[],|r|r.get(0))
            .map_err(|e|Error::Corrupt(format!("SQLite quick_check failed: {e}")))?;
        if integrity != "ok" { return Err(Error::Corrupt(format!("SQLite quick_check: {}", &integrity[..integrity.len().min(128)]))); }
        let (stored,stored_schema): (String,String) = connection.query_row(
            "SELECT settings,schema_digest FROM format WHERE id=1",[],|r|Ok((r.get(0)?,r.get(1)?)))
            .map_err(|_| Error::Corrupt("missing format row or signature".into()))?;
        if stored_schema != schema_signature(&connection)? {
            return Err(Error::UnsupportedFormat("SQLite schema SQL differs from versioned cache layout".into()));
        }
        let stored_settings: Settings = serde_json::from_str(&stored)
            .map_err(|_| Error::UnsupportedFormat("invalid settings serialization".into()))?;
        if stored_settings != settings { return Err(Error::UnsupportedFormat("incompatible index settings; rebuild derived cache".into())); }
        let owner: String = connection.query_row("SELECT root_id FROM workspace WHERE id=1", [], |r|r.get(0))
            .map_err(|_| Error::Corrupt("missing workspace identity".into()))?;
        if owner != root_id { return Err(Error::Stale("physical checkout root mismatch".into())); }
        // Explicitly reject unknown/partial schema, including a crash after
        // header initialization but before the complete object set exists.
        let mut names = connection.prepare("SELECT name FROM sqlite_master WHERE type IN ('table','index','trigger') AND name NOT LIKE 'sqlite_autoindex_%' ORDER BY name LIMIT 16")?;
        let found = names.query_map([],|r|r.get::<_,String>(0))?.collect::<rusqlite::Result<Vec<_>>>()?;
        let mut expected = vec!["blocks","blocks_in_order","bucket_counts","documents","facts","facts_by_kind","facts_by_symbol",
            "format","postings","postings_by_fingerprint","postings_deleted","postings_inserted","workspace"];
        expected.sort_unstable();
        if found.iter().any(|name|name.len()>128) || found.iter().map(String::as_str).collect::<Vec<_>>() != expected {
            return Err(Error::UnsupportedFormat("unexpected SQLite object inventory".into()));
        }
        drop(names);
        let fk_problem: i64 = connection.query_row("SELECT COUNT(*) FROM pragma_foreign_key_check", [], |r|r.get(0))?;
        if fk_problem != 0 { return Err(Error::Corrupt("invalid foreign keys".into())); }
        let pages = settings.maximum_disk_bytes / PAGE_SIZE;
        connection.execute_batch(&format!("PRAGMA max_page_count={pages};"))?;
        Ok(Self { connection, path: path.to_owned(), settings })
    }

    pub fn state(&self) -> Result<Fence> { read_fence(&self.connection) }
    pub fn settings(&self) -> Settings { self.settings }

    /// A complete batch is admitted against one durable revision and applied in
    /// one SQLite transaction, including records, postings and the new fence.
    /// Any error before COMMIT rolls everything back (including a mid-batch
    /// interrupted transaction). No durable predecessor generations are retained.
    pub fn apply(&mut self, batch: Batch) -> Result<CommitReceipt> {
        batch.fence.validate()?;
        if batch.operations.is_empty() || batch.operations.len() > 128 { return Err(Error::Budget("operation count must be 1..128".into())); }
        if batch.fence.committed_revision != batch.expected_revision.checked_add(1).ok_or_else(|| Error::Invalid("revision overflow".into()))? {
            return Err(Error::Invalid("next revision must equal expected + 1".into()));
        }
        // Validate all supplied snapshots before modifying an existing partition.
        // Captured frames are parsed independently of a future fact-provider API.
        let mut operations = Vec::with_capacity(batch.operations.len());
        let mut batch_bytes = 0usize;
        for operation in batch.operations {
            let operation = match operation {
                Mutation::CaptureRoslyn { facts } => Mutation::Replace { document: Document::from_roslyn_json(&facts)? },
                other => other,
            };
            match &operation {
                Mutation::Replace { document } => document.validate(self.settings)?,
                Mutation::Delete { document_id } | Mutation::Invalidate { document_id } => id(document_id, "document_id")?,
                Mutation::Rename { document_id, new_document_id, new_source_id, new_version,
                    new_logical_path, new_physical_path, new_context_key } => {
                    id(document_id,"document_id")?; id(new_document_id,"new_document_id")?;
                    for (name,value) in [("new_source_id",new_source_id),("new_version",new_version),
                        ("new_logical_path",new_logical_path),("new_physical_path",new_physical_path),
                        ("new_context_key",new_context_key)] {
                        if let Some(v)=value { id(v,name)?; }
                    }
                }
                Mutation::CaptureRoslyn { .. } => unreachable!(),
            }
            // The Rust library must not bypass the executable's 4 MiB input
            // frontier by accepting a large sequence of individually valid
            // documents in one transaction.
            batch_bytes = batch_bytes.checked_add(serde_json::to_vec(&operation)?.len())
                .ok_or_else(|| Error::Budget("batch byte overflow".into()))?;
            if batch_bytes > FRAME_MAX_BYTES {
                return Err(Error::Budget("aggregate batch exceeds 4 MiB".into()));
            }
            operations.push(operation);
        }
        let only_withdrawals = operations.iter().all(|op| matches!(op, Mutation::Delete{..} | Mutation::Invalidate{..}));
        // Reserve room for a modest WAL burst; allow removal even under pressure.
        if !only_withdrawals && disk_bytes(&self.path)? > self.settings.maximum_disk_bytes.saturating_sub(2 * FRAME_MAX_BYTES as u64) {
            return Err(Error::Budget("disk headroom exhausted; prune or rebuild derived cache".into()));
        }
        let tx = self.connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let old = read_fence(&tx)?;
        if old.root_id != batch.fence.root_id || old.committed_revision != batch.expected_revision || batch.fence.epoch < old.epoch {
            return Err(Error::Stale("root, revision or epoch superseded".into()));
        }
        for operation in &operations {
            match operation {
                Mutation::Replace { document } => replace(&tx, document, self.settings)?,
                Mutation::Delete { document_id } => {
                    tx.execute("DELETE FROM postings WHERE document_id=?1",[document_id])?;
                    tx.execute("DELETE FROM documents WHERE document_id=?1",[document_id])?;
                }
                Mutation::Invalidate { document_id } => {
                    let changed = tx.execute("UPDATE documents SET valid=0 WHERE document_id=?1",[document_id])?;
                    if changed != 1 { return Err(Error::Invalid("invalidate unknown document".into())); }
                    tx.execute("DELETE FROM facts WHERE document_id=?1",[document_id])?;
                    tx.execute("DELETE FROM postings WHERE document_id=?1",[document_id])?;
                    tx.execute("DELETE FROM blocks WHERE document_id=?1",[document_id])?;
                }
                Mutation::Rename { document_id, new_document_id, new_source_id, new_version,
                    new_logical_path, new_physical_path, new_context_key } => {
                    let sort = ordinal_key(new_document_id);
                    let count = tx.execute("UPDATE documents SET document_id=?1,document_sort=?2,
                        source_id=COALESCE(?3,source_id),version=COALESCE(?4,version),
                        logical_path=COALESCE(?5,logical_path),physical_path=COALESCE(?6,physical_path),
                        context_key=COALESCE(?7,context_key) WHERE document_id=?8",
                        params![new_document_id,sort,new_source_id,new_version,new_logical_path,
                            new_physical_path,new_context_key,document_id])?;
                    if count != 1 { return Err(Error::Invalid("rename unknown document".into())); }
                    tx.execute("UPDATE blocks SET document_sort=?1 WHERE document_id=?2", params![sort,new_document_id])?;
                    tx.execute("UPDATE postings SET document_sort=?1 WHERE document_id=?2", params![sort,new_document_id])?;
                }
                Mutation::CaptureRoslyn { .. } => unreachable!(),
            }
        }
        tx.execute("UPDATE workspace SET epoch=?1,revision=?2,input_digest=?3,tool_digest=?4,policy_digest=?5 WHERE id=1",
            params![i64_bound(batch.fence.epoch,"epoch")?,i64_bound(batch.fence.committed_revision,"revision")?,
                batch.fence.input_digest,batch.fence.tool_digest,batch.fence.policy_digest])?;
        tx.commit()?;
        // Checkpoint is post-commit maintenance, never a condition for claiming a
        // rolled-back transaction. A busy/failed checkpoint is an explicit frontier.
        let checkpoint = self.connection.query_row("PRAGMA wal_checkpoint(TRUNCATE)",[],|r|r.get::<_,i64>(0));
        let retention_frontier = match checkpoint {
            Ok(0) => if disk_bytes(&self.path).unwrap_or(u64::MAX) > self.settings.maximum_disk_bytes {
                Some("post-commit disk quota exceeded; writer must suspend and prune".into())
            } else { None },
            Ok(_) => Some("WAL checkpoint busy: long reader holds retention; defer further writes".into()),
            Err(_) => Some("WAL checkpoint failed: owner must verify disk retention".into()),
        };
        Ok(CommitReceipt { fence: batch.fence, operations: operations.len() as u32, retention_frontier })
    }

    /// Reads a short, single-revision SQLite snapshot with explicit scan,
    /// candidate, decoded-token, output and gap frontiers.
    pub fn query(&mut self, fence: &Fence, limits: QueryLimits) -> Result<QueryResult> {
        fence.validate()?;
        limits.validate()?;
        let tx = self.connection.transaction_with_behavior(TransactionBehavior::Deferred)?;
        let observed = read_fence(&tx)?;
        if *fence != observed { return Err(Error::Stale("query fence differs from persisted revision, root, policy or inputs".into())); }
        let mut stats = statistics(&tx, self.settings)?;
        let mut gaps = Vec::new();
        {
            let mut stmt = tx.prepare("SELECT b.document_id,b.block_id,b.token_count FROM blocks b JOIN documents d ON d.document_id=b.document_id WHERE d.valid=1 AND d.generated=0 AND d.excluded=0 AND b.token_count>?1 ORDER BY b.document_sort,b.block_sort")?;
            let mut rows = stmt.query([self.settings.maximum_tokens_per_block])?;
            while let Some(row) = rows.next()? {
                let doc: String = row.get(0)?;
                let block: String = row.get(1)?;
                let count: u32 = row.get(2)?;
                if !push_gap(&mut gaps, limits, Gap { reason: GapReason::OversizedBlock, scope_key: block_id(&doc,&block), affected_count: count as u64 }) { break; }
            }
        }
        if gaps.last().is_some_and(|g|g.reason==GapReason::GapLimit) {
            gaps.sort_by(|a,b|a.reason.cmp(&b.reason).then_with(||ordinal_cmp(&a.scope_key,&b.scope_key)));
            tx.commit()?;
            return Ok(QueryResult {fence:observed,normalization_rule_version:RULE_VERSION.into(),pairs:Vec::new(),
                statistics:stats,gaps,index_settings:self.settings,applied_limits:limits});
        }
        let mut pairs = Vec::new();
        let mut seen = HashSet::<String>::new();
        let mut suppressed = HashSet::<String>::new();
        let mut output_bytes = 0usize;
        'passes: for pass in [0i64,1i64] {
            let mut cursor: Option<(Vec<u8>,Vec<u8>)> = None;
            loop {
                // Keyset rather than OFFSET pagination. A page includes metadata
                // only; token bodies are loaded only for charged candidates.
                let page = read_page(&tx, pass, cursor.as_ref())?;
                if page.is_empty() { break; }
                for left in &page {
                    if stats.scanned_blocks >= limits.maximum_scanned_blocks as u64 {
                        let key = block_id(&left.location.document_id,&left.location.block_id);
                        push_gap(&mut gaps, limits, Gap { reason: GapReason::ScannedBlockLimit,scope_key:key,affected_count:1 });
                        break 'passes;
                    }
                    stats.scanned_blocks += 1;
                    let fp = if pass == 0 { &left.exact_fp } else { &left.normalized_fp };
                    // Detect missing or stale postings even when an old bucket
                    // would otherwise produce no candidate for this left key.
                    let recorded: Option<Vec<u8>> = tx.query_row("SELECT fingerprint FROM postings WHERE pass=?1 AND document_id=?2 AND block_id=?3",
                        params![pass,left.location.document_id,left.location.block_id],|r|r.get(0)).optional()?;
                    if recorded.as_deref() != Some(fp.as_slice()) {
                        return Err(Error::Corrupt("missing or inconsistent self posting".into()));
                    }
                    let count: i64 = tx.query_row("SELECT posting_count FROM bucket_counts WHERE pass=?1 AND fingerprint=?2",
                        params![pass,fp.as_slice()],|r|r.get(0))
                        .map_err(|_|Error::Corrupt("missing posting count".into()))?;
                    if count <= 0 { return Err(Error::Corrupt("nonpositive posting count".into())); }
                    if count > self.settings.maximum_fingerprint_postings as i64 {
                        let bucket = format!("{}:{}",if pass==0 {"exact"}else{"normalized"},hex(fp));
                        if suppressed.insert(bucket.clone()) {
                            if !push_gap(&mut gaps,limits,Gap{reason:GapReason::BoilerplatePostingLimit,scope_key:bucket,affected_count:((count*(count-1))/2) as u64}) {break 'passes;}
                        }
                        continue;
                    }
                    let rights = read_bucket(&tx, pass, fp, count)?;
                    for right in rights {
                        if (left.document_sort.as_slice(),left.block_sort.as_slice()) >= (right.document_sort.as_slice(),right.block_sort.as_slice()) { continue; }
                        let key = pair_id(&left.location.document_id,&left.location.block_id,&right.location.document_id,&right.location.block_id);
                        if seen.contains(&key) { continue; }
                        if stats.candidate_pairs_examined >= limits.maximum_candidate_pairs as u64 {
                            push_gap(&mut gaps,limits,Gap { reason: GapReason::CandidatePairLimit,scope_key:key,affected_count:1 });
                            break 'passes;
                        }
                        let cost = left.location.token_count as u64 + right.location.token_count as u64;
                        if cost > limits.maximum_compared_tokens.saturating_sub(stats.compared_tokens_charged) {
                            push_gap(&mut gaps,limits,Gap { reason: GapReason::ComparedTokenLimit,scope_key:key,affected_count:cost });
                            break 'passes;
                        }
                        stats.candidate_pairs_examined += 1;
                        stats.compared_tokens_charged += cost;
                        if left.location.token_count != right.location.token_count { continue; }
                        let (left_raw,left_norm) = load_verified(&tx,left,self.settings)?;
                        let (right_raw,right_norm) = load_verified(&tx,&right,self.settings)?;
                        if left_norm != right_norm { continue; }
                        let exact = left_raw == right_raw;
                        if pass == 0 && !exact { continue; } // collision, NOT evidence
                        if pairs.len() >= limits.maximum_output_pairs as usize {
                            push_gap(&mut gaps,limits,Gap{reason:GapReason::OutputPairLimit,scope_key:key,affected_count:1});
                            break 'passes;
                        }
                        let approx_bytes = key.len().saturating_add(left.location.document_id.len()).saturating_add(right.location.document_id.len())
                            .saturating_add(left.location.block_id.len()).saturating_add(right.location.block_id.len())
                            .saturating_add(left.location.document_version.len()).saturating_add(right.location.document_version.len())
                            .saturating_add(left.location.context_key.len()).saturating_add(right.location.context_key.len())
                            .saturating_add(left.location.source_id.len()).saturating_add(right.location.source_id.len())
                            .saturating_add(left.location.provider_id.len()).saturating_add(right.location.provider_id.len()).saturating_add(1024);
                        if output_bytes.saturating_add(approx_bytes) > limits.maximum_output_bytes as usize {
                            push_gap(&mut gaps,limits,Gap { reason:GapReason::OutputBytesLimit,scope_key:key,affected_count:1 });
                            break 'passes;
                        }
                        output_bytes += approx_bytes;
                        seen.insert(key.clone());
                        let exact_count = if exact { left.location.token_count } else {
                            left_raw.iter().zip(&right_raw).filter(|(a,b)|a==b).count() as u32
                        };
                        pairs.push(Pair { pair_id:key, match_class:if exact{MatchClass::Exact}else{MatchClass::RenamedLocalIdentifiers},
                            normalization_rule_version:RULE_VERSION.into(), left:left.location.clone(),right:right.location,
                            exact_matched_tokens:exact_count, normalized_matched_tokens:left.location.token_count,score:1.0 });
                    }
                }
                let last = page.last().expect("nonempty page");
                cursor = Some((last.document_sort.clone(), last.block_sort.clone()));
            }
        }
        pairs.sort_by(|a,b| a.match_class.cmp(&b.match_class)
            .then_with(||b.normalized_matched_tokens.cmp(&a.normalized_matched_tokens))
            .then_with(||ordinal_cmp(&a.pair_id,&b.pair_id)));
        gaps.sort_by(|a,b|a.reason.cmp(&b.reason).then_with(||ordinal_cmp(&a.scope_key,&b.scope_key)));
        tx.commit()?;
        Ok(QueryResult { fence:observed, normalization_rule_version:RULE_VERSION.into(),pairs,
            statistics:stats,gaps,index_settings:self.settings,applied_limits:limits })
    }

    /// Bounded fact browsing, ordered by document then provider record ordinal.
    /// A stale caller cannot mistake a new generation for its requested one.
    pub fn facts_page(&mut self, fence: &Fence, document_id: &str, from_ordinal: u32, max_records: u32) -> Result<Vec<Fact>> {
        fence.validate()?; id(document_id,"document_id")?;
        if max_records == 0 || max_records > 128 { return Err(Error::Budget("fact page must be 1..128".into())); }
        let tx = self.connection.transaction_with_behavior(TransactionBehavior::Deferred)?;
        if read_fence(&tx)? != *fence { return Err(Error::Stale("fact cursor expired".into())); }
        let mut stmt = tx.prepare("SELECT kind,fact_id,symbol_id,owner_id,span_json,payload_json FROM facts WHERE document_id=?1 AND ordinal>=?2 ORDER BY ordinal LIMIT ?3")?;
        let mut rows = stmt.query(params![document_id,from_ordinal,max_records])?;
        let mut result = Vec::new();
        let mut bytes = 0usize;
        while let Some(r) = rows.next()? {
            let payload_json: String = r.get(5)?;
            let span_json: Option<String> = r.get(4)?;
            bytes = bytes.saturating_add(payload_json.len()).saturating_add(span_json.as_ref().map_or(0,String::len));
            if bytes > 1024*1024 { return Err(Error::Budget("fact page decoded bytes".into())); }
            result.push(Fact { kind:r.get(0)?,fact_id:r.get(1)?,symbol_id:r.get(2)?,owner_id:r.get(3)?,
                span:span_json.map(|s|serde_json::from_str(&s)).transpose()?,payload:serde_json::from_str(&payload_json)? });
        }
        drop(rows); drop(stmt);
        tx.commit()?;
        Ok(result)
    }
}

fn disk_bytes(path: &Path) -> Result<u64> {
    let mut n = 0u64;
    for suffix in ["", "-wal", "-shm"] {
        let mut name = path.as_os_str().to_os_string();
        name.push(suffix); // preserve non-UTF-8 filesystem identities
        match fs::metadata(PathBuf::from(name)) {
            Ok(meta) => { n = n.checked_add(meta.len())
                .ok_or_else(|| Error::Budget("disk accounting overflow".into()))?; }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {},
            Err(e) => return Err(Error::Io(e)), // never interpret unknown bytes as zero
        }
    }
    Ok(n)
}
fn read_fence(conn: &Connection) -> Result<Fence> {
    let row = conn.query_row("SELECT root_id,epoch,revision,input_digest,tool_digest,policy_digest FROM workspace WHERE id=1",[],|r| {
        Ok((r.get::<_,String>(0)?,r.get::<_,i64>(1)?,r.get::<_,i64>(2)?,r.get::<_,String>(3)?,r.get::<_,String>(4)?,r.get::<_,String>(5)?))
    }).map_err(|_|Error::Corrupt("missing or unreadable workspace fence".into()))?;
    let fence = Fence {root_id:row.0,epoch:u64::try_from(row.1).map_err(|_|Error::Corrupt("epoch underflow".into()))?,
        committed_revision:u64::try_from(row.2).map_err(|_|Error::Corrupt("revision underflow".into()))?,
        input_digest:row.3,tool_digest:row.4,policy_digest:row.5};
    fence.validate().map_err(|_|Error::Corrupt("invalid stored revision identity".into()))?;
    Ok(fence)
}
fn replace(tx: &Transaction<'_>, doc: &Document, settings: Settings) -> Result<()> {
    tx.execute("DELETE FROM postings WHERE document_id=?1",[&doc.document_id])?;
    tx.execute("DELETE FROM documents WHERE document_id=?1",[&doc.document_id])?;
    let doc_sort = ordinal_key(&doc.document_id);
    tx.execute("INSERT INTO documents(document_id,document_sort,context_key,source_id,physical_path,logical_path,provider_id,provider_schema,coordinate_unit,version,source_digest,generated,excluded,valid) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,1)",
        params![doc.document_id,doc_sort,doc.context_key,doc.source_id,doc.physical_path,doc.logical_path,
            doc.provider_id,doc.provider_schema,doc.coordinate_unit,doc.version,doc.source_digest,doc.is_generated as i32,doc.is_excluded as i32])?;
    let mut statement = tx.prepare("INSERT INTO facts(document_id,ordinal,kind,fact_id,symbol_id,owner_id,span_start,span_end,span_json,payload_json) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10)")?;
    for (ordinal,fact) in doc.facts.iter().enumerate() {
        statement.execute(params![doc.document_id,ordinal as i64,fact.kind,fact.fact_id,fact.symbol_id,fact.owner_id,
            fact.span.map(|s|s.start_offset),fact.span.map(|s|s.end_offset),
            fact.span.map(|s|serde_json::to_string(&s)).transpose()?,serde_json::to_string(&fact.payload)?])?;
    }
    drop(statement);
    let mut insert_block = tx.prepare("INSERT INTO blocks(document_id,block_id,document_sort,block_sort,start_offset,end_offset,span_json,token_count,raw,normalized,exact_fp,normalized_fp,indexed) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13)")?;
    let mut posting = tx.prepare("INSERT INTO postings(pass,fingerprint,document_id,block_id,document_sort,block_sort) VALUES(?1,?2,?3,?4,?5,?6)")?;
    for block in &doc.blocks {
        let count = block.tokens.len();
        let is_indexed = !doc.is_generated && !doc.is_excluded && count >= settings.minimum_tokens as usize
            && count <= settings.maximum_tokens_per_block as usize;
        let (raw,norm,exact_fp,normal_fp) = if is_indexed {
            let normalized = normalize(&block.tokens);
            (Some(encode_tokens(&block.tokens)),Some(encode_tokens(&normalized)),
                Some(fingerprint(&block.tokens).to_vec()),Some(fingerprint(&normalized).to_vec()))
        } else { (None,None,None,None) };
        let block_sort = ordinal_key(&block.block_id);
        insert_block.execute(params![doc.document_id,block.block_id,doc_sort,block_sort,block.span.start_offset,block.span.end_offset,
            serde_json::to_string(&block.span)?,count as i64,raw,norm,exact_fp,normal_fp,is_indexed as i32])?;
        if let (Some(e),Some(n))=(exact_fp.as_ref(),normal_fp.as_ref()) {
            posting.execute(params![0,e,doc.document_id,block.block_id,doc_sort,block_sort])?;
            posting.execute(params![1,n,doc.document_id,block.block_id,doc_sort,block_sort])?;
        }
    }
    Ok(())
}

fn statistics(tx: &Transaction<'_>, settings: Settings) -> Result<Statistics> {
    let (docs,generated,excluded) = tx.query_row("SELECT COUNT(*), COALESCE(SUM(CASE WHEN excluded=0 AND generated=1 THEN 1 ELSE 0 END),0), COALESCE(SUM(excluded),0) FROM documents",[],|r|Ok((r.get::<_,u64>(0)?,r.get::<_,u64>(1)?,r.get::<_,u64>(2)?)))?;
    let (indexed,below,oversized) = tx.query_row("SELECT COALESCE(SUM(indexed),0),COALESCE(SUM(CASE WHEN d.generated=0 AND d.excluded=0 AND b.token_count<?1 THEN 1 ELSE 0 END),0),COALESCE(SUM(CASE WHEN d.generated=0 AND d.excluded=0 AND b.token_count>?2 THEN 1 ELSE 0 END),0) FROM blocks b JOIN documents d ON d.document_id=b.document_id",
        params![settings.minimum_tokens,settings.maximum_tokens_per_block],|r|Ok((r.get::<_,u64>(0)?,r.get::<_,u64>(1)?,r.get::<_,u64>(2)?)))?;
    Ok(Statistics { document_count:docs,indexed_block_count:indexed,generated_documents:generated,
        excluded_documents:excluded,below_minimum_blocks:below,oversized_blocks:oversized,..Statistics::default() })
}
fn push_gap(gaps: &mut Vec<Gap>, limits: QueryLimits, next: Gap) -> bool {
    if gaps.len() >= limits.maximum_gaps as usize {
        if let Some(last) = gaps.last_mut() { *last = Gap {reason:GapReason::GapLimit,scope_key:next.scope_key,affected_count:1}; }
        return false;
    }
    gaps.push(next);
    true
}

#[derive(Clone)]
struct Header {
    location: Location,
    document_sort: Vec<u8>, block_sort: Vec<u8>,
    exact_fp: [u8;32], normalized_fp: [u8;32],
}
fn header(row: &Row<'_>) -> rusqlite::Result<Header> {
    let exact: Vec<u8> = row.get(7)?;
    let normalized: Vec<u8> = row.get(8)?;
    let to_digest = |v: Vec<u8>| -> rusqlite::Result<[u8;32]> {
        v.try_into().map_err(|_|rusqlite::Error::InvalidColumnType(7,"fingerprint".into(),rusqlite::types::Type::Blob))
    };
    let span: Span = serde_json::from_str(&row.get::<_,String>(13)?)
        .map_err(|e|rusqlite::Error::FromSqlConversionFailure(13,rusqlite::types::Type::Text,Box::new(e)))?;
    let start: i32 = row.get(4)?;
    let end: i32 = row.get(5)?;
    if start < 0 || end <= start || span.start_offset != start || span.end_offset != end {
        return Err(rusqlite::Error::InvalidColumnType(13,"span_json".into(),rusqlite::types::Type::Text));
    }
    Ok(Header { location:Location {
        document_id:row.get(0)?,block_id:row.get(1)?,span,
        token_count:row.get(6)?,
        document_version:row.get(9)?,context_key:row.get(10)?,source_id:row.get(11)?,provider_id:row.get(12)?,
    }, document_sort:row.get(2)?,block_sort:row.get(3)?,
    exact_fp:to_digest(exact)?,normalized_fp:to_digest(normalized)? })
}
const HEADER_COLS: &str = "b.document_id,b.block_id,b.document_sort,b.block_sort,b.start_offset,b.end_offset,b.token_count,b.exact_fp,b.normalized_fp,d.version,d.context_key,d.source_id,d.provider_id,b.span_json";
fn read_page(tx: &Transaction<'_>, pass: i64, cursor: Option<&(Vec<u8>,Vec<u8>)>) -> Result<Vec<Header>> {
    let sql = format!("SELECT {HEADER_COLS} FROM blocks b JOIN documents d ON d.document_id=b.document_id WHERE b.indexed=1 AND (?1=1 OR (b.document_sort,b.block_sort)>(?2,?3)) ORDER BY b.document_sort,b.block_sort LIMIT ?4");
    let mut statement = tx.prepare(&sql)?;
    let none = Vec::new();
    let (start,d,b) = if let Some((d,b))=cursor {(0i32,d,b)} else {(1i32,&none,&none)};
    let rows = statement.query_map(params![start,d,b,POSTING_PAGE], header)?;
    let page: Vec<Header> = rows.collect::<rusqlite::Result<Vec<_>>>()?;
    if page.iter().any(|h|h.document_sort != ordinal_key(&h.location.document_id)
        || h.block_sort != ordinal_key(&h.location.block_id) || h.location.token_count>4096) {
        return Err(Error::Corrupt("block key, ordinal sort or count inconsistent".into()));
    }
    let _ = pass; // both passes deliberately scan the same canonical ordered blocks
    let bytes: usize = page.iter().map(|h| h.location.document_id.len() + h.location.block_id.len()
        + h.location.context_key.len() + h.location.source_id.len() + h.location.provider_id.len()
        + h.location.document_version.len() + 256).sum();
    if bytes > 1024*1024 {return Err(Error::Budget("candidate page decoded bytes".into()));}
    Ok(page)
}
fn read_bucket(tx: &Transaction<'_>, pass: i64, fingerprint: &[u8;32], expected_count: i64) -> Result<Vec<Header>> {
    let sql = format!("SELECT {HEADER_COLS} FROM postings p JOIN blocks b
        ON b.document_id=p.document_id AND b.block_id=p.block_id
        AND b.document_sort=p.document_sort AND b.block_sort=p.block_sort
        JOIN documents d ON d.document_id=b.document_id
        WHERE p.pass=?1 AND p.fingerprint=?2 ORDER BY p.document_sort,p.block_sort LIMIT 65");
    let mut stmt = tx.prepare(&sql)?;
    let items: Vec<Header> = stmt.query_map(params![pass,fingerprint.as_slice()],header)?.collect::<rusqlite::Result<Vec<_>>>()?;
    if items.len() as i64 != expected_count || items.iter().any(|h|
        h.document_sort != ordinal_key(&h.location.document_id)
        || h.block_sort != ordinal_key(&h.location.block_id)
        || (if pass==0 {&h.exact_fp!=fingerprint}else{&h.normalized_fp!=fingerprint})) {
        return Err(Error::Corrupt("posting count, keys or fingerprint disagree with block".into()));
    }
    let bytes: usize = items.iter().map(|h| h.location.document_id.len() + h.location.block_id.len()
        + h.location.context_key.len() + h.location.source_id.len() + h.location.provider_id.len()
        + h.location.document_version.len() + 256).sum();
    if bytes > 1024*1024 {return Err(Error::Budget("posting bucket decoded bytes".into()));}
    Ok(items)
}
fn load_verified(tx: &Transaction<'_>, h: &Header, settings:Settings) -> Result<(Vec<Token>,Vec<Token>)> {
    let (raw,norm): (Vec<u8>,Vec<u8>) = tx.query_row("SELECT raw,normalized FROM blocks WHERE document_id=?1 AND block_id=?2 AND indexed=1",
        params![h.location.document_id,h.location.block_id],|r|Ok((r.get(0)?,r.get(1)?)))
        .map_err(|_|Error::Corrupt("indexed token body missing".into()))?;
    let a = decode_tokens(&raw,h.location.token_count,settings.maximum_token_bytes)?;
    let b = decode_tokens(&norm,h.location.token_count,settings.maximum_token_bytes)?;
    if normalize(&a)!=b || fingerprint(&a)!=h.exact_fp || fingerprint(&b)!=h.normalized_fp {
        return Err(Error::Corrupt("token body/fingerprint inconsistency".into()));
    }
    Ok((a,b))
}
