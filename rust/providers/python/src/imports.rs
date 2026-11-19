//! Opt-in, bounded direct-import syntax for an admitted-inventory producer.
//!
//! These are structural references, not resolved runtime imports. No package
//! discovery, filesystem access, interpreter invocation, or execution occurs.
//! The existing [`crate::analyze`] contract is independent and unchanged.

use sha2::{Digest, Sha256};

use crate::{
    ByteSpan, Encoding, FileKind, HARD_MAX_SOURCE_BYTES, Outcome, PARSER_CRATE,
    PARSER_VERSION, SourceIdentity, SourceRequest, ruff_adapter,
};

pub const PROVIDER_ID: &str = "slophawk.python.ruff.direct-import-syntax";
pub const SCHEMA_VERSION: &str = "python-direct-import-syntax/v1";
pub const HARD_MAX_TOKENS: usize = 131_072;
pub const HARD_MAX_FACTS: usize = 32_768;
pub const HARD_MAX_NAME_BYTES: usize = 256;
pub const HARD_MAX_PATH_BYTES: usize = 1_024;
pub const HARD_MAX_DOCUMENT_ID_BYTES: usize = 160;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AbsoluteImport {
    /// The explicit, ASCII dotted module name, at most 256 bytes. An `as`
    /// binding is irrelevant.
    /// This is not a claim that the named module exists or executes.
    pub name: String,
    /// The complete import statement in the original byte slice. Aliases in
    /// one statement share its span, including multiline continuation bytes.
    pub span: ByteSpan,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnsupportedKind {
    FromImport,
    RelativeImport,
    UnresolvedGuard,
    DynamicOrUnsupportedStatement,
    UnsupportedName,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UnsupportedStatement {
    pub kind: UnsupportedKind,
    /// Enclosing top-level statement; nested bodies are not traversed.
    pub span: ByteSpan,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImportFacts {
    pub identity: SourceIdentity,
    /// Source order, then alias order; duplicates are retained as witnesses.
    pub imports: Vec<AbsoluteImport>,
    /// A nonempty frontier prevents a claim that this module is complete.
    /// Multiple unsupported names in one import may share a statement span.
    pub unsupported: Vec<UnsupportedStatement>,
}

impl ImportFacts {
    /// Complete only for this schema's structural slice, never for general
    /// Python runtime import behavior or dependency discovery.
    pub fn is_complete(&self) -> bool {
        self.unsupported.is_empty()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Failure {
    pub outcome: Outcome,
    pub reason: &'static str,
}

/// Extract top-level absolute `import a.b [as alias]` references only.
///
/// `from` imports, explicit relative imports, guards, definitions, assignments
/// and other executable statements retain an unknown frontier. Only comments,
/// whitespace, `pass` and literal-string expression statements are inert.
/// Imports under any compound statement are not promoted to unconditional
/// references. Names over 256 bytes, containing `__init__`, or having a
/// non-ASCII source spelling are outside this slice, even if Ruff normalizes
/// that spelling to ASCII.
///
/// All returned spans are nonempty original UTF-8 byte ranges. Failure returns
/// no partial records. Input limits precede encoding work; parser token and
/// fact limits precede output allocation. Identity hashing happens only after
/// successful validation. The byte cap does not supervise parser time or RSS.
pub fn extract(request: SourceRequest<'_>) -> Result<ImportFacts, Failure> {
    if request.path.len() > HARD_MAX_PATH_BYTES
        || request.document_id.len() > HARD_MAX_DOCUMENT_ID_BYTES
        || request.bytes.len() > request.limits.max_source_bytes.min(HARD_MAX_SOURCE_BYTES)
        || request.limits.max_source_bytes == 0
        || request.limits.max_tokens == 0
        || request.limits.max_facts == 0
    {
        return Err(Failure {
            outcome: Outcome::ResourceLimit,
            reason: "direct-import input identity or source quota exceeded",
        });
    }
    if !request.path.ends_with(".py") {
        return Err(Failure {
            outcome: Outcome::UnsupportedExtension,
            reason: "direct-import syntax admits .py files only, not stubs",
        });
    }
    if request.target.major != 3 || request.target.minor != 12 {
        return Err(Failure {
            outcome: Outcome::UnsupportedGrammar,
            reason: "direct-import syntax is reviewed only for Python 3.12",
        });
    }
    let (encoding, prefix) = ruff_adapter::detect_source_encoding(request.bytes)
        .map_err(|encoding| match encoding {
            Encoding::InvalidUtf8 => Failure {
                outcome: Outcome::InvalidEncoding,
                reason: "direct-import source bytes are not valid UTF-8",
            },
            _ => Failure {
                outcome: Outcome::UnsupportedEncoding,
                reason: "direct-import source encoding has no exact UTF-8 mapping",
            },
        })?;
    let source = std::str::from_utf8(&request.bytes[prefix..]).map_err(|_| Failure {
        outcome: Outcome::InvalidEncoding,
        reason: "direct-import source bytes are not valid UTF-8",
    })?;
    let (imports, unsupported) =
        ruff_adapter::extract_imports(source, prefix as u32, request.limits)?;
    let identity = SourceIdentity {
        provider: PROVIDER_ID,
        parser_crate: PARSER_CRATE,
        parser_version: PARSER_VERSION,
        schema: SCHEMA_VERSION,
        document_id: request.document_id.to_owned(),
        path: request.path.to_owned(),
        source_sha256: format!("{:x}", Sha256::digest(request.bytes)),
        original_bytes: request.bytes.len(),
        kind: FileKind::Python,
        target: request.target,
        encoding,
        span_encoding: "utf8-original-byte-offsets-half-open",
    };
    Ok(ImportFacts {
        identity,
        imports,
        unsupported,
    })
}
