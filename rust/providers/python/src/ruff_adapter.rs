//! Only this module uses Ruff's unstable, version-pinned internal AST surface.
//! Do not re-export Ruff types from the provider boundary.

use std::collections::{BTreeMap, HashMap, HashSet};

use ruff_python_ast::token::TokenKind;
use ruff_python_ast::visitor::{self, Visitor};
use ruff_python_ast::{self as ast, Expr, ExprContext, Pattern, PySourceType, PythonVersion, Stmt};
use ruff_python_parser::{ParseOptions, parse_unchecked};
use ruff_text_size::{Ranged, TextRange};

use crate::{
    ByteSpan, Capabilities, CommentFact, Coverage, Encoding, FileKind, Gap, GapKind,
    IdentifierFact, IdentifierRole, Limits, LocalBinding, Outcome, Region, RegionKind, SourceFacts,
    SourceIdentity, TokenCategory, TokenFact, TokenLayout,
};

fn bytespan(range: TextRange, prefix: u32) -> ByteSpan {
    ByteSpan {
        start: range.start().to_u32() + prefix,
        end: range.end().to_u32() + prefix,
    }
}

/// Detect only the encodings for which raw UTF-8 byte coordinates are exact.
/// Follow tokenize.detect_encoding's first/second physical line convention:
/// the second line is eligible only if the first is blank or comment-only.
/// An unknown declaration is *not* silently interpreted as UTF-8.
pub(super) fn detect_source_encoding(raw: &[u8]) -> Result<(Encoding, usize), Encoding> {
    let prefix = if raw.starts_with(&[0xEF, 0xBB, 0xBF]) {
        3
    } else {
        0
    };
    let text = &raw[prefix..];
    let (line1, next) = first_line(text);
    let coding = cookie(line1).or_else(|| {
        if blank_or_comment(line1) && next < text.len() {
            let (line2, _) = first_line(&text[next..]);
            cookie(line2)
        } else {
            None
        }
    });
    if let Some(enc) = coding {
        let normalized = enc.to_ascii_lowercase().replace('_', "-");
        if !matches!(normalized.as_str(), "utf-8" | "utf8" | "utf-8-sig") {
            return Err(Encoding::Unsupported(enc));
        }
    }
    if std::str::from_utf8(text).is_err() {
        return Err(Encoding::InvalidUtf8);
    }
    Ok((
        if prefix == 0 {
            Encoding::Utf8
        } else {
            Encoding::Utf8Bom
        },
        prefix,
    ))
}

fn first_line(bytes: &[u8]) -> (&[u8], usize) {
    let end = bytes
        .iter()
        .position(|byte| *byte == b'\r' || *byte == b'\n');
    match end {
        Some(pos) if bytes[pos] == b'\r' && bytes.get(pos + 1) == Some(&b'\n') => {
            (&bytes[..pos], pos + 2)
        }
        Some(pos) => (&bytes[..pos], pos + 1),
        None => (bytes, bytes.len()),
    }
}

fn blank_or_comment(line: &[u8]) -> bool {
    let trimmed = line
        .iter()
        .copied()
        .find(|b| !matches!(*b, b' ' | b'\t' | 0x0C | b'\r'));
    trimmed.is_none() || trimmed == Some(b'#')
}

fn cookie(line: &[u8]) -> Option<String> {
    let trimmed = line
        .iter()
        .copied()
        .skip_while(|b| matches!(*b, b' ' | b'\t' | 0x0C))
        .collect::<Vec<_>>();
    if !trimmed.starts_with(b"#") {
        return None;
    }
    for (pos, part) in trimmed.windows(6).enumerate() {
        if part != b"coding" {
            continue;
        }
        let rest = &trimmed[pos + 6..];
        if !matches!(rest.first().copied(), Some(b':' | b'=')) {
            continue;
        }
        let name = rest[1..]
            .iter()
            .copied()
            .skip_while(|b| matches!(*b, b' ' | b'\t'))
            .take_while(|b| b.is_ascii_alphanumeric() || matches!(*b, b'-' | b'_' | b'.'))
            .collect::<Vec<_>>();
        if !name.is_empty() {
            return Some(String::from_utf8_lossy(&name).into_owned());
        }
    }
    None
}

#[derive(Clone, Copy)]
struct Mark {
    scope: usize,
    role: IdentifierRole,
}

struct Scope {
    kind: RegionKind,
    // Local declarations, keyed by spelling, with earliest binding byte.
    defs: BTreeMap<String, u32>,
    excluded: HashSet<String>,
}

struct Collector {
    prefix: u32,
    scopes: Vec<Scope>,
    current_scope: usize,
    marks: HashMap<u32, Mark>,
    regions: Vec<Region>,
    region_stack: Vec<usize>,
    gaps: Vec<Gap>,
}

impl Collector {
    fn new(source_len: usize, prefix: u32) -> Self {
        Self {
            prefix,
            scopes: vec![Scope {
                kind: RegionKind::Module,
                defs: BTreeMap::new(),
                excluded: HashSet::new(),
            }],
            current_scope: 0,
            marks: HashMap::new(),
            regions: vec![Region {
                id: 0,
                parent: None,
                kind: RegionKind::Module,
                span: ByteSpan {
                    start: prefix,
                    end: prefix + source_len as u32,
                },
                name: None,
            }],
            region_stack: vec![0],
            gaps: Vec::new(),
        }
    }

    fn region(&mut self, kind: RegionKind, range: TextRange, name: Option<String>) -> usize {
        let id = self.regions.len();
        self.regions.push(Region {
            id,
            parent: self.region_stack.last().copied(),
            kind,
            span: bytespan(range, self.prefix),
            name,
        });
        id
    }

    fn with_region(&mut self, id: usize, visit: impl FnOnce(&mut Self)) {
        self.region_stack.push(id);
        visit(self);
        self.region_stack.pop();
    }

    fn with_scope(&mut self, kind: RegionKind, visit: impl FnOnce(&mut Self)) {
        let previous = self.current_scope;
        self.current_scope = self.scopes.len();
        self.scopes.push(Scope {
            kind,
            defs: BTreeMap::new(),
            excluded: HashSet::new(),
        });
        visit(self);
        self.current_scope = previous;
    }

    fn mark(&mut self, name: &str, range: TextRange, role: IdentifierRole) {
        let start = range.start().to_u32();
        if role == IdentifierRole::Binding {
            let defs = &mut self.scopes[self.current_scope].defs;
            defs.entry(name.to_owned())
                .and_modify(|first| *first = (*first).min(start))
                .or_insert(start);
        }
        self.marks.insert(
            start,
            Mark {
                scope: self.current_scope,
                role,
            },
        );
    }

    fn identifier(&mut self, id: &ast::Identifier, role: IdentifierRole) {
        self.mark(id.id.as_str(), id.range(), role);
    }

    fn parameters(&mut self, parameters: &ast::Parameters) {
        for entry in parameters.iter_non_variadic_params() {
            self.identifier(&entry.parameter.name, IdentifierRole::Binding);
        }
        if let Some(vararg) = &parameters.vararg {
            self.identifier(&vararg.name, IdentifierRole::Binding);
        }
        if let Some(kwarg) = &parameters.kwarg {
            self.identifier(&kwarg.name, IdentifierRole::Binding);
        }
    }

    // Defaults are evaluated in the *enclosing* scope, not the new function or
    // lambda scope. AST annotation evaluation depends on target and context;
    // annotation-only references are deliberately left unclassified.
    fn defaults_in_parent(&mut self, parameters: &ast::Parameters) {
        for entry in parameters.iter_non_variadic_params() {
            if let Some(default) = &entry.default {
                self.visit_expr(default);
            }
        }
    }

    fn body(&mut self, body: &[Stmt]) {
        for statement in body {
            self.visit_stmt(statement);
        }
    }

    fn gap(&mut self, kind: GapKind, range: TextRange, detail: &'static str) {
        self.gaps.push(Gap {
            kind,
            span: bytespan(range, self.prefix),
            detail,
        });
    }

    fn import_aliases(&mut self, aliases: &[ast::Alias]) {
        for alias in aliases {
            let binding = alias.asname.as_ref().unwrap_or(&alias.name);
            self.identifier(binding, IdentifierRole::Import);
            let name = binding.id.as_str().split('.').next().unwrap_or_default();
            self.scopes[self.current_scope]
                .excluded
                .insert(name.to_owned());
        }
    }

    fn comprehension(
        &mut self,
        range: TextRange,
        generators: &[ast::Comprehension],
        visit_output: impl FnOnce(&mut Self),
    ) {
        // Python evaluates the leftmost iterator outside the comprehension
        // scope (e.g. `[x for x in x]`). Other generators belong inside it.
        if let Some(first) = generators.first() {
            self.visit_expr(&first.iter);
        }
        let id = self.region(RegionKind::Comprehension, range, None);
        self.with_region(id, |this| {
            this.with_scope(RegionKind::Comprehension, |this| {
                for (i, generator) in generators.iter().enumerate() {
                    if i > 0 {
                        this.visit_expr(&generator.iter);
                    }
                    this.visit_expr(&generator.target);
                    for condition in &generator.ifs {
                        this.visit_expr(condition);
                    }
                }
                visit_output(this);
            });
        });
    }

    fn slot(&self, mark: Mark, name: &str) -> Option<LocalBinding> {
        let scope = &self.scopes[mark.scope];
        if !matches!(
            scope.kind,
            RegionKind::Function | RegionKind::Lambda | RegionKind::Comprehension
        ) || scope.excluded.contains(name)
        {
            return None;
        }
        let first = scope.defs.get(name)?;
        // A read can be classified only when the current scope has an AST
        // binding site. No implicit closure/import/member resolution.
        if !matches!(
            mark.role,
            IdentifierRole::Binding | IdentifierRole::Unresolved
        ) {
            return None;
        }
        let ordinal = scope
            .defs
            .iter()
            .filter(|(candidate, _)| !scope.excluded.contains(candidate.as_str()))
            .filter(|(_, offset)| **offset < *first)
            .count();
        Some(LocalBinding {
            scope_id: mark.scope,
            ordinal,
        })
    }

    fn region_for(&self, span: ByteSpan) -> usize {
        self.regions
            .iter()
            .filter(|r| r.span.start <= span.start && r.span.end >= span.end)
            .min_by_key(|r| (r.span.end - r.span.start, std::cmp::Reverse(r.id)))
            .map_or(0, |r| r.id)
    }
}

impl<'a> Visitor<'a> for Collector {
    fn visit_stmt(&mut self, stmt: &'a Stmt) {
        match stmt {
            Stmt::FunctionDef(node) => {
                // Function name is bound in the *enclosing* lexical scope.
                self.identifier(&node.name, IdentifierRole::Binding);
                self.defaults_in_parent(&node.parameters);
                let id = self.region(
                    RegionKind::Function,
                    node.range(),
                    Some(node.name.id.to_string()),
                );
                self.with_region(id, |this| {
                    this.with_scope(RegionKind::Function, |this| {
                        this.parameters(&node.parameters);
                        this.body(&node.body);
                    });
                });
            }
            Stmt::ClassDef(node) => {
                self.identifier(&node.name, IdentifierRole::Binding);
                if let Some(arguments) = &node.arguments {
                    self.visit_arguments(arguments);
                }
                let id = self.region(
                    RegionKind::Class,
                    node.range(),
                    Some(node.name.id.to_string()),
                );
                self.with_region(id, |this| {
                    this.with_scope(RegionKind::Class, |this| this.body(&node.body));
                });
            }
            Stmt::Import(node) => {
                self.import_aliases(&node.names);
                self.gap(
                    GapKind::UnresolvedImport,
                    node.range(),
                    "module identity not resolved; static package graph is a subsequent slice",
                );
            }
            Stmt::ImportFrom(node) => {
                self.import_aliases(&node.names);
                self.gap(
                    GapKind::UnresolvedImport,
                    node.range(),
                    "relative or absolute import target not resolved",
                );
            }
            Stmt::Global(node) => {
                for id in &node.names {
                    self.identifier(id, IdentifierRole::ExternalScope);
                    self.scopes[self.current_scope]
                        .excluded
                        .insert(id.id.to_string());
                    self.gap(
                        GapKind::GlobalOrNonlocal,
                        id.range(),
                        "global declaration does not provide a local binding",
                    );
                }
            }
            Stmt::Nonlocal(node) => {
                for id in &node.names {
                    self.identifier(id, IdentifierRole::ExternalScope);
                    self.scopes[self.current_scope]
                        .excluded
                        .insert(id.id.to_string());
                    self.gap(
                        GapKind::GlobalOrNonlocal,
                        id.range(),
                        "nonlocal closure target is not resolved",
                    );
                }
            }
            _ => {
                let kind = match stmt {
                    Stmt::If(_) => Some(RegionKind::If),
                    Stmt::For(_) => Some(RegionKind::For),
                    Stmt::While(_) => Some(RegionKind::While),
                    Stmt::With(_) => Some(RegionKind::With),
                    Stmt::Try(_) => Some(RegionKind::Try),
                    Stmt::Match(_) => Some(RegionKind::Match),
                    _ => None,
                };
                if let Some(kind) = kind {
                    let id = self.region(kind, stmt.range(), None);
                    self.with_region(id, |this| visitor::walk_stmt(this, stmt));
                } else {
                    visitor::walk_stmt(self, stmt);
                }
            }
        }
    }

    fn visit_expr(&mut self, expr: &'a Expr) {
        match expr {
            Expr::Name(name) => {
                let role = if name.ctx == ExprContext::Store {
                    IdentifierRole::Binding
                } else {
                    IdentifierRole::Unresolved
                };
                self.mark(name.id.as_str(), name.range(), role);
            }
            Expr::Attribute(attribute) => {
                self.identifier(&attribute.attr, IdentifierRole::Attribute);
                visitor::walk_expr(self, expr);
            }
            Expr::Call(call) => {
                let dispatch = match call.func.as_ref() {
                    Expr::Attribute(_) | Expr::Subscript(_) | Expr::Call(_) => true,
                    Expr::Name(n) => matches!(
                        n.id.as_str(),
                        "getattr"
                            | "setattr"
                            | "eval"
                            | "exec"
                            | "__import__"
                            | "globals"
                            | "locals"
                    ),
                    _ => false,
                };
                if dispatch {
                    self.gap(
                        GapKind::DynamicDispatch,
                        call.range(),
                        "reflective or receiver dispatch is not resolved",
                    );
                }
                visitor::walk_expr(self, expr);
            }
            Expr::Lambda(node) => {
                if let Some(parameters) = &node.parameters {
                    self.defaults_in_parent(parameters);
                }
                let id = self.region(RegionKind::Lambda, node.range(), None);
                self.with_region(id, |this| {
                    this.with_scope(RegionKind::Lambda, |this| {
                        if let Some(parameters) = &node.parameters {
                            this.parameters(parameters);
                        }
                        this.visit_expr(&node.body);
                    });
                });
            }
            Expr::ListComp(node) => {
                self.comprehension(node.range(), &node.generators, |this| {
                    this.visit_expr(&node.elt);
                });
            }
            Expr::SetComp(node) => {
                self.comprehension(node.range(), &node.generators, |this| {
                    this.visit_expr(&node.elt);
                });
            }
            Expr::Generator(node) => {
                self.comprehension(node.range(), &node.generators, |this| {
                    this.visit_expr(&node.elt);
                });
            }
            Expr::DictComp(node) => {
                self.comprehension(node.range(), &node.generators, |this| {
                    if let Some(key) = &node.key {
                        this.visit_expr(key);
                    }
                    this.visit_expr(&node.value);
                });
            }
            _ => visitor::walk_expr(self, expr),
        }
    }

    fn visit_pattern(&mut self, pattern: &'a Pattern) {
        match pattern {
            Pattern::MatchAs(node) => {
                if let Some(name) = &node.name {
                    self.identifier(name, IdentifierRole::Binding);
                }
            }
            Pattern::MatchStar(node) => {
                if let Some(name) = &node.name {
                    self.identifier(name, IdentifierRole::Binding);
                }
            }
            Pattern::MatchMapping(node) => {
                if let Some(name) = &node.rest {
                    self.identifier(name, IdentifierRole::Binding);
                }
            }
            _ => {}
        }
        visitor::walk_pattern(self, pattern);
    }
}

pub(super) fn extract(
    identity: SourceIdentity,
    source: &str,
    prefix: u32,
    limits: Limits,
) -> SourceFacts {
    let source_type = if identity.kind == FileKind::Stub {
        PySourceType::Stub
    } else {
        PySourceType::Python
    };
    let version = PythonVersion {
        major: identity.target.major,
        minor: identity.target.minor,
    };
    let options = ParseOptions::from(source_type).with_target_version(version);
    let parsed = parse_unchecked(source, options)
        .try_into_module()
        .expect("Python or stub ParseOptions must produce a module");

    if !parsed.has_valid_syntax() {
        return SourceFacts::empty(
            identity,
            Outcome::InvalidSyntax,
            Some(format!("Ruff parse error: {:?}", parsed.errors().first())),
        );
    }
    if !parsed.unsupported_syntax_errors().is_empty() {
        return SourceFacts::empty(
            identity,
            Outcome::UnsupportedGrammar,
            Some(format!(
                "syntax is outside configured Python target: {:?}",
                parsed.unsupported_syntax_errors().first()
            )),
        );
    }
    // Check the parser output budget *before* emitting any records. Ruff's
    // allocation is bounded by the input-byte cap; a worker will enforce a
    // hard time/RSS cap at the later integration boundary.
    if parsed.tokens().len() > limits.max_tokens.min(131_072) {
        return SourceFacts::empty(
            identity,
            Outcome::ResourceLimit,
            Some("Ruff token quota exceeded".into()),
        );
    }

    let mut collector = Collector::new(source.len(), prefix);
    for statement in parsed.suite() {
        collector.visit_stmt(statement);
    }

    if parsed.tokens().len()
        + collector.marks.len()
        + collector.regions.len()
        + collector.gaps.len()
        > limits.max_facts.min(262_144)
    {
        return SourceFacts::empty(
            identity,
            Outcome::ResourceLimit,
            Some("fact, region, or gap quota exceeded".into()),
        );
    }

    let mut facts = SourceFacts::empty(identity, Outcome::Complete, None);
    let mut unknown_lexical = false;
    let mut tokens = Vec::new();
    let mut identifiers = Vec::new();
    let mut comments = Vec::new();
    let mut indent_depth = 0u32;
    let mut starts_logical_line = true;

    for token in parsed.tokens() {
        let kind = token.kind();
        match kind {
            TokenKind::Newline => {
                starts_logical_line = true;
                continue;
            }
            TokenKind::Indent => {
                // The admitted parser token count is at most 131,072.
                indent_depth += 1;
                continue;
            }
            TokenKind::Dedent => {
                let Some(depth) = indent_depth.checked_sub(1) else {
                    return SourceFacts::empty(
                        facts.identity,
                        Outcome::InvalidSyntax,
                        Some("parser returned an unbalanced indentation stream".into()),
                    );
                };
                indent_depth = depth;
                continue;
            }
            TokenKind::NonLogicalNewline | TokenKind::EndOfFile => continue,
            _ => {}
        }
        let span = bytespan(token.range(), prefix);
        let token_text =
            match source.get(token.range().start().to_usize()..token.range().end().to_usize()) {
                Some(text) => text,
                None => {
                    return SourceFacts::empty(
                        facts.identity,
                        Outcome::InvalidSyntax,
                        Some("parser returned a non-UTF8-boundary token range".into()),
                    );
                }
            };
        let region_id = collector.region_for(span);
        if kind == TokenKind::Comment {
            comments.push(CommentFact {
                span,
                region_id,
                text: token_text.to_owned(),
            });
            continue;
        }
        let category = if kind == TokenKind::Identifier {
            TokenCategory::Identifier
        } else if kind.is_keyword() {
            TokenCategory::Keyword
        } else if matches!(kind, TokenKind::Int | TokenKind::Float | TokenKind::Complex) {
            TokenCategory::Number
        } else if matches!(
            kind,
            TokenKind::String
                | TokenKind::FStringStart
                | TokenKind::FStringMiddle
                | TokenKind::FStringEnd
                | TokenKind::TStringStart
                | TokenKind::TStringMiddle
                | TokenKind::TStringEnd
        ) {
            TokenCategory::String
        } else if kind.is_operator() {
            TokenCategory::OperatorOrPunctuation
        } else {
            TokenCategory::Other
        };
        let mut renamed_local = token_text.to_owned();
        if category == TokenCategory::Identifier {
            let mark = collector
                .marks
                .get(&token.range().start().to_u32())
                .copied();
            let local = mark.and_then(|mark| collector.slot(mark, token_text));
            let role = match (mark, local) {
                (
                    Some(Mark {
                        role: IdentifierRole::Binding,
                        ..
                    }),
                    _,
                ) => IdentifierRole::Binding,
                (
                    Some(Mark {
                        role: IdentifierRole::Unresolved,
                        ..
                    }),
                    Some(_),
                ) => IdentifierRole::LocalReference,
                (Some(mark), _) => mark.role,
                (None, _) => IdentifierRole::Unresolved,
            };
            if let Some(slot) = local {
                renamed_local = format!("$L{}", slot.ordinal);
            } else if role == IdentifierRole::Unresolved {
                unknown_lexical = true;
            }
            identifiers.push(IdentifierFact {
                name: token_text.to_owned(),
                span,
                region_id,
                role,
                local,
                spelling_candidate: true,
            });
        }
        tokens.push(TokenFact {
            span,
            region_id,
            category,
            layout: TokenLayout { indent_depth, starts_logical_line },
            exact: token_text.to_owned(),
            renamed_local,
        });
        starts_logical_line = false;
    }

    if tokens.len()
        + identifiers.len()
        + comments.len()
        + collector.regions.len()
        + collector.gaps.len()
        > limits.max_facts.min(262_144)
    {
        return SourceFacts::empty(
            facts.identity,
            Outcome::ResourceLimit,
            Some("materialized fact quota exceeded".into()),
        );
    }

    let partial = unknown_lexical || !collector.gaps.is_empty();
    let relevant = !tokens.is_empty() || !identifiers.is_empty() || !comments.is_empty();
    facts.outcome = if !relevant {
        Outcome::NoRelevantFacts
    } else if partial {
        Outcome::Partial
    } else {
        Outcome::Complete
    };
    facts.coverage = Capabilities {
        clone_tokens: Coverage::Complete,
        spelling_identifiers: Coverage::Complete,
        comments: Coverage::Complete,
        declaration_ranges: Coverage::Complete,
        local_renaming: if partial {
            Coverage::Partial
        } else {
            Coverage::Complete
        },
    };
    facts.regions = collector.regions;
    facts.gaps = collector.gaps;
    facts.tokens = tokens;
    facts.identifiers = identifiers;
    facts.comments = comments;
    if unknown_lexical {
        // Free/closure name occurrences are present with Unresolved roles;
        // avoid creating a duplicate gap per identifier. Coverage signals
        // the unverified frontier for renamed-local normalization.
        facts.reason = Some("unresolved lexical or closure references remain".into());
    }
    facts
}
