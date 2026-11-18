//! Proposed source-only regression controls; not executed by the NEVO packet.
use serde_json::{Value, json};
use slophawk_native_checks::{base64, bounded_report, shadow};
use slophawk_native_checks::contract::{
    CloneQueryGapReason, CloneQueryNotRunReason, CloneQueryReport, CloneQueryStatus,
    Language, Report, ScopeRequest, Status, hex_digest,
};
use std::sync::atomic::AtomicBool;

// Seven substantive tokens: def, f, (, ), :, return, 1.
const PYTHON: &str = "def f():\n    return 1\n";
const RUST: &str = "fn f() { let x = 1; x + 2; }";

fn source(language: &str, id: &str, text: &str) -> Value {
    let root = if cfg!(windows) { "C:/nevo/project" } else { "/nevo/project" };
    let extension = if language == "python" { "py" } else { "rs" };
    let path = format!("{id}.{extension}");
    json!({"language":language,"documentId":id,"logicalPath":path,
        "physicalPath":format!("{root}/{path}"),"provenance":"normal",
        "bytesBase64":base64::encode(text.as_bytes()),"sha256":hex_digest(text.as_bytes()),
        "byteMapping":"exactUtf8"})
}

fn request(sources: Vec<Value>) -> Value {
    let root = if cfg!(windows) { "C:/nevo/project" } else { "/nevo/project" };
    json!({"version":1,"workspaceEpoch":"nevo","committedRevision":"fixture",
        "workspaceRoot":root,"linkedRoots":[],
        "context":{"key":"source-only-fixture","pythonMinor":12,"rustEdition":"2024"},
        "sources":sources,"limits":{}})
}

fn run(value: Value) -> Report {
    let request: ScopeRequest = serde_json::from_value(value).unwrap();
    shadow(request, &AtomicBool::new(false)).unwrap()
}

fn query(report: &Report, language: Language) -> &CloneQueryReport {
    report.clone_queries.iter().find(|row| row.language == language).unwrap()
}

#[test]
fn complete_query_preserves_hand_counted_work_and_rule_scope() {
    let report = run(request(vec![source("python", "a", PYTHON), source("python", "b", PYTHON)]));
    let row = query(&report, Language::Python);
    assert_eq!(report.clone_queries.len(), 1);
    assert_eq!(row.rule_version, "sg10.token-clones.v1");
    assert_eq!(row.requested_documents, 2);
    assert_eq!(row.status, CloneQueryStatus::Complete);
    assert_eq!(row.not_run_reason, None);
    assert!(row.gap_reasons.is_empty());
    let stats = row.statistics.as_ref().unwrap();
    assert_eq!(stats.document_count, 2);
    assert_eq!(stats.indexed_block_count, 2);
    assert_eq!(stats.candidate_pairs_examined, 1);
    assert_eq!(stats.compared_tokens_charged, 14);
    assert_eq!((row.pairs_returned, row.pairs_retained), (1, 1));
    // A complete index query does not promote unsupported near-clone coverage.
    assert_eq!(report.status, Status::Partial);
    assert_eq!(report.worker_invocations, 0);
}

#[test]
fn python_layout_rejection_is_distinct_from_query_or_report_truncation() {
    let report = run(request(vec![
        source("python", "a", "def f(x):\n    if x:\n        x = 1\n        return x\n"),
        source("python", "b", "def f(x):\n    if x:\n        x = 1\n    return x\n"),
    ]));
    let row = query(&report, Language::Python);
    assert_eq!(row.status, CloneQueryStatus::Complete);
    assert_eq!((row.pairs_returned, row.pairs_accepted, row.pairs_retained), (1, 0, 0));
    assert!(row.gap_reasons.is_empty());
    assert!(report.pairs.is_empty());
}

#[test]
fn generated_and_below_minimum_inputs_are_counted_as_policy_exclusions() {
    let mut generated = source("python", "generated", PYTHON);
    generated["provenance"] = json!("generated");
    let report = run(request(vec![source("python", "a", PYTHON),
        source("python", "b", PYTHON), generated]));
    let row = query(&report, Language::Python);
    let stats = row.statistics.as_ref().unwrap();
    assert_eq!(row.requested_documents, 3);
    assert_eq!(stats.document_count, 3);
    assert_eq!(stats.generated_documents, 1);
    assert_eq!(stats.excluded_documents, 0);
    assert_eq!(stats.indexed_block_count, 2);
    assert_eq!(stats.candidate_pairs_examined, 1);
    assert_eq!(stats.compared_tokens_charged, 14);
    assert_eq!(row.status, CloneQueryStatus::Complete);
    assert_eq!(row.pairs_returned, 1);

    // x, =, 1: three tokens, below the default minimum of four.
    let small = run(request(vec![source("python", "small", "x = 1\n")]));
    let row = query(&small, Language::Python);
    let stats = row.statistics.as_ref().unwrap();
    assert_eq!(row.status, CloneQueryStatus::Complete);
    assert_eq!(stats.document_count, 1);
    assert_eq!(stats.indexed_block_count, 0);
    assert_eq!(stats.below_minimum_blocks, 1);
    assert_eq!(stats.candidate_pairs_examined, 0);
    assert_eq!(stats.compared_tokens_charged, 0);
    assert_eq!(row.pairs_returned, 0);
}

#[test]
fn each_query_budget_has_typed_evidence_and_independent_counters() {
    // Three identical seven-token blocks have three true pairs. Limits below
    // restrict work or output; returned pairs do not claim all three were seen.
    let cases = [
        ("maxCandidatePairs", 1, CloneQueryGapReason::CandidatePairLimit, 1, 14, 1),
        ("maxComparedTokens", 13, CloneQueryGapReason::ComparedTokenLimit, 0, 0, 0),
        ("maxOutputPairs", 1, CloneQueryGapReason::OutputPairLimit, 2, 28, 1),
        ("maxFingerprintPostings", 2, CloneQueryGapReason::BoilerplatePostingLimit, 0, 0, 0),
        ("maxTokensPerBlock", 4, CloneQueryGapReason::OversizedBlock, 0, 0, 0),
    ];
    for (setting, limit, reason, examined, charged, returned) in cases {
        let mut input = request(vec![source("python", "a", PYTHON),
            source("python", "b", PYTHON), source("python", "c", PYTHON)]);
        input["limits"][setting] = json!(limit);
        let report = run(input);
        let row = query(&report, Language::Python);
        let stats = row.statistics.as_ref().unwrap();
        assert_eq!(row.status, CloneQueryStatus::Partial, "{setting}");
        assert_eq!(row.gap_reasons, vec![reason], "{setting}");
        assert_eq!(stats.document_count, 3, "{setting}");
        assert_eq!(stats.candidate_pairs_examined, examined, "{setting}");
        assert_eq!(stats.compared_tokens_charged, charged, "{setting}");
        assert_eq!((row.pairs_returned, row.pairs_retained), (returned, returned), "{setting}");
        if reason == CloneQueryGapReason::OversizedBlock {
            assert_eq!(stats.oversized_blocks, 3);
            assert_eq!(stats.indexed_block_count, 0);
        }
        assert!(report.gaps.iter().any(|gap| gap.reason.starts_with("clone-index:")));
    }
}

#[test]
fn unsupported_blocks_are_not_a_completed_zero_candidate_query() {
    let report = run(request(vec![source("rust", "macro", "fn f() { unknown!(); }")]));
    let row = query(&report, Language::Rust);
    assert_eq!(row.requested_documents, 1);
    assert_eq!(row.status, CloneQueryStatus::NotRun);
    assert_eq!(row.not_run_reason, Some(CloneQueryNotRunReason::NoAdmittedBlocks));
    assert!(row.statistics.is_none());
    assert!(row.gap_reasons.is_empty());
    assert_eq!((row.pairs_returned, row.pairs_retained), (0, 0));
    assert!(report.documents[0].gaps.iter().any(|gap| gap.reason.contains("MacroExpansion")));
    let json = serde_json::to_value(row).unwrap();
    assert_eq!(json["status"], "not_run");
    assert!(json.get("statistics").is_none());
}

#[test]
fn failed_batch_marks_every_requested_language_not_run() {
    let report = run(request(vec![source("python", "bad", "def broken(:\n"),
        source("rust", "a", RUST), source("rust", "b", RUST)]));
    assert_eq!(report.status, Status::Failed);
    assert!(report.pairs.is_empty());
    assert_eq!(report.clone_queries.len(), 2);
    assert_eq!(query(&report, Language::Python).requested_documents, 1);
    assert_eq!(query(&report, Language::Rust).requested_documents, 2);
    for row in &report.clone_queries {
        assert_eq!(row.status, CloneQueryStatus::NotRun);
        assert_eq!(row.not_run_reason, Some(CloneQueryNotRunReason::FailedBatch));
        assert!(row.statistics.is_none());
        assert_eq!((row.pairs_returned, row.pairs_retained), (0, 0));
    }
}

#[test]
fn requested_documents_remain_distinct_from_index_documents() {
    let mut unmapped = source("python", "unmapped", PYTHON);
    unmapped["byteMapping"] = json!("unavailable");
    let report = run(request(vec![source("python", "a", PYTHON),
        source("python", "b", PYTHON), unmapped]));
    let row = query(&report, Language::Python);
    assert_eq!(row.requested_documents, 3);
    assert_eq!(row.statistics.as_ref().unwrap().document_count, 2);
    assert_eq!(row.status, CloneQueryStatus::Complete);
    assert_eq!(row.pairs_returned, 1);
    assert!(report.gaps.iter().any(|gap| gap.reason.starts_with("caller_declared_unmapped")));
}

#[test]
fn mixed_language_rows_and_counters_ignore_source_enumeration_order() {
    let sources = vec![source("rust", "r1", RUST), source("python", "p1", PYTHON),
        source("rust", "r2", RUST), source("python", "p2", PYTHON)];
    let mut reversed = sources.clone();
    reversed.reverse();
    let left = run(request(sources));
    let right = run(request(reversed));
    assert_eq!(serde_json::to_value(&left.clone_queries).unwrap(),
        serde_json::to_value(&right.clone_queries).unwrap());
    assert_eq!(left.clone_queries.iter().map(|row| row.language).collect::<Vec<_>>(),
        vec![Language::Python, Language::Rust]);
    assert_eq!(query(&left, Language::Python).pairs_returned, 1);
    assert_eq!(query(&left, Language::Rust).pairs_returned, 1);
}

#[test]
fn report_byte_trimming_preserves_query_work_and_accounts_for_retained_pairs() {
    let sources = vec![source("python", "p1", PYTHON), source("python", "p2", PYTHON),
        source("python", "p3", PYTHON), source("rust", "r1", RUST),
        source("rust", "r2", RUST), source("rust", "r3", RUST)];
    let mut report = run(request(sources));
    // Three duplicate blocks per language give three pairs each, with no
    // cross-language pair. Derive a byte budget from this concrete receipt;
    // reserve less than its current size without hardcoding formatting bytes.
    assert_eq!(report.pairs.len(), 6);
    let original_queries = serde_json::to_value(&report.clone_queries).unwrap();
    let maximum = serde_json::to_vec_pretty(&report).unwrap().len() - 256;
    assert!((4096..=4 * 1024 * 1024).contains(&maximum));
    report.limits.max_report_bytes = maximum;
    let encoded = bounded_report(report).unwrap();
    assert!(encoded.len() <= maximum);
    let published: Value = serde_json::from_slice(&encoded).unwrap();
    let pairs = published["pairs"].as_array().unwrap();
    assert!(pairs.len() < 6);
    assert!(published["gaps"].as_array().unwrap().iter()
        .any(|gap| gap["reason"] == "report:pair_budget"));
    for (row, original) in published["cloneQueries"].as_array().unwrap().iter()
        .zip(original_queries.as_array().unwrap()) {
        let retained = pairs.iter().filter(|pair| pair["language"] == row["language"]).count();
        assert_eq!(row["status"], "complete");
        assert_eq!(row["statistics"], original["statistics"]);
        assert_eq!(row["pairsReturned"], 3);
        assert_eq!(row["pairsRetained"], json!(retained));
    }
}

#[test]
fn metadata_that_cannot_fit_still_fails_instead_of_omitting_query_evidence() {
    let sources = (0..16).map(|i| source("python", &format!("p{i}"), PYTHON)).collect();
    let mut report = run(request(sources));
    report.limits.max_report_bytes = 4096;
    assert!(bounded_report(report).unwrap_err().contains("scope and gap metadata"));
}
