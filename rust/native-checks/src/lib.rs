//! SG-MIG-02: opt-in one-shot native shadow checker.
//! No changes to the existing C# CLI, daemon, watcher, publication or test-runner.
#![deny(unsafe_code)]
pub mod contract;
pub mod base64;
mod near;
mod providers;
mod worker;
mod supervisor;

use contract::{CloneQueryNotRunReason, CloneQueryReport, CloneQueryStatus, DocumentReport, Gap, Language, Loaded, Pair, PYTHON_CLONE_VERSION, Report, ScopeRequest, Status};
use slophawk_clone_index::{CloneIndexSettings, CloneMatchClass, CloneQueryLimits, CloneTokenDocument, TokenCloneIndex};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::atomic::AtomicBool;
use std::time::Instant;

/// Included in resident receipt identity whenever native checker semantics change.
pub const SEMANTICS_VERSION: &str = "slophawk.native-checks/4";

pub fn shadow(request: ScopeRequest, cancel: &AtomicBool) -> Result<Report, String> {
    let started = Instant::now();
    let (job, mut sources) = request.admit()?;
    // Sorting makes worker closure and index updates independent of enumeration order.
    sources.sort_by(|a,b| a.source.document_id.cmp(&b.source.document_id));
    let csharp_sources: Vec<&Loaded> = sources.iter().filter(|s| s.source.language == Language::Csharp).collect();
    let attempted_workers = csharp_sources.len();
    let mut projections = Vec::new();
    for source in &sources {
        if cancel.load(std::sync::atomic::Ordering::SeqCst) {
            return Err("shadow job cancelled before publication".into());
        }
        let mut p = match source.source.language {
            Language::Python => providers::python(&job, source),
            Language::Rust => providers::rust(&job, source),
            Language::Fluxr => providers::fluxr(&job, source),
            Language::Csharp => providers::csharp(&job, source, &csharp_sources, cancel),
        };
        if source.source.byte_mapping == "unavailable" && source.source.language != Language::Csharp {
            p.blocks.clear();
            p.original_byte_spans.clear();
            p.python_layouts.clear();
            p.report.status = Status::Partial;
            p.report.blocks_indexed = 0;
            p.report.tokens_indexed = 0;
            p.report.gaps.push(Gap {
                reason: "caller_declared_unmapped: no original byte coordinates admitted".into(),
                document_id: source.source.document_id.clone(), span: None,
            });
            for r in &mut p.report.rules {
                let (exact, renamed) = source.source.language.clone_rules();
                if r.id == exact || r.id == renamed { r.status = Status::Unsupported; }
            }
        }
        projections.push(p);
    }
    // A failed worker, stale terminal, overflow, or source-provider failure
    // aborts the *whole* staged operation, not just the failing document.
    // The receipt retains failures/frontiers but publishes no clone pairs.
    let failed_batch = projections.iter().any(|p| p.report.status == Status::Failed);
    if failed_batch {
        for p in &mut projections {
            p.blocks.clear();
            p.original_byte_spans.clear();
            p.python_layouts.clear();
            p.report.blocks_indexed = 0;
            p.report.tokens_indexed = 0;
            if p.report.status != Status::Failed {
                p.report.status = Status::Partial;
                p.report.gaps.push(Gap { reason: "job:peer_failed_staged_facts_discarded".into(),
                    document_id: p.report.document_id.clone(), span: None });
                for r in &mut p.report.rules {
                    if r.status == Status::Complete { r.status = Status::Partial; }
                }
            }
        }
    }
    let mut clone_queries = BTreeMap::new();
    for source in &sources {
        clone_queries.entry(source.source.language).or_insert_with(|| CloneQueryReport {
            language: source.source.language,
            rule_version: slophawk_clone_index::RULE_VERSION,
            requested_documents: 0,
            status: CloneQueryStatus::NotRun,
            not_run_reason: Some(if failed_batch { CloneQueryNotRunReason::FailedBatch }
                else { CloneQueryNotRunReason::NoAdmittedBlocks }),
            statistics: None,
            gap_reasons: Vec::new(),
            pairs_returned: 0,
            pairs_accepted: 0,
            pairs_retained: 0,
        }).requested_documents += 1;
    }
    let mut indices: BTreeMap<Language, TokenCloneIndex> = BTreeMap::new();
    let mut locations: BTreeMap<(Language, String, String), contract::ByteSpan> = BTreeMap::new();
    let mut python_layouts = BTreeMap::new();
    let mut all_gaps = Vec::new();
    for p in &projections {
        all_gaps.extend(p.report.gaps.clone());
    }
    for (source, projected) in sources.iter().zip(&mut projections) {
        for (block_id, layout) in std::mem::take(&mut projected.python_layouts) {
            python_layouts.insert((source.source.document_id.clone(), block_id), layout);
        }
        for (block_id, byte_span) in &projected.original_byte_spans {
            locations.insert((source.source.language, source.source.document_id.clone(), block_id.clone()), byte_span.clone());
        }
        if projected.blocks.is_empty() || projected.report.status == Status::Failed { continue; }
        let index = if let Some(index) = indices.get_mut(&source.source.language) { index } else {
            indices.insert(source.source.language, TokenCloneIndex::new(CloneIndexSettings {
                minimum_tokens: job.limits.min_clone_tokens,
                maximum_tokens_per_block: job.limits.max_tokens_per_block,
                maximum_fingerprint_postings: job.limits.max_fingerprint_postings,
            }).map_err(|e| format!("clone settings: {e}"))?);
            indices.get_mut(&source.source.language).ok_or("missing fresh clone index")?
        };
        index.upsert(CloneTokenDocument {
            document_id: source.source.document_id.clone(), version: source.digest.clone(),
            is_generated: source.source.provenance == contract::Provenance::Generated,
            is_excluded: false, blocks: projected.blocks.clone(),
        }).map_err(|e| format!("clone upsert for {}: {e}", source.source.document_id))?;
    }
    let mut pairs = Vec::new();
    let mut seen = BTreeSet::new();
    for (language, index) in &indices {
        let query = index.query(CloneQueryLimits {
            maximum_candidate_pairs: job.limits.max_candidate_pairs,
            maximum_output_pairs: job.limits.max_output_pairs,
            maximum_compared_tokens: job.limits.max_compared_tokens,
        }).map_err(|e| format!("clone query: {e}"))?;
        let row = clone_queries.get_mut(language).ok_or("clone query has no requested language")?;
        row.status = if query.gaps.is_empty() { CloneQueryStatus::Complete } else { CloneQueryStatus::Partial };
        row.not_run_reason = None;
        row.gap_reasons = query.gaps.iter().map(|gap| contract::CloneQueryGapReason::from(gap.reason))
            .collect::<BTreeSet<_>>().into_iter().collect();
        row.statistics = Some(query.statistics.into());
        row.pairs_returned = query.pairs.len();
        for g in query.gaps {
            all_gaps.push(Gap { reason: format!("clone-index:{:?}:{}:{}",g.reason,g.scope_key,g.affected_count),
                document_id: "*".into(), span: None });
        }
        for p in query.pairs {
            // The shared index nominates lexical matches. Python also requires
            // suite/statement layout equality; never encode layout as fake text.
            // Rejected pairs still consume the query's existing budgets.
            if *language == Language::Python {
                let left = python_layouts.get(&(p.left.document_id.clone(), p.left.block_id.clone()))
                    .ok_or("Python clone candidate has no left layout")?;
                let right = python_layouts.get(&(p.right.document_id.clone(), p.right.block_id.clone()))
                    .ok_or("Python clone candidate has no right layout")?;
                if left.len() != p.left.token_count || right.len() != p.right.token_count {
                    return Err("Python layout length does not match clone candidate".into());
                }
                if left != right { continue; }
            }
            let pair_id = if *language == Language::Python {
                let suffix = p.pair_id.strip_prefix(slophawk_clone_index::RULE_VERSION)
                    .ok_or("clone index returned an unexpected rule version")?;
                format!("{PYTHON_CLONE_VERSION}{suffix}")
            } else { p.pair_id };
            let classification = match p.match_class {
                CloneMatchClass::Exact => "exact",
                CloneMatchClass::RenamedLocalIdentifiers => "renamed_proven_local",
            };
            let key = (*language, pair_id.clone());
            if !seen.insert(key) { return Err("clone checker produced duplicate pair identity".into()); }
            let left_bytes = locations.get(&(*language, p.left.document_id.clone(), p.left.block_id.clone()))
                .ok_or("clone index returned unknown left block identity")?;
            let right_bytes = locations.get(&(*language, p.right.document_id.clone(), p.right.block_id.clone()))
                .ok_or("clone index returned unknown right block identity")?;
            row.pairs_accepted += 1;
            row.pairs_retained += 1;
            pairs.push(Pair {
                pair_id, language: *language, classification: classification.into(),
                left_bytes: [left_bytes.start, left_bytes.end],
                right_bytes: [right_bytes.start, right_bytes.end],
                left_document: p.left.document_id, right_document: p.right.document_id,
                left_block: p.left.block_id, right_block: p.right.block_id,
                left_utf16: [p.left.span.start_offset, p.left.span.end_offset],
                right_utf16: [p.right.span.start_offset, p.right.span.end_offset],
                token_count: p.left.token_count,
            });
        }
    }
    let near_miss_clones = if job.near_miss_clones {
        near::collect(&mut projections, &indices, &locations, &python_layouts, failed_batch, cancel)?
    } else {
        Vec::new()
    };
    pairs.sort_by(|a,b| (a.language, &a.pair_id).cmp(&(b.language,&b.pair_id)));
    all_gaps.sort_by(|a,b| (&a.document_id, &a.reason, a.span.as_ref().map(|s| s.start))
        .cmp(&(&b.document_id,&b.reason,b.span.as_ref().map(|s|s.start))));
    let documents: Vec<DocumentReport> = projections.into_iter().map(|p| p.report).collect();
    let status = if documents.iter().any(|d| d.status == Status::Failed) { Status::Failed }
        else if documents.iter().any(|d| d.status != Status::Complete ||
            d.rules.iter().any(|r| r.status != Status::Complete)) || !all_gaps.is_empty() {
            Status::Partial
        } else { Status::Complete };
    Ok(Report {
        schema: "slophawk.native-shadow/1", workspace_epoch: job.workspace_epoch,
        committed_revision: job.committed_revision, context_key: job.context.key,
        status, documents, pairs, gaps: all_gaps,
        clone_queries: clone_queries.into_values().collect(),
        near_miss_clones,
        limits: job.limits, worker_invocations: attempted_workers,
        elapsed_ms: started.elapsed().as_millis(),
    })
}

/// Produce a bounded deterministic JSON receipt. Limit truncation never silently drops findings.
pub fn bounded_report(mut report: Report) -> Result<Vec<u8>, String> {
    let maximum = report.limits.max_report_bytes;
    loop {
        let encoded = serde_json::to_vec_pretty(&report).map_err(|e| e.to_string())?;
        if encoded.len() <= maximum { return Ok(encoded); }
        if let Some(query) = report.near_miss_clones.iter_mut().rev().find(|query| !query.pairs.is_empty()) {
            query.pairs.pop();
            query.status = Status::Partial;
            if !query.gaps.iter().any(|gap| gap == "report_byte_limit") {
                query.gaps.push("report_byte_limit".into());
            }
            report.status = Status::Partial;
            continue;
        }
        if let Some(pair) = report.pairs.pop() {
            let row = report.clone_queries.iter_mut().find(|row| row.language == pair.language)
                .ok_or("report pair has no clone query receipt")?;
            row.pairs_retained = row.pairs_retained.checked_sub(1)
                .ok_or("report pair count exceeds clone query receipt")?;
            if !report.gaps.iter().any(|g| g.reason == "report:pair_budget") {
                report.gaps.push(Gap { reason: "report:pair_budget".into(), document_id: "*".into(), span: None });
            }
            report.status = Status::Partial;
        } else {
            return Err("report budget too small to include complete scope and gap metadata".into());
        }
    }
}

/// Read an explicit, bounded shadow request and emit the shared report format.
pub fn write_shadow_report(
    path: &std::path::Path,
    mut writer: impl std::io::Write,
) -> Result<i32, String> {
    use std::io::Read;
    let started = std::time::Instant::now();
    let file = std::fs::File::open(path).map_err(|e| format!("cannot open explicit request: {e}"))?;
    if file.metadata().map_err(|e| e.to_string())?.len() > contract::MAX_REQUEST_BYTES { return Err("request file exceeds 4MiB".into()); }
    let mut raw = Vec::new();
    file.take(contract::MAX_REQUEST_BYTES+1).read_to_end(&mut raw).map_err(|e| e.to_string())?;
    if raw.len() as u64 > contract::MAX_REQUEST_BYTES { return Err("request exceeded 4MiB while reading".into()); }
    let req: contract::ScopeRequest = serde_json::from_slice(&raw).map_err(|e| format!("invalid typed request: {e}"))?;
    let mut report = shadow(req, &AtomicBool::new(false))?;
    report.elapsed_ms = started.elapsed().as_millis();
    let failed = report.status == contract::Status::Failed;
    let output = bounded_report(report)?;
    writer.write_all(&output).map_err(|e| format!("could not emit report: {e}"))?;
    writer.write_all(b"\n").map_err(|e| e.to_string())?;
    Ok(if failed { 1 } else { 0 })
}
