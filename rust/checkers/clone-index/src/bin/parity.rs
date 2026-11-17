//! Fixture-driven diffable C#/Rust parity trace. No JSON, source extraction or I/O in the index.
use slophawk_clone_index::*;
use std::collections::HashMap;

fn parse_kind(raw: &str) -> CloneTokenKind {
    match raw {
        "O" => CloneTokenKind::Other,
        "L" => CloneTokenKind::LocalIdentifier,
        "B" => CloneTokenKind::BoundIdentifier,
        "T" => CloneTokenKind::Literal,
        _ => panic!("invalid fixture token kind {raw}"),
    }
}
fn ordinal(a: &str, b: &str) -> std::cmp::Ordering {
    a.encode_utf16().cmp(b.encode_utf16())
}
fn flag(v: &str) -> bool {
    match v {
        "true" => true,
        "false" => false,
        _ => panic!("invalid boolean {v}"),
    }
}
fn num<T: std::str::FromStr>(v: &str) -> T {
    v.parse().unwrap_or_else(|_| panic!("invalid number {v}"))
}

fn print_query(label: &str, result: &CloneQueryResult) {
    let s = &result.statistics;
    let a = &result.index_settings;
    let l = &result.applied_limits;
    println!(
        "Q\t{label}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}",
        s.document_count,
        s.indexed_block_count,
        s.generated_documents,
        s.excluded_documents,
        s.below_minimum_blocks,
        s.oversized_blocks,
        s.candidate_pairs_examined,
        s.compared_tokens_charged,
        a.minimum_tokens,
        a.maximum_tokens_per_block,
        a.maximum_fingerprint_postings,
        l.maximum_candidate_pairs,
        l.maximum_output_pairs,
        l.maximum_compared_tokens,
        result.normalization_rule_version(),
        result.is_truncated(),
        result.pairs.len()
    );
    for pair in &result.pairs {
        let left = &pair.left;
        let right = &pair.right;
        println!(
            "P\t{}\t{:?}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{:.1}",
            pair.pair_id,
            pair.match_class,
            pair.normalization_rule_version,
            left.document_id,
            left.document_version,
            left.block_id,
            left.span.start_offset,
            left.span.end_offset,
            left.token_count,
            right.document_id,
            right.document_version,
            right.block_id,
            right.span.start_offset,
            right.span.end_offset,
            right.token_count,
            pair.exact_matched_tokens,
            pair.normalized_matched_tokens,
            pair.score
        );
    }
    for gap in &result.gaps {
        println!(
            "G\t{:?}\t{}\t{}",
            gap.reason, gap.scope_key, gap.affected_count
        );
    }
}

fn main() {
    let fallback = include_str!("../../fixtures/parity.tsv");
    let contents = std::env::args()
        .nth(1)
        .map(|file| std::fs::read_to_string(file).expect("fixture"))
        .unwrap_or_else(|| fallback.to_owned());
    let mut settings = CloneIndexSettings::default();
    let mut limits = CloneQueryLimits::default();
    let mut index = TokenCloneIndex::new(settings).unwrap();
    let mut current = HashMap::<String, CloneTokenDocument>::new();
    let mut pending: Option<CloneTokenDocument> = None;
    for (line_no, line) in contents.lines().enumerate() {
        let line = line.trim_end_matches('\r');
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let cells: Vec<&str> = line.split('\t').collect();
        match cells[0] {
            "SETTINGS" => {
                settings = CloneIndexSettings {
                    minimum_tokens: num(cells[1]),
                    maximum_tokens_per_block: num(cells[2]),
                    maximum_fingerprint_postings: num(cells[3]),
                };
                index = TokenCloneIndex::new(settings).unwrap();
                current.clear();
            }
            "LIMITS" => {
                limits = CloneQueryLimits {
                    maximum_candidate_pairs: num(cells[1]),
                    maximum_output_pairs: num(cells[2]),
                    maximum_compared_tokens: num(cells[3]),
                };
            }
            "UPSERT" => {
                assert!(pending.is_none());
                pending = Some(CloneTokenDocument {
                    document_id: cells[1].into(),
                    version: cells[2].into(),
                    is_generated: flag(cells[3]),
                    is_excluded: flag(cells[4]),
                    blocks: Vec::new(),
                });
            }
            "BLOCK" => {
                let tokens = cells[4..]
                    .iter()
                    .map(|raw| {
                        let (kind, text) = raw.split_once(':').expect("kind:text");
                        CloneToken {
                            kind: parse_kind(kind),
                            text: text.into(),
                        }
                    })
                    .collect();
                pending
                    .as_mut()
                    .expect("BLOCK outside UPSERT")
                    .blocks
                    .push(CloneTokenBlock {
                        block_id: cells[1].into(),
                        span: CloneSourceSpan {
                            start_offset: num(cells[2]),
                            end_offset: num(cells[3]),
                        },
                        tokens,
                    });
            }
            "END" => {
                let doc = pending.take().expect("END outside UPSERT");
                index.upsert(doc.clone()).unwrap();
                current.insert(doc.document_id.clone(), doc);
            }
            "DELETE" => {
                index.delete(cells[1]).unwrap();
                current.remove(cells[1]);
            }
            "REBUILD" => {
                let mut docs: Vec<_> = current.values().cloned().collect();
                docs.sort_by(|a, b| ordinal(&b.document_id, &a.document_id));
                index = TokenCloneIndex::new(settings).unwrap();
                for doc in docs {
                    index.upsert(doc).unwrap();
                }
            }
            "QUERY" => {
                print_query(cells[1], &index.query(limits).unwrap());
            }
            _ => panic!(
                "unknown fixture statement on line {}: {}",
                line_no + 1,
                cells[0]
            ),
        }
    }
    assert!(pending.is_none(), "unclosed UPSERT");
}
