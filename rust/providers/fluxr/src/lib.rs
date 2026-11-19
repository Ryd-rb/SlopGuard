//! Read-only checked-source facts from the pinned FluxR 1 compiler.
//!
//! This is a local, typed pilot boundary, not a daemon protocol or an executor.
//! It deliberately does not infer checked declaration spans, identifier roles,
//! bindings, or native Rust semantics that the public upstream API lacks.
//! All returned byte ranges refer to the *original* UTF-8 request bytes.

use fluxr::structured::{SourceDocument, Type};
use sha2::{Digest, Sha256};

pub const PROVIDER_ID: &str = "fluxr.checked-source";
pub const SCHEMA_VERSION: u32 = 1;
pub const PARSER_COMMIT: &str = "e089fa301668c0d28821f72b08f6e4086a06ef27";
pub const PARSER_TREE: &str = "20f89b64aa479efd19e9e60d367eaa9a83e804d3";
pub const DEFAULT_MAX_SOURCE_BYTES: usize = 256 * 1024;
pub const HARD_MAX_SOURCE_BYTES: usize = 8 * 1024 * 1024;
pub const DEFAULT_MAX_FACTS: usize = 32_768;
pub const HARD_MAX_FACTS: usize = 65_536;

/// Input format is never inferred from the content or filename.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SourceFormat {
    CanonicalChecked,
    Compact,
    NativeHost,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Limits {
    pub max_source_bytes: usize,
    /// Combined maximum of checked tokens, checked AST declarations and native items.
    pub max_facts: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            max_source_bytes: DEFAULT_MAX_SOURCE_BYTES,
            max_facts: DEFAULT_MAX_FACTS,
        }
    }
}

/// Identifiers and module membership are declared by the caller, not resolved
/// from Cargo, a workspace graph, or filesystem paths by this pilot.
pub struct Request<'a> {
    pub logical_document_id: &'a str,
    pub declared_module_context: &'a str,
    pub source_bytes: &'a [u8],
    pub format: SourceFormat,
    pub limits: Limits,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SpanEncoding {
    /// Half-open `[start, end)` byte offsets from the beginning of source_bytes.
    OriginalUtf8BytesHalfOpen,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ByteSpan {
    pub start: usize,
    pub end: usize,
}

impl ByteSpan {
    pub fn text<'a>(&self, original: &'a str) -> Option<&'a str> {
        original.get(self.start..self.end)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Provenance {
    pub provider_id: &'static str,
    pub schema_version: u32,
    pub parser_commit: &'static str,
    pub parser_tree: &'static str,
    pub logical_document_id: String,
    pub declared_module_context: String,
    pub source_sha256: String,
    pub source_byte_len: usize,
    pub span_encoding: SpanEncoding,
    /// Hash of the upstream checked projection, NOT a mapped source buffer.
    pub checked_projection_sha256: String,
    /// Upstream domain-separated digest of opaque native item bytes.
    pub native_binding_digest: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TokenShape {
    /// ASCII identifier-shaped lexeme; its grammar role is *unknown*.
    IdentifierLike,
    Other,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CheckedToken {
    pub span: ByteSpan,
    pub text: String,
    pub shape: TokenShape,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DeclarationKind {
    Input,
    Record,
    RecordField,
    Enum,
    EnumVariant,
    Function,
    Method,
    Parameter,
    Local,
}

/// A compiler-accepted checked AST name. The public AST has no original range;
/// `span` MUST remain None until an upstream span-bearing export is adopted.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CheckedDeclaration {
    pub kind: DeclarationKind,
    pub name: String,
    /// Owning record, enum or function name; not a resolved symbol ID.
    pub owner_name: Option<String>,
    pub declared_type: Option<Type>,
    pub is_exported: bool,
    pub span: Option<ByteSpan>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OpaqueNativeItem {
    pub span: ByteSpan,
    /// Only the upstream-recognized native tag is exported; no Rust parsing.
    pub tag: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Capability {
    CheckedSyntax,
    CheckedTypeChecking,
    LexicalCheckedTokens,
    CheckedDeclarationInventory,
    CheckerSemanticFacts,
    ExactDeclarationSpans,
    IdentifierRolesAndBindings,
    OpaqueNativeItemSpans,
    NativeRustSemantics,
    CompactSource,
    FullDocumentSemantics,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Completeness {
    Complete,
    Partial,
    Unsupported,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CapabilityCoverage {
    pub capability: Capability,
    pub completeness: Completeness,
    /// Scope is part of the capability assertion, not an inferred guarantee.
    pub scope: &'static str,
    pub limitation: &'static str,
}

#[derive(Clone, Debug)]
pub struct FactBatch {
    pub provenance: Provenance,
    /// Checked compiler AST declarations; spans and bindings are not inferred.
    pub declarations: Vec<CheckedDeclaration>,
    /// Exact original-coordinate whitespace tokens, excluding native bodies.
    pub tokens: Vec<CheckedToken>,
    pub native_items: Vec<OpaqueNativeItem>,
    pub coverage: Vec<CapabilityCoverage>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RuleGate {
    /// Capabilities are available; no rule has actually been evaluated.
    Eligible,
    Insufficient {
        unavailable: Vec<Capability>,
    },
}

impl FactBatch {
    /// A rule requiring unavailable evidence MUST NOT claim a clean result.
    /// Only completeness marked `Complete` is admissible to that rule.
    pub fn gate(&self, required: &[Capability]) -> RuleGate {
        let unavailable = required
            .iter()
            .filter(|requested| {
                !self.coverage.iter().any(|coverage| {
                    coverage.capability == **requested
                        && coverage.completeness == Completeness::Complete
                })
            })
            .copied()
            .collect::<Vec<_>>();
        if unavailable.is_empty() {
            RuleGate::Eligible
        } else {
            RuleGate::Insufficient { unavailable }
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FailureKind {
    InvalidIdentity,
    InvalidLimits,
    UnsupportedFormat,
    SourceTooLarge,
    InvalidUtf8,
    UnsupportedEncoding,
    ParseRejected,
    CheckRejected,
    InconsistentNativeSpan,
    FactLimitExceeded,
}

/// A failed analysis never yields a fact batch or an admissible rule result.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AnalysisFailure {
    pub kind: FailureKind,
    pub message: String,
    pub logical_document_id: String,
    pub declared_module_context: String,
    pub source_byte_len: usize,
    /// Absent when rejected before hashing due to source size or bad limits.
    pub source_sha256: Option<String>,
}

fn sha256(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn failure(
    request: &Request<'_>,
    digest: Option<&str>,
    kind: FailureKind,
    message: impl AsRef<str>,
) -> AnalysisFailure {
    AnalysisFailure {
        kind,
        // Upstream errors may echo hostile input. Do not expose unbounded diagnostics.
        message: message.as_ref().chars().take(512).collect(),
        logical_document_id: request.logical_document_id.to_owned(),
        declared_module_context: request.declared_module_context.to_owned(),
        source_byte_len: request.source_bytes.len(),
        source_sha256: digest.map(str::to_owned),
    }
}

fn is_fluxr_space(byte: u8) -> bool {
    matches!(byte, b' ' | b'\t' | b'\r' | b'\n' | 0x0b | 0x0c)
}

fn token_shape(token: &str) -> TokenShape {
    let mut bytes = token.bytes();
    if token.len() <= 64
        && bytes
            .next()
            .is_some_and(|b| b.is_ascii_alphabetic() || b == b'_')
        && bytes.all(|b| b.is_ascii_alphanumeric() || b == b'_')
    {
        TokenShape::IdentifierLike
    } else {
        TokenShape::Other
    }
}

fn charge(request: &Request<'_>, digest: &str, count: &mut usize) -> Result<(), AnalysisFailure> {
    if *count == request.limits.max_facts {
        return Err(failure(
            request,
            Some(digest),
            FailureKind::FactLimitExceeded,
            "FluxR provider fact budget exceeded",
        ));
    }
    *count += 1;
    Ok(())
}

fn checked_tokens(
    request: &Request<'_>,
    original: &str,
    native_spans: &[ByteSpan],
    digest: &str,
    count: &mut usize,
) -> Result<Vec<CheckedToken>, AnalysisFailure> {
    let bytes = original.as_bytes();
    let mut tokens = Vec::new();
    let mut cursor = 0;
    let mut native_index = 0;
    while cursor < bytes.len() {
        if let Some(native) = native_spans.get(native_index)
            && cursor == native.start
        {
            cursor = native.end;
            native_index += 1;
            continue;
        }
        if is_fluxr_space(bytes[cursor]) {
            cursor += 1;
            continue;
        }
        let start = cursor;
        let next_native_start = native_spans
            .get(native_index)
            .map_or(bytes.len(), |span| span.start);
        while cursor < next_native_start && !is_fluxr_space(bytes[cursor]) {
            cursor += 1;
        }
        let lexeme = original.get(start..cursor).ok_or_else(|| {
            failure(
                request,
                Some(digest),
                FailureKind::InconsistentNativeSpan,
                "token does not end at an original UTF-8 boundary",
            )
        })?;
        charge(request, digest, count)?;
        tokens.push(CheckedToken {
            span: ByteSpan { start, end: cursor },
            text: lexeme.to_owned(),
            shape: token_shape(lexeme),
        });
    }
    Ok(tokens)
}

fn append_declaration(
    request: &Request<'_>,
    digest: &str,
    facts: &mut Vec<CheckedDeclaration>,
    count: &mut usize,
    fact: CheckedDeclaration,
) -> Result<(), AnalysisFailure> {
    charge(request, digest, count)?;
    facts.push(fact);
    Ok(())
}

fn declaration(
    kind: DeclarationKind,
    name: &str,
    owner_name: Option<&str>,
    declared_type: Option<&Type>,
    is_exported: bool,
) -> CheckedDeclaration {
    CheckedDeclaration {
        kind,
        name: name.to_owned(),
        owner_name: owner_name.map(str::to_owned),
        declared_type: declared_type.cloned(),
        is_exported,
        span: None,
    }
}

fn declarations(
    request: &Request<'_>,
    document: &SourceDocument,
    digest: &str,
    count: &mut usize,
) -> Result<Vec<CheckedDeclaration>, AnalysisFailure> {
    let source = &document.source;
    let mut facts = Vec::new();
    for binding in &source.inputs {
        append_declaration(
            request,
            digest,
            &mut facts,
            count,
            declaration(
                DeclarationKind::Input,
                &binding.name,
                None,
                Some(&binding.ty),
                false,
            ),
        )?;
    }
    for record in &source.records {
        append_declaration(
            request,
            digest,
            &mut facts,
            count,
            declaration(DeclarationKind::Record, &record.name, None, None, false),
        )?;
        for field in &record.fields {
            append_declaration(
                request,
                digest,
                &mut facts,
                count,
                declaration(
                    DeclarationKind::RecordField,
                    &field.name,
                    Some(&record.name),
                    Some(&field.ty),
                    false,
                ),
            )?;
        }
    }
    for enumeration in &source.enums {
        append_declaration(
            request,
            digest,
            &mut facts,
            count,
            declaration(DeclarationKind::Enum, &enumeration.name, None, None, false),
        )?;
        for variant in &enumeration.variants {
            append_declaration(
                request,
                digest,
                &mut facts,
                count,
                declaration(
                    DeclarationKind::EnumVariant,
                    &variant.name,
                    Some(&enumeration.name),
                    variant.payload.as_ref(),
                    false,
                ),
            )?;
        }
    }
    for function in &source.functions {
        append_declaration(
            request,
            digest,
            &mut facts,
            count,
            declaration(
                if function.receiver.is_some() {
                    DeclarationKind::Method
                } else {
                    DeclarationKind::Function
                },
                &function.name,
                function.receiver.as_deref(),
                Some(&function.result),
                function.export,
            ),
        )?;
        for binding in &function.params {
            append_declaration(
                request,
                digest,
                &mut facts,
                count,
                declaration(
                    DeclarationKind::Parameter,
                    &binding.name,
                    Some(&function.name),
                    Some(&binding.ty),
                    false,
                ),
            )?;
        }
        for binding in &function.locals {
            append_declaration(
                request,
                digest,
                &mut facts,
                count,
                declaration(
                    DeclarationKind::Local,
                    &binding.name,
                    Some(&function.name),
                    Some(&binding.ty),
                    false,
                ),
            )?;
        }
    }
    Ok(facts)
}

fn coverage() -> Vec<CapabilityCoverage> {
    use Capability as C;
    use Completeness as S;
    vec![
        CapabilityCoverage {
            capability: C::CheckedSyntax,
            completeness: S::Complete,
            scope: "canonical FluxR 1 checked projection",
            limitation: "upstream parser accepted source; native items are opaque",
        },
        CapabilityCoverage {
            capability: C::CheckedTypeChecking,
            completeness: S::Complete,
            scope: "checked structured SourceDocument.source only",
            limitation: "no FluxR/native execution or external host service validation",
        },
        CapabilityCoverage {
            capability: C::LexicalCheckedTokens,
            completeness: S::Complete,
            scope: "original ASCII-whitespace-delimited checked tokens, including header",
            limitation: "no grammar token kinds, binding roles, or native-body tokens",
        },
        CapabilityCoverage {
            capability: C::CheckedDeclarationInventory,
            completeness: S::Complete,
            scope: "compiler-accepted checked AST, in declaration order",
            limitation: "names and types only; no source spans or resolved symbol IDs",
        },
        CapabilityCoverage {
            capability: C::CheckerSemanticFacts,
            completeness: S::Partial,
            scope: "checked declarations, lexical occurrences and binding-sensitive rules",
            limitation: "inventory and byte-exact tokens exist but spans/roles/bindings do not",
        },
        CapabilityCoverage {
            capability: C::ExactDeclarationSpans,
            completeness: S::Unsupported,
            scope: "checked AST declarations",
            limitation: "upstream public AST has no declaration coordinates",
        },
        CapabilityCoverage {
            capability: C::IdentifierRolesAndBindings,
            completeness: S::Unsupported,
            scope: "checked identifier occurrences",
            limitation: "the public AST does not carry per-occurrence ranges/binding IDs",
        },
        CapabilityCoverage {
            capability: C::OpaqueNativeItemSpans,
            completeness: S::Complete,
            scope: "native items recognized by pinned unified parser",
            limitation: "opaque span includes tag and payload; not checked Rust",
        },
        CapabilityCoverage {
            capability: C::NativeRustSemantics,
            completeness: S::Unsupported,
            scope: "native items and external host code",
            limitation: "never parsed as Rust, compiled or executed by this provider",
        },
        CapabilityCoverage {
            capability: C::CompactSource,
            completeness: S::Unsupported,
            scope: "legacy compact / alternate source encodings",
            limitation: "canonical checked FluxR 1 text only",
        },
        CapabilityCoverage {
            capability: C::FullDocumentSemantics,
            completeness: S::Unsupported,
            scope: "checked and native/host effects together",
            limitation: "static checked validation does not prove external native/host behavior",
        },
    ]
}

/// Parse a compact module fragment in the declared language version. The
/// optional synthetic header grants syntax evidence only, with no original byte spans
/// or claims about type checking in the eventual multi-file module.
pub fn validate_compact_fragment(bytes: &[u8], version: u8) -> Result<(), String> {
    if !matches!(version, 1 | 2) || bytes.len() > DEFAULT_MAX_SOURCE_BYTES {
        return Err("Compact fragment exceeds its version or source scope.".into());
    }
    let fragment = std::str::from_utf8(bytes).map_err(|error| error.to_string())?;
    let header = format!("v{version}");
    let parsed = match fragment.split_whitespace().next() {
        Some(actual @ ("v1" | "v2")) if actual == header => {
            fluxr::structured::Source::parse_compact(fragment)
        }
        Some("v1" | "v2") => return Err("Compact source version conflicts with its declared module.".into()),
        _ => fluxr::structured::Source::parse_compact(&format!("{header}\n{fragment}")),
    };
    parsed.map(|_| ()).map_err(|error| error.to_string())
}

/// Parse and check one UTF-8 source document, without running any of its code.
/// Native item positions come *only* from upstream `SourceDocument::parse`.
/// Checked tokens are separated using the same six ASCII whitespace bytes as
/// the pinned upstream unified reader, excluding its exact native ranges.
/// Any rejection returns no facts and no "clean" determination.
pub fn analyze(request: Request<'_>) -> Result<FactBatch, AnalysisFailure> {
    if request.logical_document_id.is_empty()
        || request.logical_document_id.len() > 1024
        || request.declared_module_context.trim().is_empty()
        || request.declared_module_context.len() > 1024
        || request.logical_document_id.chars().any(char::is_control)
        || request
            .declared_module_context
            .chars()
            .any(char::is_control)
    {
        return Err(failure(
            &request,
            None,
            FailureKind::InvalidIdentity,
            "logical document and caller-declared module context must be bounded nonempty text",
        ));
    }
    if request.limits.max_source_bytes == 0
        || request.limits.max_source_bytes > HARD_MAX_SOURCE_BYTES
        || request.limits.max_facts == 0
        || request.limits.max_facts > HARD_MAX_FACTS
    {
        return Err(failure(
            &request,
            None,
            FailureKind::InvalidLimits,
            "source and fact limits must be nonzero and no larger than provider hard limits",
        ));
    }
    if request.source_bytes.len() > request.limits.max_source_bytes {
        return Err(failure(
            &request,
            None,
            FailureKind::SourceTooLarge,
            "source exceeds explicitly configured byte budget",
        ));
    }
    let digest = sha256(request.source_bytes);
    if request.format != SourceFormat::CanonicalChecked
        || !request.logical_document_id.ends_with(".fluxr")
    {
        return Err(failure(
            &request,
            Some(&digest),
            FailureKind::UnsupportedFormat,
            "only canonical checked .fluxr documents are supported",
        ));
    }
    // Upstream requires the exact `fluxr 1\n` prefix. Never remove a BOM or
    // normalize newlines, because those changes would invalidate source spans.
    if request.source_bytes.starts_with(&[0xef, 0xbb, 0xbf]) {
        return Err(failure(
            &request,
            Some(&digest),
            FailureKind::UnsupportedEncoding,
            "UTF-8 BOM is unsupported for canonical FluxR 1 source",
        ));
    }
    let source = std::str::from_utf8(request.source_bytes).map_err(|error| {
        failure(
            &request,
            Some(&digest),
            FailureKind::InvalidUtf8,
            error.to_string(),
        )
    })?;
    let document = SourceDocument::parse(source).map_err(|error| {
        failure(
            &request,
            Some(&digest),
            FailureKind::ParseRejected,
            error.to_string(),
        )
    })?;
    // This is the checked *compiler* entry point, never Module::run or any AOT/JIT route.
    let _compiled_checked_module = document.compile().map_err(|error| {
        failure(
            &request,
            Some(&digest),
            FailureKind::CheckRejected,
            error.to_string(),
        )
    })?;

    let mut fact_count = 0;
    let mut native_spans = Vec::new();
    let mut native_items = Vec::new();
    let mut prior_end = "fluxr 1\n".len();
    for span in &document.native_spans {
        if span.start < prior_end
            || span.start >= span.end
            || span.end > source.len()
            || !source.is_char_boundary(span.start)
            || !source.is_char_boundary(span.end)
        {
            return Err(failure(
                &request,
                Some(&digest),
                FailureKind::InconsistentNativeSpan,
                "upstream native spans overlap or are not original UTF-8 ranges",
            ));
        }
        let text = source.get(span.start..span.end).ok_or_else(|| {
            failure(
                &request,
                Some(&digest),
                FailureKind::InconsistentNativeSpan,
                "upstream native item not sliceable in original source",
            )
        })?;
        let tag_end = text.bytes().position(is_fluxr_space).unwrap_or(text.len());
        charge(&request, &digest, &mut fact_count)?;
        let original_span = ByteSpan {
            start: span.start,
            end: span.end,
        };
        native_spans.push(original_span);
        native_items.push(OpaqueNativeItem {
            span: original_span,
            tag: text[..tag_end].to_owned(),
        });
        prior_end = span.end;
    }
    let declarations = declarations(&request, &document, &digest, &mut fact_count)?;
    let tokens = checked_tokens(&request, source, &native_spans, &digest, &mut fact_count)?;
    Ok(FactBatch {
        provenance: Provenance {
            provider_id: PROVIDER_ID,
            schema_version: SCHEMA_VERSION,
            parser_commit: PARSER_COMMIT,
            parser_tree: PARSER_TREE,
            logical_document_id: request.logical_document_id.to_owned(),
            declared_module_context: request.declared_module_context.to_owned(),
            source_sha256: digest,
            source_byte_len: source.len(),
            span_encoding: SpanEncoding::OriginalUtf8BytesHalfOpen,
            checked_projection_sha256: sha256(document.checked_text.as_bytes()),
            native_binding_digest: document.native_digest,
        },
        declarations,
        tokens,
        native_items,
        coverage: coverage(),
    })
}
