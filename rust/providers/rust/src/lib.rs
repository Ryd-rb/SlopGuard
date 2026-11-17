//! Bounded Rust syntax facts. No Cargo project model, macro execution, HIR or LSP process.
//!
//! `analyze_source` and `analyze_file` consume one explicitly edited Rust source unit.
//! All spans are half-open **UTF-8 byte offsets into the original input** (not chars,
//! UTF-16 units or line/column coordinates). Facts retain a syntax-only provenance;
//! lexical appearances must not be used as compiler-resolved references.

#![forbid(unsafe_code)]

use ra_ap_syntax::{
    AstNode, Edition, SourceFile, SyntaxKind, SyntaxNode, SyntaxToken, TextRange, ast,
};
use sha2::{Digest, Sha256};
use std::fmt;
use std::fs::File;
use std::io::{self, Read};
use std::path::Path;

pub const PROVIDER_ID: &str = "slophawk.rust.syntax";
pub const SCHEMA_VERSION: u32 = 1;
pub const PARSER_VERSION: &str = "0.0.363";
pub const MAX_SOURCE_BYTES: usize = 1024 * 1024;
pub const MAX_FACTS: usize = 20_000;
const MAX_DOCUMENT_ID_BYTES: usize = 512;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SourceEdition {
    E2015,
    E2018,
    E2021,
    E2024,
}

impl SourceEdition {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::E2015 => "2015",
            Self::E2018 => "2018",
            Self::E2021 => "2021",
            Self::E2024 => "2024",
        }
    }

    fn parser_edition(self) -> Edition {
        match self {
            Self::E2015 => Edition::Edition2015,
            Self::E2018 => Edition::Edition2018,
            Self::E2021 => Edition::Edition2021,
            Self::E2024 => Edition::Edition2024,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Limits {
    /// A caller can lower, but cannot raise, the hard per-unit bound.
    pub source_bytes: usize,
    /// Total output records across *all* categories, including diagnostics/frontiers.
    pub facts: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            source_bytes: MAX_SOURCE_BYTES,
            facts: MAX_FACTS,
        }
    }
}

impl Limits {
    fn checked(self) -> Result<Self, Error> {
        if self.source_bytes == 0 || self.facts < 2 {
            return Err(Error::InvalidLimits);
        }
        Ok(Self {
            source_bytes: self.source_bytes.min(MAX_SOURCE_BYTES),
            facts: self.facts.min(MAX_FACTS),
        })
    }
}

#[derive(Debug)]
pub enum Error {
    InvalidDocumentId,
    InvalidLimits,
    SourceTooLarge { limit: usize },
    NotUtf8,
    Io(io::Error),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidDocumentId => write!(
                f,
                "logical document ID must be nonempty and at most 512 bytes without controls"
            ),
            Self::InvalidLimits => write!(
                f,
                "limits must allow at least two facts and one source byte"
            ),
            Self::SourceTooLarge { limit } => write!(f, "Rust source exceeds {limit} bytes"),
            Self::NotUtf8 => write!(f, "Rust source is not valid UTF-8"),
            Self::Io(err) => write!(f, "Rust source read failed: {err}"),
        }
    }
}

impl std::error::Error for Error {}

impl From<io::Error> for Error {
    fn from(value: io::Error) -> Self {
        Self::Io(value)
    }
}

/// Inclusive start, exclusive end; offsets address the original source byte buffer.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub struct Span {
    pub start: u32,
    pub end: u32,
}

impl Span {
    fn from_range(range: TextRange) -> Self {
        Self {
            start: u32::from(range.start()),
            end: u32::from(range.end()),
        }
    }

    fn contains(self, inner: Self) -> bool {
        self.start <= inner.start && inner.end <= self.end
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Identity {
    pub provider: &'static str,
    pub provider_version: &'static str,
    pub schema_version: u32,
    pub parser_crate: &'static str,
    pub parser_version: &'static str,
    pub edition: SourceEdition,
    pub logical_document: String,
    pub sha256: String,
    pub source_bytes: usize,
    pub span_encoding: &'static str,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Completeness {
    /// Only the declared syntax capabilities are complete; not semantic resolution.
    CompleteSyntax,
    /// Diagnostics, unknown source interpretation or resource limits prevent a full result.
    PartialSyntax,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DeclarationKind {
    Function,
    Implementation,
    Module,
    Structure,
    Enumeration,
    Union,
    Trait,
    TypeAlias,
    Constant,
    Static,
    MacroDefinition,
    ExternalCrate,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Declaration {
    pub kind: DeclarationKind,
    /// Syntax-level name only. `impl` has no declared name in this sense.
    pub name: Option<String>,
    pub name_span: Option<Span>,
    pub span: Span,
    /// Innermost syntactic parent declaration, if any; not a resolved symbol.
    pub parent: Option<Span>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BlockKind {
    Expression,
    AssociatedItems,
    ModuleItems,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Block {
    pub kind: BlockKind,
    pub span: Span,
    pub owner: Option<Span>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SignificantToken {
    /// Debug representation of rust-analyzer's syntax kind, not a stable wire enum.
    pub kind: String,
    pub text: String,
    pub span: Span,
    /// For clone input: original token except for conservatively proven locals.
    pub clone_key: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Comment {
    pub span: Span,
    pub text: String,
    pub documentation: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum IdentifierRole {
    /// A spelling occurrence; no local or semantic binding established.
    Lexical,
    /// A declaration's syntactic name, not a resolved symbol identity.
    DeclarationName,
    /// A simple `let name` in an admitted, single function-body scope.
    ProvenLocalBinding { ordinal: u32 },
    /// A simple unqualified path with an unambiguous in-scope `let` binding.
    ProvenLocalReference { binding: Span, ordinal: u32 },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Identifier {
    pub spelling: String,
    pub span: Span,
    pub scope: Option<Span>,
    pub role: IdentifierRole,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ParseDiagnostic {
    pub span: Span,
    pub message: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FrontierKind {
    MacroExpansion,
    ConditionalCompilation,
    GeneratedItems,
    TraitDispatch,
    ExternalModule,
    ParseRecovery,
    FactLimit,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Unresolved {
    pub kind: FrontierKind,
    pub span: Span,
    pub reason: &'static str,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SourceFacts {
    pub identity: Identity,
    /// Capability names deliberately local to this pilot; no shared wire contract.
    pub capabilities: &'static [&'static str],
    pub completeness: Completeness,
    pub declarations: Vec<Declaration>,
    pub blocks: Vec<Block>,
    pub tokens: Vec<SignificantToken>,
    pub identifiers: Vec<Identifier>,
    pub comments: Vec<Comment>,
    pub diagnostics: Vec<ParseDiagnostic>,
    pub unresolved: Vec<Unresolved>,
    pub truncated: bool,
}

const CAPABILITIES: &[&str] = &[
    "declarations.syntax",
    "blocks.syntax",
    "tokens.lexical",
    "identifiers.lexical",
    "identifiers.local-conservative",
    "comments.lexical",
    "clone-input.exact-and-proven-locals",
    "spelling-input.identifiers-and-comments",
    "diagnostics.parse-recovery",
    "unresolved-frontiers.syntax",
];

/// Read no more than one byte above the requested limit even for special files.
/// Neither this function nor the parser runs Cargo, procedural macros or build scripts.
pub fn analyze_file(
    edition: SourceEdition,
    logical_document: &str,
    path: impl AsRef<Path>,
    limits: Limits,
) -> Result<SourceFacts, Error> {
    let limits = limits.checked()?;
    validate_document_id(logical_document)?;
    let file = File::open(path)?;
    if file.metadata()?.len() > limits.source_bytes as u64 {
        return Err(Error::SourceTooLarge {
            limit: limits.source_bytes,
        });
    }
    let mut bytes = Vec::new();
    file.take(limits.source_bytes as u64 + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() > limits.source_bytes {
        return Err(Error::SourceTooLarge {
            limit: limits.source_bytes,
        });
    }
    let source = String::from_utf8(bytes).map_err(|_| Error::NotUtf8)?;
    analyze_source(edition, logical_document, &source, limits)
}

pub fn analyze_source(
    edition: SourceEdition,
    logical_document: &str,
    source: &str,
    limits: Limits,
) -> Result<SourceFacts, Error> {
    let limits = limits.checked()?;
    validate_document_id(logical_document)?;
    if source.len() > limits.source_bytes {
        return Err(Error::SourceTooLarge {
            limit: limits.source_bytes,
        });
    }
    let digest = Sha256::digest(source.as_bytes());
    let sha256 = digest
        .iter()
        .map(|part| format!("{part:02x}"))
        .collect::<String>();
    let mut result = SourceFacts {
        identity: Identity {
            provider: PROVIDER_ID,
            provider_version: env!("CARGO_PKG_VERSION"),
            schema_version: SCHEMA_VERSION,
            parser_crate: "ra_ap_syntax",
            parser_version: PARSER_VERSION,
            edition,
            logical_document: logical_document.to_owned(),
            sha256,
            source_bytes: source.len(),
            span_encoding: "utf8-byte-offset-half-open-original",
        },
        capabilities: CAPABILITIES,
        completeness: Completeness::CompleteSyntax,
        declarations: Vec::new(),
        blocks: Vec::new(),
        tokens: Vec::new(),
        identifiers: Vec::new(),
        comments: Vec::new(),
        diagnostics: Vec::new(),
        unresolved: Vec::new(),
        truncated: false,
    };

    // Keep the syntax tree in this block: neither green trees nor AST nodes escape.
    {
        let parse = SourceFile::parse(source, edition.parser_edition());
        let errors = parse.errors();
        let root = parse.syntax_node();
        let mut budget = Budget {
            remaining: limits.facts - 1,
            exhausted: false,
        };
        for error in errors.iter() {
            if !budget.take() {
                break;
            }
            result.diagnostics.push(ParseDiagnostic {
                span: Span::from_range(error.range()),
                message: error.to_string().chars().take(256).collect(),
            });
        }
        if !errors.is_empty() {
            push_frontier(
                &mut result,
                &mut budget,
                FrontierKind::ParseRecovery,
                Span {
                    start: 0,
                    end: source.len() as u32,
                },
                "parser recovered from malformed input; no name resolution is asserted",
            );
        }

        for node in root.descendants() {
            let kind = node.kind();
            if let Some(declaration_kind) = declaration_kind(kind) {
                if !budget.take() {
                    break;
                }
                let name_node = node
                    .children()
                    .find(|child| child.kind() == SyntaxKind::NAME);
                result.declarations.push(Declaration {
                    kind: declaration_kind,
                    name: name_node.as_ref().map(|name| name.text().to_string()),
                    name_span: name_node
                        .as_ref()
                        .map(|name| Span::from_range(name.text_range())),
                    span: Span::from_range(node.text_range()),
                    parent: enclosing_declaration(&node),
                });
            }
            if let Some(block_kind) = block_kind(kind) {
                if !budget.take() {
                    break;
                }
                result.blocks.push(Block {
                    kind: block_kind,
                    span: Span::from_range(node.text_range()),
                    owner: enclosing_declaration(&node),
                });
            }
            match kind {
                SyntaxKind::MACRO_CALL | SyntaxKind::MACRO_DEF | SyntaxKind::MACRO_RULES => {
                    push_frontier(
                        &mut result,
                        &mut budget,
                        FrontierKind::MacroExpansion,
                        Span::from_range(node.text_range()),
                        "token trees are source syntax, not expanded macro output",
                    );
                    if kind == SyntaxKind::MACRO_CALL && is_include_macro(&node) {
                        push_frontier(
                            &mut result,
                            &mut budget,
                            FrontierKind::GeneratedItems,
                            Span::from_range(node.text_range()),
                            "included or generated source has not been read or parsed",
                        );
                    }
                }
                SyntaxKind::ATTR => {
                    if is_conditional_attribute(&node) {
                        push_frontier(
                            &mut result,
                            &mut budget,
                            FrontierKind::ConditionalCompilation,
                            Span::from_range(node.text_range()),
                            "cfg or cfg_attr not evaluated against a target/feature set",
                        );
                    } else {
                        // Without a compiler, even unfamiliar custom attributes cannot be
                        // proven inert; a derive also synthesizes impl items.
                        push_frontier(
                            &mut result,
                            &mut budget,
                            FrontierKind::MacroExpansion,
                            Span::from_range(node.text_range()),
                            "attribute expansion and attribute-macro identity are not resolved",
                        );
                        if is_derive_attribute(&node) {
                            push_frontier(
                                &mut result,
                                &mut budget,
                                FrontierKind::GeneratedItems,
                                Span::from_range(node.text_range()),
                                "derive output is not part of the parsed source",
                            );
                        }
                    }
                }
                SyntaxKind::METHOD_CALL_EXPR => {
                    push_frontier(
                        &mut result,
                        &mut budget,
                        FrontierKind::TraitDispatch,
                        Span::from_range(node.text_range()),
                        "method target and trait dispatch need semantic resolution",
                    );
                }
                SyntaxKind::MODULE
                    if node
                        .children()
                        .any(|child| child.kind() == SyntaxKind::NAME)
                        && !node
                            .children()
                            .any(|child| child.kind() == SyntaxKind::ITEM_LIST) =>
                {
                    push_frontier(
                        &mut result,
                        &mut budget,
                        FrontierKind::ExternalModule,
                        Span::from_range(node.text_range()),
                        "out-of-line module contents need project and file resolution",
                    );
                }
                _ => {}
            }
            if budget.exhausted {
                break;
            }
        }

        for token in root
            .descendants_with_tokens()
            .filter_map(|item| item.into_token())
        {
            if matches!(token.kind(), SyntaxKind::WHITESPACE | SyntaxKind::NEWLINE) {
                continue;
            }
            let span = Span::from_range(token.text_range());
            if matches!(
                token.kind(),
                SyntaxKind::COMMENT | SyntaxKind::INNER_DOC_COMMENT | SyntaxKind::OUTER_DOC_COMMENT
            ) {
                if !budget.take() {
                    break;
                }
                result.comments.push(Comment {
                    span,
                    text: token.text().to_string(),
                    documentation: matches!(
                        token.kind(),
                        SyntaxKind::INNER_DOC_COMMENT | SyntaxKind::OUTER_DOC_COMMENT
                    ),
                });
                continue;
            }
            if !budget.take() {
                break;
            }
            result.tokens.push(SignificantToken {
                kind: format!("{:?}", token.kind()),
                text: token.text().to_string(),
                span,
                clone_key: token.text().to_string(),
            });
            if token.kind() == SyntaxKind::IDENT {
                if !budget.take() {
                    break;
                }
                result.identifiers.push(Identifier {
                    spelling: token.text().to_string(),
                    span,
                    scope: nearest_block(&token),
                    role: IdentifierRole::Lexical,
                });
            }
        }

        for declaration in &result.declarations {
            if let Some(name_span) = declaration.name_span
                && let Some(id) = result
                    .identifiers
                    .iter_mut()
                    .find(|id| id.span == name_span)
            {
                id.role = IdentifierRole::DeclarationName;
            }
        }

        // Never infer resolution from broken, conditional or macro-bearing source.
        if errors.is_empty() && !budget.exhausted && result.unresolved.is_empty() {
            normalize_simple_locals(&root, &mut result);
        }
        if budget.exhausted {
            result.truncated = true;
            result.unresolved.push(Unresolved {
                kind: FrontierKind::FactLimit,
                span: Span {
                    start: 0,
                    end: source.len() as u32,
                },
                reason: "at least one record was omitted by the output fact limit",
            });
        }
    }
    if result.truncated || !result.diagnostics.is_empty() || !result.unresolved.is_empty() {
        result.completeness = Completeness::PartialSyntax;
    }
    Ok(result)
}

fn validate_document_id(document: &str) -> Result<(), Error> {
    if document.is_empty()
        || document.len() > MAX_DOCUMENT_ID_BYTES
        || document.chars().any(char::is_control)
    {
        return Err(Error::InvalidDocumentId);
    }
    Ok(())
}

struct Budget {
    remaining: usize,
    exhausted: bool,
}

impl Budget {
    fn take(&mut self) -> bool {
        if self.remaining == 0 {
            self.exhausted = true;
            false
        } else {
            self.remaining -= 1;
            true
        }
    }
}

fn push_frontier(
    result: &mut SourceFacts,
    budget: &mut Budget,
    kind: FrontierKind,
    span: Span,
    reason: &'static str,
) {
    if budget.take() {
        result.unresolved.push(Unresolved { kind, span, reason });
    }
}

fn declaration_kind(kind: SyntaxKind) -> Option<DeclarationKind> {
    Some(match kind {
        SyntaxKind::FN => DeclarationKind::Function,
        SyntaxKind::IMPL => DeclarationKind::Implementation,
        SyntaxKind::MODULE => DeclarationKind::Module,
        SyntaxKind::STRUCT => DeclarationKind::Structure,
        SyntaxKind::ENUM => DeclarationKind::Enumeration,
        SyntaxKind::UNION => DeclarationKind::Union,
        SyntaxKind::TRAIT => DeclarationKind::Trait,
        SyntaxKind::TYPE_ALIAS => DeclarationKind::TypeAlias,
        SyntaxKind::CONST => DeclarationKind::Constant,
        SyntaxKind::STATIC => DeclarationKind::Static,
        SyntaxKind::MACRO_DEF | SyntaxKind::MACRO_RULES => DeclarationKind::MacroDefinition,
        SyntaxKind::EXTERN_CRATE => DeclarationKind::ExternalCrate,
        _ => return None,
    })
}

fn block_kind(kind: SyntaxKind) -> Option<BlockKind> {
    Some(match kind {
        SyntaxKind::BLOCK_EXPR => BlockKind::Expression,
        SyntaxKind::ASSOC_ITEM_LIST => BlockKind::AssociatedItems,
        SyntaxKind::ITEM_LIST => BlockKind::ModuleItems,
        _ => return None,
    })
}

fn enclosing_declaration(node: &SyntaxNode) -> Option<Span> {
    node.ancestors()
        .skip(1)
        .find(|ancestor| declaration_kind(ancestor.kind()).is_some())
        .map(|ancestor| Span::from_range(ancestor.text_range()))
}

fn nearest_block(token: &SyntaxToken) -> Option<Span> {
    token
        .parent()
        .and_then(|node| {
            node.ancestors()
                .find(|node| node.kind() == SyntaxKind::BLOCK_EXPR)
        })
        .map(|node| Span::from_range(node.text_range()))
}

/// Get the first meaningful token in the attribute path. Comments, CRLF and
/// whitespace before `cfg` are ignored; scanning only the head prevents false
/// positives from nested meta or literal strings.
fn attribute_head(node: &SyntaxNode) -> Option<String> {
    node.descendants_with_tokens()
        .filter_map(|element| element.into_token())
        .filter(|token| {
            !matches!(
                token.kind(),
                SyntaxKind::WHITESPACE | SyntaxKind::NEWLINE | SyntaxKind::COMMENT
            )
        })
        .map(|token| token.text().to_string())
        .find(|text| !matches!(text.as_str(), "#" | "!" | "["))
}

fn is_conditional_attribute(node: &SyntaxNode) -> bool {
    matches!(attribute_head(node).as_deref(), Some("cfg" | "cfg_attr"))
}

fn is_derive_attribute(node: &SyntaxNode) -> bool {
    attribute_head(node).as_deref() == Some("derive")
}

fn is_include_macro(node: &SyntaxNode) -> bool {
    let compact: String = node
        .text()
        .to_string()
        .chars()
        .filter(|c| !c.is_whitespace())
        .take(32)
        .collect();
    compact.starts_with("include!") || compact.starts_with("::include!")
}

#[derive(Clone)]
struct LocalBinding {
    name: String,
    span: Span,
    visible_after: u32,
    ordinal: u32,
}

/// Deliberately narrow: only simple, direct `let name` statements in the
/// immediate body of an ordinary function and single-token path expressions in
/// the *same* body. In particular, never bind through control-flow patterns,
/// nested blocks, macro hygiene, method selection or a recovered parse.
fn normalize_simple_locals(root: &SyntaxNode, facts: &mut SourceFacts) {
    for function in root.descendants().filter_map(ast::Fn::cast) {
        let Some(body) = function.body() else {
            continue;
        };
        let Some(statements) = body.stmt_list() else {
            continue;
        };
        if function.syntax().descendants().any(|node| {
            matches!(
                node.kind(),
                SyntaxKind::MACRO_CALL
                    | SyntaxKind::MACRO_DEF
                    | SyntaxKind::ATTR
                    | SyntaxKind::CLOSURE_EXPR
                    | SyntaxKind::LET_EXPR
                    | SyntaxKind::MACRO_RULES
            )
        }) {
            continue;
        }
        let mut bindings = Vec::new();
        let mut invalid_pattern = false;
        for statement in statements
            .syntax()
            .children()
            .filter_map(ast::LetStmt::cast)
        {
            let Some(pattern) = statement.pat() else {
                invalid_pattern = true;
                break;
            };
            let Some(identifier) = ast::IdentPat::cast(pattern.syntax().clone()) else {
                invalid_pattern = true;
                break;
            };
            if !identifier.is_simple_ident() || identifier.at_token().is_some() {
                invalid_pattern = true;
                break;
            }
            use ast::HasName;
            let Some(name) = identifier.name() else {
                invalid_pattern = true;
                break;
            };
            let name_span = Span::from_range(name.syntax().text_range());
            bindings.push(LocalBinding {
                name: name.syntax().text().to_string(),
                span: name_span,
                visible_after: u32::from(statement.syntax().text_range().end()),
                ordinal: bindings.len() as u32,
            });
        }
        if invalid_pattern || bindings.is_empty() {
            continue;
        }
        let body_span = Span::from_range(body.syntax().text_range());
        for binding in &bindings {
            if let Some(id) = facts
                .identifiers
                .iter_mut()
                .find(|id| id.span == binding.span)
            {
                id.role = IdentifierRole::ProvenLocalBinding {
                    ordinal: binding.ordinal,
                };
                set_clone_key(&mut facts.tokens, binding.span, binding.ordinal);
            }
        }
        for path in body
            .syntax()
            .descendants()
            .filter(|node| node.kind() == SyntaxKind::PATH_EXPR)
        {
            // The nearest block must be the selected body, not a nested lexical scope.
            if path
                .ancestors()
                .find(|node| node.kind() == SyntaxKind::BLOCK_EXPR)
                .as_ref()
                != Some(body.syntax())
            {
                continue;
            }
            // For-loops, match guards etc. introduce additional syntactic bindings.
            let mut unsafe_context = false;
            for ancestor in path.ancestors() {
                if &ancestor == body.syntax() {
                    break;
                }
                if matches!(
                    ancestor.kind(),
                    SyntaxKind::FOR_EXPR
                        | SyntaxKind::MATCH_EXPR
                        | SyntaxKind::IF_EXPR
                        | SyntaxKind::WHILE_EXPR
                        | SyntaxKind::CLOSURE_EXPR
                        | SyntaxKind::LET_EXPR
                        | SyntaxKind::MACRO_CALL
                ) {
                    unsafe_context = true;
                    break;
                }
            }
            if unsafe_context {
                continue;
            }
            let mut candidates = path
                .descendants_with_tokens()
                .filter_map(|element| element.into_token())
                .filter(|token| token.kind() != SyntaxKind::WHITESPACE);
            let Some(token) = candidates.next() else {
                continue;
            };
            if token.kind() != SyntaxKind::IDENT || candidates.next().is_some() {
                continue;
            }
            let span = Span::from_range(token.text_range());
            if !body_span.contains(span) {
                continue;
            }
            let latest = bindings
                .iter()
                .filter(|binding| {
                    binding.name == token.text() && binding.visible_after <= span.start
                })
                .max_by_key(|binding| binding.visible_after);
            let Some(binding) = latest else { continue };
            if let Some(id) = facts.identifiers.iter_mut().find(|id| id.span == span) {
                id.role = IdentifierRole::ProvenLocalReference {
                    binding: binding.span,
                    ordinal: binding.ordinal,
                };
                set_clone_key(&mut facts.tokens, span, binding.ordinal);
            }
        }
    }
}

fn set_clone_key(tokens: &mut [SignificantToken], span: Span, ordinal: u32) {
    if let Some(token) = tokens.iter_mut().find(|token| token.span == span) {
        token.clone_key = format!("$local{ordinal}");
    }
}
