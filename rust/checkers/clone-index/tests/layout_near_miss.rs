//! Regression coverage for layout-aware token alignment.
use slophawk_clone_index::near_miss::*;
use slophawk_clone_index::*;
use std::collections::BTreeMap;

type Layouts = BTreeMap<(String, String), NearMissLayout>;

fn settings() -> CloneIndexSettings {
    CloneIndexSettings {
        minimum_tokens: 1,
        maximum_tokens_per_block: 128,
        maximum_fingerprint_postings: 64,
    }
}

fn block(
    id: &str,
    start: i32,
    lines: &[(u32, &str)],
) -> (CloneTokenBlock, NearMissLayout) {
    let span = CloneSourceSpan {
        start_offset: start,
        end_offset: start + 100,
    };
    let mut tokens = Vec::new();
    let mut layout = Vec::new();
    for (depth, line) in lines {
        for (ordinal, word) in line.split_whitespace().enumerate() {
            tokens.push(CloneToken {
                kind: CloneTokenKind::Other,
                text: word.into(),
            });
            layout.push(TokenLayout {
                indent_depth: *depth,
                starts_logical_line: ordinal == 0,
            });
        }
    }
    (
        CloneTokenBlock {
            block_id: id.into(),
            span,
            tokens,
        },
        NearMissLayout {
            document_version: "1".into(),
            span,
            tokens: layout,
        },
    )
}

fn document(id: &str, lines: &[(u32, &str)]) -> (CloneTokenDocument, NearMissLayout) {
    let (block, layout) = block("m", 0, lines);
    (
        CloneTokenDocument {
            document_id: id.into(),
            version: "1".into(),
            is_generated: false,
            is_excluded: false,
            blocks: vec![block],
        },
        layout,
    )
}

fn fixture(rows: &[(&str, &[(u32, &str)])]) -> (TokenCloneIndex, Layouts) {
    let mut index = TokenCloneIndex::new(settings()).unwrap();
    let mut layouts = Layouts::new();
    for (id, lines) in rows {
        let (document, layout) = document(id, lines);
        layouts.insert(((*id).into(), "m".into()), layout);
        index.upsert(document).unwrap();
    }
    (index, layouts)
}

fn query(
    index: &TokenCloneIndex,
    layouts: &Layouts,
    options: NearMissOptions,
    limits: NearMissLimits,
) -> NearMissQueryResult {
    index
        .query_near_misses_with_layout(options, limits, |location| {
            layouts
                .get(&(location.document_id.clone(), location.block_id.clone()))
                .cloned()
        })
        .unwrap()
}

fn has_gap(result: &NearMissQueryResult, reason: NearMissGapReason) -> bool {
    result.gaps.iter().any(|gap| gap.reason == reason)
}

#[test]
fn substantive_replacement_keeps_scoring_spans_and_a_separate_identity() {
    let (index, layouts) = fixture(&[
        ("a", &[(0, "x = 1 + 2")]),
        ("b", &[(0, "x = 9 + 2")]),
    ]);
    let before = index.query(CloneQueryLimits::default()).unwrap();
    let result = query(
        &index,
        &layouts,
        NearMissOptions::default(),
        NearMissLimits::default(),
    );
    assert!(!result.is_truncated());
    assert_eq!(result.rule_version, PYTHON_NEAR_MISS_RULE_VERSION);
    assert_eq!(result.normalization_rule_version, RULE_VERSION);
    assert_eq!(result.pairs.len(), 1);
    let pair = &result.pairs[0];
    assert_eq!(pair.pair_id, "sg10.python-layout-near-misses.v1:1:a1:m/1:b1:m");
    assert_eq!(pair.edit_distance, 2);
    assert_eq!(pair.normalized_matched_tokens, 4);
    assert_eq!(pair.exact_matched_tokens, 4);
    assert_eq!(pair.score, 0.8);
    assert_eq!(pair.left.token_count, 5);
    assert_eq!(pair.right.token_count, 5);
    assert_eq!(pair.left.span, layouts[&("a".into(), "m".into())].span);
    assert_eq!(pair.right.span, layouts[&("b".into(), "m".into())].span);
    assert_eq!(index.query(CloneQueryLimits::default()).unwrap(), before);
    assert_eq!(
        index
            .query_near_misses(NearMissOptions::default(), NearMissLimits::default())
            .unwrap()
            .rule_version,
        NEAR_MISS_RULE_VERSION,
    );
}

#[test]
fn token_insertion_and_deletion_inside_a_logical_line_are_symmetric() {
    for (left, right) in [
        ("return a + b", "return a + b + c"),
        ("return a + b + c", "return a + b"),
    ] {
        let (index, layouts) = fixture(&[("a", &[(0, left)]), ("b", &[(0, right)])]);
        let result = query(
            &index,
            &layouts,
            NearMissOptions::default(),
            NearMissLimits::default(),
        );
        assert_eq!(result.pairs.len(), 1);
        let pair = &result.pairs[0];
        assert_eq!(pair.edit_distance, 2);
        assert_eq!(pair.normalized_matched_tokens, 4);
        assert_eq!(pair.exact_matched_tokens, 4);
        assert_eq!(pair.score, 0.8);
    }
}

#[test]
fn a_new_simple_line_does_not_change_nested_suite_correspondence() {
    let original = &[
        (0, "def f ( x ) :"),
        (1, "if x :"),
        (2, "value = 1"),
        (1, "return value"),
    ];
    let inserted = &[
        (0, "def f ( x ) :"),
        (1, "audit ( )"),
        (1, "if x :"),
        (2, "value = 1"),
        (1, "return value"),
    ];
    for (left, right) in [
        (original.as_slice(), inserted.as_slice()),
        (inserted.as_slice(), original.as_slice()),
    ] {
        let (index, layouts) = fixture(&[("a", left), ("b", right)]);
        // An equal-vector postqualifier would reject these unequal lengths.
        assert_ne!(layouts[&("a".into(), "m".into())].tokens.len(),
            layouts[&("b".into(), "m".into())].tokens.len());
        let result = query(
            &index,
            &layouts,
            NearMissOptions::default(),
            NearMissLimits::default(),
        );
        assert_eq!(result.pairs.len(), 1);
        let pair = &result.pairs[0];
        assert_eq!(pair.edit_distance, 3);
        assert_eq!(pair.normalized_matched_tokens, 14);
        assert_eq!(pair.exact_matched_tokens, 14);
        assert_eq!(pair.score, 28.0 / 31.0);
    }
}

#[test]
fn mixed_new_line_and_replacement_is_a_deliberate_conservative_miss() {
    let (index, layouts) = fixture(&[
        ("a", &[
            (0, "def f ( x ) :"),
            (1, "if x :"),
            (2, "value = 1"),
            (1, "return value"),
        ]),
        ("b", &[
            (0, "def f ( x ) :"),
            (1, "audit ( )"),
            (1, "if x :"),
            (2, "value = 2"),
            (1, "return value"),
        ]),
    ]);
    let flat = index
        .query_near_misses(NearMissOptions::default(), NearMissLimits::default())
        .unwrap();
    assert_eq!(flat.pairs.len(), 1);
    assert_eq!(flat.pairs[0].edit_distance, 5);
    let result = query(
        &index,
        &layouts,
        NearMissOptions::default(),
        NearMissLimits::default(),
    );
    assert!(result.pairs.is_empty());
    assert_eq!(result.statistics.layout_rejected_pairs, 1);
    assert!(!result.is_truncated());
}

#[test]
fn outdenting_a_return_cannot_hide_behind_a_genuine_literal_edit() {
    let (index, layouts) = fixture(&[
        ("a", &[
            (0, "def f ( flag ) :"),
            (1, "result = 0"),
            (1, "if flag :"),
            (2, "result = 1"),
            (2, "return result"),
        ]),
        ("b", &[
            (0, "def f ( flag ) :"),
            (1, "result = 0"),
            (1, "if flag :"),
            (2, "result = 2"),
            (1, "return result"),
        ]),
    ]);
    let flat = index
        .query_near_misses(NearMissOptions::default(), NearMissLimits::default())
        .unwrap();
    assert_eq!(flat.pairs.len(), 1);
    assert_eq!(flat.pairs[0].edit_distance, 2);
    let result = query(
        &index,
        &layouts,
        NearMissOptions::default(),
        NearMissLimits::default(),
    );
    assert!(result.pairs.is_empty());
    assert_eq!(result.statistics.layout_rejected_pairs, 1);
}

#[test]
fn split_logical_lines_are_rejected_with_surviving_or_replaced_suffixes() {
    for (left_expression, first_line, second_line) in [
        ("result = 1 + 2", "result = 9", "+ 2"),
        ("result = a + b", "result = a", "- c"),
    ] {
        let (index, layouts) = fixture(&[
            ("a", &[
                (0, "def f ( a ) :"),
                (1, left_expression),
                (1, "return result"),
            ]),
            ("b", &[
                (0, "def f ( a ) :"),
                (1, first_line),
                (1, second_line),
                (1, "return result"),
            ]),
        ]);
        assert_eq!(index
            .query_near_misses(NearMissOptions::default(), NearMissLimits::default())
            .unwrap().pairs.len(), 1);
        let result = query(
            &index,
            &layouts,
            NearMissOptions::default(),
            NearMissLimits::default(),
        );
        assert!(result.pairs.is_empty());
        assert_eq!(result.statistics.layout_rejected_pairs, 1);
    }
}

#[test]
fn a_new_nested_suite_and_moved_sibling_ownership_are_not_simple_line_gaps() {
    let cases: [(&[(u32, &str)], &[(u32, &str)]); 2] = [
        (
            &[(0, "def f ( flag ) :"), (1, "value = 1"), (1, "return value")],
            &[(0, "def f ( flag ) :"), (1, "if flag :"),
                (2, "value = 1"), (1, "return value")],
        ),
        (
            &[(0, "def f ( x ) :"), (1, "if first :"), (2, "value = 1"),
                (2, "return x"), (1, "if second :"), (2, "value = 2")],
            &[(0, "def f ( x ) :"), (1, "if first :"), (2, "value = 1"),
                (1, "if second :"), (2, "value = 2"), (2, "return x")],
        ),
    ];
    for (left, right) in cases {
        let (index, layouts) = fixture(&[("a", left), ("b", right)]);
        assert_eq!(index
            .query_near_misses(NearMissOptions::default(), NearMissLimits::default())
            .unwrap().pairs.len(), 1);
        let result = query(
            &index,
            &layouts,
            NearMissOptions::default(),
            NearMissLimits::default(),
        );
        assert!(result.pairs.is_empty());
        assert_eq!(result.statistics.layout_rejected_pairs, 1);
    }
}

#[test]
fn repeated_locals_keep_substantive_exact_and_normalized_accounting() {
    let rows: [(&str, &[(u32, &str)]); 2] = [
        ("a", &[(0, "x = 1 + x")]),
        ("b", &[(0, "y = 1 + y + 0")]),
    ];
    let (mut index, layouts) = fixture(&rows);
    for (id, lines) in rows {
        let (mut document, _) = document(id, lines);
        for token in &mut document.blocks[0].tokens {
            if token.text == "x" || token.text == "y" {
                token.kind = CloneTokenKind::LocalIdentifier;
            }
        }
        index.upsert(document).unwrap();
    }
    let result = query(
        &index,
        &layouts,
        NearMissOptions::default(),
        NearMissLimits::default(),
    );
    assert_eq!(result.pairs.len(), 1);
    let pair = &result.pairs[0];
    assert_eq!(pair.edit_distance, 2);
    assert_eq!(pair.normalized_matched_tokens, 5);
    assert_eq!(pair.exact_matched_tokens, 3);
    assert_eq!(pair.score, 10.0 / 12.0);
}

#[test]
fn inserted_first_local_does_not_silently_change_normalization() {
    let rows: [(&str, &[(u32, &str)]); 2] = [
        ("a", &[(0, "x x + y y")]),
        ("b", &[(0, "z x x + y y")]),
    ];
    let (mut index, layouts) = fixture(&rows);
    for (id, lines) in rows {
        let (mut document, _) = document(id, lines);
        for token in &mut document.blocks[0].tokens {
            if token.text != "+" {
                token.kind = CloneTokenKind::LocalIdentifier;
            }
        }
        index.upsert(document).unwrap();
    }
    let options = NearMissOptions { maximum_edits: 1, ..NearMissOptions::default() };
    let result = query(&index, &layouts, options, NearMissLimits::default());
    assert!(result.pairs.is_empty());
    assert!(!result.is_truncated());
    assert!(result.unsupported.contains(&NearMissUnsupported::RenamingInvariantAcrossEdits));
}

#[test]
fn exact_match_ties_are_maximized_within_the_allowed_line_alignment() {
    let rows: [(&str, &[(u32, &str)]); 2] = [
        ("a", &[(0, "x +")]),
        ("b", &[(0, "+ y")]),
    ];
    let (mut index, layouts) = fixture(&rows);
    for (id, lines) in rows {
        let (mut document, _) = document(id, lines);
        for token in &mut document.blocks[0].tokens {
            if token.text != "+" {
                token.kind = CloneTokenKind::LocalIdentifier;
            }
        }
        index.upsert(document).unwrap();
    }
    let result = query(
        &index,
        &layouts,
        NearMissOptions { minimum_similarity_percent: 50, maximum_edits: 2 },
        NearMissLimits::default(),
    );
    let pair = &result.pairs[0];
    assert_eq!(pair.edit_distance, 2);
    assert_eq!(pair.normalized_matched_tokens, 1);
    assert_eq!(pair.exact_matched_tokens, 1);
    assert_eq!(pair.score, 0.5);
}

#[test]
fn repeated_tokens_search_all_permitted_multiline_optima() {
    let short = &[(0, "x"), (0, "x")];
    let long = &[(0, "x x"), (0, "x")];
    for (left, right) in [(short.as_slice(), long.as_slice()), (long.as_slice(), short.as_slice())] {
        let (index, layouts) = fixture(&[("a", left), ("b", right)]);
        // A flat LCS may match both shorter-side tokens into the longer first
        // line. Another minimum path preserves both logical lines. Checking
        // one arbitrary flat traceback would lose that valid correspondence.
        let result = query(
            &index,
            &layouts,
            NearMissOptions::default(),
            NearMissLimits::default(),
        );
        assert_eq!(result.pairs.len(), 1);
        let pair = &result.pairs[0];
        assert_eq!(pair.edit_distance, 1);
        assert_eq!(pair.normalized_matched_tokens, 2);
        assert_eq!(pair.exact_matched_tokens, 2);
        assert_eq!(pair.score, 4.0 / 5.0);
        assert_eq!(result.statistics.layout_rejected_pairs, 0);
        assert!(!result.is_truncated());
    }
}

#[test]
fn exact_tokens_with_only_a_layout_change_never_become_a_near_pair() {
    let (index, layouts) = fixture(&[
        ("a", &[(0, "x = 1 + 2")]),
        ("b", &[(0, "x = 1"), (0, "+ 2")]),
    ]);
    let result = query(
        &index,
        &layouts,
        NearMissOptions::default(),
        NearMissLimits::default(),
    );
    assert!(result.pairs.is_empty());
    assert_eq!(result.statistics.v1_pairs_skipped, 1);
    assert!(!result.is_truncated());
}

#[test]
fn missing_stale_and_malformed_sidecars_fail_closed() {
    let (index, layouts) = fixture(&[
        ("a", &[(0, "x = 1 + 2")]),
        ("b", &[(0, "x = 9 + 2")]),
    ]);
    for fault in [
        "missing", "version", "span", "short", "long", "first_start",
        "first_depth", "depth_inside_line", "depth_jump",
    ] {
        let result = index.query_near_misses_with_layout(
            NearMissOptions::default(),
            NearMissLimits::default(),
            |location| {
                let mut layout = layouts
                    .get(&(location.document_id.clone(), location.block_id.clone()))?
                    .clone();
                if location.document_id == "b" {
                    match fault {
                        "missing" => return None,
                        "version" => layout.document_version = "stale".into(),
                        "span" => layout.span.start_offset += 1,
                        "short" => { layout.tokens.pop(); }
                        "long" => layout.tokens.push(layout.tokens[0]),
                        "first_start" => layout.tokens[0].starts_logical_line = false,
                        "first_depth" => layout.tokens[0].indent_depth = 1,
                        "depth_inside_line" => layout.tokens[1].indent_depth = 1,
                        "depth_jump" => {
                            layout.tokens[1].starts_logical_line = true;
                            layout.tokens[1].indent_depth = 2;
                        }
                        _ => unreachable!(),
                    }
                }
                Some(layout)
            },
        );
        assert!(matches!(result, Err(CloneIndexError::InvalidLayout(_))), "{fault}");
    }
    // The unqualified legacy query remains separately callable with its old API.
    assert_eq!(index
        .query_near_misses(NearMissOptions::default(), NearMissLimits::default())
        .unwrap().pairs.len(), 1);
}

#[test]
fn lexical_and_structural_confirmation_share_one_cell_budget() {
    let (index, layouts) = fixture(&[
        ("a", &[(0, "x = 1 + 2")]),
        ("b", &[(0, "x = 9 + 2")]),
    ]);
    let flat = index
        .query_near_misses(NearMissOptions::default(), NearMissLimits::default())
        .unwrap();
    assert_eq!(flat.pairs.len(), 1);
    let maximum = flat.statistics.alignment_cells;
    let result = query(
        &index,
        &layouts,
        NearMissOptions::default(),
        NearMissLimits { maximum_alignment_cells: maximum, ..NearMissLimits::default() },
    );
    assert!(result.pairs.is_empty());
    assert!(has_gap(&result, NearMissGapReason::AlignmentCellLimit));
    assert_eq!(result.statistics.verified_pairs, 0);
    assert_eq!(result.statistics.distance_rejected_pairs, 0);
    assert_eq!(result.statistics.layout_rejected_pairs, 0);
    assert!(result.statistics.alignment_cells <= maximum);
    let complete = query(
        &index,
        &layouts,
        NearMissOptions::default(),
        NearMissLimits::default(),
    );
    assert_eq!(complete.pairs.len(), 1);
    assert!(complete.statistics.alignment_cells > maximum);
}

#[test]
fn layout_lookup_runs_only_after_whole_block_token_and_byte_admission() {
    let (index, layouts) = fixture(&[
        ("a", &[(0, "x = 1 + 2")]),
        ("b", &[(0, "x = 9 + 2")]),
    ]);
    for (limits, reason) in [
        (
            NearMissLimits { maximum_tokens: 5, ..NearMissLimits::default() },
            NearMissGapReason::TokenLimit,
        ),
        (
            NearMissLimits { maximum_token_bytes: 5, ..NearMissLimits::default() },
            NearMissGapReason::TokenByteLimit,
        ),
    ] {
        let mut calls = 0;
        let result = index.query_near_misses_with_layout(
            NearMissOptions::default(),
            limits,
            |location| {
                calls += 1;
                assert_eq!(location.document_id, "a");
                layouts.get(&(location.document_id.clone(), location.block_id.clone())).cloned()
            },
        ).unwrap();
        assert_eq!(calls, 1);
        assert_eq!(result.statistics.admitted_blocks, 1);
        assert!(result.pairs.is_empty());
        assert!(has_gap(&result, reason));
    }
}

#[test]
fn unproven_inline_or_statement_boundaries_are_explicit_coverage_gaps() {
    let cases: [(&[(u32, &str)], &[(u32, &str)]); 4] = [
        (&[(0, "if flag : return value")], &[(0, "if flag : return other")]),
        (&[(0, "x = 1 ; y = 2")], &[(0, "x = 1 ; y = 3")]),
        (&[(0, "x = values [ 1 : 2 ]")], &[(0, "x = values [ 1 : 3 ]")]),
        // Structurally consistent depths cannot authenticate a suite opener
        // whose substantive tokens lack the required terminal colon.
        (
            &[(0, "def f ( x )"), (1, "return x")],
            &[(0, "def f ( x )"), (1, "return y")],
        ),
    ];
    for (left, right) in cases {
        let (index, layouts) = fixture(&[("a", left), ("b", right)]);
        assert_eq!(index
            .query_near_misses(NearMissOptions::default(), NearMissLimits::default())
            .unwrap().pairs.len(), 1);
        let result = query(
            &index,
            &layouts,
            NearMissOptions::default(),
            NearMissLimits::default(),
        );
        assert!(result.pairs.is_empty());
        assert!(has_gap(&result, NearMissGapReason::UnsupportedLayout));
        assert_eq!(result.gaps.iter()
            .find(|gap| gap.reason == NearMissGapReason::UnsupportedLayout)
            .unwrap().affected_count, 2);
        assert_eq!(result.statistics.unsupported_layout_blocks, 2);
        assert_eq!(result.statistics.admitted_blocks, 0);
        assert_eq!(result.statistics.verified_pairs, 0);
        assert_eq!(result.statistics.alignment_cells, 0);
        assert!(result.is_truncated());
    }

    // The gate checks punctuation tokens, not substrings of literal tokens.
    for (left, right) in [
        ("x = ':' + 1", "x = ':' + 2"),
        ("x = ';' + 1", "x = ';' + 2"),
    ] {
        let (index, layouts) = fixture(&[("a", &[(0, left)]), ("b", &[(0, right)])]);
        let result = query(
            &index,
            &layouts,
            NearMissOptions::default(),
            NearMissLimits::default(),
        );
        assert_eq!(result.pairs.len(), 1);
        assert_eq!(result.statistics.unsupported_layout_blocks, 0);
    }

    // Multiple colons on a proven opening line include ordinary annotations.
    let (index, layouts) = fixture(&[
        ("a", &[(0, "def f ( x : int ) :"), (1, "return x + 1")]),
        ("b", &[(0, "def f ( x : int ) :"), (1, "return x + 2")]),
    ]);
    let result = query(
        &index,
        &layouts,
        NearMissOptions::default(),
        NearMissLimits::default(),
    );
    assert_eq!(result.pairs.len(), 1);
    assert_eq!(result.statistics.unsupported_layout_blocks, 0);
}

#[test]
fn unsupported_blocks_still_consume_token_and_byte_preprocessing_budgets() {
    let (index, layouts) = fixture(&[
        ("a", &[(0, "if x : return x")]),
        ("b", &[(0, "x = 1 + 2")]),
        ("c", &[(0, "x = 9 + 2")]),
    ]);
    let complete_admission = query(
        &index,
        &layouts,
        NearMissOptions::default(),
        NearMissLimits::default(),
    );
    assert_eq!(complete_admission.pairs.len(), 1);
    assert_eq!(complete_admission.statistics.unsupported_layout_blocks, 1);
    assert_eq!(complete_admission.statistics.admitted_blocks, 2);
    assert_eq!(complete_admission.statistics.preprocessed_tokens, 15);
    // The unavailable block occupies 11 token bytes; b and c occupy five each.
    assert_eq!(complete_admission.statistics.preprocessed_token_bytes, 21);

    for (limits, reason) in [
        (
            NearMissLimits { maximum_tokens: 10, ..NearMissLimits::default() },
            NearMissGapReason::TokenLimit,
        ),
        (
            NearMissLimits { maximum_token_bytes: 16, ..NearMissLimits::default() },
            NearMissGapReason::TokenByteLimit,
        ),
    ] {
        let mut looked_up = Vec::new();
        let result = index.query_near_misses_with_layout(
            NearMissOptions::default(),
            limits,
            |location| {
                looked_up.push(location.document_id.clone());
                layouts.get(&(location.document_id.clone(), location.block_id.clone())).cloned()
            },
        ).unwrap();
        assert_eq!(looked_up, vec![String::from("a"), String::from("b")]);
        assert!(result.pairs.is_empty());
        assert!(has_gap(&result, NearMissGapReason::UnsupportedLayout));
        assert!(has_gap(&result, reason));
        assert_eq!(result.statistics.unsupported_layout_blocks, 1);
        assert_eq!(result.statistics.admitted_blocks, 1);
        assert_eq!(result.statistics.preprocessed_tokens, 10);
        assert_eq!(result.statistics.preprocessed_token_bytes, 16);
    }
}

#[test]
fn overlapping_blocks_are_skipped_but_adjacent_spans_are_eligible() {
    for (right_start, expected) in [(99, 0), (100, 1)] {
        let mut index = TokenCloneIndex::new(settings()).unwrap();
        let (left, left_layout) = block("left", 0, &[(0, "return a + b")]);
        let (right, right_layout) = block("right", right_start, &[(0, "return a + b + c")]);
        let layouts = Layouts::from([
            (("a".into(), "left".into()), left_layout),
            (("a".into(), "right".into()), right_layout),
        ]);
        index.upsert(CloneTokenDocument {
            document_id: "a".into(),
            version: "1".into(),
            is_generated: false,
            is_excluded: false,
            blocks: vec![left, right],
        }).unwrap();
        let result = query(
            &index,
            &layouts,
            NearMissOptions::default(),
            NearMissLimits::default(),
        );
        assert_eq!(result.pairs.len(), expected);
        assert_eq!(result.statistics.overlapping_pairs_skipped, 1 - expected);
        assert!(!result.is_truncated());
    }
}

#[test]
fn reordered_inputs_and_output_caps_keep_unicode_identity_deterministic() {
    let rows: [(&str, &[(u32, &str)]); 3] = [
        ("\u{E000}", &[(0, "x x x x x")]),
        ("𐐀", &[(0, "x x x x")]),
        ("a", &[(0, "x x x x x x")]),
    ];
    let (left, left_layouts) = fixture(&rows);
    let (right, right_layouts) = fixture(&[rows[2], rows[1], rows[0]]);
    for limits in [
        NearMissLimits::default(),
        NearMissLimits { maximum_output_pairs: 1, ..NearMissLimits::default() },
    ] {
        let a = query(&left, &left_layouts, NearMissOptions::default(), limits);
        let b = query(&right, &right_layouts, NearMissOptions::default(), limits);
        assert_eq!(a, b);
        if limits.maximum_output_pairs == 1 {
            assert_eq!(a.pairs.len(), 1);
            assert!(has_gap(&a, NearMissGapReason::OutputPairLimit));
        } else {
            assert_eq!(a.pairs.len(), 3);
            let unicode = a.pairs.iter().find(|pair| pair.left.document_id == "𐐀").unwrap();
            assert_eq!(unicode.pair_id,
                "sg10.python-layout-near-misses.v1:2:𐐀1:m/1:\u{E000}1:m");
        }
    }
}
