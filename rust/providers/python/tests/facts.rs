use slophawk_python_syntax_provider::{
    ByteSpan, Coverage, Encoding, FileKind, GapKind, IdentifierRole, Limits, Outcome, PythonTarget,
    RegionKind, SourceRequest, analyze,
};

fn check(path: &str, bytes: &[u8]) -> slophawk_python_syntax_provider::SourceFacts {
    analyze(SourceRequest::new(path, path, bytes))
}

fn region(
    facts: &slophawk_python_syntax_provider::SourceFacts,
    kind: RegionKind,
) -> &slophawk_python_syntax_provider::Region {
    facts
        .regions
        .iter()
        .find(|region| region.kind == kind)
        .expect("expected region")
}

#[test]
fn pinned_identity_and_original_sha256() {
    let facts = check("a.py", b"abc");
    assert_eq!(facts.identity.parser_version, "0.0.22");
    assert_eq!(facts.identity.schema, "python-syntax-facts/v2");
    assert_eq!(
        facts.identity.source_sha256,
        "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
    );
    assert_eq!(facts.identity.document_id, "a.py");
    assert_eq!(
        facts.identity.span_encoding,
        "utf8-original-byte-offsets-half-open"
    );
}

#[test]
fn functions_and_classes_are_bounded_declaration_regions() {
    let source = b"class A:\n    def method(self, value):\n        local_v = value + 1\n        return local_v\n";
    let facts = check("a.py", source);
    assert!(matches!(
        facts.outcome,
        Outcome::Complete | Outcome::Partial
    ));
    let class = region(&facts, RegionKind::Class);
    let function = region(&facts, RegionKind::Function);
    assert_eq!(class.name.as_deref(), Some("A"));
    assert_eq!(function.name.as_deref(), Some("method"));
    assert_eq!(function.parent, Some(class.id));
    assert!(
        function
            .span
            .slice(source)
            .unwrap()
            .starts_with(b"def method")
    );
    assert!(
        facts
            .identifiers
            .iter()
            .any(|name| name.name == "local_v" && name.local.is_some())
    );
}

#[test]
fn python_layout_distinguishes_nesting_and_logical_statement_boundaries() {
    let cases: [(&[u8], &[u8]); 2] = [
        (
            b"def f(flag):\n    value = 0\n    if flag:\n        value = 1\n        return value\n",
            b"def f(flag):\n    value = 0\n    if flag:\n        value = 1\n    return value\n",
        ),
        (
            b"def f():\n    value = 1\n    + 2\n    return value\n",
            b"def f():\n    value = 1 + 2\n    return value\n",
        ),
    ];
    for (left, right) in cases {
        let a = check("a.py", left);
        let b = check("b.py", right);
        assert_eq!(a.outcome, Outcome::Complete);
        assert_eq!(b.outcome, Outcome::Complete);
        let ar = region(&a, RegionKind::Function).id;
        let br = region(&b, RegionKind::Function).id;
        // The independently different programs have the same ordinary lexemes.
        assert_eq!(a.clone_stream(ar, false), b.clone_stream(br, false));
        assert_ne!(a.clone_layout(ar).unwrap(), b.clone_layout(br).unwrap());
    }
}

#[test]
fn python_layout_ignores_formatting_and_preserves_original_token_bytes() {
    let plain = b"def f():\n    value = (1 + 2)\n    return value\n";
    let formatted = b"\xef\xbb\xbfdef f():\r\n\t# comment\r\n\tvalue = (\r\n\t    1 + 2\r\n\t)\r\n\r\n\treturn value";
    let a = check("a.py", plain);
    let b = check("b.py", formatted);
    assert_eq!(a.outcome, Outcome::Complete);
    assert_eq!(b.outcome, Outcome::Complete);
    let ar = region(&a, RegionKind::Function).id;
    let br = region(&b, RegionKind::Function).id;
    assert_eq!(a.clone_stream(ar, false), b.clone_stream(br, false));
    assert_eq!(a.clone_layout(ar).unwrap(), b.clone_layout(br).unwrap());
    for token in &b.tokens {
        assert_eq!(token.span.slice(formatted).unwrap(), token.exact.as_bytes());
    }
}

#[test]
fn python_layout_ignores_explicit_line_continuation() {
    let a = check("a.py", b"def f():\n    value = 1 + 2\n    return value\n");
    let b = check("b.py", b"def f():\n    value = 1 + \\\n        2\n    return value\n");
    let ar = region(&a, RegionKind::Function).id;
    let br = region(&b, RegionKind::Function).id;
    assert_eq!(a.clone_stream(ar, false), b.clone_stream(br, false));
    assert_eq!(a.clone_layout(ar).unwrap(), b.clone_layout(br).unwrap());
}

#[test]
fn python_layout_is_relative_and_rejects_an_escaping_depth() {
    let a = check("a.py", b"def f(value):\n    return value + 1\n");
    let mut b = check("b.py", b"class C:\n    def f(value):\n        return value + 1\n");
    let ar = region(&a, RegionKind::Function).id;
    let br = region(&b, RegionKind::Function).id;
    assert_eq!(a.clone_stream(ar, false), b.clone_stream(br, false));
    assert_eq!(a.clone_layout(ar).unwrap(), b.clone_layout(br).unwrap());
    assert!(b.clone_layout(usize::MAX).is_none());
    // Malformed caller-owned metadata must not wrap or saturate into a match.
    b.tokens.iter_mut().find(|token| token.exact == "return").unwrap()
        .layout.indent_depth = 0;
    assert!(b.clone_layout(br).is_none());
}

#[test]
fn nested_scopes_do_not_invent_closure_bindings() {
    let source = b"def outer(value):\n    temp = value\n    def inner(number):\n        return number + temp\n    return inner(temp)\n";
    let facts = check("a.py", source);
    let locals: Vec<_> = facts
        .identifiers
        .iter()
        .filter(|name| name.name == "temp")
        .collect();
    assert!(locals.iter().any(|name| name.local.is_some()));
    assert!(
        locals
            .iter()
            .any(|name| name.local.is_none() && name.role == IdentifierRole::Unresolved)
    );
    assert_eq!(facts.coverage.local_renaming, Coverage::Partial);
}

#[test]
fn comprehension_iterator_uses_parent_scope() {
    let source = b"def f(items):\n    values = [item * 2 for item in items if item > 2]\n    return values\n";
    let facts = check("a.py", source);
    assert!(
        facts
            .regions
            .iter()
            .any(|r| r.kind == RegionKind::Comprehension)
    );
    let items = facts
        .identifiers
        .iter()
        .filter(|n| n.name == "items")
        .collect::<Vec<_>>();
    assert_eq!(items.len(), 2);
    assert!(items.iter().all(|n| n.local.is_some()));
    let item = facts
        .identifiers
        .iter()
        .filter(|n| n.name == "item")
        .collect::<Vec<_>>();
    assert!(item.len() >= 3);
    assert!(item.iter().all(|n| n.local.is_some()));
}

#[test]
fn patterns_bind_without_resolving_external_calls() {
    let source = b"def f(data):\n    match data:\n        case [head, *tail]:\n            return head\n    return None\n";
    let mut req = SourceRequest::new("pattern", "p.py", source);
    req.target = PythonTarget {
        major: 3,
        minor: 10,
    };
    let facts = analyze(req);
    assert!(facts.regions.iter().any(|r| r.kind == RegionKind::Match));
    assert!(
        facts
            .identifiers
            .iter()
            .any(|n| n.name == "head" && n.role == IdentifierRole::Binding && n.local.is_some())
    );
    assert!(
        facts
            .identifiers
            .iter()
            .any(|n| n.name == "tail" && n.role == IdentifierRole::Binding && n.local.is_some())
    );
}

#[test]
fn comments_and_strings_are_not_spelling_candidates() {
    let source = b"# teh typo in comment\nmessage = 'teh typo in string'\nteh_var = 42\n";
    let facts = check("word.py", source);
    assert_eq!(facts.comments.len(), 1);
    assert!(facts.comments[0].text.contains("teh"));
    assert!(
        facts
            .tokens
            .iter()
            .any(|t| t.exact.contains("teh typo in string"))
    );
    assert!(
        facts
            .identifiers
            .iter()
            .any(|n| n.name == "teh_var" && n.spelling_candidate)
    );
    assert!(!facts.identifiers.iter().any(|n| n.name.contains("typo")));
}

#[test]
fn bom_unicode_crlf_offsets_map_to_original_bytes() {
    let source = "\u{feff}def café(élève):\r\n    naïve = élève\r\n    return naïve\r\n";
    let bytes = source.as_bytes();
    let facts = check("accent.py", bytes);
    assert_eq!(facts.identity.encoding, Encoding::Utf8Bom);
    assert_eq!(
        facts.regions[0].span,
        ByteSpan {
            start: 3,
            end: bytes.len() as u32
        }
    );
    for identifier in &facts.identifiers {
        assert_eq!(
            identifier.span.slice(bytes),
            Some(identifier.name.as_bytes())
        );
    }
    let cafe = facts.identifiers.iter().find(|n| n.name == "café").unwrap();
    assert_eq!(cafe.span.start, 7);
    let naive = facts
        .identifiers
        .iter()
        .find(|n| n.name == "naïve")
        .unwrap();
    assert_eq!(naive.span.end - naive.span.start, "naïve".len() as u32);
}

#[test]
fn declarations_override_naive_utf8_guess() {
    let declared = b"# coding: latin-1\nname = '\xe9'\n";
    let unsupported = check("old.py", declared);
    assert_eq!(unsupported.outcome, Outcome::UnsupportedEncoding);
    assert!(unsupported.tokens.is_empty());
    assert_eq!(
        unsupported.identity.encoding,
        Encoding::Unsupported("latin-1".to_owned())
    );
    // Cookie on line 2 is allowed after a shebang.
    let declared = b"#!/usr/bin/python\n# coding= cp1252\nprint('x')\n";
    assert_eq!(
        check("x.py", declared).outcome,
        Outcome::UnsupportedEncoding
    );
    // A BOM cannot override a conflicting coding declaration.
    let bom = b"\xef\xbb\xbf# coding: latin-1\npass\n";
    assert_eq!(check("x.py", bom).outcome, Outcome::UnsupportedEncoding);
    assert_eq!(
        check("x.py", b"x = '\xff'\n").outcome,
        Outcome::InvalidEncoding
    );
}

#[test]
fn malformed_and_unsupported_grammar_are_distinct() {
    let invalid = check("broken.py", b"def f(\n");
    assert_eq!(invalid.outcome, Outcome::InvalidSyntax);
    assert!(invalid.regions.is_empty());
    let source = b"match n:\n    case 1: pass\n";
    let mut req = SourceRequest::new("versioned", "a.py", source);
    req.target = PythonTarget { major: 3, minor: 9 };
    assert_eq!(analyze(req).outcome, Outcome::UnsupportedGrammar);
    req.target = PythonTarget {
        major: 3,
        minor: 10,
    };
    assert_ne!(analyze(req).outcome, Outcome::UnsupportedGrammar);
    req.target = PythonTarget {
        major: 3,
        minor: 16,
    };
    assert_eq!(analyze(req).outcome, Outcome::UnsupportedGrammar);
}

#[test]
fn stubs_are_syntax_only_with_import_gaps() {
    let source = include_bytes!("fixtures/stub.pyi");
    let facts = check("typing.pyi", source);
    assert_eq!(facts.identity.kind, FileKind::Stub);
    assert_eq!(facts.outcome, Outcome::Partial);
    assert!(
        facts
            .gaps
            .iter()
            .any(|g| g.kind == GapKind::UnresolvedImport)
    );
    assert!(
        facts
            .identifiers
            .iter()
            .any(|n| n.name == "candiate" && n.spelling_candidate)
    );
}

#[test]
fn clone_fixture_exact_renamed_and_near_change() {
    let exact = check("exact.py", include_bytes!("fixtures/exact.py"));
    let renamed = check("renamed.py", include_bytes!("fixtures/renamed.py"));
    let near = check("near.py", include_bytes!("fixtures/near.py"));
    for facts in [&exact, &renamed, &near] {
        assert_eq!(facts.coverage.clone_tokens, Coverage::Complete);
    }
    let (a, b, c) = (
        region(&exact, RegionKind::Function).id,
        region(&renamed, RegionKind::Function).id,
        region(&near, RegionKind::Function).id,
    );
    assert_ne!(exact.clone_stream(a, false), renamed.clone_stream(b, false));
    assert_eq!(exact.clone_stream(a, true), renamed.clone_stream(b, true));
    let near_tokens = near.clone_stream(c, true);
    let exact_tokens = exact.clone_stream(a, true);
    assert_ne!(exact_tokens, near_tokens);
    assert_eq!(exact_tokens.len(), near_tokens.len());
    let agreements = exact_tokens
        .iter()
        .zip(&near_tokens)
        .filter(|(x, y)| x == y)
        .count();
    assert!(agreements * 5 >= exact_tokens.len() * 4); // >= 0.80
}

#[test]
fn dynamic_dispatch_is_a_gap_not_a_made_up_edge() {
    let facts = check("dynamic.py", b"def run(obj):\n    return obj.meth()\n");
    assert_eq!(facts.outcome, Outcome::Partial);
    assert!(
        facts
            .gaps
            .iter()
            .any(|g| g.kind == GapKind::DynamicDispatch)
    );
}

#[test]
fn no_relevant_facts_resource_caps_and_extensions_have_distinct_outcomes() {
    assert_eq!(
        check("empty.py", b" \r\n").outcome,
        Outcome::NoRelevantFacts
    );
    assert_eq!(
        check("wrong.txt", b"x = 1").outcome,
        Outcome::UnsupportedExtension
    );
    let mut req = SourceRequest::new("small", "small.py", b"x = 1\n");
    req.limits = Limits {
        max_source_bytes: 4,
        max_tokens: 10,
        max_facts: 10,
    };
    let caps = analyze(req);
    assert_eq!(caps.outcome, Outcome::ResourceLimit);
    assert!(caps.tokens.is_empty());
}

#[test]
fn global_and_nonlocal_are_explicit_unresolved_scope_gaps() {
    let bytes = b"counter = 0\ndef outer():\n    value = 2\n    def inner():\n        nonlocal value\n        global counter\n        counter = value\n        return counter\n    return inner()\n";
    let facts = check("scopes.py", bytes);
    assert_eq!(facts.outcome, Outcome::Partial);
    assert!(
        facts
            .gaps
            .iter()
            .any(|g| g.kind == GapKind::GlobalOrNonlocal)
    );
    assert!(
        facts
            .identifiers
            .iter()
            .filter(|id| id.name == "counter")
            .all(|id| id.local.is_none())
    );
    assert!(
        facts
            .identifiers
            .iter()
            .any(|id| id.name == "value" && id.role == IdentifierRole::ExternalScope)
    );
}

#[test]
fn utf8_cookies_are_accepted_only_in_eligible_lines() {
    let accepted = check(
        "cookie.py",
        b"#!/usr/bin/env python3\n# coding: utf-8\ndef f(): pass\n",
    );
    assert_eq!(accepted.identity.encoding, Encoding::Utf8);
    assert_eq!(accepted.coverage.clone_tokens, Coverage::Complete);
    let irrelevant_second_line = check("cookie.py", b"x = 1\n# coding: latin-1\ny = 2\n");
    assert_ne!(irrelevant_second_line.outcome, Outcome::UnsupportedEncoding);
}
