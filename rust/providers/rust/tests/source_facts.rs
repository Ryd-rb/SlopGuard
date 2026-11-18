use slophawk_rust_source_provider::{
    BlockKind, Completeness, DeclarationKind, Error, FrontierKind, IdentifierRole, Limits,
    MAX_SOURCE_BYTES, SourceEdition, analyze_file, analyze_source,
};

fn run(source: &str) -> slophawk_rust_source_provider::SourceFacts {
    analyze_source(
        SourceEdition::E2024,
        "crate/src/lib.rs",
        source,
        Limits::default(),
    )
    .unwrap()
}

#[test]
fn functions_impls_and_parent_scopes() {
    let source =
        "struct Tool; impl Tool { fn edit(&self) { let text = 1; text + 1; } } fn free() {}";
    let facts = run(source);
    assert_eq!(facts.completeness, Completeness::CompleteSyntax);
    let kinds = facts
        .declarations
        .iter()
        .map(|x| x.kind)
        .collect::<Vec<_>>();
    assert!(kinds.contains(&DeclarationKind::Structure));
    assert!(kinds.contains(&DeclarationKind::Implementation));
    let edit = facts
        .declarations
        .iter()
        .find(|d| d.name.as_deref() == Some("edit"))
        .unwrap();
    assert_eq!(edit.kind, DeclarationKind::Function);
    assert!(edit.parent.is_some());
    assert!(
        facts
            .blocks
            .iter()
            .any(|b| b.kind == BlockKind::AssociatedItems)
    );
    assert!(facts.blocks.iter().any(|b| b.kind == BlockKind::Expression));
    for d in &facts.declarations {
        assert!(d.span.start < d.span.end);
        assert!((d.span.end as usize) <= source.len());
        assert!(source.is_char_boundary(d.span.start as usize));
        assert!(source.is_char_boundary(d.span.end as usize));
        if let Some(name_span) = d.name_span {
            assert!(d.span.start <= name_span.start && name_span.end <= d.span.end);
        }
    }
}

#[test]
fn parser_respects_declared_edition() {
    let source = "fn async() {}";
    let old = analyze_source(SourceEdition::E2015, "lib.rs", source, Limits::default()).unwrap();
    let new = analyze_source(SourceEdition::E2021, "lib.rs", source, Limits::default()).unwrap();
    assert!(
        old.diagnostics.is_empty(),
        "2015 edition: {:?}",
        old.diagnostics
    );
    assert!(
        !new.diagnostics.is_empty(),
        "async is a keyword in Rust 2021"
    );
    assert_eq!(old.identity.edition.as_str(), "2015");
    let async_function = run("async fn do_work() { let x = 1; x; }");
    assert!(async_function.diagnostics.is_empty());
}

#[test]
fn raw_unicode_crlf_nested_comments_and_bytes() {
    let source = "// café\r\n/* outer /* inner */ tail */\r\nfn r#async() { let r#value = \"é\"; r#value; }\r\n";
    let facts = run(source);
    assert_eq!(facts.identity.source_bytes, source.len());
    assert!(facts.diagnostics.is_empty(), "{:?}", facts.diagnostics);
    assert!(
        facts
            .comments
            .iter()
            .any(|x| x.text.contains("inner */ tail"))
    );
    assert!(facts.comments.iter().any(|x| x.text.contains("café")));
    assert!(
        facts
            .comments
            .iter()
            .all(|x| source.is_char_boundary(x.span.start as usize)
                && source.is_char_boundary(x.span.end as usize))
    );
    let id = facts
        .identifiers
        .iter()
        .find(|x| x.spelling == "r#value")
        .unwrap();
    assert_eq!(
        &source[id.span.start as usize..id.span.end as usize],
        "r#value"
    );
    assert_eq!(id.span.start as usize, source.find("r#value").unwrap());
    assert!(facts.tokens.iter().any(|x| x.text == "\"é\""));
    assert!(
        facts
            .blocks
            .iter()
            .all(|x| x.span.end as usize <= source.len())
    );
}

#[test]
fn macro_cfg_dispatch_and_generated_items_are_unresolved() {
    let source = "#[cfg(feature = \"extra\")] fn chosen() {}\nmacro_rules! make { () => { fn generated() {} } }\ninclude!(\"other.rs\"); fn f(x: &X) { x.method(); make!(); }";
    let facts = run(source);
    let unresolved = &facts.unresolved;
    assert!(
        unresolved
            .iter()
            .any(|x| x.kind == FrontierKind::ConditionalCompilation)
    );
    assert!(
        unresolved
            .iter()
            .any(|x| x.kind == FrontierKind::MacroExpansion)
    );
    assert!(
        unresolved
            .iter()
            .any(|x| x.kind == FrontierKind::GeneratedItems)
    );
    assert!(
        unresolved
            .iter()
            .any(|x| x.kind == FrontierKind::TraitDispatch)
    );
    assert_eq!(facts.completeness, Completeness::PartialSyntax);
    assert!(
        !facts
            .capabilities
            .iter()
            .any(|x| x.contains("cargo-metadata"))
    );
}

#[test]
fn out_of_line_modules_are_partial() {
    let facts = run("mod external; mod local { fn f() {} }");
    assert!(
        facts
            .unresolved
            .iter()
            .any(|u| u.kind == FrontierKind::ExternalModule)
    );
    assert!(
        facts
            .declarations
            .iter()
            .any(|d| d.name.as_deref() == Some("external"))
    );
}

#[test]
fn malformed_source_recovers_but_no_local_resolution() {
    let facts = run("fn broken( { let pear = 3; pear; ");
    assert!(!facts.diagnostics.is_empty());
    assert_eq!(facts.completeness, Completeness::PartialSyntax);
    assert!(
        facts
            .unresolved
            .iter()
            .any(|u| u.kind == FrontierKind::ParseRecovery)
    );
    assert!(
        !facts
            .identifiers
            .iter()
            .any(|id| matches!(id.role, IdentifierRole::ProvenLocalReference { .. }))
    );
}

#[test]
fn renamed_local_clones_and_shadowing_are_scoped() {
    let first = run("fn f() { let alpha = 1; alpha + 2; }");
    let second = run("fn f() { let omega = 1; omega + 2; }");
    let keys = |facts: &slophawk_rust_source_provider::SourceFacts| {
        let body = facts
            .blocks
            .iter()
            .find(|b| b.kind == BlockKind::Expression)
            .unwrap();
        facts
            .tokens
            .iter()
            .filter(|t| t.span.start >= body.span.start && t.span.end <= body.span.end)
            .map(|t| t.clone_key.clone())
            .collect::<Vec<_>>()
    };
    assert_eq!(keys(&first), keys(&second));
    assert!(
        first
            .identifiers
            .iter()
            .any(|i| matches!(i.role, IdentifierRole::ProvenLocalBinding { ordinal: 0 }))
    );
    assert!(first.identifiers.iter().any(|i| matches!(
        i.role,
        IdentifierRole::ProvenLocalReference { ordinal: 0, .. }
    )));
    let shadowed = run("fn f() { let a = 0; a; let a = 1; a; }");
    let ordinals = shadowed
        .identifiers
        .iter()
        .filter_map(|x| match x.role {
            IdentifierRole::ProvenLocalReference { ordinal, .. } => Some(ordinal),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(ordinals, [0, 1]);
}

#[test]
fn complex_patterns_and_nested_blocks_are_not_assumed_resolved() {
    let facts = run("fn f() { let (pear, plum) = (1, 2); { let x = pear; } pear; }");
    assert!(
        !facts
            .identifiers
            .iter()
            .any(|id| matches!(id.role, IdentifierRole::ProvenLocalReference { .. }))
    );
    let other = run("fn f() { let pear = 1; { let pear = 5; pear; } pear; }");
    assert!(!other.identifiers.iter().any(|x| {
        x.spelling == "pear"
            && x.scope
                != other
                    .blocks
                    .iter()
                    .find(|b| b.kind == BlockKind::Expression)
                    .map(|b| b.span)
            && matches!(x.role, IdentifierRole::ProvenLocalReference { .. })
    }));
}

#[test]
fn spelling_inputs_stay_original_and_deterministic() {
    let source = "fn mispelled() { // comnent with typoo\n let variabel = 3; variabel; }";
    let first = run(source);
    for _ in 0..8 {
        assert_eq!(first, run(source));
    }
    assert!(first.identifiers.iter().any(|x| x.spelling == "mispelled"));
    assert!(first.identifiers.iter().any(|x| x.spelling == "variabel"));
    assert!(first.comments.iter().any(|x| x.text.contains("typoo")));
    assert_eq!(first.identity.sha256.len(), 64);
    assert_ne!(
        first.identity.sha256,
        run("fn different() {}").identity.sha256
    );
}

#[test]
fn hard_read_bound_and_combined_fact_budget() {
    let huge = "a".repeat(MAX_SOURCE_BYTES + 1);
    let err =
        analyze_source(SourceEdition::E2024, "huge.rs", &huge, Limits::default()).unwrap_err();
    assert!(matches!(err, Error::SourceTooLarge { .. }));
    let many = "fn f(){".to_owned() + &"let x = 0;".repeat(500) + "}";
    let short = analyze_source(
        SourceEdition::E2024,
        "short.rs",
        &many,
        Limits {
            source_bytes: MAX_SOURCE_BYTES,
            facts: 36,
        },
    )
    .unwrap();
    let emitted = short.declarations.len()
        + short.blocks.len()
        + short.tokens.len()
        + short.identifiers.len()
        + short.comments.len()
        + short.diagnostics.len()
        + short.unresolved.len();
    assert!(emitted <= 36);
    assert!(short.truncated);
    assert!(
        short
            .unresolved
            .iter()
            .any(|x| x.kind == FrontierKind::FactLimit)
    );
    assert_eq!(short.completeness, Completeness::PartialSyntax);
}

#[test]
fn invalid_input_and_file_read_limit() {
    assert!(matches!(
        analyze_source(SourceEdition::E2024, "", "fn f() {}", Limits::default()),
        Err(Error::InvalidDocumentId)
    ));
    assert!(matches!(
        analyze_source(
            SourceEdition::E2024,
            "a.rs",
            "",
            Limits {
                source_bytes: 1,
                facts: 1
            }
        ),
        Err(Error::InvalidLimits)
    ));
    let filename = std::env::temp_dir().join(format!(
        "sg-rust5-{}-{}.rs",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::write(&filename, "abcéabc").unwrap();
    let small = analyze_file(
        SourceEdition::E2024,
        "some.rs",
        &filename,
        Limits {
            source_bytes: 5,
            facts: 100,
        },
    );
    std::fs::remove_file(&filename).unwrap();
    assert!(matches!(small, Err(Error::SourceTooLarge { limit: 5 })));
}

#[test]
fn exact_sha256_is_of_original_bytes() {
    let facts = run("abc");
    assert_eq!(
        facts.identity.sha256,
        "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
    );
}

#[test]
fn documentation_comments_stay_separate_from_significant_tokens() {
    let source = "//! module docs\n/// function docs\nfn f() { /* normal */ }";
    let facts = run(source);
    assert_eq!(facts.comments.len(), 3);
    assert!(facts.comments.iter().take(2).all(|x| x.documentation));
    assert!(!facts.comments[2].documentation);
    assert!(!facts.tokens.iter().any(|x| x.kind.contains("COMMENT")));
    for comment in &facts.comments {
        assert_eq!(
            &source[comment.span.start as usize..comment.span.end as usize],
            comment.text
        );
    }
}

#[test]
fn derive_and_unknown_attributes_do_not_claim_generated_facts() {
    let facts = run("#[derive(Clone)] struct Example; #[custom] fn f() {}");
    assert!(
        facts
            .unresolved
            .iter()
            .any(|u| u.kind == FrontierKind::MacroExpansion)
    );
    assert!(
        facts
            .unresolved
            .iter()
            .any(|u| u.kind == FrontierKind::GeneratedItems)
    );
    assert_eq!(facts.completeness, Completeness::PartialSyntax);
    assert!(
        !facts
            .declarations
            .iter()
            .any(|d| d.name.as_deref() == Some("clone"))
    );
}

#[test]
fn cfg_with_intervening_comment_is_still_conditional() {
    let facts = run(r#"#[ /* comment */ cfg (feature = "x")] fn f() {}"#);
    assert!(
        facts
            .unresolved
            .iter()
            .any(|u| u.kind == FrontierKind::ConditionalCompilation)
    );
}

#[test]
fn attribute_frontiers_cover_owned_bodies_without_covering_neighbor_items() {
    let owners = [
        "#[cfg(any())] fn hidden() { 1 + 2; }",
        "#[cfg_attr(feature = \"x\", custom)] fn hidden() { 1 + 2; }",
        "#[custom] fn hidden() { 1 + 2; }",
        "#[allow(dead_code)] fn hidden() { 1 + 2; }",
        "#[cfg(any())] mod hidden { fn nested() { 1 + 2; } }",
        "#[cfg(any())] impl S { fn hidden() { 1 + 2; } }",
        "mod hidden { #![cfg(any())] fn nested() { 1 + 2; } }",
        "impl S { #![cfg(any())] fn hidden() { 1 + 2; } }",
        "fn hidden() { #![cfg(any())] 1 + 2; }",
        "fn hidden() { #[cfg(any())] let x = { 1 + 2; }; }",
    ];
    for owner in owners {
        // Prefix makes char counts differ from original UTF-8 byte positions.
        let source = format!("// café\r\n{owner}\r\nfn visible() {{ 3 + 4; }}");
        let facts = run(&source);
        assert!(
            facts.diagnostics.is_empty(),
            "{owner}: {:?}",
            facts.diagnostics
        );
        assert_eq!(facts.completeness, Completeness::PartialSyntax);
        let affected_start = source.find("1 + 2").unwrap() as u32;
        let affected_end = affected_start + "1 + 2".len() as u32;
        let visible_start = source.find("fn visible").unwrap() as u32;
        assert!(
            facts.unresolved.iter().any(|frontier| {
                frontier.span.start <= affected_start
                    && frontier.span.end >= affected_end
                    && frontier.span.end <= visible_start
            }),
            "{owner}: {:?}",
            facts.unresolved
        );
    }
}

#[test]
fn crate_inner_attribute_frontier_covers_later_items() {
    let source = "#![cfg(any())]\nfn hidden() { 1 + 2; }";
    let facts = run(source);
    assert!(facts.diagnostics.is_empty());
    let frontier = facts
        .unresolved
        .iter()
        .find(|frontier| frontier.kind == FrontierKind::ConditionalCompilation)
        .unwrap();
    assert_eq!(frontier.span.start, 0);
    assert_eq!(frontier.span.end as usize, source.len());
}

#[test]
fn derive_frontiers_cover_the_same_attached_item() {
    let owner = "#[derive(Clone)] struct Item;";
    let source = format!("{owner}\nfn neighbor() {{ 3 + 4; }}");
    let facts = run(&source);
    for kind in [FrontierKind::MacroExpansion, FrontierKind::GeneratedItems] {
        let frontier = facts
            .unresolved
            .iter()
            .find(|frontier| frontier.kind == kind)
            .unwrap();
        assert_eq!(frontier.span.start, 0);
        assert_eq!(frontier.span.end as usize, owner.len());
    }
}
