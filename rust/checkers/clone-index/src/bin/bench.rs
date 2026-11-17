//! Matched pretokenized engine benchmark; controlled by docs/tokens/repeats/candidate cap.
//! The clock excludes process startup, fixture construction and PSS sampling.
use slophawk_clone_index::*;
use std::hint::black_box;
use std::time::Instant;

fn token(kind: CloneTokenKind, text: String) -> CloneToken {
    CloneToken { kind, text }
}
fn make_doc(n: usize, tokens: usize, changed: bool) -> CloneTokenDocument {
    let local = format!("local-{}", n / 3);
    let mut ts = Vec::with_capacity(tokens);
    for i in 0..tokens {
        let t = match i % 6 {
            0 | 4 => token(CloneTokenKind::LocalIdentifier, local.clone()),
            1 => token(CloneTokenKind::Other, "=".into()),
            2 => token(
                CloneTokenKind::Literal,
                if changed {
                    "9".into()
                } else {
                    (n / 3).to_string()
                },
            ),
            3 => token(CloneTokenKind::Other, ";".into()),
            _ => token(CloneTokenKind::BoundIdentifier, "Console".into()),
        };
        ts.push(t);
    }
    CloneTokenDocument {
        document_id: format!("d{n:06}"),
        version: "v1".into(),
        is_generated: false,
        is_excluded: false,
        blocks: vec![CloneTokenBlock {
            block_id: "body".into(),
            span: CloneSourceSpan {
                start_offset: 0,
                end_offset: (tokens * 2) as i32,
            },
            tokens: ts,
        }],
    }
}
fn pss_kib() -> i64 {
    let Ok(s) = std::fs::read_to_string("/proc/self/smaps_rollup") else {
        return -1;
    };
    s.lines()
        .find_map(|line| {
            line.strip_prefix("Pss:")
                .and_then(|v| v.split_whitespace().next())
                .and_then(|v| v.parse().ok())
        })
        .unwrap_or(-1)
}
fn phase<F>(
    name: &str,
    sample: usize,
    docs: usize,
    tokens: usize,
    repeats: usize,
    candidates: usize,
    work: F,
) where
    F: FnOnce(),
{
    let begin = pss_kib();
    let start = Instant::now();
    work();
    let elapsed = start.elapsed().as_micros();
    println!(
        "M\trust\t{docs}\t{tokens}\t{repeats}\t{candidates}\t{sample}\t{name}\t{elapsed}\t{begin}\t{}",
        pss_kib()
    );
}
fn main() {
    let args: Vec<_> = std::env::args().skip(1).collect();
    assert_eq!(
        args.len(),
        4,
        "usage: clone-bench DOCS TOKENS REPEATS QUERY-CANDIDATE-CAP"
    );
    let docs: usize = args[0].parse().unwrap();
    let tokens: usize = args[1].parse().unwrap();
    let repeats: usize = args[2].parse().unwrap();
    let candidates: usize = args[3].parse().unwrap();
    assert!(docs >= 3 && (4..=4096).contains(&tokens) && repeats >= 1 && candidates >= 1);
    let settings = CloneIndexSettings {
        minimum_tokens: 4,
        maximum_tokens_per_block: 4096,
        maximum_fingerprint_postings: 64,
    };
    let limits = CloneQueryLimits {
        maximum_candidate_pairs: candidates,
        maximum_output_pairs: candidates,
        maximum_compared_tokens: 2_000_000,
    };
    // Precompute the identical inputs outside timed phases as the C# harness.
    let fixture: Vec<_> = (0..docs).map(|n| make_doc(n, tokens, false)).collect();
    for sample in 0..repeats {
        let mut index = TokenCloneIndex::new(settings).unwrap();
        phase("upsert", sample, docs, tokens, repeats, candidates, || {
            for doc in &fixture {
                index.upsert(black_box(doc.clone())).unwrap();
            }
        });
        phase(
            "cold_query",
            sample,
            docs,
            tokens,
            repeats,
            candidates,
            || {
                black_box(index.query(limits).unwrap());
            },
        );
        phase(
            "warm_query",
            sample,
            docs,
            tokens,
            repeats,
            candidates,
            || {
                black_box(index.query(limits).unwrap());
            },
        );
        phase(
            "replacement",
            sample,
            docs,
            tokens,
            repeats,
            candidates,
            || {
                index
                    .upsert(black_box(make_doc(docs / 2, tokens, true)))
                    .unwrap();
            },
        );
        phase(
            "deletion",
            sample,
            docs,
            tokens,
            repeats,
            candidates,
            || {
                black_box(index.delete(&format!("d{:06}", docs / 3)).unwrap());
            },
        );
    }
}
