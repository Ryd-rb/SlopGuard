//! Draft regression cases with hand-derived expectations; not executed by this packet.
use slophawk_clone_index::near_miss::*;
use slophawk_clone_index::*;

fn token(kind: CloneTokenKind, text: &str) -> CloneToken {
    CloneToken {
        kind,
        text: text.into(),
    }
}

fn words(text: &str) -> Vec<CloneToken> {
    text.split_whitespace()
        .map(|s| token(CloneTokenKind::Other, s))
        .collect()
}

fn block(id: &str, start: i32, tokens: Vec<CloneToken>) -> CloneTokenBlock {
    CloneTokenBlock {
        block_id: id.into(),
        span: CloneSourceSpan {
            start_offset: start,
            end_offset: start + 100,
        },
        tokens,
    }
}

fn document(id: &str, text: &str) -> CloneTokenDocument {
    CloneTokenDocument {
        document_id: id.into(),
        version: "1".into(),
        is_generated: false,
        is_excluded: false,
        blocks: vec![block("m", 0, words(text))],
    }
}

fn settings() -> CloneIndexSettings {
    CloneIndexSettings {
        minimum_tokens: 1,
        maximum_tokens_per_block: 64,
        maximum_fingerprint_postings: 64,
    }
}

fn fixture(rows: &[(&str, &str)]) -> TokenCloneIndex {
    let mut index = TokenCloneIndex::new(settings()).unwrap();
    for (id, text) in rows {
        index.upsert(document(id, text)).unwrap();
    }
    index
}

fn options(edits: usize) -> NearMissOptions {
    NearMissOptions {
        minimum_similarity_percent: 80,
        maximum_edits: edits,
    }
}

fn query(index: &TokenCloneIndex, edits: usize) -> NearMissQueryResult {
    index
        .query_near_misses(options(edits), NearMissLimits::default())
        .unwrap()
}

fn has_gap(result: &NearMissQueryResult, reason: NearMissGapReason) -> bool {
    result.gaps.iter().any(|gap| gap.reason == reason)
}

fn trio() -> TokenCloneIndex {
    fixture(&[
        ("a", "x x x x"),
        ("b", "x x x x x"),
        ("c", "x x x x x x"),
    ])
}

#[test]
fn insertion_and_deletion_have_whole_block_dice_evidence() {
    for (left, right) in [("a b c d", "a b X c d"), ("a b X c d", "a b c d")] {
        let result = query(&fixture(&[("a", left), ("b", right)]), 1);
        assert!(!result.is_truncated());
        assert_eq!(result.pairs.len(), 1);
        let pair = &result.pairs[0];
        assert_eq!(pair.edit_distance, 1);
        assert_eq!(pair.normalized_matched_tokens, 4);
        assert_eq!(pair.exact_matched_tokens, 4);
        assert_eq!(pair.score, 8.0 / 9.0);
        assert_eq!(pair.left.token_count + pair.right.token_count, 9);
        assert_eq!(
            pair.left.span,
            CloneSourceSpan {
                start_offset: 0,
                end_offset: 100,
            }
        );
        assert_eq!(pair.pair_id, "sg10.token-near-misses.v1:1:a1:m/1:b1:m");
        assert_eq!(result.rule_version, NEAR_MISS_RULE_VERSION);
        assert_eq!(result.normalization_rule_version, RULE_VERSION);
        assert!(
            result
                .unsupported
                .contains(&NearMissUnsupported::SemanticEquivalence)
        );
        assert!(
            result
                .unsupported
                .contains(&NearMissUnsupported::SubBlockSourceLocations)
        );
    }
}

#[test]
fn adjacent_reorder_is_not_a_one_edit_match() {
    // Four shared bag tokens, but moving b past c needs one deletion and insertion.
    let index = fixture(&[("a", "a b c d"), ("b", "a c b d")]);
    let result = query(&index, 1);
    assert!(result.pairs.is_empty());
    assert_eq!(result.statistics.distance_rejected_pairs, 1);
    assert!(!result.is_truncated());
    let permissive = index
        .query_near_misses(
            NearMissOptions {
                minimum_similarity_percent: 75,
                maximum_edits: 2,
            },
            NearMissLimits::default(),
        )
        .unwrap();
    assert_eq!(permissive.pairs[0].edit_distance, 2);
    assert_eq!(permissive.pairs[0].score, 0.75);
}

#[test]
fn bound_names_literals_and_token_kinds_are_not_erased() {
    for kind in [
        CloneTokenKind::BoundIdentifier,
        CloneTokenKind::Literal,
        CloneTokenKind::Other,
    ] {
        let mut a = document("a", "p old q r s");
        let mut b = document("b", "p new q r s");
        a.blocks[0].tokens[1].kind = kind;
        b.blocks[0].tokens[1].kind = kind;
        let mut index = TokenCloneIndex::new(settings()).unwrap();
        index.upsert(a).unwrap();
        index.upsert(b).unwrap();
        assert!(query(&index, 1).pairs.is_empty());
        let pair = query(&index, 2).pairs.remove(0);
        assert_eq!(
            (
                pair.edit_distance,
                pair.normalized_matched_tokens,
                pair.exact_matched_tokens,
            ),
            (2, 4, 4)
        );
        assert_eq!(pair.score, 0.8);
    }
    let mut b = document("b", "p same q r s");
    b.blocks[0].tokens[1].kind = CloneTokenKind::Literal;
    let mut index = fixture(&[("a", "p same q r s")]);
    index.upsert(b).unwrap();
    assert_eq!(query(&index, 2).pairs[0].edit_distance, 2);
}

#[test]
fn local_renaming_preserves_v1_and_counts_exact_matches_in_alignment() {
    let mut index = TokenCloneIndex::new(settings()).unwrap();
    for (id, local, added) in [
        ("a", "x", false),
        ("b", "x", false),
        ("c", "y", false),
        ("d", "z", true),
    ] {
        let mut doc = document(id, "");
        doc.blocks[0].tokens = vec![
            token(CloneTokenKind::LocalIdentifier, local),
            token(CloneTokenKind::Other, "="),
            token(CloneTokenKind::Literal, "1"),
            token(CloneTokenKind::Other, ";"),
            token(CloneTokenKind::LocalIdentifier, local),
        ];
        if added {
            doc.blocks[0].tokens.push(token(CloneTokenKind::Other, "+"));
        }
        index.upsert(doc).unwrap();
    }
    let before = index.query(CloneQueryLimits::default()).unwrap();
    assert_eq!(before.pairs.len(), 3);
    assert_eq!(before.pairs[0].match_class, CloneMatchClass::Exact);
    assert_eq!(before.pairs[0].pair_id, "sg10.token-clones.v1:1:a1:m/1:b1:m");
    assert!(
        before.pairs[1..]
            .iter()
            .all(|p| p.match_class == CloneMatchClass::RenamedLocalIdentifiers)
    );
    let result = query(&index, 1);
    assert_eq!(result.statistics.v1_pairs_skipped, 3);
    assert_eq!(result.pairs.len(), 3);
    for pair in &result.pairs {
        assert_eq!(
            (
                pair.edit_distance,
                pair.normalized_matched_tokens,
                pair.exact_matched_tokens,
            ),
            (1, 5, 3)
        );
        assert_eq!(pair.score, 10.0 / 11.0);
    }
    index
        .query_near_misses(
            options(1),
            NearMissLimits {
                maximum_candidate_pairs: 1,
                ..NearMissLimits::default()
            },
        )
        .unwrap();
    assert_eq!(index.query(CloneQueryLimits::default()).unwrap(), before);
}

#[test]
fn inserting_a_new_first_local_exposes_the_documented_normalization_limit() {
    let mut index = TokenCloneIndex::new(settings()).unwrap();
    let mut a = document("a", "");
    a.blocks[0].tokens = vec![
        token(CloneTokenKind::LocalIdentifier, "x"),
        token(CloneTokenKind::LocalIdentifier, "x"),
        token(CloneTokenKind::Other, "+"),
        token(CloneTokenKind::LocalIdentifier, "y"),
        token(CloneTokenKind::LocalIdentifier, "y"),
    ];
    let mut b = a.clone();
    b.document_id = "b".into();
    b.blocks[0]
        .tokens
        .insert(0, token(CloneTokenKind::LocalIdentifier, "z"));
    index.upsert(a).unwrap();
    index.upsert(b).unwrap();
    // Raw insertion shifts v1 ordinals: 0,0,+,1,1 versus 0,1,1,+,2,2.
    // This API deliberately does not solve renaming consistently across edits.
    let result = query(&index, 1);
    assert!(result.pairs.is_empty());
    assert!(!result.is_truncated());
    assert!(
        result
            .unsupported
            .contains(&NearMissUnsupported::RenamingInvariantAcrossEdits)
    );
}

#[test]
fn minimum_distance_ties_prefer_the_greatest_exact_match_count() {
    let mut a = document("a", "");
    a.blocks[0].tokens = vec![
        token(CloneTokenKind::LocalIdentifier, "x"),
        token(CloneTokenKind::Other, "+"),
    ];
    let mut b = document("b", "");
    b.blocks[0].tokens = vec![
        token(CloneTokenKind::Other, "+"),
        token(CloneTokenKind::LocalIdentifier, "y"),
    ];
    let mut index = TokenCloneIndex::new(settings()).unwrap();
    index.upsert(a).unwrap();
    index.upsert(b).unwrap();
    let result = index
        .query_near_misses(
            NearMissOptions {
                minimum_similarity_percent: 50,
                maximum_edits: 2,
            },
            NearMissLimits::default(),
        )
        .unwrap();
    // Both '+' and normalized local 0 give LCS one, but only '+' matches exactly.
    assert_eq!(result.pairs.len(), 1);
    let pair = &result.pairs[0];
    assert_eq!(pair.edit_distance, 2);
    assert_eq!(pair.normalized_matched_tokens, 1);
    assert_eq!(pair.exact_matched_tokens, 1);
    assert_eq!(pair.score, 0.5);
}

#[test]
fn edited_clone_without_common_four_token_shingle_and_disjoint_control() {
    // Changed d and h break every four-token shingle. Eight ordered tokens remain.
    let index = fixture(&[
        ("a", "a b c d e f g h i j"),
        ("b", "a b c X e f g Y i j"),
    ]);
    let result = query(&index, 4);
    assert_eq!(result.pairs.len(), 1);
    let pair = &result.pairs[0];
    assert_eq!(pair.edit_distance, 4);
    assert_eq!(pair.normalized_matched_tokens, 8);
    assert_eq!(pair.exact_matched_tokens, 8);
    assert_eq!(pair.score, 0.8);
    assert!(!result.is_truncated());

    let control = query(&fixture(&[("a", "a b c d"), ("b", "u v w z")]), 4);
    assert!(control.pairs.is_empty());
    assert_eq!(control.statistics.candidate_pairs, 0);
    assert_eq!(control.statistics.alignment_cells, 0);
    assert!(!control.is_truncated());
}

#[test]
fn generated_excluded_small_and_oversized_blocks_stay_visible() {
    let mut index = TokenCloneIndex::new(CloneIndexSettings {
        minimum_tokens: 3,
        maximum_tokens_per_block: 6,
        ..settings()
    })
    .unwrap();
    let mut generated = document("generated", "x x x x");
    generated.is_generated = true;
    let mut excluded = document("excluded", "x x x x");
    excluded.is_excluded = true;
    for doc in [
        document("a", "x x x x"),
        document("b", "x x x x x"),
        generated,
        excluded,
        document("small", "x x"),
        document("large", "x x x x x x x"),
    ] {
        index.upsert(doc).unwrap();
    }
    let result = query(&index, 1);
    assert_eq!(result.pairs.len(), 1);
    assert_eq!(result.statistics.document_count, 6);
    assert_eq!(result.statistics.indexed_block_count, 2);
    assert_eq!(result.statistics.generated_documents, 1);
    assert_eq!(result.statistics.excluded_documents, 1);
    assert_eq!(result.statistics.below_minimum_blocks, 1);
    assert_eq!(result.statistics.oversized_blocks, 1);
    assert_eq!(
        result.gaps,
        vec![NearMissGap {
            reason: NearMissGapReason::OversizedBlock,
            affected_count: 1,
        }]
    );
}

#[test]
fn unequal_lengths_and_same_document_overlap_have_distinct_dispositions() {
    let result = query(&fixture(&[("a", "x x"), ("b", "x x x x x")]), 1);
    assert!(result.pairs.is_empty());
    assert_eq!(result.statistics.length_rejected_pairs, 1);
    assert_eq!(result.statistics.alignment_cells, 0);
    for (start, expected) in [(99, 0), (100, 1)] {
        let mut doc = document("a", "");
        doc.blocks = vec![
            block("first", 0, words("x x x x")),
            block("second", start, words("x x x x x")),
        ];
        let mut index = TokenCloneIndex::new(settings()).unwrap();
        index.upsert(doc).unwrap();
        let result = query(&index, 1);
        assert_eq!(result.pairs.len(), expected);
        assert_eq!(result.statistics.overlapping_pairs_skipped, 1 - expected);
        assert!(!result.is_truncated());
    }
}

#[test]
fn same_id_same_version_replacement_and_failed_upsert_are_atomic() {
    let mut index = fixture(&[("a", "a b c d"), ("b", "a b X c d")]);
    let original = query(&index, 1).pairs.remove(0);
    let mut replacement = document("b", "a b Y c d");
    replacement.blocks[0].span = CloneSourceSpan {
        start_offset: 200,
        end_offset: 300,
    };
    index.upsert(replacement.clone()).unwrap();
    let changed = query(&index, 1);
    assert_eq!(changed.pairs[0].pair_id, original.pair_id);
    assert_eq!(changed.pairs[0].right.document_version, "1");
    assert_eq!(changed.pairs[0].right.span.start_offset, 200);
    replacement.blocks[0].tokens[0].text.clear();
    assert!(index.upsert(replacement).is_err());
    assert_eq!(query(&index, 1), changed);
    index.upsert(document("b", "u v w z q")).unwrap();
    assert!(query(&index, 1).pairs.is_empty());
    index.upsert(document("b", "a b X c d")).unwrap();
    assert_eq!(query(&index, 1).pairs.len(), 1);
    assert!(index.delete("b").unwrap());
    assert!(query(&index, 1).pairs.is_empty());
    assert!(!index.delete("b").unwrap());
}

#[test]
fn unicode_identity_permutations_and_cold_rebuild_agree_even_at_caps() {
    let rows = [
        ("\u{E000}", "x x x x x"),
        ("𐐀", "x x x x"),
        ("a", "x x x x x x"),
    ];
    let mut warm = fixture(&rows);
    warm.upsert(document("temporary", "x x x x")).unwrap();
    warm.delete("temporary").unwrap();
    let cold = fixture(&[rows[2], rows[1], rows[0]]);
    for limits in [
        NearMissLimits::default(),
        NearMissLimits {
            maximum_candidate_pairs: 1,
            ..NearMissLimits::default()
        },
        NearMissLimits {
            maximum_output_pairs: 1,
            ..NearMissLimits::default()
        },
    ] {
        assert_eq!(
            warm.query_near_misses(options(2), limits).unwrap(),
            cold.query_near_misses(options(2), limits).unwrap()
        );
    }
    let result = query(&warm, 2);
    assert_eq!(result.pairs.len(), 3);
    let unicode = result
        .pairs
        .iter()
        .find(|p| p.left.document_id == "𐐀")
        .unwrap();
    assert_eq!(
        unicode.pair_id,
        "sg10.token-near-misses.v1:2:𐐀1:m/1:\u{E000}1:m"
    );
    assert_eq!(result.pairs[0].score, 10.0 / 11.0);
}

#[test]
fn boilerplate_limit_suppresses_the_whole_bucket_and_recovers_after_delete() {
    let mut index = trio();
    let limited = NearMissLimits {
        maximum_postings_per_token: 2,
        ..NearMissLimits::default()
    };
    let result = index.query_near_misses(options(2), limited).unwrap();
    assert!(result.pairs.is_empty());
    assert_eq!(result.statistics.posting_visits, 0);
    assert_eq!(result.statistics.suppressed_posting_buckets, 1);
    assert_eq!(
        result.gaps,
        vec![NearMissGap {
            reason: NearMissGapReason::BoilerplatePostingLimit,
            affected_count: 1,
        }]
    );
    let boundary = index
        .query_near_misses(
            options(2),
            NearMissLimits {
                maximum_postings_per_token: 3,
                ..limited
            },
        )
        .unwrap();
    assert_eq!(boundary.pairs.len(), 3);
    index.delete("c").unwrap();
    let recovered = index.query_near_misses(options(2), limited).unwrap();
    assert!(!recovered.is_truncated());
    assert_eq!(recovered.pairs.len(), 1);
}

#[test]
fn candidate_visit_and_output_caps_report_encountered_frontiers() {
    let index = trio();
    for (limits, reason) in [
        (
            NearMissLimits {
                maximum_candidate_pairs: 1,
                ..NearMissLimits::default()
            },
            NearMissGapReason::CandidatePairLimit,
        ),
        (
            NearMissLimits {
                maximum_posting_visits: 1,
                ..NearMissLimits::default()
            },
            NearMissGapReason::PostingVisitLimit,
        ),
        (
            NearMissLimits {
                maximum_output_pairs: 1,
                ..NearMissLimits::default()
            },
            NearMissGapReason::OutputPairLimit,
        ),
    ] {
        let result = index.query_near_misses(options(2), limits).unwrap();
        assert_eq!(result.pairs.len(), 1);
        assert_eq!(
            result.pairs[0].pair_id,
            "sg10.token-near-misses.v1:1:a1:m/1:b1:m"
        );
        assert!(has_gap(&result, reason));
        assert!(result.statistics.candidate_pairs <= limits.maximum_candidate_pairs);
        assert!(result.statistics.posting_visits <= limits.maximum_posting_visits);
    }
    let boundary = NearMissLimits {
        maximum_candidate_pairs: 3,
        maximum_posting_visits: 3,
        maximum_output_pairs: 3,
        ..NearMissLimits::default()
    };
    assert!(
        !index
            .query_near_misses(options(2), boundary)
            .unwrap()
            .is_truncated()
    );
    // Equal document frequencies leave two shared prefix atoms for this single pair.
    let result = fixture(&[("a", "a b c d"), ("b", "a c b d")])
        .query_near_misses(
            NearMissOptions {
                minimum_similarity_percent: 75,
                maximum_edits: 2,
            },
            NearMissLimits {
                maximum_posting_visits: 1,
                ..NearMissLimits::default()
            },
        )
        .unwrap();
    assert_eq!(result.statistics.candidate_pairs, 1);
    assert_eq!(result.pairs.len(), 1);
    assert!(has_gap(&result, NearMissGapReason::PostingVisitLimit));
}

#[test]
fn verification_cell_cap_is_not_a_negative_match_result() {
    let index = fixture(&[("a", "x x x x"), ("b", "x x x x x")]);
    // At radius one, row zero has two active cells; four following rows have three each.
    let limits = NearMissLimits {
        maximum_alignment_cells: 14,
        ..NearMissLimits::default()
    };
    let complete = index.query_near_misses(options(1), limits).unwrap();
    assert_eq!(complete.statistics.alignment_cells, 14);
    assert_eq!(complete.pairs.len(), 1);
    assert!(!complete.is_truncated());
    let partial = index
        .query_near_misses(
            options(1),
            NearMissLimits {
                maximum_alignment_cells: 13,
                ..limits
            },
        )
        .unwrap();
    assert!(partial.pairs.is_empty());
    assert!(has_gap(&partial, NearMissGapReason::AlignmentCellLimit));
    assert_eq!(partial.statistics.verified_pairs, 0);
    assert_eq!(partial.statistics.distance_rejected_pairs, 0);
    assert!(partial.statistics.alignment_cells <= 13);
}

#[test]
fn input_caps_admit_whole_blocks_and_include_utf8_occurrence_bytes() {
    let index = fixture(&[("a", "x é x x"), ("b", "x é x x x")]);
    // Nine token occurrences occupy eleven UTF-8 bytes; each endpoint has 3 metadata bytes.
    let boundary = NearMissLimits {
        maximum_blocks: 2,
        maximum_tokens: 9,
        maximum_token_bytes: 11,
        maximum_metadata_bytes_per_block: 3,
        ..NearMissLimits::default()
    };
    let complete = index.query_near_misses(options(1), boundary).unwrap();
    assert_eq!(complete.pairs.len(), 1);
    assert_eq!(complete.statistics.preprocessed_token_bytes, 11);
    assert!(!complete.is_truncated());
    for (limits, reason) in [
        (
            NearMissLimits {
                maximum_blocks: 1,
                ..boundary
            },
            NearMissGapReason::BlockLimit,
        ),
        (
            NearMissLimits {
                maximum_tokens: 8,
                ..boundary
            },
            NearMissGapReason::TokenLimit,
        ),
        (
            NearMissLimits {
                maximum_token_bytes: 10,
                ..boundary
            },
            NearMissGapReason::TokenByteLimit,
        ),
    ] {
        let result = index.query_near_misses(options(1), limits).unwrap();
        assert!(result.pairs.is_empty());
        assert_eq!(result.statistics.admitted_blocks, 1);
        assert!(has_gap(&result, reason));
    }
    let result = index
        .query_near_misses(
            options(1),
            NearMissLimits {
                maximum_metadata_bytes_per_block: 2,
                ..boundary
            },
        )
        .unwrap();
    assert_eq!(result.statistics.metadata_limited_blocks, 2);
    assert_eq!(result.statistics.admitted_blocks, 0);
    assert!(has_gap(&result, NearMissGapReason::MetadataByteLimit));
}

#[test]
fn invalid_options_and_limits_are_rejected_without_mutating_v1() {
    let index = trio();
    let before = index.query(CloneQueryLimits::default()).unwrap();
    for bad in [
        NearMissOptions {
            minimum_similarity_percent: 0,
            ..options(1)
        },
        NearMissOptions {
            minimum_similarity_percent: 101,
            ..options(1)
        },
        options(0),
    ] {
        assert_eq!(
            index
                .query_near_misses(bad, NearMissLimits::default())
                .unwrap_err(),
            CloneIndexError::InvalidSettings
        );
    }
    for bad in [
        NearMissLimits {
            maximum_blocks: 0,
            ..NearMissLimits::default()
        },
        NearMissLimits {
            maximum_tokens: 0,
            ..NearMissLimits::default()
        },
        NearMissLimits {
            maximum_token_bytes: 0,
            ..NearMissLimits::default()
        },
        NearMissLimits {
            maximum_metadata_bytes_per_block: 0,
            ..NearMissLimits::default()
        },
        NearMissLimits {
            maximum_postings_per_token: 0,
            ..NearMissLimits::default()
        },
        NearMissLimits {
            maximum_posting_visits: 0,
            ..NearMissLimits::default()
        },
        NearMissLimits {
            maximum_candidate_pairs: 0,
            ..NearMissLimits::default()
        },
        NearMissLimits {
            maximum_alignment_cells: 0,
            ..NearMissLimits::default()
        },
        NearMissLimits {
            maximum_output_pairs: 0,
            ..NearMissLimits::default()
        },
    ] {
        assert_eq!(
            index.query_near_misses(options(1), bad).unwrap_err(),
            CloneIndexError::InvalidLimits
        );
    }
    let strict = index
        .query_near_misses(
            NearMissOptions {
                minimum_similarity_percent: 100,
                maximum_edits: usize::MAX,
            },
            NearMissLimits::default(),
        )
        .unwrap();
    assert!(strict.pairs.is_empty());
    assert!(!strict.is_truncated());
    assert_eq!(index.query(CloneQueryLimits::default()).unwrap(), before);
}
