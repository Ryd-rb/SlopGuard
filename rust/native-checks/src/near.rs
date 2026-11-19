//! Project the opt-in token alignment query onto admitted source bytes.
use crate::contract::{ByteSpan, Language, NEAR, Status};
use crate::providers::Projection;
use serde::Serialize;
use slophawk_clone_index::{
    CloneMatchLocation, TokenCloneIndex,
    near_miss::{NEAR_MISS_RULE_VERSION, PYTHON_NEAR_MISS_RULE_VERSION, NearMissLayout,
        NearMissGapReason, NearMissLimits, NearMissOptions, TokenLayout},
};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::atomic::{AtomicBool, Ordering};

type Locations = BTreeMap<(Language, String, String), ByteSpan>;
type PythonLayouts = BTreeMap<(String, String), Vec<slophawk_python_syntax_provider::TokenLayout>>;

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Query {
    language: Language,
    rule_version: &'static str,
    pub(crate) status: Status,
    reason: Option<&'static str>,
    pub(crate) gaps: Vec<String>,
    pub(crate) pairs: Vec<Match>,
    minimum_similarity_percent: u8,
    maximum_edits: usize,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Match {
    pair_id: String,
    left: Endpoint,
    right: Endpoint,
    edit_distance: usize,
    normalized_matched_tokens: usize,
    exact_matched_tokens: usize,
    score: f64,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct Endpoint {
    document_id: String,
    block_id: String,
    bytes: ByteSpan,
    utf16: [i32; 2],
    token_count: usize,
}

fn endpoint(
    language: Language,
    location: CloneMatchLocation,
    locations: &Locations,
) -> Result<Endpoint, String> {
    let bytes = locations
        .get(&(
            language,
            location.document_id.clone(),
            location.block_id.clone(),
        ))
        .ok_or("near-miss query returned an unbound source span")?
        .clone();
    Ok(Endpoint {
        document_id: location.document_id,
        block_id: location.block_id,
        bytes,
        utf16: [location.span.start_offset, location.span.end_offset],
        token_count: location.token_count,
    })
}

pub(crate) fn collect(
    projections: &mut [Projection],
    indices: &BTreeMap<Language, TokenCloneIndex>,
    locations: &Locations,
    python_layouts: &PythonLayouts,
    failed_batch: bool,
    cancel: &AtomicBool,
) -> Result<Vec<Query>, String> {
    let languages = projections
        .iter()
        .map(|doc| doc.report.language)
        .collect::<BTreeSet<_>>();
    let options = NearMissOptions::default();
    let limits = NearMissLimits {
        maximum_output_pairs: 32,
        ..NearMissLimits::default()
    };
    let mut rows = Vec::new();
    for language in languages {
        if cancel.load(Ordering::Acquire) {
            return Err("near-miss query cancelled before publication".into());
        }
        let mut row = Query {
            language,
            rule_version: if language == Language::Python {
                PYTHON_NEAR_MISS_RULE_VERSION
            } else {
                NEAR_MISS_RULE_VERSION
            },
            status: Status::Unsupported,
            reason: None,
            gaps: Vec::new(),
            pairs: Vec::new(),
            minimum_similarity_percent: options.minimum_similarity_percent,
            maximum_edits: options.maximum_edits,
        };
        if failed_batch {
            row.reason = Some("failed_batch");
        } else if let Some(index) = indices.get(&language) {
            let result = if language == Language::Python {
                // Borrow the producer's independently verified spans. This adds
                // one small entry per projected block, like the existing byte
                // location map; token/layout conversion still follows admission.
                let spans: BTreeMap<_, _> = projections.iter()
                    .filter(|projection| projection.report.language == language)
                    .flat_map(|projection| {
                        let document = projection.report.document_id.as_str();
                        projection.blocks.iter().map(move |block|
                            ((document, block.block_id.as_str()), block.span))
                    }).collect();
                index.query_near_misses_with_layout(options, limits, |location| {
                    // The sidecar and indexed span came from the same verified
                    // projection. Bind to its digest, not a caller's old receipt.
                    // Lookup/conversion runs only after core token/byte admission.
                    let projection = projections.iter().find(|projection|
                        projection.report.language == language
                            && projection.report.document_id == location.document_id)?;
                    let layout = python_layouts.get(&(
                        location.document_id.clone(), location.block_id.clone(),
                    ))?;
                    Some(NearMissLayout {
                        document_version: projection.report.source_sha256.clone(),
                        span: *spans.get(&(
                            location.document_id.as_str(), location.block_id.as_str(),
                        ))?,
                        tokens: layout.iter().map(|token| TokenLayout {
                            indent_depth: token.indent_depth,
                            starts_logical_line: token.starts_logical_line,
                        }).collect(),
                    })
                })
            } else {
                index.query_near_misses(options, limits)
            }.map_err(|e| e.to_string())?;
            if language == Language::Python && result.statistics.admitted_blocks == 0
                && result.statistics.unsupported_layout_blocks > 0 {
                row.reason = Some("unqualified_statement_layout");
                if result.gaps.iter().any(|gap| gap.reason != NearMissGapReason::UnsupportedLayout) {
                    row.status = Status::Partial;
                }
            } else if result.is_truncated() {
                row.status = Status::Partial;
            } else if result.statistics.admitted_blocks == 0 {
                row.reason = Some("no_eligible_blocks");
            } else {
                row.status = Status::Complete;
            }
            row.gaps = result
                .gaps
                .iter()
                .map(|gap| format!("{:?}:{}", gap.reason, gap.affected_count))
                .collect();
            for pair in result.pairs {
                row.pairs.push(Match {
                    pair_id: pair.pair_id,
                    left: endpoint(language, pair.left, locations)?,
                    right: endpoint(language, pair.right, locations)?,
                    edit_distance: pair.edit_distance,
                    normalized_matched_tokens: pair.normalized_matched_tokens,
                    exact_matched_tokens: pair.exact_matched_tokens,
                    score: pair.score,
                });
            }
        } else {
            row.reason = Some("no_admitted_blocks");
        }
        for projection in projections
            .iter_mut()
            .filter(|doc| doc.report.language == language)
        {
            if let Some(rule) = projection
                .report
                .rules
                .iter_mut()
                .find(|rule| rule.id == NEAR)
            {
                rule.id = row.rule_version.into();
                rule.scope = if language == Language::Python {
                    "bounded whole-block token alignment with Python suite and logical-line evidence"
                } else {
                    "bounded, same-language whole-block token alignment"
                }.into();
                rule.status = if projection.blocks.is_empty() {
                    Status::Unsupported
                } else if row.status == Status::Complete
                    && projection.report.status != Status::Complete
                {
                    Status::Partial
                } else {
                    row.status
                };
                rule.reason = row
                    .reason
                    .unwrap_or(if language == Language::Python {
                        "same suite structure; edits within paired logical lines or pure whole-line insertions/deletions"
                    } else {
                        "insertions and deletions over admitted token blocks"
                    })
                    .into();
            }
        }
        rows.push(row);
    }
    if cancel.load(Ordering::Acquire) {
        return Err("near-miss query cancelled before publication".into());
    }
    Ok(rows)
}
