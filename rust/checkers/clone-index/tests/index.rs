use slophawk_clone_index::*;

fn token(kind: CloneTokenKind, text: &str) -> CloneToken {
    CloneToken {
        kind,
        text: text.into(),
    }
}
fn pattern(local: &str, literal: &str, bound: &str) -> Vec<CloneToken> {
    vec![
        token(CloneTokenKind::LocalIdentifier, local),
        token(CloneTokenKind::Other, "="),
        token(CloneTokenKind::Literal, literal),
        token(CloneTokenKind::Other, ";"),
        token(CloneTokenKind::LocalIdentifier, local),
        token(CloneTokenKind::BoundIdentifier, bound),
    ]
}
fn block(id: &str, at: i32, tokens: Vec<CloneToken>) -> CloneTokenBlock {
    CloneTokenBlock {
        block_id: id.into(),
        span: CloneSourceSpan {
            start_offset: at,
            end_offset: at + 2 * tokens.len() as i32,
        },
        tokens,
    }
}
fn doc(id: &str, version: &str, blocks: Vec<CloneTokenBlock>) -> CloneTokenDocument {
    CloneTokenDocument {
        document_id: id.into(),
        version: version.into(),
        is_generated: false,
        is_excluded: false,
        blocks,
    }
}
fn settings() -> CloneIndexSettings {
    CloneIndexSettings {
        minimum_tokens: 4,
        maximum_tokens_per_block: 64,
        maximum_fingerprint_postings: 64,
    }
}
fn unlimited() -> CloneQueryLimits {
    CloneQueryLimits {
        maximum_candidate_pairs: usize::MAX,
        maximum_output_pairs: usize::MAX,
        maximum_compared_tokens: i64::MAX,
    }
}
fn index() -> TokenCloneIndex {
    TokenCloneIndex::new(settings()).unwrap()
}

#[test]
fn class_order_ids_offsets_and_exact_counts() {
    let mut i = index();
    i.upsert(doc(
        "beta",
        "rev1",
        vec![block("body", 10, pattern("counter", "1", "Console"))],
    ))
    .unwrap();
    i.upsert(doc(
        "gamma",
        "rev1",
        vec![block("body", 20, pattern("value", "1", "Console"))],
    ))
    .unwrap();
    i.upsert(doc(
        "alpha",
        "rev4",
        vec![block("body", 0, pattern("counter", "1", "Console"))],
    ))
    .unwrap();
    let q = i.query(unlimited()).unwrap();
    assert!(!q.is_truncated());
    assert_eq!(q.pairs.len(), 3);
    assert_eq!(q.pairs[0].match_class, CloneMatchClass::Exact);
    assert_eq!(
        q.pairs[0].pair_id,
        "sg10.token-clones.v1:5:alpha4:body/4:beta4:body"
    );
    assert_eq!(q.pairs[0].left.document_version, "rev4");
    assert_eq!(q.pairs[0].left.span.start_offset, 0);
    assert_eq!(q.pairs[0].right.span.end_offset, 22);
    assert_eq!(q.pairs[0].exact_matched_tokens, 6);
    assert_eq!(q.pairs[1].exact_matched_tokens, 4);
    assert_eq!(q.pairs[2].exact_matched_tokens, 4);
    assert!(
        q.pairs
            .iter()
            .all(|p| p.normalized_matched_tokens == 6 && p.score == 1.0)
    );
}

#[test]
fn only_local_names_are_renamed_and_equality_pattern_matters() {
    let mut i = index();
    i.upsert(doc(
        "source",
        "1",
        vec![block("m", 0, pattern("a", "1", "Console"))],
    ))
    .unwrap();
    i.upsert(doc(
        "renamed",
        "1",
        vec![block("m", 0, pattern("b", "1", "Console"))],
    ))
    .unwrap();
    i.upsert(doc(
        "literal",
        "1",
        vec![block("m", 0, pattern("b", "2", "Console"))],
    ))
    .unwrap();
    i.upsert(doc(
        "bound",
        "1",
        vec![block("m", 0, pattern("b", "1", "OtherType"))],
    ))
    .unwrap();
    i.upsert(doc(
        "operator",
        "1",
        vec![block(
            "m",
            0,
            vec![
                token(CloneTokenKind::LocalIdentifier, "b"),
                token(CloneTokenKind::Other, "="),
                token(CloneTokenKind::Literal, "1"),
                token(CloneTokenKind::Other, ";"),
                token(CloneTokenKind::LocalIdentifier, "b"),
                token(CloneTokenKind::Other, "-"),
            ],
        )],
    ))
    .unwrap();
    let q = i.query(unlimited()).unwrap();
    assert_eq!(q.pairs.len(), 1);
    assert_eq!(
        q.pairs[0].match_class,
        CloneMatchClass::RenamedLocalIdentifiers
    );
    let mut x = index();
    x.upsert(doc(
        "base",
        "1",
        vec![block(
            "m",
            0,
            vec![
                token(CloneTokenKind::LocalIdentifier, "x"),
                token(CloneTokenKind::Other, "+"),
                token(CloneTokenKind::LocalIdentifier, "x"),
                token(CloneTokenKind::Other, "*"),
                token(CloneTokenKind::LocalIdentifier, "y"),
            ],
        )],
    ))
    .unwrap();
    x.upsert(doc(
        "positive",
        "1",
        vec![block(
            "m",
            0,
            vec![
                token(CloneTokenKind::LocalIdentifier, "a"),
                token(CloneTokenKind::Other, "+"),
                token(CloneTokenKind::LocalIdentifier, "a"),
                token(CloneTokenKind::Other, "*"),
                token(CloneTokenKind::LocalIdentifier, "b"),
            ],
        )],
    ))
    .unwrap();
    x.upsert(doc(
        "negative",
        "1",
        vec![block(
            "m",
            0,
            vec![
                token(CloneTokenKind::LocalIdentifier, "a"),
                token(CloneTokenKind::Other, "+"),
                token(CloneTokenKind::LocalIdentifier, "b"),
                token(CloneTokenKind::Other, "*"),
                token(CloneTokenKind::LocalIdentifier, "a"),
            ],
        )],
    ))
    .unwrap();
    let pair = x.query(unlimited()).unwrap().pairs.remove(0);
    assert_eq!(pair.pair_id, "sg10.token-clones.v1:4:base1:m/8:positive1:m");
    assert_eq!(pair.exact_matched_tokens, 2);
    assert_eq!(x.query(unlimited()).unwrap().pairs.len(), 1);
}

#[test]
fn repeated_blocks_are_distinct_endpoints() {
    let mut i = index();
    i.upsert(doc(
        "dir/f.cs|target",
        "rev1",
        vec![
            block("method/1", 10, pattern("id", "1", "Console")),
            block("method/2", 80, pattern("id", "1", "Console")),
        ],
    ))
    .unwrap();
    let q = i.query(unlimited()).unwrap();
    assert_eq!(q.pairs.len(), 1);
    assert_eq!(q.pairs[0].left.document_id, q.pairs[0].right.document_id);
    assert_ne!(q.pairs[0].left.span, q.pairs[0].right.span);
}

#[test]
fn repeated_versions_replace_snapshot_and_delete_both_pair_endpoints() {
    let mut i = index();
    for name in ["A", "B", "C"] {
        i.upsert(doc(
            name,
            "v1",
            vec![block("one", 0, pattern("x", "1", "Console"))],
        ))
        .unwrap();
    }
    assert_eq!(i.query(unlimited()).unwrap().pairs.len(), 3);
    i.upsert(doc(
        "B",
        "v1",
        vec![block("one", 30, pattern("x", "9", "Console"))],
    ))
    .unwrap();
    let pairs = i.query(unlimited()).unwrap().pairs;
    assert_eq!(pairs.len(), 1);
    assert_eq!(pairs[0].right.document_id, "C");
    i.upsert(doc(
        "A",
        "v2",
        vec![block("one", 40, pattern("x", "9", "Console"))],
    ))
    .unwrap();
    assert_eq!(
        i.query(unlimited()).unwrap().pairs[0].right.document_id,
        "B"
    );
    assert!(i.delete("B").unwrap());
    assert!(i.query(unlimited()).unwrap().pairs.is_empty());
    assert!(!i.delete("B").unwrap());
    i.upsert(doc(
        "C",
        "v2",
        vec![block("one", 50, pattern("x", "9", "Console"))],
    ))
    .unwrap();
    assert_eq!(i.query(unlimited()).unwrap().pairs.len(), 1);
    assert!(i.delete("A").unwrap());
    assert!(i.query(unlimited()).unwrap().pairs.is_empty());
}

#[test]
fn validation_is_atomic_for_each_failure() {
    let mut i = index();
    i.upsert(doc(
        "a",
        "v1",
        vec![block("m", 0, pattern("id", "1", "Console"))],
    ))
    .unwrap();
    i.upsert(doc(
        "b",
        "v1",
        vec![block("m", 0, pattern("id", "1", "Console"))],
    ))
    .unwrap();
    let before = i.query(unlimited()).unwrap();
    let cases = [
        doc(
            "a",
            "v2",
            vec![
                block("duplicate", 0, pattern("id", "1", "Console")),
                block("duplicate", 10, pattern("id", "1", "Console")),
            ],
        ),
        doc(
            "a",
            "v2",
            vec![block("bad", -1, pattern("id", "1", "Console"))],
        ),
        doc(
            "a",
            "v2",
            vec![block("bad", 0, vec![token(CloneTokenKind::Other, "")])],
        ),
        doc("a", "", vec![block("m", 0, pattern("id", "1", "Console"))]),
    ];
    for invalid in cases {
        assert!(i.upsert(invalid).is_err());
        assert_eq!(i.query(unlimited()).unwrap(), before);
    }
    assert!(i.delete("  ").is_err());
    assert_eq!(i.query(unlimited()).unwrap(), before);
}

#[test]
fn warm_and_cold_rebuild_same_pairs_and_gaps() {
    let fixture = vec![
        doc("c", "r3", vec![block("z", 0, pattern("v", "1", "Console"))]),
        doc(
            "a",
            "r1",
            vec![
                block("x", 0, pattern("x", "1", "Console")),
                block("y", 30, pattern("v", "1", "Console")),
            ],
        ),
        doc("b", "r4", vec![block("x", 0, pattern("x", "2", "Console"))]),
        doc(
            "d",
            "r3",
            vec![block("z", 0, pattern("value", "1", "Console"))],
        ),
    ];
    let mut warm = index();
    for document in &fixture {
        warm.upsert(document.clone()).unwrap();
    }
    warm.upsert(doc(
        "temp",
        "1",
        vec![block("x", 0, pattern("v", "1", "Console"))],
    ))
    .unwrap();
    warm.delete("temp").unwrap();
    let mut cold = index();
    for document in fixture.iter().rev() {
        cold.upsert(document.clone()).unwrap();
    }
    assert_eq!(
        warm.query(unlimited()).unwrap(),
        cold.query(unlimited()).unwrap()
    );
    warm.upsert(doc(
        "b",
        "r5",
        vec![block("x", 70, pattern("x", "1", "Console"))],
    ))
    .unwrap();
    cold = index();
    for document in fixture.iter().filter(|d| d.document_id != "b") {
        cold.upsert(document.clone()).unwrap();
    }
    cold.upsert(doc(
        "b",
        "r5",
        vec![block("x", 70, pattern("x", "1", "Console"))],
    ))
    .unwrap();
    assert_eq!(
        warm.query(unlimited()).unwrap(),
        cold.query(unlimited()).unwrap()
    );
}

#[test]
fn budget_frontiers_are_deterministic() {
    let mut i = index();
    for name in ["d", "a", "c", "b"] {
        i.upsert(doc(
            name,
            "1",
            vec![block("m", 0, pattern("x", "1", "Console"))],
        ))
        .unwrap();
    }
    assert_eq!(i.query(unlimited()).unwrap().pairs.len(), 6);
    let candidate = i
        .query(CloneQueryLimits {
            maximum_candidate_pairs: 1,
            ..unlimited()
        })
        .unwrap();
    assert_eq!(candidate.pairs.len(), 1);
    assert_eq!(
        candidate.gaps[0].reason,
        CloneScopeGapReason::CandidatePairLimit
    );
    assert_eq!(candidate.statistics.candidate_pairs_examined, 1);
    let output = i
        .query(CloneQueryLimits {
            maximum_output_pairs: 1,
            ..unlimited()
        })
        .unwrap();
    assert_eq!(output.gaps[0].reason, CloneScopeGapReason::OutputPairLimit);
    assert_eq!(output.pairs[0].pair_id, candidate.pairs[0].pair_id);
    let token_cap = i
        .query(CloneQueryLimits {
            maximum_compared_tokens: 11,
            ..unlimited()
        })
        .unwrap();
    assert_eq!(
        token_cap.gaps[0].reason,
        CloneScopeGapReason::ComparedTokenLimit
    );
    assert_eq!(token_cap.gaps[0].affected_count, 12);
    assert_eq!(token_cap.statistics.candidate_pairs_examined, 0);
    assert_eq!(token_cap.gaps[0].scope_key, candidate.pairs[0].pair_id);
    assert!(
        i.query(CloneQueryLimits {
            maximum_candidate_pairs: 0,
            ..unlimited()
        })
        .is_err()
    );
}

#[test]
fn posting_limit_and_oversized_blocks_emit_distinct_gaps() {
    let mut i = TokenCloneIndex::new(CloneIndexSettings {
        minimum_tokens: 4,
        maximum_tokens_per_block: 6,
        maximum_fingerprint_postings: 2,
    })
    .unwrap();
    for name in ["a", "b"] {
        i.upsert(doc(
            name,
            "1",
            vec![block("m", 0, pattern("x", "1", "Console"))],
        ))
        .unwrap();
    }
    assert_eq!(i.query(unlimited()).unwrap().pairs.len(), 1);
    i.upsert(doc(
        "c",
        "1",
        vec![block("m", 0, pattern("x", "1", "Console"))],
    ))
    .unwrap();
    let q = i.query(unlimited()).unwrap();
    assert_eq!(q.pairs.len(), 0);
    assert_eq!(q.gaps.len(), 2);
    assert!(
        q.gaps
            .iter()
            .all(|g| g.reason == CloneScopeGapReason::BoilerplatePostingLimit
                && g.affected_count == 3)
    );
    let mut extra = pattern("x", "1", "Console");
    extra.push(token(CloneTokenKind::Other, "extra"));
    i.upsert(doc("too-large", "1", vec![block("big", 0, extra)]))
        .unwrap();
    let q = i.query(unlimited()).unwrap();
    assert_eq!(q.statistics.oversized_blocks, 1);
    assert!(
        q.gaps
            .iter()
            .any(|g| g.reason == CloneScopeGapReason::OversizedBlock && g.affected_count == 7)
    );
    assert_eq!(q.gaps[0].reason, CloneScopeGapReason::OversizedBlock);
}

#[test]
fn generated_excluded_and_small_are_metadata_not_postings() {
    let mut i = index();
    let mut generated = doc(
        "generated",
        "1",
        vec![block("m", 0, pattern("x", "1", "Console"))],
    );
    generated.is_generated = true;
    let mut excluded = doc(
        "excluded",
        "1",
        vec![block("m", 0, pattern("x", "1", "Console"))],
    );
    excluded.is_excluded = true;
    i.upsert(generated).unwrap();
    i.upsert(excluded).unwrap();
    i.upsert(doc(
        "tiny",
        "1",
        vec![block(
            "m",
            0,
            vec![token(CloneTokenKind::LocalIdentifier, "x")],
        )],
    ))
    .unwrap();
    i.upsert(doc(
        "first",
        "1",
        vec![block("m", 0, pattern("x", "1", "Console"))],
    ))
    .unwrap();
    let q = i.query(unlimited()).unwrap();
    assert_eq!(q.statistics.generated_documents, 1);
    assert_eq!(q.statistics.excluded_documents, 1);
    assert_eq!(q.statistics.below_minimum_blocks, 1);
    assert_eq!(q.statistics.indexed_block_count, 1);
    assert!(q.pairs.is_empty());
    i.upsert(doc(
        "generated",
        "2",
        vec![block("m", 0, pattern("x", "1", "Console"))],
    ))
    .unwrap();
    assert_eq!(i.query(unlimited()).unwrap().pairs.len(), 1);
}

#[test]
fn unicode_ordinal_order_and_pair_identity_count_utf16_units() {
    let mut i = index();
    i.upsert(doc(
        "\u{E000}",
        "1",
        vec![block("Ω", 0, pattern("é", "🌙", "Console"))],
    ))
    .unwrap();
    i.upsert(doc(
        "𐐀",
        "1",
        vec![block("𐐀", 0, pattern("é", "🌙", "Console"))],
    ))
    .unwrap();
    let p = i.query(unlimited()).unwrap().pairs.remove(0);
    assert_eq!(p.left.document_id, "𐐀");
    assert_eq!(p.pair_id, "sg10.token-clones.v1:2:𐐀2:𐐀/1:\u{E000}1:Ω");
}
