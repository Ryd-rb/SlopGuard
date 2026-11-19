//! Explicit, conservative projections from the four pinned provider APIs.
//! A lexical token never becomes a semantic local unless the provider proves it.
use crate::contract::{ByteSpan, DocumentReport, EXACT, Gap, Loaded, NEAR, PYTHON_EXACT, PYTHON_RENAMED, RENAMED, Rule, ScopeRequest, Status};
use crate::worker;
use slophawk_clone_index::{CloneSourceSpan, CloneToken, CloneTokenBlock, CloneTokenKind};
use slophawk_fluxr_provider as fluxr;
use slophawk_python_syntax_provider as python;
use slophawk_rust_source_provider as rust_source;
use std::sync::atomic::AtomicBool;
use std::collections::BTreeMap;

pub struct Projection {
    pub report: DocumentReport,
    pub blocks: Vec<CloneTokenBlock>,
    pub original_byte_spans: BTreeMap<String, ByteSpan>,
    pub python_layouts: BTreeMap<String, Vec<python::TokenLayout>>,
}

fn gap(out: &mut DocumentReport, reason: impl Into<String>, span: Option<ByteSpan>) {
    out.gaps.push(Gap {reason: reason.into(), document_id: out.document_id.clone(), span});
}
fn rule(report: &mut DocumentReport, id: &str, status: Status, scope: &str, reason: &str) {
    report.rules.push(Rule { id: id.into(), status, scope: scope.into(), reason: reason.into() });
}
fn initial(job: &ScopeRequest, source: &Loaded) -> DocumentReport {
    DocumentReport {
        document_id: source.source.document_id.clone(), provider: source.source.language.provider().into(),
        language: source.source.language, context_key: job.context.key.clone(),
        source_sha256: source.digest.clone(), status: Status::Complete,
        rules: Vec::new(), gaps: Vec::new(), blocks_indexed: 0, tokens_indexed: 0,
    }
}
fn finish(mut report: DocumentReport, mapped: Vec<(CloneTokenBlock, ByteSpan)>) -> Projection {
    let mut blocks = Vec::with_capacity(mapped.len());
    let mut original_byte_spans = BTreeMap::new();
    for (block, span) in mapped {
        original_byte_spans.insert(block.block_id.clone(), span);
        blocks.push(block);
    }
    report.blocks_indexed = blocks.len();
    report.tokens_indexed = blocks.iter().map(|b| b.tokens.len()).sum();
    Projection { report, blocks, original_byte_spans, python_layouts: BTreeMap::new() }
}
fn unsupported(report: &mut DocumentReport, reason: &str) {
    let (exact, renamed) = report.language.clone_rules();
    rule(report, exact, Status::Unsupported, "original source byte block", reason);
    rule(report, renamed, Status::Unsupported, "proven local bindings within a block", reason);
    rule(report, NEAR, Status::Unsupported, "near-clone edit distance", "clone-index v1 does not implement near-clone alignment");
}
fn no_near(report: &mut DocumentReport) {
    rule(report, NEAR, Status::Unsupported, "near-clone edit distance", "clone-index v1 supports only exact and consistently renamed-local fingerprints");
}
fn bytespan(start: usize, end: usize) -> ByteSpan { ByteSpan { start, end } }
fn intersects_or_point(a: &ByteSpan, b: &ByteSpan) -> bool {
    if a.start == a.end { b.start <= a.start && a.start <= b.end }
    else if b.start == b.end { a.start <= b.start && b.start <= a.end }
    else { a.start < b.end && b.start < a.end }
}
fn utf16_at(source: &[u8], pos: usize) -> Option<i32> {
    let bom = if source.starts_with(&[0xef, 0xbb, 0xbf]) { 3 } else { 0 };
    if pos < bom { return if pos == 0 { Some(0) } else { None }; }
    let text = std::str::from_utf8(source.get(bom..pos)?).ok()?;
    i32::try_from(text.encode_utf16().count()).ok()
}
fn clone_span(source: &[u8], span: &ByteSpan) -> Option<CloneSourceSpan> {
    if !span.verified(source) { return None; }
    Some(CloneSourceSpan {
        start_offset: utf16_at(source, span.start)?,
        end_offset: utf16_at(source, span.end)?,
    })
}
fn block_from_tokens(
    source: &Loaded, block_id: &str, span: &ByteSpan,
    tokens: &[(ByteSpan, CloneToken)],
) -> Option<(CloneTokenBlock, ByteSpan)> {
    let clone_span = clone_span(&source.bytes, span)?;
    let tokens = tokens.iter().filter(|(s,_)| s.start >= span.start && s.end <= span.end)
        .map(|(_, t)| t.clone()).collect();
    Some((CloneTokenBlock { block_id: block_id.into(), span: clone_span, tokens }, span.clone()))
}
fn validate_token(source: &Loaded, span: &ByteSpan, exact: &str) -> bool {
    span.verified(&source.bytes) && std::str::from_utf8(&source.bytes[span.start..span.end]) == Ok(exact)
}
fn fail(source: &Loaded, job: &ScopeRequest, reason: &str, status: Status) -> Projection {
    let mut report = initial(job, source);
    report.status = status;
    gap(&mut report, reason, None);
    unsupported(&mut report, reason);
    finish(report, Vec::new())
}

pub fn python(job: &ScopeRequest, source: &Loaded) -> Projection {
    let request = python::SourceRequest { document_id: &source.source.document_id,
        path: &source.source.logical_path, bytes: &source.bytes,
        target: python::PythonTarget { major: 3, minor: job.context.python_minor },
        limits: python::Limits::default() };
    let facts = python::analyze(request);
    if facts.identity.provider != python::PROVIDER_ID
        || facts.identity.schema != python::SCHEMA_VERSION
        || facts.identity.path != source.source.logical_path
        || facts.identity.original_bytes != source.bytes.len()
        || facts.identity.source_sha256 != source.digest
        || facts.identity.document_id != source.source.document_id {
        return fail(source, job, "Python provider provenance, path or source digest mismatch", Status::Failed);
    }
    // A valid source may contain only layout, a BOM, or comments. Preserve the
    // provider's known-empty outcome instead of failing every peer in the job.
    // Do not construct an empty clone block (a BOM-only unit has no UTF-16 span).
    if matches!(facts.outcome, python::Outcome::Complete | python::Outcome::NoRelevantFacts)
        && facts.coverage.clone_tokens == python::Coverage::Complete
        && facts.tokens.is_empty() && facts.gaps.is_empty() {
        let mut report = initial(job, source);
        rule(&mut report, PYTHON_EXACT, Status::Complete,
            "Python source unit without substantive tokens",
            "Complete token extraction found no substantive clone tokens; no blocks submitted");
        rule(&mut report, PYTHON_RENAMED, Status::Unsupported,
            "AST-proven local occurrences only", "No local-binding occurrences to compare");
        no_near(&mut report);
        return finish(report, Vec::new());
    }
    if !matches!(facts.outcome, python::Outcome::Complete | python::Outcome::Partial) {
        let status = match facts.outcome {
            python::Outcome::UnsupportedExtension | python::Outcome::UnsupportedGrammar | python::Outcome::UnsupportedEncoding => Status::Unsupported,
            _ => Status::Failed,
        };
        return fail(source, job, facts.reason.as_deref().unwrap_or("Python source analysis not complete"), status);
    }
    let mut report = initial(job, source);
    let mut tokens = Vec::with_capacity(facts.tokens.len());
    for t in &facts.tokens {
        let span = bytespan(t.span.start as usize, t.span.end as usize);
        if !validate_token(source, &span, &t.exact) { return fail(source, job, "Python fact has unmapped or mismatched byte span", Status::Failed); }
        let kind = if t.category == python::TokenCategory::Identifier && t.renamed_local != t.exact {
            CloneTokenKind::LocalIdentifier
        } else if matches!(t.category, python::TokenCategory::String | python::TokenCategory::Number) {
            CloneTokenKind::Literal
        } else { CloneTokenKind::Other };
        tokens.push((span, CloneToken { kind, text: t.exact.clone() }));
    }
    for g in &facts.gaps { gap(&mut report, format!("python:{:?}:{}", g.kind, g.detail), Some(bytespan(g.span.start as usize, g.span.end as usize))); }
    let ranges = facts.regions.iter().filter(|r| matches!(r.kind, python::RegionKind::Function | python::RegionKind::Class)).collect::<Vec<_>>();
    let mut blocks = Vec::new();
    let mut layouts = BTreeMap::new();
    if ranges.is_empty() {
        if let Some(block) = block_from_tokens(source, "document", &bytespan(0,source.bytes.len()), &tokens) {
            let Some(layout) = facts.clone_layout(0) else {
                return fail(source, job, "Python document layout is invalid", Status::Failed);
            };
            layouts.insert("document".into(), layout);
            blocks.push(block);
        }
    } else {
        for region in ranges {
            let span = bytespan(region.span.start as usize, region.span.end as usize);
            let block_id = format!("region:{}", region.id);
            if let Some(block) = block_from_tokens(source, &block_id, &span, &tokens) {
                let Some(layout) = facts.clone_layout(region.id) else {
                    return fail(source, job, "Python region layout is invalid", Status::Failed);
                };
                layouts.insert(block_id, layout);
                blocks.push(block);
            }
        }
    }
    let token_complete = facts.coverage.clone_tokens == python::Coverage::Complete;
    let local_capable = facts.coverage.local_renaming == python::Coverage::Complete;
    if !local_capable {
        for (_, token) in &mut tokens {
            if token.kind == CloneTokenKind::LocalIdentifier {
                token.kind = CloneTokenKind::Other;
            }
        }
        for (block, _) in &mut blocks {
            for token in &mut block.tokens {
                if token.kind == CloneTokenKind::LocalIdentifier {
                    token.kind = CloneTokenKind::Other;
                }
            }
        }
    }
    if facts.outcome == python::Outcome::Partial {
        report.status = Status::Partial;
        gap(&mut report, format!("python:partial_local_bindings:{}",
            facts.reason.as_deref().unwrap_or("unresolved names")), None);
    }
    // A partial token stream can create false exact matches between only the
    // emitted prefixes. Do not publish *any* block from that stream.
    if !token_complete { blocks.clear(); layouts.clear(); }
    if !token_complete || blocks.is_empty() {
        report.status = Status::Partial;
        gap(&mut report, "python:missing_exact_block_or_token_capability", None);
    }
    if !local_capable {
        gap(&mut report, "python:local_binding_frontier", None);
    }
    rule(&mut report, PYTHON_EXACT, if token_complete && !blocks.is_empty() { Status::Complete } else { Status::Unsupported },
        "Python function/class or complete source unit", "Ruff original-byte tokens plus equal relative indentation and logical-line boundaries");
    let has_local_evidence = blocks.iter().flat_map(|(b, _)| b.tokens.iter())
        .any(|t| t.kind == CloneTokenKind::LocalIdentifier);
    rule(&mut report, PYTHON_RENAMED, if token_complete && local_capable && has_local_evidence { Status::Complete } else { Status::Unsupported },
        "AST-proven local occurrences with equal Python layout", "Unresolved and external-scope names remain exact; no semantic import binding is inferred; at least one proven local required");
    no_near(&mut report);
    let mut projection = finish(report, blocks);
    projection.python_layouts = layouts;
    projection
}

pub fn rust(job: &ScopeRequest, source: &Loaded) -> Projection {
    let edition = match job.context.rust_edition.as_str() {
        "2015" => rust_source::SourceEdition::E2015, "2018" => rust_source::SourceEdition::E2018,
        "2021" => rust_source::SourceEdition::E2021, "2024" => rust_source::SourceEdition::E2024,
        _ => return fail(source, job, "unsupported Rust edition", Status::Unsupported),
    };
    let text = match std::str::from_utf8(&source.bytes) { Ok(t) => t, Err(_) => return fail(source, job, "Rust input is not UTF-8", Status::Unsupported) };
    let facts = match rust_source::analyze_source(edition, &source.source.document_id, text, rust_source::Limits::default()) {
        Ok(f) => f, Err(e) => return fail(source, job, &format!("Rust provider: {e}"), Status::Failed),
    };
    if facts.identity.provider != rust_source::PROVIDER_ID
        || facts.identity.schema_version != rust_source::SCHEMA_VERSION
        || facts.identity.source_bytes != source.bytes.len()
        || facts.identity.edition != edition
        || facts.identity.sha256 != source.digest
        || facts.identity.logical_document != source.source.document_id {
        return fail(source, job, "Rust provider provenance, edition or source digest mismatch", Status::Failed);
    }
    let mut report = initial(job, source);
    let mut tokens = Vec::with_capacity(facts.tokens.len());
    for t in &facts.tokens {
        let span = bytespan(t.span.start as usize, t.span.end as usize);
        if !validate_token(source, &span, &t.text) { return fail(source, job, "Rust fact has unmapped or mismatched byte span", Status::Failed); }
        let kind = if t.clone_key != t.text { CloneTokenKind::LocalIdentifier }
            else if t.kind.contains("STRING") || t.kind.contains("NUMBER") || t.kind.contains("CHAR") { CloneTokenKind::Literal }
            else { CloneTokenKind::Other };
        tokens.push((span, CloneToken {kind, text:t.text.clone()}));
    }
    for f in &facts.unresolved { gap(&mut report, format!("rust:{:?}:{}", f.kind, f.reason), Some(bytespan(f.span.start as usize, f.span.end as usize))); }
    for d in &facts.diagnostics { gap(&mut report, format!("rust:parse:{}", d.message), Some(bytespan(d.span.start as usize, d.span.end as usize))); }
    if facts.truncated { gap(&mut report, "rust:fact_limit", None); }
    let mut blocks = Vec::new();
    for block in facts.blocks.iter().filter(|b| b.kind == rust_source::BlockKind::Expression) {
        let span = bytespan(block.span.start as usize, block.span.end as usize);
        // Unknown macro/cfg/generated/parse areas may not become 'clean' clone results.
        if facts.truncated || facts.unresolved.iter().any(|f|
                intersects_or_point(&bytespan(f.span.start as usize, f.span.end as usize), &span))
            || facts.diagnostics.iter().any(|d|
                intersects_or_point(&bytespan(d.span.start as usize, d.span.end as usize), &span)) {
            continue;
        }
        if let Some(b) = block_from_tokens(source, &format!("block:{}-{}", span.start, span.end), &span, &tokens) { blocks.push(b); }
    }
    if blocks.is_empty() && !facts.truncated && facts.unresolved.is_empty() && facts.diagnostics.is_empty() {
        if let Some(b) = block_from_tokens(source, "document", &bytespan(0,source.bytes.len()), &tokens) { blocks.push(b); }
    }
    let clean = !facts.truncated && facts.unresolved.is_empty() && facts.diagnostics.is_empty();
    if !clean || blocks.is_empty() { report.status = Status::Partial; }
    rule(&mut report, EXACT, if blocks.is_empty() { Status::Unsupported } else if clean { Status::Complete } else { Status::Partial },
        "Rust syntactic block without unresolved frontiers", "rust-analyzer token stream; macro/cfg/parse regions excluded");
    let has_local_evidence = blocks.iter().flat_map(|(b, _)| b.tokens.iter())
        .any(|t| t.kind == CloneTokenKind::LocalIdentifier);
    rule(&mut report, RENAMED, if !has_local_evidence { Status::Unsupported } else if clean { Status::Complete } else { Status::Partial },
        "only simple same-body proven let bindings", "No semantic trait/path resolution; all other identifiers retain exact spelling; at least one proven local required");
    no_near(&mut report);
    finish(report, blocks)
}

pub fn fluxr(job: &ScopeRequest, source: &Loaded) -> Projection {
    let module = match &job.context.fluxr_module { Some(s) if !s.trim().is_empty() => s, _ => return fail(source, job, "FluxR requires an explicit module context", Status::Unsupported) };
    if let Some(version) = job.context.fluxr_compact_version {
        if let Err(error) = fluxr::validate_compact_fragment(&source.bytes, version) {
            return fail(source, job, &format!("FluxR compact fragment: {error}"), Status::Failed);
        }
        let mut report = initial(job, source);
        report.status = Status::Partial;
        rule(&mut report, "fluxr.compact-syntax.v1/check", Status::Complete,
            "declared compact fragment", "Parsed using the pinned FluxR compiler");
        gap(&mut report, "fluxr:compact_fragment_type_and_token_scope_unavailable", None);
        unsupported(&mut report, "Compact fragment parsing does not provide mapped checked tokens");
        return finish(report, Vec::new());
    }
    if source.source.logical_path.ends_with(".flux") {
        return fail(source, job, "Compact .flux sources require an explicit module version", Status::Unsupported);
    }
    let facts = match fluxr::analyze(fluxr::Request {
        logical_document_id: &source.source.logical_path, declared_module_context: module,
        source_bytes: &source.bytes, format: fluxr::SourceFormat::CanonicalChecked, limits: fluxr::Limits::default(),
    }) { Ok(f) => f, Err(e) => return fail(source, job, &format!("FluxR checked source: {}", e.message), Status::Failed) };
    if facts.provenance.provider_id != fluxr::PROVIDER_ID
        || facts.provenance.schema_version != fluxr::SCHEMA_VERSION
        || facts.provenance.source_byte_len != source.bytes.len()
        || facts.provenance.declared_module_context.as_str() != module.as_str()
        || facts.provenance.source_sha256 != source.digest
        || facts.provenance.logical_document_id != source.source.logical_path {
        return fail(source, job, "FluxR provider provenance, context or source digest mismatch", Status::Failed);
    }
    let mut report = initial(job, source);
    for d in &facts.declarations {
        if d.span.is_none() { gap(&mut report, format!("fluxr:missing_declaration_range:{:?}", d.kind), None); }
    }
    for n in &facts.native_items {
        gap(&mut report, format!("fluxr:opaque_native_item:{}", n.tag), Some(bytespan(n.span.start, n.span.end)));
    }
    // The upstream provider exports lexical checked tokens, not per-function blocks or bindings.
    // A complete native-free checked *source unit* is an admitted exact unit; no subfunction
    // or renamed-local assertions are made.
    let mut blocks = Vec::new();
    let lexical_complete = matches!(facts.gate(&[fluxr::Capability::CheckedSyntax,
        fluxr::Capability::LexicalCheckedTokens]), fluxr::RuleGate::Eligible);
    if !lexical_complete { gap(&mut report, "fluxr:checked_token_capability_incomplete", None); }
    if facts.native_items.is_empty() && lexical_complete && source.source.byte_mapping == "exactUtf8" {
        let mut tokens = Vec::new();
        for t in &facts.tokens {
            let span = bytespan(t.span.start, t.span.end);
            if !validate_token(source, &span, &t.text) { return fail(source, job, "FluxR checked token unmapped", Status::Failed); }
            tokens.push((span, CloneToken {kind: CloneTokenKind::Other, text:t.text.clone()}));
        }
        if let Some(block) = block_from_tokens(source, "checked-source-unit", &bytespan(0,source.bytes.len()), &tokens) { blocks.push(block); }
    }
    if blocks.is_empty() || !report.gaps.is_empty() { report.status = Status::Partial; }
    rule(&mut report, EXACT, if blocks.is_empty() { Status::Unsupported } else { Status::Complete },
        "complete canonical checked document only", "Whitespace tokens; no declaration/block ranges and no native payloads");
    rule(&mut report, RENAMED, Status::Unsupported, "semantic local-binding occurrences", "FluxR checked inventory has no occurrence bindings or declaration byte ranges");
    no_near(&mut report);
    finish(report, blocks)
}

pub fn csharp(job: &ScopeRequest, source: &Loaded, csharp_sources: &[&Loaded], cancel: &AtomicBool) -> Projection {
    let facts = match worker::run(job, csharp_sources, source, cancel) {
        Ok(f) => f, Err(e) => return fail(source, job, &format!("csharp:worker:{e}"), Status::Failed),
    };
    let mut report = initial(job, source);
    for g in facts.gaps { gap(&mut report, format!("csharp:{g}"), None); }
    let mut blocks = Vec::new();
    let complete_mapping = source.source.byte_mapping == "exactUtf8" && facts.spans_complete
        && facts.tokens_complete && facts.tokens.iter().all(|t| !t.truncated && t.span.is_some());
    if complete_mapping {
        let mut tokens = Vec::new();
        for t in &facts.tokens {
            let span = match &t.span { Some(s) => s.clone(), None => continue };
            tokens.push((span, CloneToken {
                kind: if t.category == "literal" { CloneTokenKind::Literal } else { CloneTokenKind::Other },
                text: t.text.clone(),
            }));
        }
        if let Some(block) = block_from_tokens(source, "source-unit", &bytespan(0,source.bytes.len()), &tokens) { blocks.push(block); }
    } else { gap(&mut report, "csharp:no_complete_mapped_untruncated_token_stream", None); }
    if facts.partial || !complete_mapping { report.status = Status::Partial; }
    rule(&mut report, EXACT, if blocks.is_empty() { Status::Unsupported } else { Status::Complete },
        "complete C# source unit only", "worker syntax token frames with original-byte mapping; worker does not expose complete method blocks");
    rule(&mut report, RENAMED, Status::Unsupported, "Roslyn-resolved local symbols", "worker token localRole is syntax-only and cannot establish a local binding");
    no_near(&mut report);
    finish(report, blocks)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn utf16_coordinates_preserve_crlf_and_supplementary_characters() {
        let bytes = "a🌙\r\nb".as_bytes();
        assert_eq!(utf16_at(bytes, 5), Some(3));
        assert_eq!(utf16_at(bytes, 7), Some(5));
        assert_eq!(utf16_at(bytes, 2), None);
    }
}
