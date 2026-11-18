//! Source-only proposal fixtures. Owner-side execution is required.
use serde_json::{Value, json};
use slophawk_native_checks::base64;
use slophawk_native_checks::contract::{ScopeRequest, Status, hex_digest};
use slophawk_native_checks::shadow;
use std::sync::atomic::AtomicBool;

fn compare_copies(text: &str) -> slophawk_native_checks::contract::Report {
    let root = if cfg!(windows) {
        "C:/nevo/project"
    } else {
        "/nevo/project"
    };
    let sources: Vec<Value> = ["a", "b"]
        .into_iter()
        .map(|id| {
            json!({
                "language": "rust", "documentId": id,
                "logicalPath": format!("src/{id}.rs"),
                "physicalPath": format!("{root}/src/{id}.rs"),
                "provenance": "normal", "byteMapping": "exactUtf8",
                "bytesBase64": base64::encode(text.as_bytes()),
                "sha256": hex_digest(text.as_bytes())
            })
        })
        .collect();
    let request: ScopeRequest = serde_json::from_value(json!({
        "version": 1, "workspaceEpoch": "nevo-attribute-scope",
        "committedRevision": "source-only-fixture", "workspaceRoot": root,
        "linkedRoots": [], "sources": sources,
        "context": { "key": "rust|2024", "rustEdition": "2024" },
        "limits": { "minCloneTokens": 4, "maxTokensPerBlock": 4096 }
    }))
    .unwrap();
    shadow(request, &AtomicBool::new(false)).unwrap()
}

#[test]
fn attributed_bodies_are_excluded_and_clean_neighbor_still_matches() {
    let owners = [
        "#[cfg(any())] fn hidden() { 1 + 2; }",
        "#[cfg_attr(feature = \"x\", custom)] fn hidden() { 1 + 2; }",
        "#[custom] fn hidden() { 1 + 2; }",
        "#[allow(dead_code)] fn hidden() { 1 + 2; }",
        "#[cfg(any())] mod hidden { fn nested() { 1 + 2; } }",
        "#[cfg(any())] impl S { fn hidden() { 1 + 2; } }",
        "mod hidden { #![cfg(any())] fn nested() { 1 + 2; } }",
        "fn hidden() { #[cfg(any())] let x = { 1 + 2; }; }",
    ];
    for owner in owners {
        let text = format!("// café\r\n{owner}\r\nfn visible() {{ 3 + 4; }}");
        let report = compare_copies(&text);
        let start = text.rfind("{ 3 + 4; }").unwrap();
        let clean_span = [start, start + "{ 3 + 4; }".len()];
        assert_eq!(report.pairs.len(), 1, "{owner}: {:?}", report.pairs);
        let pair = &report.pairs[0];
        assert_eq!(pair.classification, "exact");
        assert_eq!(pair.left_bytes, clean_span);
        assert_eq!(pair.right_bytes, clean_span);
        assert_eq!(report.worker_invocations, 0);
        assert!(
            report.documents.iter().all(|document| document.status == Status::Partial)
        );
        assert!(
            report.documents.iter().all(|document| document.blocks_indexed == 1)
        );
    }
}

#[test]
fn crate_inner_attribute_cannot_publish_later_body_matches() {
    let report = compare_copies("#![cfg(any())]\nfn hidden() { 1 + 2; }");
    assert!(report.pairs.is_empty());
    assert!(
        report.documents.iter().all(|document| document.blocks_indexed == 0)
    );
    assert!(report.documents.iter().all(|document| {
        document.rules.iter().any(|rule| {
            rule.id.ends_with("/exact") && rule.status == Status::Unsupported
        })
    }));
}

#[test]
fn documentation_comments_do_not_suppress_exact_body_matches() {
    let report = compare_copies("/// Source documentation\nfn visible() { 3 + 4; }");
    assert_eq!(report.pairs.len(), 1);
    assert_eq!(report.pairs[0].classification, "exact");
    assert!(
        report.documents.iter().all(|document| document.blocks_indexed == 1)
    );
}
