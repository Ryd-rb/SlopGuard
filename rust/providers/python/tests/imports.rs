use slophawk_python_syntax_provider::imports::{
    self, HARD_MAX_DOCUMENT_ID_BYTES, HARD_MAX_FACTS, HARD_MAX_NAME_BYTES, HARD_MAX_PATH_BYTES,
    HARD_MAX_TOKENS, ImportFacts, UnsupportedKind,
};
use slophawk_python_syntax_provider::{
    ByteSpan, Coverage, Encoding, GapKind, HARD_MAX_SOURCE_BYTES, Limits, Outcome, PythonTarget,
    SourceRequest, analyze,
};

fn check(bytes: &[u8]) -> ImportFacts {
    imports::extract(SourceRequest::new("module", "pkg/consumer.py", bytes)).unwrap()
}

#[test]
fn absolute_aliases_keep_source_order_and_whole_statement_witnesses() {
    let source = b"# header\r\nimport pkg.api as api, pkg.low\r\nimport pkg.api; import other._2\n";
    let facts = check(source);
    assert!(facts.is_complete());
    let names: Vec<_> = facts.imports.iter().map(|fact| fact.name.as_str()).collect();
    assert_eq!(names, ["pkg.api", "pkg.low", "pkg.api", "other._2"]);
    assert_eq!(facts.imports[0].span, facts.imports[1].span);
    assert_eq!(
        facts.imports[0].span.slice(source),
        Some(b"import pkg.api as api, pkg.low".as_slice())
    );
    assert_eq!(
        facts.imports[2].span.slice(source),
        Some(b"import pkg.api".as_slice())
    );
    assert_eq!(
        facts.imports[3].span.slice(source),
        Some(b"import other._2".as_slice())
    );
}

#[test]
fn original_bom_and_continuation_bytes_are_preserved() {
    let source = b"\xef\xbb\xbfimport pkg.api as api, \\\r\n    pkg.low\r\n";
    let facts = check(source);
    assert_eq!(facts.identity.encoding, Encoding::Utf8Bom);
    assert_eq!(facts.identity.original_bytes, source.len());
    assert_eq!(facts.imports.len(), 2);
    let expected = ByteSpan { start: 3, end: (source.len() - 2) as u32 };
    assert_eq!(facts.imports[0].span, expected);
    assert_eq!(facts.imports[1].span, expected);
    assert_eq!(
        expected.slice(source),
        Some(b"import pkg.api as api, \\\r\n    pkg.low".as_slice())
    );
}

#[test]
fn empty_comments_pass_and_plain_strings_have_no_unknown_frontier() {
    for source in ["", "# import pkg.hidden\n", "pass\n", "'import pkg.hidden'\n"] {
        let facts = check(source.as_bytes());
        assert!(facts.is_complete());
        assert!(facts.imports.is_empty());
    }
    let empty = check(b"");
    assert_eq!(
        empty.identity.source_sha256,
        "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
    );
    assert_eq!(empty.identity.provider, imports::PROVIDER_ID);
    assert_eq!(empty.identity.schema, "python-direct-import-syntax/v1");
    assert_eq!(empty.identity.parser_version, "0.0.22");
}

#[test]
fn from_and_relative_imports_retain_unknown_even_for_package_initializers() {
    let source = b"from pkg import api\nfrom . import low\nfrom ..api import Thing\nfrom pkg.api import *\n";
    for path in ["pkg/consumer.py", "pkg/__init__.py"] {
        let facts = imports::extract(SourceRequest::new("module", path, source)).unwrap();
        assert!(facts.imports.is_empty());
        let kinds: Vec<_> = facts.unsupported.iter().map(|fact| fact.kind).collect();
        assert_eq!(kinds, [
            UnsupportedKind::FromImport,
            UnsupportedKind::RelativeImport,
            UnsupportedKind::RelativeImport,
            UnsupportedKind::FromImport,
        ]);
        assert!(!facts.is_complete());
    }
}

#[test]
fn guard_names_and_runtime_else_branches_are_not_guessed() {
    for guard in ["TYPE_CHECKING", "typing.TYPE_CHECKING", "unbound.TYPE_CHECKING", "True"] {
        let source = format!(
            "if {guard}:\n    import pkg.api\nelse:\n    import pkg.low\n"
        );
        let facts = check(source.as_bytes());
        assert!(facts.imports.is_empty());
        assert_eq!(facts.unsupported.len(), 1);
        assert_eq!(facts.unsupported[0].kind, UnsupportedKind::UnresolvedGuard);
        assert_eq!(facts.unsupported[0].span.start, 0);
        assert_eq!(facts.unsupported[0].span.end as usize, source.len() - 1);
    }
    let source = b"try:\n    import pkg.api\nexcept ImportError:\n    import pkg.low\n";
    let facts = check(source);
    assert!(facts.imports.is_empty());
    assert_eq!(facts.unsupported[0].kind, UnsupportedKind::UnresolvedGuard);
}

#[test]
fn hidden_dynamic_work_and_nested_imports_do_not_disappear() {
    let cases = [
        "importlib.import_module('pkg.api')\n",
        "loader = __import__\n",
        "loader('pkg.api')\n",
        "def load():\n    import pkg.api\n",
        "def load(value=__import__('pkg.api')):\n    pass\n",
        "class C:\n    import pkg.api\n",
        "for item in items:\n    import pkg.api\n",
        "with scope():\n    import pkg.api\n",
        "f'{__import__(name)}'\n",
        "b'import pkg.api'\n",
        "value = 1\n",
    ];
    for source in cases {
        let facts = check(source.as_bytes());
        assert!(facts.imports.is_empty(), "{source}");
        assert_eq!(facts.unsupported.len(), 1, "{source}");
        assert_eq!(
            facts.unsupported[0].kind,
            UnsupportedKind::DynamicOrUnsupportedStatement,
            "{source}"
        );
    }
}

#[test]
fn original_non_ascii_names_are_unknown_even_after_nfkc_normalization() {
    let source = "import pkg.api, pkg.café, pkg.K, pkg.__init__\n";
    let facts = check(source.as_bytes());
    assert_eq!(facts.imports.len(), 1);
    assert_eq!(facts.imports[0].name, "pkg.api");
    assert_eq!(facts.unsupported.len(), 3);
    assert!(facts.unsupported.iter().all(|fact| fact.kind == UnsupportedKind::UnsupportedName));
    for fact in &facts.unsupported {
        assert_eq!(fact.span, facts.imports[0].span);
    }
    // Binding spelling has no role in identifying the explicit module name.
    let bound = check("import pkg.api as café\n".as_bytes());
    assert!(bound.is_complete());
    assert_eq!(bound.imports[0].name, "pkg.api");
}

#[test]
fn oversized_names_retain_unknown_instead_of_crossing_the_inventory_contract() {
    for size in [HARD_MAX_NAME_BYTES - 1, HARD_MAX_NAME_BYTES] {
        let name = "a".repeat(size);
        let source = format!("import {name}\n");
        let facts = check(source.as_bytes());
        assert!(facts.is_complete());
        assert_eq!(facts.imports[0].name, name);
    }
    let source = format!("import {}\n", "a".repeat(HARD_MAX_NAME_BYTES + 1));
    let facts = check(source.as_bytes());
    assert!(facts.imports.is_empty());
    assert_eq!(facts.unsupported.len(), 1);
    assert_eq!(facts.unsupported[0].kind, UnsupportedKind::UnsupportedName);
}

#[test]
fn invalid_syntax_encoding_targets_and_stubs_return_no_partial_facts() {
    let cases: &[(&[u8], Outcome)] = &[
        (b"import pkg.api\nimport (", Outcome::InvalidSyntax),
        (b"# coding: latin-1\nimport pkg.api\n", Outcome::UnsupportedEncoding),
        (b"\xef\xbb\xbf# coding: latin-1\npass\n", Outcome::UnsupportedEncoding),
        (b"# \xff\nimport pkg.api\n", Outcome::InvalidEncoding),
    ];
    for &(source, expected) in cases {
        let error = imports::extract(SourceRequest::new("module", "pkg/a.py", source)).unwrap_err();
        assert_eq!(error.outcome, expected);
    }
    for path in ["pkg/a.pyi", "pkg/a.txt"] {
        let error = imports::extract(SourceRequest::new("module", path, b"import pkg.api\n"))
            .unwrap_err();
        assert_eq!(error.outcome, Outcome::UnsupportedExtension);
    }
    let mut request = SourceRequest::new("module", "pkg/a.py", b"import pkg.api\n");
    for target in [PythonTarget { major: 3, minor: 11 }, PythonTarget { major: 3, minor: 13 }] {
        request.target = target;
        assert_eq!(imports::extract(request).unwrap_err().outcome, Outcome::UnsupportedGrammar);
    }
}

#[test]
fn budgets_reject_complete_results_instead_of_truncating() {
    let source = b"import a, b\nfrom c import d\n";
    let mut request = SourceRequest::new("module", "pkg/a.py", source);
    request.limits.max_facts = 2;
    assert_eq!(imports::extract(request).unwrap_err().outcome, Outcome::ResourceLimit);
    request.limits.max_facts = 3;
    let admitted = imports::extract(request).unwrap();
    assert_eq!(admitted.imports.len(), 2);
    assert_eq!(admitted.unsupported.len(), 1);
    request.limits.max_tokens = 1;
    assert_eq!(imports::extract(request).unwrap_err().outcome, Outcome::ResourceLimit);
    request.limits = Limits { max_source_bytes: 1, max_tokens: 1, max_facts: 1 };
    assert_eq!(imports::extract(request).unwrap_err().outcome, Outcome::ResourceLimit);

    // Input limits precede encoding work, including for malformed bytes.
    let mut invalid = SourceRequest::new("module", "pkg/a.py", b"\xff");
    invalid.limits.max_source_bytes = 0;
    assert_eq!(imports::extract(invalid).unwrap_err().outcome, Outcome::ResourceLimit);
}

#[test]
fn caller_cannot_raise_hard_caps() {
    let bytes = vec![b' '; HARD_MAX_SOURCE_BYTES + 1];
    let many_facts = "import a\n".repeat(HARD_MAX_FACTS + 1);
    let many_tokens = "pass\n".repeat(HARD_MAX_TOKENS / 2 + 1);
    for source in [bytes.as_slice(), many_facts.as_bytes(), many_tokens.as_bytes()] {
        let mut request = SourceRequest::new("module", "pkg/a.py", source);
        request.limits = Limits {
            max_source_bytes: usize::MAX,
            max_tokens: usize::MAX,
            max_facts: usize::MAX,
        };
        assert_eq!(imports::extract(request).unwrap_err().outcome, Outcome::ResourceLimit);
    }
}

#[test]
fn identity_lengths_are_bounded_before_copying() {
    let document_id = "d".repeat(HARD_MAX_DOCUMENT_ID_BYTES);
    let path = format!("{}.py", "p".repeat(HARD_MAX_PATH_BYTES - 3));
    assert!(imports::extract(SourceRequest::new(&document_id, &path, b"pass\n")).is_ok());
    let long_document_id = format!("{document_id}x");
    let long_path = format!("x{path}");
    for (id, name) in [
        (long_document_id.as_str(), path.as_str()),
        (document_id.as_str(), long_path.as_str()),
    ] {
        assert_eq!(
            imports::extract(SourceRequest::new(id, name, b"pass\n")).unwrap_err().outcome,
            Outcome::ResourceLimit
        );
    }
}

#[test]
fn source_revision_and_existing_syntax_contract_remain_separate() {
    let source = b"import pkg.api\n";
    let direct = check(source);
    let revised = check(b"import pkg.api\r\n");
    assert_ne!(direct.identity.source_sha256, revised.identity.source_sha256);
    assert_eq!(direct, check(source));
    let syntax = analyze(SourceRequest::new("module", "pkg/consumer.py", source));
    assert_eq!(syntax.identity.schema, "python-syntax-facts/v2");
    assert_eq!(syntax.identity.provider, "slophawk.python.ruff.syntax");
    assert_eq!(syntax.coverage.clone_tokens, Coverage::Complete);
    assert_eq!(syntax.outcome, Outcome::Partial);
    assert!(syntax.gaps.iter().any(|gap| gap.kind == GapKind::UnresolvedImport));
    let stub = analyze(SourceRequest::new("stub", "pkg/a.pyi", source));
    assert_eq!(stub.coverage.clone_tokens, Coverage::Complete);
}
