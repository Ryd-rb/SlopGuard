use slophawk_fluxr_provider::{
    AnalysisFailure, ByteSpan, Capability, Completeness, DeclarationKind, FactBatch, FailureKind,
    Limits, PARSER_COMMIT, PARSER_TREE, Request, RuleGate, SourceFormat, SpanEncoding, TokenShape,
    analyze,
};

const UPSTREAM_EXAMPLE: &[u8] = include_bytes!("../fixtures/checked_affine.fluxr");
const INDEPENDENT: &[u8] = include_bytes!("../fixtures/independent_identity.fluxr");

fn request<'a>(bytes: &'a [u8]) -> Request<'a> {
    Request {
        logical_document_id: "src/example.fluxr",
        declared_module_context: "example-module@target=checked-v1",
        source_bytes: bytes,
        format: SourceFormat::CanonicalChecked,
        limits: Limits::default(),
    }
}

fn expect_failure(bytes: &[u8], expected: FailureKind) -> AnalysisFailure {
    let rejected = analyze(request(bytes)).expect_err("must refuse document");
    assert_eq!(rejected.kind, expected);
    assert!(rejected.message.len() <= 2048);
    rejected
}

fn assert_all_ranges_are_original(facts: &FactBatch, original: &str) {
    assert_eq!(facts.provenance.source_byte_len, original.len());
    assert_eq!(
        facts.provenance.span_encoding,
        SpanEncoding::OriginalUtf8BytesHalfOpen
    );
    for token in &facts.tokens {
        assert!(!token.text.is_empty());
        assert_eq!(token.span.text(original), Some(token.text.as_str()));
        assert!(token.span.start < token.span.end);
        for item in &facts.native_items {
            assert!(
                token.span.end <= item.span.start || token.span.start >= item.span.end,
                "token must not intrude into opaque native item"
            );
        }
    }
    for item in &facts.native_items {
        assert!(item.span.start < item.span.end);
        assert!(item.span.text(original).is_some());
    }
}

#[test]
fn canonical_upstream_fixture_uses_checked_compiler_and_exact_source_identity() {
    let facts = analyze(request(UPSTREAM_EXAMPLE)).expect("pinned canonical fixture must check");
    assert_eq!(facts.provenance.parser_commit, PARSER_COMMIT);
    assert_eq!(facts.provenance.parser_tree, PARSER_TREE);
    assert_eq!(
        facts.provenance.source_sha256,
        "9d50f0dbfbbea4022a2cf690d41ba6f699bd2206be94afe66449376232531bc6"
    );
    assert_eq!(facts.provenance.logical_document_id, "src/example.fluxr");
    assert_eq!(
        facts.provenance.declared_module_context,
        "example-module@target=checked-v1"
    );
    assert_eq!(facts.declarations.len(), 4);
    assert_eq!(facts.declarations[0].kind, DeclarationKind::Function);
    assert_eq!(facts.declarations[0].name, "run");
    assert!(facts.declarations[0].is_exported);
    assert_eq!(facts.declarations[1].kind, DeclarationKind::Parameter);
    assert_eq!(facts.declarations[1].name, "x");
    assert!(facts.declarations.iter().all(|d| d.span.is_none()));
    assert!(facts.native_items.is_empty());
    assert_all_ranges_are_original(&facts, std::str::from_utf8(UPSTREAM_EXAMPLE).unwrap());
    assert_eq!(
        facts.gate(&[
            Capability::CheckedTypeChecking,
            Capability::LexicalCheckedTokens
        ]),
        RuleGate::Eligible
    );
}

#[test]
fn independent_positive_fixture_exposes_inputs_and_function_params() {
    let facts = analyze(request(INDEPENDENT)).expect("independent checked source must compile");
    assert_eq!(facts.declarations.len(), 3);
    assert_eq!(facts.declarations[0].kind, DeclarationKind::Input);
    assert_eq!(facts.declarations[0].name, "seed");
    assert_eq!(facts.declarations[1].kind, DeclarationKind::Function);
    assert_eq!(facts.declarations[1].name, "identity");
    assert_eq!(facts.declarations[2].kind, DeclarationKind::Parameter);
    assert_eq!(facts.declarations[2].name, "value");
    assert_all_ranges_are_original(&facts, std::str::from_utf8(INDEPENDENT).unwrap());
}

#[test]
fn lexical_facts_do_not_pretend_to_be_resolved_identifiers() {
    // `add` is both a grammar operator and a legal ASCII identifier spelling.
    let facts = analyze(request(UPSTREAM_EXAMPLE)).unwrap();
    let add = facts.tokens.iter().find(|t| t.text == "add").unwrap();
    assert_eq!(add.shape, TokenShape::IdentifierLike);
    assert_eq!(
        facts.gate(&[Capability::IdentifierRolesAndBindings]),
        RuleGate::Insufficient {
            unavailable: vec![Capability::IdentifierRolesAndBindings],
        }
    );
    assert_eq!(
        facts.gate(&[Capability::ExactDeclarationSpans]),
        RuleGate::Insufficient {
            unavailable: vec![Capability::ExactDeclarationSpans],
        }
    );
    assert!(facts.coverage.iter().any(|c| {
        c.capability == Capability::CheckedDeclarationInventory
            && c.completeness == Completeness::Complete
    }));
}

#[test]
fn mixed_native_item_preserves_opaque_original_utf8_ranges_only() {
    let raw = "fluxr 1\nrust 2\nβ\npub run 0 int ret 1\n";
    let facts = analyze(request(raw.as_bytes())).expect("native content must be opaque");
    assert_eq!(facts.native_items.len(), 1);
    let item = &facts.native_items[0];
    assert_eq!(item.tag, "rust");
    assert_eq!(
        item.span,
        ByteSpan {
            start: "fluxr 1\n".len(),
            end: "fluxr 1\nrust 2\nβ".len(),
        }
    );
    assert_eq!(item.span.text(raw), Some("rust 2\nβ"));
    assert_eq!(facts.declarations[0].name, "run");
    assert!(!facts.tokens.iter().any(|t| t.text == "β"));
    assert_all_ranges_are_original(&facts, raw);
    for unavailable in [
        Capability::NativeRustSemantics,
        Capability::FullDocumentSemantics,
    ] {
        assert!(matches!(
            facts.gate(&[unavailable]),
            RuleGate::Insufficient { .. }
        ));
    }
}

#[test]
fn native_use_is_also_opaque_and_does_not_hide_checked_tokens() {
    let raw = "fluxr 1\nnative-use std::fmt\npub read 0 int ret 7\n";
    let facts = analyze(request(raw.as_bytes())).unwrap();
    assert_eq!(facts.native_items.len(), 1);
    assert_eq!(facts.native_items[0].tag, "native-use");
    assert_eq!(
        facts.native_items[0].span.text(raw),
        Some("native-use std::fmt")
    );
    assert!(facts.tokens.iter().any(|t| t.text == "read"));
    assert!(!facts.tokens.iter().any(|t| t.text == "std::fmt"));
    assert_all_ranges_are_original(&facts, raw);
}

#[test]
fn multiple_native_items_keep_independent_upstream_byte_spans() {
    let raw = "fluxr 1\nnative-use std::fmt\nrust 2\nβ\npub run 0 int ret 1\n";
    let facts = analyze(request(raw.as_bytes())).expect("valid mixed source must compile");
    assert_eq!(facts.native_items.len(), 2);
    assert_eq!(facts.native_items[0].tag, "native-use");
    assert_eq!(facts.native_items[1].tag, "rust");
    assert_eq!(
        facts.native_items[0].span.text(raw),
        Some("native-use std::fmt")
    );
    assert_eq!(facts.native_items[1].span.text(raw), Some("rust 2\nβ"));
    assert!(facts.native_items[0].span.end < facts.native_items[1].span.start);
    assert!(
        !facts
            .tokens
            .iter()
            .any(|t| t.text == "std::fmt" || t.text == "β")
    );
    assert_all_ranges_are_original(&facts, raw);
}

#[test]
fn checked_crlf_and_vertical_whitespace_do_not_rebase_spans() {
    let raw = "fluxr 1\npub\x0bidentity 1 value int int ret\x0cvalue\r\n";
    let facts = analyze(request(raw.as_bytes())).unwrap();
    assert_all_ranges_are_original(&facts, raw);
    assert!(
        facts
            .tokens
            .iter()
            .any(|token| { token.text == "value" && token.span.text(raw) == Some("value") })
    );
    assert_eq!(facts.tokens.first().unwrap().span.start, 0);
    assert_eq!(facts.tokens.first().unwrap().text, "fluxr");
}

#[test]
fn malformed_syntax_is_refused_with_source_digest() {
    let malformed = b"fluxr 1\npub f 0 int ret add 1\n";
    let failure = expect_failure(malformed, FailureKind::ParseRejected);
    assert!(failure.source_sha256.is_some());
    assert_eq!(failure.source_byte_len, malformed.len());
    assert_eq!(failure.logical_document_id, "src/example.fluxr");
}

#[test]
fn checked_compiler_failure_does_not_emit_validated_facts() {
    let invalid_binding = b"fluxr 1\npub f 0 int ret missing\n";
    let failure = expect_failure(invalid_binding, FailureKind::CheckRejected);
    assert!(failure.source_sha256.is_some());
}

#[test]
fn exact_header_and_source_encoding_are_enforced_without_normalization() {
    assert_eq!(
        expect_failure(
            b"fluxr 1\r\npub f 0 int ret 1\n",
            FailureKind::ParseRejected
        )
        .kind,
        FailureKind::ParseRejected
    );
    expect_failure(b"fa1 v2\npub f 0 int ret 1\n", FailureKind::ParseRejected);
    expect_failure(b"\xef\xbb\xbffluxr 1\n", FailureKind::UnsupportedEncoding);
    expect_failure(b"fluxr 1\n\xff", FailureKind::InvalidUtf8);
}

#[test]
fn source_changes_and_declared_context_changes_are_attributed_separately() {
    let original = analyze(request(INDEPENDENT)).unwrap();
    let modified = analyze(request(
        b"fluxr 1\ninputs 1 seed int\npub identity 1 value int int ret value \n",
    ))
    .unwrap();
    assert_ne!(
        original.provenance.source_sha256,
        modified.provenance.source_sha256
    );
    let mut alternate = request(INDEPENDENT);
    alternate.declared_module_context = "another-target";
    let moved = analyze(alternate).unwrap();
    assert_eq!(
        original.provenance.source_sha256,
        moved.provenance.source_sha256
    );
    assert_ne!(
        original.provenance.declared_module_context,
        moved.provenance.declared_module_context
    );
}

#[test]
fn oversized_input_is_rejected_before_source_copy_or_hash() {
    let oversized = format!("fluxr 1\n{}pub f 0 int ret 1", " ".repeat(2048));
    let mut limited = request(oversized.as_bytes());
    limited.limits.max_source_bytes = 256;
    let failure = analyze(limited).expect_err("byte limit must stop upstream parsing");
    assert_eq!(failure.kind, FailureKind::SourceTooLarge);
    assert!(failure.source_sha256.is_none());
}

#[test]
fn too_many_facts_produces_failure_not_a_short_clean_batch() {
    let mut limited = request(UPSTREAM_EXAMPLE);
    limited.limits.max_facts = 4;
    assert_eq!(
        analyze(limited).unwrap_err().kind,
        FailureKind::FactLimitExceeded
    );
    let mut with_native = request(b"fluxr 1\nrust 0\n\npub f 0 int ret 1\n");
    with_native.limits.max_facts = 1;
    assert_eq!(
        analyze(with_native).unwrap_err().kind,
        FailureKind::FactLimitExceeded
    );
}

#[test]
fn invalid_limits_and_unbounded_identity_are_not_admitted() {
    let mut limited = request(INDEPENDENT);
    limited.limits.max_facts = 0;
    assert_eq!(
        analyze(limited).unwrap_err().kind,
        FailureKind::InvalidLimits
    );
    let mut limited = request(INDEPENDENT);
    limited.limits.max_source_bytes = 8 * 1024 * 1024 + 1;
    assert_eq!(
        analyze(limited).unwrap_err().kind,
        FailureKind::InvalidLimits
    );
    let mut invalid = request(INDEPENDENT);
    invalid.declared_module_context = "line\nbreak";
    assert_eq!(
        analyze(invalid).unwrap_err().kind,
        FailureKind::InvalidIdentity
    );
}

#[test]
fn alternate_source_formats_and_extensions_are_explicitly_unsupported() {
    for format in [SourceFormat::Compact, SourceFormat::NativeHost] {
        let mut unsupported = request(UPSTREAM_EXAMPLE);
        unsupported.format = format;
        assert_eq!(
            analyze(unsupported).unwrap_err().kind,
            FailureKind::UnsupportedFormat
        );
    }
    let mut unsupported = request(UPSTREAM_EXAMPLE);
    unsupported.logical_document_id = "other/source.compact";
    assert_eq!(
        analyze(unsupported).unwrap_err().kind,
        FailureKind::UnsupportedFormat
    );
}

#[test]
fn unavailable_capability_cannot_silently_upgrade_to_clean() {
    let facts = analyze(request(UPSTREAM_EXAMPLE)).unwrap();
    for requirement in [
        Capability::CheckerSemanticFacts,
        Capability::ExactDeclarationSpans,
        Capability::IdentifierRolesAndBindings,
        Capability::NativeRustSemantics,
        Capability::CompactSource,
        Capability::FullDocumentSemantics,
    ] {
        let decision = facts.gate(&[Capability::CheckedSyntax, requirement]);
        assert_eq!(
            decision,
            RuleGate::Insufficient {
                unavailable: vec![requirement],
            }
        );
    }
}

#[test]
fn split_utf8_native_raw_byte_count_is_rejected_by_upstream() {
    // The native length of one byte would split the two-byte UTF-8 beta.
    expect_failure(
        "fluxr 1\nrust 1\nβ\npub f 0 int ret 1\n".as_bytes(),
        FailureKind::ParseRejected,
    );
}
