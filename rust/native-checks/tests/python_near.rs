//! Regression coverage for layout-aware Python near-miss matching.
//! Expected token counts and scores below are derived from substantive tokens;
//! indentation and logical boundaries never count as synthetic matching tokens.

use serde_json::{Value, json};
use slophawk_native_checks::base64;
use slophawk_native_checks::contract::{Report, ScopeRequest, Status, hex_digest};
use slophawk_native_checks::shadow;
use std::sync::atomic::AtomicBool;

const RULE: &str = "sg10.python-layout-near-misses.v1";
const LEFT: &str = "def f(value):\n    total = value + 1\n    total = total * 2\n    return total + 3\n";
const RIGHT: &str = "def f(value):\n    total = value + 1\n    total = total * 2\n    return total + 4\n";

fn root() -> &'static str {
    if cfg!(windows) { "C:/kivo/project" } else { "/w/project" }
}

fn source_bytes(language: &str, id: &str, content: &[u8]) -> Value {
    let extension = if language == "python" { "py" } else { "rs" };
    json!({
        "language": language,
        "documentId": id,
        "logicalPath": format!("src/{id}.{extension}"),
        "physicalPath": format!("{}/src/{id}.{extension}", root()),
        "provenance": "normal",
        "bytesBase64": base64::encode(content),
        "sha256": hex_digest(content),
        "byteMapping": "exactUtf8"
    })
}

fn source(language: &str, id: &str, content: &str) -> Value {
    source_bytes(language, id, content.as_bytes())
}

fn request(sources: Vec<Value>) -> Value {
    json!({
        "version": 1,
        "workspaceEpoch": "kivo-epoch",
        "committedRevision": "kivo-source-fixture",
        "workspaceRoot": root(),
        "context": { "key": "python-layout-near", "pythonMinor": 12, "rustEdition": "2024" },
        "sources": sources,
        "limits": { "minCloneTokens": 4, "maxTokensPerBlock": 4096 }
    })
}

fn run(input: Value) -> Report {
    let request: ScopeRequest = serde_json::from_value(input).unwrap();
    shadow(request, &AtomicBool::new(false)).unwrap()
}

fn enabled(sources: Vec<Value>) -> Report {
    let mut input = request(sources);
    input["nearMissClones"] = json!(true);
    run(input)
}

fn query(report: &Report, language: &str) -> Value {
    let rows = serde_json::to_value(&report.near_miss_clones).unwrap();
    rows.as_array().unwrap().iter()
        .find(|row| row["language"] == language).unwrap().clone()
}

fn pairs(query: &Value) -> &[Value] {
    query["pairs"].as_array().unwrap()
}

fn assert_layout_rejected(left: &str, right: &str) {
    let report = enabled(vec![source("python", "a", left), source("python", "b", right)]);
    let row = query(&report, "python");
    // A completed negative comparison must not be the old unsupported result.
    assert_eq!(row["ruleVersion"], RULE);
    assert_eq!(row["status"], "complete", "{row:#}");
    assert!(pairs(&row).is_empty(), "{row:#}");
    assert!(report.pairs.is_empty());
}

fn assert_layout_gap(row: &Value, status: &str, withheld: usize) {
    assert_eq!(row["ruleVersion"], RULE);
    assert_eq!(row["status"], status, "{row:#}");
    assert_eq!(row["gaps"], json!([format!("UnsupportedLayout:{withheld}")]), "{row:#}");
}

#[test]
fn python_near_literal_edit_has_reproducible_score_and_two_byte_spans() {
    let report = enabled(vec![source("python", "a", LEFT), source("python", "b", RIGHT)]);
    let row = query(&report, "python");
    assert_eq!(row["ruleVersion"], RULE);
    assert_eq!(row["status"], "complete");
    assert_eq!(row["minimumSimilarityPercent"], 80);
    assert_eq!(row["maximumEdits"], 64);
    assert_eq!(pairs(&row).len(), 1, "{row:#}");
    let pair = &pairs(&row)[0];
    assert!(pair["pairId"].as_str().unwrap().starts_with(&format!("{RULE}:")));
    assert_eq!(pair["left"]["documentId"], "a");
    assert_eq!(pair["right"]["documentId"], "b");
    assert_eq!(pair["left"]["bytes"], json!({ "start": 0, "end": 78 }));
    assert_eq!(pair["right"]["bytes"], json!({ "start": 0, "end": 78 }));
    assert_eq!(pair["left"]["utf16"], json!([0, 78]));
    assert_eq!(pair["right"]["utf16"], json!([0, 78]));
    assert_eq!(&LEFT.as_bytes()[0..78], LEFT.trim_end().as_bytes());
    assert_eq!(&RIGHT.as_bytes()[0..78], RIGHT.trim_end().as_bytes());
    // Header 6 + assignment 5 + assignment 5 + return 4 = 20 tokens.
    // Replacing literal 3 with 4 costs one deletion and one insertion.
    assert_eq!(pair["left"]["tokenCount"], 20);
    assert_eq!(pair["right"]["tokenCount"], 20);
    assert_eq!(pair["editDistance"], 2);
    assert_eq!(pair["normalizedMatchedTokens"], 19);
    assert_eq!(pair["exactMatchedTokens"], 19);
    assert_eq!(pair["score"].as_f64().unwrap(), 38.0 / 40.0);
    assert!(report.pairs.is_empty());
    assert_eq!(report.worker_invocations, 0);
}

#[test]
fn python_near_formatting_preserves_layout_and_original_coordinates() {
    let left = "def f():\n    value = (1 + 2)\n    return value\n";
    let right = b"\xef\xbb\xbfdef f():\r\n\t# comment\r\n\tvalue = (\r\n\t    1 + 3\r\n\t)\r\n\r\n\treturn value";
    let report = enabled(vec![source("python", "a", left), source_bytes("python", "b", right)]);
    let row = query(&report, "python");
    assert_eq!(pairs(&row).len(), 1, "{row:#}");
    let pair = &pairs(&row)[0];
    assert_eq!(pair["left"]["bytes"], json!({ "start": 0, "end": 45 }));
    assert_eq!(pair["right"]["bytes"], json!({ "start": 3, "end": 68 }));
    assert_eq!(pair["left"]["utf16"], json!([0, 45]));
    assert_eq!(pair["right"]["utf16"], json!([0, 65]));
    assert_eq!(&left.as_bytes()[0..45], left.trim_end().as_bytes());
    assert_eq!(&right[3..68], &right[3..]);
    // Header 5 + parenthesized assignment 7 + return 2 = 14 tokens.
    assert_eq!(pair["left"]["tokenCount"], 14);
    assert_eq!(pair["right"]["tokenCount"], 14);
    assert_eq!(pair["editDistance"], 2);
    assert_eq!(pair["normalizedMatchedTokens"], 13);
    assert_eq!(pair["exactMatchedTokens"], 13);
    assert_eq!(pair["score"].as_f64().unwrap(), 26.0 / 28.0);
}

#[test]
fn python_near_statement_insertion_shifts_nested_line_ordinals() {
    let left = "def f(flag):\n    value = 1\n    if flag:\n        value = value + 2\n        if value:\n            value = value * 3\n    return value\n";
    let inserted = "def f(flag):\n    value = 1\n    value = value + 0\n    if flag:\n        value = value + 2\n        if value:\n            value = value * 3\n    return value\n";
    let report = enabled(vec![source("python", "a", left), source("python", "b", inserted)]);
    let row = query(&report, "python");
    assert_eq!(pairs(&row).len(), 1, "{row:#}");
    let pair = &pairs(&row)[0];
    // An inserted five-token statement shifts both later suite-opening lines.
    // All 27 old tokens retain their original suite owner and logical line.
    assert_eq!(pair["left"]["tokenCount"], 27);
    assert_eq!(pair["right"]["tokenCount"], 32);
    assert_eq!(pair["editDistance"], 5);
    assert_eq!(pair["normalizedMatchedTokens"], 27);
    assert_eq!(pair["exactMatchedTokens"], 27);
    assert_eq!(pair["score"].as_f64().unwrap(), 54.0 / 59.0);

    // Deliberate recall bound: a complete-line gap plus an independent
    // replacement is outside the pure insertion/deletion whole-line gate.
    let mixed = inserted.replacen("value = 1", "value = 9", 1);
    assert_layout_rejected(left, &mixed);
}

#[test]
fn python_near_outdented_return_cannot_be_hidden_as_extra_token_edits() {
    let left = "def f(flag):\n    value = 0\n    value = value + 1\n    if flag:\n        value = value * 2\n        return value + 3\n";
    let right = "def f(flag):\n    value = 0\n    value = value + 1\n    if flag:\n        value = value * 4\n    return value + 3\n";
    // Both streams have 26 tokens and lexical distance 2. Matching only
    // depth-compatible tokens must not spend extra edits to erase the return.
    assert_layout_rejected(left, right);
    assert_layout_rejected(right, left);
}

#[test]
fn python_near_logical_statement_split_or_merge_is_not_a_continuation() {
    let joined = "def f():\n    value = 1 + 2\n    value = value * 3\n    return value + 4\n";
    let split = "def f():\n    value = 1\n    + 2\n    value = value * 5\n    return value + 4\n";
    // Independent 3 -> 5 replacement prevents distance-zero exclusion from
    // concealing a missing logical-line check. Both have 19 substantive tokens.
    assert_layout_rejected(joined, split);
    assert_layout_rejected(split, joined);
}

#[test]
fn python_near_equal_depth_sibling_suites_keep_distinct_owners() {
    let left = "def f(first, second):\n    value = 0\n    if first:\n        value = value + 1\n        value = value * 2\n    if second:\n        value = value + 3\n    return value\n";
    let right = "def f(first, second):\n    value = 0\n    if first:\n        value = value + 4\n    if second:\n        value = value + 3\n        value = value * 2\n    return value\n";
    // The moved statement has equal depth but a different header owner.
    // A flat script of at most 12 edits meets 80% for these 34-token blocks.
    assert_layout_rejected(left, right);
}

#[test]
fn python_near_inline_wrap_is_unavailable_in_both_orientations() {
    let plain = "def f(flag):\n    value = 1\n    value = value + 2\n    return value\n";
    let wrapped = "def f(flag):\n    value = 1\n    value = value + 2\n    if flag: return value\n";
    // The 16-token stream is a subsequence of the 19-token stream. Flat d=3
    // and unchanged INDENT facts cannot establish this inline suite's owner.
    for (left, right) in [(plain, wrapped), (wrapped, plain)] {
        let report = enabled(vec![source("python", "a", left), source("python", "b", right)]);
        let row = query(&report, "python");
        assert_layout_gap(&row, "partial", 1);
        assert!(row["reason"].is_null());
        assert!(pairs(&row).is_empty(), "{row:#}");
        assert!(report.pairs.is_empty());
        assert!(report.documents.iter().all(|document| document.blocks_indexed == 1));
    }

    // Withholding one block must retain a valid comparison among the rest.
    let edited = plain.replacen("+ 2", "+ 3", 1);
    let report = enabled(vec![source("python", "a", plain), source("python", "b", &edited),
        source("python", "wrapped", wrapped)]);
    let row = query(&report, "python");
    assert_layout_gap(&row, "partial", 1);
    assert_eq!(pairs(&row).len(), 1, "{row:#}");
    assert_eq!(pairs(&row)[0]["left"]["documentId"], "a");
    assert_eq!(pairs(&row)[0]["right"]["documentId"], "b");
    assert_eq!(pairs(&row)[0]["editDistance"], 2);
}

#[test]
fn python_near_inline_only_functions_are_unsupported() {
    let left = "def f(value): return value + 1\n";
    let right = "def f(value): return value + 2\n";
    let report = enabled(vec![source("python", "a", left), source("python", "b", right)]);
    let row = query(&report, "python");
    assert_layout_gap(&row, "unsupported", 2);
    assert_eq!(row["reason"], "unqualified_statement_layout");
    assert!(pairs(&row).is_empty());
    assert!(report.documents.iter().all(|document| document.blocks_indexed == 1
        && document.rules.iter().any(|rule| rule.id == RULE && rule.status == Status::Unsupported)));
}

#[test]
fn python_near_semicolon_statements_are_unsupported() {
    let left = "def f(value):\n    total = value + 1; total = total * 2\n    return total + 3\n";
    let right = "def f(value):\n    total = value + 1; total = total * 2\n    return total + 4\n";
    // Both sides retain the same logical lines; a lexical literal replacement
    // would otherwise pass without qualification of the semicolon boundary.
    let report = enabled(vec![source("python", "a", left), source("python", "b", right)]);
    let row = query(&report, "python");
    assert_layout_gap(&row, "unsupported", 2);
    assert_eq!(row["reason"], "unqualified_statement_layout");
    assert!(pairs(&row).is_empty());
    assert!(report.pairs.is_empty());
}

#[test]
fn python_near_body_dicts_and_annotations_are_conservative_misses() {
    for left in [
        "def f():\n    value = {\"key\": 1}\n    return value\n",
        "def f():\n    value: int = 1\n    return value\n",
    ] {
        let right = left.replacen('1', "2", 1);
        let report = enabled(vec![source("python", "a", left), source("python", "b", &right)]);
        let row = query(&report, "python");
        // These valid colons are not inline suites, but the bounded punctuation
        // gate deliberately withholds them rather than claiming complete search.
        assert_layout_gap(&row, "unsupported", 2);
        assert_eq!(row["reason"], "unqualified_statement_layout");
        assert!(pairs(&row).is_empty());
        assert!(report.documents.iter().all(|document| document.blocks_indexed == 1));
    }
}

#[test]
fn python_near_typed_headers_that_open_suites_remain_eligible() {
    let left = "def f(value: int) -> int:\n    total = value + 1\n    total = total * 2\n    return total + 3\n";
    let right = "def f(value: int) -> int:\n    total = value + 1\n    total = total * 2\n    return total + 4\n";
    let report = enabled(vec![source("python", "a", left), source("python", "b", right)]);
    let row = query(&report, "python");
    assert_eq!(row["ruleVersion"], RULE);
    assert_eq!(row["status"], "complete", "{row:#}");
    assert!(row["gaps"].as_array().unwrap().is_empty());
    assert_eq!(pairs(&row).len(), 1, "{row:#}");
    let pair = &pairs(&row)[0];
    // Header 10 + assignment 5 + assignment 5 + return 4 = 24 tokens.
    // Parameter and final header colons both belong to the opening line.
    assert_eq!(pair["left"]["tokenCount"], 24);
    assert_eq!(pair["right"]["tokenCount"], 24);
    assert_eq!(pair["editDistance"], 2);
    assert_eq!(pair["normalizedMatchedTokens"], 23);
    assert_eq!(pair["exactMatchedTokens"], 23);
    assert_eq!(pair["score"].as_f64().unwrap(), 46.0 / 48.0);
}

#[test]
fn python_near_repeated_locals_preserve_normalized_and_exact_scoring() {
    let consistent = "def f(other):\n    result = other + 1\n    result = result * 2\n    return result + 4\n";
    let changed_use = "def f(other):\n    result = other + 1\n    result = other * 2\n    return result + 4\n";
    for (right, distance, matched) in [(consistent, 2, 19), (changed_use, 4, 18)] {
        let report = enabled(vec![source("python", "a", LEFT), source("python", "b", right)]);
        let row = query(&report, "python");
        assert_eq!(pairs(&row).len(), 1, "{row:#}");
        let pair = &pairs(&row)[0];
        assert_eq!(pair["editDistance"], distance);
        assert_eq!(pair["normalizedMatchedTokens"], matched);
        // Six local occurrences change spelling in the consistent case;
        // changing one repeated use is another edit, not a wildcard match.
        assert_eq!(pair["exactMatchedTokens"], 13);
        assert_eq!(pair["score"].as_f64().unwrap(), 2.0 * f64::from(matched) / 40.0);
    }
}

#[test]
fn python_near_same_document_disjoint_blocks_remain_eligible() {
    let disjoint = format!("{LEFT}\n{}", RIGHT.replacen("def f", "def g", 1));
    let report = enabled(vec![source("python", "both", &disjoint)]);
    let row = query(&report, "python");
    assert_eq!(report.documents[0].blocks_indexed, 2);
    assert_eq!(pairs(&row).len(), 1, "{row:#}");
    let pair = &pairs(&row)[0];
    assert_eq!(pair["left"]["documentId"], "both");
    assert_eq!(pair["right"]["documentId"], "both");
    assert_eq!(pair["editDistance"], 4); // Function name and final literal.
    let left_end = pair["left"]["bytes"]["end"].as_u64().unwrap();
    let right_start = pair["right"]["bytes"]["start"].as_u64().unwrap();
    assert!(left_end <= right_start);

    let nested = "def outer():\n    def inner():\n        value = 1\n        value = value + 2\n        value = value + 3\n        value = value + 4\n        value = value + 5\n        return value\n    return 0\n";
    let nested_report = enabled(vec![source("python", "nested", nested)]);
    assert_eq!(nested_report.documents[0].blocks_indexed, 2);
    assert!(pairs(&query(&nested_report, "python")).is_empty());
    // The index-level fixture separately isolates overlap exclusion from the
    // additional suite-shape rejection of these enclosing Python functions.
}

#[test]
fn python_near_unmapped_sources_never_get_lexical_fallback() {
    let mut left = source("python", "a", LEFT);
    let mut right = source("python", "b", RIGHT);
    left["byteMapping"] = json!("unavailable");
    right["byteMapping"] = json!("unavailable");
    let report = enabled(vec![left, right]);
    let row = query(&report, "python");
    assert_eq!(row["ruleVersion"], RULE);
    assert_eq!(row["status"], "unsupported");
    assert_eq!(row["reason"], "no_admitted_blocks");
    assert!(pairs(&row).is_empty());
    assert!(report.documents.iter().all(|document|
        document.status == Status::Partial && document.blocks_indexed == 0
        && document.rules.iter().any(|rule| rule.id == RULE && rule.status == Status::Unsupported)));
}

#[test]
fn python_near_partial_binding_coverage_uses_exact_names_only() {
    let left = "def f(value):\n    total = value + external\n    return total + 1\n";
    let right = "def f(value):\n    total = value + external\n    return total + 2\n";
    let renamed = "def f(other):\n    result = other + external\n    return result + 2\n";
    let report = enabled(vec![source("python", "a", left), source("python", "b", right)]);
    let row = query(&report, "python");
    assert_eq!(pairs(&row).len(), 1, "{row:#}");
    let pair = &pairs(&row)[0];
    assert_eq!(pair["editDistance"], 2);
    assert_eq!(pair["normalizedMatchedTokens"], 14);
    assert_eq!(pair["exactMatchedTokens"], 14);
    assert!(report.documents.iter().all(|document| document.status == Status::Partial
        && document.gaps.iter().any(|gap| gap.reason == "python:local_binding_frontier")
        && document.rules.iter().any(|rule| rule.id == RULE && rule.status == Status::Partial)));

    // With local normalization withheld, four name changes plus the literal
    // require ten edits in two 15-token blocks, below the 80% similarity floor.
    let renamed_report = enabled(vec![source("python", "a", left), source("python", "b", renamed)]);
    assert!(pairs(&query(&renamed_report, "python")).is_empty());
}

#[test]
fn python_near_stale_and_malformed_inputs_fail_before_querying() {
    let mut stale = source("python", "b", RIGHT);
    stale["sha256"] = json!(hex_digest(LEFT.as_bytes()));
    let mut malformed = source("python", "b", RIGHT);
    malformed["bytesBase64"] = json!("%%%not_base64%%%");
    for (bad, expected) in [(stale, "SHA-256"), (malformed, "base64")] {
        let mut input = request(vec![source("python", "a", LEFT), bad]);
        input["nearMissClones"] = json!(true);
        let parsed: ScopeRequest = serde_json::from_value(input).unwrap();
        let error = shadow(parsed, &AtomicBool::new(false)).unwrap_err();
        assert!(error.contains(expected), "{error}");
    }

    let report = enabled(vec![source("python", "a", LEFT), source("python", "b", RIGHT),
        source("python", "broken", "def broken(:\n")]);
    assert_eq!(report.status, Status::Failed);
    assert!(report.pairs.is_empty());
    let row = query(&report, "python");
    assert_eq!(row["ruleVersion"], RULE);
    assert_eq!(row["status"], "unsupported");
    assert_eq!(row["reason"], "failed_batch");
    assert!(pairs(&row).is_empty());
    assert!(report.documents.iter().all(|document| document.blocks_indexed == 0));
}

#[test]
fn python_near_opt_in_preserves_existing_exact_and_renamed_pairs() {
    let renamed = "def f(other):\n    result = other + 1\n    result = result * 2\n    return result + 3\n";
    let sources = vec![source("python", "a", LEFT), source("python", "b", LEFT),
        source("python", "c", renamed), source("python", "d", RIGHT)];
    let omitted = run(request(sources.clone()));
    let mut explicit_false = request(sources.clone());
    explicit_false["nearMissClones"] = json!(false);
    let disabled = run(explicit_false);
    assert!(omitted.near_miss_clones.is_empty());
    assert!(disabled.near_miss_clones.is_empty());
    assert!(serde_json::to_value(&omitted).unwrap().get("nearMissClones").is_none());
    assert!(omitted.pairs.iter().any(|pair| pair.classification == "exact"));
    assert!(omitted.pairs.iter().any(|pair| pair.classification == "renamed_proven_local"));
    assert!(omitted.pairs.iter().all(|pair| pair.pair_id.starts_with("sg10.python-layout-clones.v2:")));

    let opted_in = enabled(sources);
    assert_eq!(serde_json::to_value(&omitted.pairs).unwrap(), serde_json::to_value(&disabled.pairs).unwrap());
    assert_eq!(serde_json::to_value(&omitted.pairs).unwrap(), serde_json::to_value(&opted_in.pairs).unwrap());
    assert_eq!(serde_json::to_value(&omitted.clone_queries).unwrap(), serde_json::to_value(&opted_in.clone_queries).unwrap());
    assert_eq!(pairs(&query(&opted_in, "python")).len(), 3);
}

#[test]
fn python_near_does_not_change_other_language_queries() {
    let rust_sources = vec![
        source("rust", "r1", "fn f() { let value = 1; value + 2; }"),
        source("rust", "r2", "fn f() { let value = 1; value + 3; }"),
    ];
    let original = enabled(rust_sources.clone());
    let mut mixed_sources = rust_sources;
    mixed_sources.extend([source("python", "a", LEFT), source("python", "b", RIGHT)]);
    let mixed = enabled(mixed_sources);
    let rust = query(&mixed, "rust");
    assert_eq!(rust, query(&original, "rust"));
    assert_eq!(rust["ruleVersion"], "sg10.token-near-misses.v1");
    assert!(!pairs(&rust).is_empty());
    assert!(pairs(&rust).iter().all(|pair| pair["pairId"].as_str().unwrap()
        .starts_with("sg10.token-near-misses.v1:")));
    assert_eq!(query(&mixed, "python")["ruleVersion"], RULE);
    assert_eq!(mixed.worker_invocations, 0);
}

#[test]
fn python_near_output_cap_is_partial_and_enumeration_independent() {
    // Ten 20-token functions differing in one literal yield 45 eligible
    // unordered pairs. Native near output is capped at 32, not silently full.
    let mut sources: Vec<_> = (10..20).map(|literal| {
        let text = format!("def f(value):\n    total = value + 1\n    total = total * 2\n    return total + {literal}\n");
        source("python", &format!("p{literal}"), &text)
    }).collect();
    let report = enabled(sources.clone());
    let row = query(&report, "python");
    assert_eq!(row["status"], "partial", "{row:#}");
    assert_eq!(pairs(&row).len(), 32);
    assert!(row["gaps"].as_array().unwrap().iter().any(|gap|
        gap.as_str().is_some_and(|reason| reason.starts_with("OutputPairLimit:"))));
    sources.reverse();
    assert_eq!(row, query(&enabled(sources), "python"));
}
