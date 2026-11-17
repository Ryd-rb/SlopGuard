//! Bounded, read-only Python syntax facts for the SlopHawk Rust checker pilot.
//!
//! This is a **local typed contract**, not a shared daemon protocol. Ruff's
//! internal AST and tokens never escape [`analyze`]. All exported ranges are
//! half-open UTF-8 **byte** offsets into the original supplied byte slice.
//! No imports, annotations, expressions, or tests are executed.

mod ruff_adapter;

use sha2::{Digest, Sha256};

/// Stable pilot identity; changing fact semantics requires a new schema ID.
pub const PROVIDER_ID: &str = "slophawk.python.ruff.syntax";
pub const SCHEMA_VERSION: &str = "python-syntax-facts/v1";
pub const PARSER_CRATE: &str = "ruff_python_parser";
pub const PARSER_VERSION: &str = "0.0.22";

/// Hard ceiling even if the caller requests a larger limit. A supervised
/// worker is still required for a production memory/time boundary.
pub const HARD_MAX_SOURCE_BYTES: usize = 4 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PythonTarget {
    pub major: u8,
    pub minor: u8,
}

impl Default for PythonTarget {
    fn default() -> Self {
        Self {
            major: 3,
            minor: 12,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limits {
    pub max_source_bytes: usize,
    pub max_tokens: usize,
    pub max_facts: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            max_source_bytes: 512 * 1024,
            max_tokens: 65_536,
            max_facts: 131_072,
        }
    }
}

/// The caller owns bytes and logical identity. No filesystem access occurs.
#[derive(Debug, Clone, Copy)]
pub struct SourceRequest<'a> {
    pub document_id: &'a str,
    pub path: &'a str,
    pub bytes: &'a [u8],
    pub target: PythonTarget,
    pub limits: Limits,
}

impl<'a> SourceRequest<'a> {
    pub fn new(document_id: &'a str, path: &'a str, bytes: &'a [u8]) -> Self {
        Self {
            document_id,
            path,
            bytes,
            target: PythonTarget::default(),
            limits: Limits::default(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileKind {
    Python,
    Stub,
    Unrecognized,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Encoding {
    Utf8,
    Utf8Bom,
    /// Declared source encoding that cannot preserve exact original ranges.
    Unsupported(String),
    /// Byte sequence is not UTF-8 and no supported encoding was declared.
    InvalidUtf8,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ByteSpan {
    /// Inclusive byte offset in the original byte slice.
    pub start: u32,
    /// Exclusive byte offset in the original byte slice.
    pub end: u32,
}

impl ByteSpan {
    pub fn slice<'a>(&self, original: &'a [u8]) -> Option<&'a [u8]> {
        original.get(self.start as usize..self.end as usize)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceIdentity {
    pub provider: &'static str,
    pub parser_crate: &'static str,
    pub parser_version: &'static str,
    pub schema: &'static str,
    pub document_id: String,
    pub path: String,
    /// Lower-case SHA-256 of **original bytes**, including any UTF-8 BOM.
    pub source_sha256: String,
    pub original_bytes: usize,
    pub kind: FileKind,
    pub target: PythonTarget,
    pub encoding: Encoding,
    /// All emitted spans use half-open, zero-based original-source UTF-8 bytes.
    pub span_encoding: &'static str,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    Complete,
    Partial,
    NoRelevantFacts,
    UnsupportedExtension,
    UnsupportedGrammar,
    UnsupportedEncoding,
    InvalidEncoding,
    InvalidSyntax,
    ResourceLimit,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Coverage {
    Complete,
    Partial,
    Unavailable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Capabilities {
    /// Raw syntax tokens for exact and near clone comparison.
    pub clone_tokens: Coverage,
    /// Source identifiers, excluding literal and comment text.
    pub spelling_identifiers: Coverage,
    pub comments: Coverage,
    pub declaration_ranges: Coverage,
    /// Only same-lexical-scope, proven local bindings may be normalized.
    pub local_renaming: Coverage,
}

impl Default for Capabilities {
    fn default() -> Self {
        Self {
            clone_tokens: Coverage::Unavailable,
            spelling_identifiers: Coverage::Unavailable,
            comments: Coverage::Unavailable,
            declaration_ranges: Coverage::Unavailable,
            local_renaming: Coverage::Unavailable,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RegionKind {
    Module,
    Function,
    Class,
    Lambda,
    Comprehension,
    If,
    For,
    While,
    With,
    Try,
    Match,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Region {
    pub id: usize,
    pub parent: Option<usize>,
    pub kind: RegionKind,
    pub span: ByteSpan,
    /// For a function/class this is its declared name, not a resolved symbol.
    pub name: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TokenCategory {
    Identifier,
    Keyword,
    Number,
    String,
    OperatorOrPunctuation,
    Other,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IdentifierRole {
    /// AST declaration / assignment / parameter / pattern capture.
    Binding,
    /// AST `Load` with local binding confirmed in the same function/scope.
    LocalReference,
    /// Import is syntactically known, but its target is not resolved.
    Import,
    /// `global` or `nonlocal`, never normalized as a local.
    ExternalScope,
    /// An attribute is not a local binding.
    Attribute,
    /// Lexical name without same-scope resolution evidence.
    Unresolved,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LocalBinding {
    /// Stable within one analysis; not a cross-revision symbol identity.
    pub scope_id: usize,
    /// Zero-based index of first binding occurrence in that scope.
    pub ordinal: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TokenFact {
    pub span: ByteSpan,
    pub region_id: usize,
    pub category: TokenCategory,
    /// Exact source spelling. This can contain string literal contents;
    /// never feed it to the spelling rule.
    pub exact: String,
    /// Same as `exact` except for AST-proven local names, which use `$L<n>`.
    pub renamed_local: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IdentifierFact {
    pub name: String,
    pub span: ByteSpan,
    pub region_id: usize,
    pub role: IdentifierRole,
    pub local: Option<LocalBinding>,
    /// Candidate only; dictionary filtering is the shared checker's job.
    pub spelling_candidate: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommentFact {
    pub span: ByteSpan,
    pub region_id: usize,
    pub text: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GapKind {
    UnresolvedImport,
    DynamicDispatch,
    FreeOrClosureName,
    GlobalOrNonlocal,
    ConservativeScope,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Gap {
    pub kind: GapKind,
    pub span: ByteSpan,
    /// Reason applies only at this span, not to an entire project graph.
    pub detail: &'static str,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceFacts {
    pub identity: SourceIdentity,
    pub outcome: Outcome,
    pub coverage: Capabilities,
    pub reason: Option<String>,
    pub regions: Vec<Region>,
    pub tokens: Vec<TokenFact>,
    pub identifiers: Vec<IdentifierFact>,
    pub comments: Vec<CommentFact>,
    pub gaps: Vec<Gap>,
}

impl SourceFacts {
    fn empty(identity: SourceIdentity, outcome: Outcome, reason: Option<String>) -> Self {
        Self {
            identity,
            outcome,
            coverage: Capabilities::default(),
            reason,
            regions: Vec::new(),
            tokens: Vec::new(),
            identifiers: Vec::new(),
            comments: Vec::new(),
            gaps: Vec::new(),
        }
    }

    /// Return a region's substantive token stream, suitable for downstream
    /// fingerprinting, collision checking, and near-match alignment.
    pub fn clone_stream(&self, region_id: usize, renamed_locals: bool) -> Vec<&str> {
        let Some(region) = self.regions.get(region_id) else {
            return Vec::new();
        };
        self.tokens
            .iter()
            .filter(|token| {
                token.span.start >= region.span.start && token.span.end <= region.span.end
            })
            .map(|token| {
                if renamed_locals {
                    token.renamed_local.as_str()
                } else {
                    token.exact.as_str()
                }
            })
            .collect()
    }
}

/// Entry point; does no repository I/O, dependency resolution, or execution.
pub fn analyze(request: SourceRequest<'_>) -> SourceFacts {
    let kind = if request.path.ends_with(".pyi") {
        FileKind::Stub
    } else if request.path.ends_with(".py") {
        FileKind::Python
    } else {
        FileKind::Unrecognized
    };

    let encoding_result = ruff_adapter::detect_source_encoding(request.bytes);
    let encoding = match &encoding_result {
        Ok((encoding, _)) => encoding.clone(),
        Err(encoding) => encoding.clone(),
    };
    let identity = SourceIdentity {
        provider: PROVIDER_ID,
        parser_crate: PARSER_CRATE,
        parser_version: PARSER_VERSION,
        schema: SCHEMA_VERSION,
        document_id: request.document_id.to_owned(),
        path: request.path.to_owned(),
        source_sha256: format!("{:x}", Sha256::digest(request.bytes)),
        original_bytes: request.bytes.len(),
        kind,
        target: request.target,
        encoding,
        span_encoding: "utf8-original-byte-offsets-half-open",
    };

    if kind == FileKind::Unrecognized {
        return SourceFacts::empty(
            identity,
            Outcome::UnsupportedExtension,
            Some("only .py and .pyi are supported".into()),
        );
    }
    if request.bytes.len() > request.limits.max_source_bytes.min(HARD_MAX_SOURCE_BYTES)
        || request.bytes.len() > u32::MAX as usize
        || request.limits.max_source_bytes == 0
        || request.limits.max_tokens == 0
        || request.limits.max_facts == 0
    {
        return SourceFacts::empty(
            identity,
            Outcome::ResourceLimit,
            Some("bounded source, tokens, or fact quota exceeded".into()),
        );
    }
    if request.target.major != 3 || !(7..=15).contains(&request.target.minor) {
        return SourceFacts::empty(
            identity,
            Outcome::UnsupportedGrammar,
            Some("target Python version is outside parser's reviewed range 3.7–3.15".into()),
        );
    }
    let prefix = match encoding_result {
        Ok((_, prefix)) => prefix,
        Err(Encoding::InvalidUtf8) => {
            return SourceFacts::empty(
                identity,
                Outcome::InvalidEncoding,
                Some("source bytes are not valid UTF-8".into()),
            );
        }
        Err(Encoding::Unsupported(name)) => {
            return SourceFacts::empty(
                identity,
                Outcome::UnsupportedEncoding,
                Some(format!(
                    "source encoding {name:?} has no exact UTF-8 mapping"
                )),
            );
        }
        Err(_) => unreachable!("encoding detection errors are explicit"),
    };
    let source = match std::str::from_utf8(&request.bytes[prefix..]) {
        Ok(source) => source,
        Err(_) => {
            return SourceFacts::empty(
                identity,
                Outcome::InvalidEncoding,
                Some("invalid UTF-8 source bytes".into()),
            );
        }
    };
    ruff_adapter::extract(identity, source, prefix as u32, request.limits)
}
