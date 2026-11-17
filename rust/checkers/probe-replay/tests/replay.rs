use slophawk_probe_replay::{
    JsonValue, MAX_FINDINGS, MAX_JSON_DEPTH, MAX_JSON_NODES, MAX_RETAINED_FINDING_BYTES,
    MAX_SOURCE_BYTES, ProbeInput, ReplayError, check, check_result_json, check_sources, compare,
    compare_sources, inputs_for_check_source, operation_results_json, parse_expectations,
    parse_inputs, parse_results,
};

fn value(source: &str) -> JsonValue {
    JsonValue::parse(source.as_bytes()).unwrap()
}

fn equal(a: &str, b: &str) -> bool {
    value(a).deep_equals(&value(b)).unwrap()
}

#[test]
fn reference_compare_interleaves_operations_and_preserves_global_first_index() {
    let inputs = br#"[
        {"Operation":"fee","Args":[2]},
        {"Operation":"status","Args":[]},
        {"Operation":"fee","Args":[3]},
        {"Operation":"status","Args":[-9223372036854775808]},
        {"Operation":"fee","Args":[4]},
        {"Operation":"status","Args":[9223372036854775807]},
        {"Operation":"fee","Args":[5]},
        {"Operation":"status","Args":[7]},
        {"Operation":"fee","Args":[6]}
    ]"#;
    let candidate =
        br#"[{"cents":4},{"ready":true},null,0,1,null,{"ready":false},{"a":1,"a":2},null]"#;
    let reference =
        br#"[{"cents":5},{"ready":true},null,null,null,9,{"ready":true},{"a":2,"a":1},null]"#;
    let results = compare_sources(inputs, candidate, reference).unwrap();

    assert_eq!(results.len(), 2);
    assert_eq!(results[0].operation, "fee");
    assert_eq!(results[0].compared, 2);
    assert_eq!(results[0].unsupported, 3);
    assert_eq!(results[0].one_sided, 1);
    assert_eq!(results[0].different, 2);
    let first = results[0].first_difference.as_ref().unwrap();
    assert_eq!(first.index, 0);
    assert_eq!(first.input.args, vec![2]);
    assert_eq!(first.candidate.raw_json(), r#"{"cents":4}"#);
    assert_eq!(first.reference.raw_json(), r#"{"cents":5}"#);

    assert_eq!(results[1].operation, "status");
    assert_eq!(results[1].compared, 2);
    assert_eq!(results[1].unsupported, 2);
    assert_eq!(results[1].one_sided, 2);
    assert_eq!(results[1].different, 1);
    assert_eq!(results[1].first_difference.as_ref().unwrap().index, 7);
    assert_eq!(
        operation_results_json(&results),
        include_str!("reference-compare.json").trim()
    );
}

#[test]
fn preserves_first_occurrence_order_and_operation_case() {
    let inputs = parse_inputs(
        br#"[{"Operation":"b","Args":[]},{"Operation":"a","Args":[]},
              {"Operation":"b","Args":[]},{"Operation":"B","Args":[]}]"#,
    )
    .unwrap();
    let same = parse_results(b"[0,0,0,0]").unwrap();
    let result = compare(&inputs, &same, &same).unwrap();
    assert_eq!(
        result
            .iter()
            .map(|item| item.operation.as_str())
            .collect::<Vec<_>>(),
        vec!["b", "a", "B"]
    );
    assert_eq!(result[0].compared, 2);
}

#[test]
fn check_collects_every_mismatch_but_excludes_adapter_null() {
    let cases = br#"[
        {"Operation":"quota","Args":[1],"Expected":3},
        {"Operation":"quota","Args":[2],"Expected":4},
        {"Operation":"quota","Args":[3],"Expected":5},
        {"Operation":"status","Args":[],"Expected":{"ready":true}}
    ]"#;
    let result = check_sources(cases, br#"[2,1,null,{"ready":true}]"#).unwrap();
    assert_eq!(result.cases, 4);
    assert_eq!(result.compared, 3);
    assert_eq!(result.unsupported, 1);
    assert_eq!(
        result.findings.iter().map(|x| x.index).collect::<Vec<_>>(),
        vec![0, 1]
    );
    assert_eq!(result.findings[1].reference.raw_json(), "4");
    assert_eq!(
        check_result_json(&result),
        include_str!("reference-check.json").trim()
    );
    assert!(matches!(
        check_sources(cases, b"[1]"),
        Err(ReplayError::LengthMismatch)
    ));
    assert_eq!(inputs_for_check_source(cases).unwrap().len(), 4);
}

#[test]
fn check_empty_cases_and_null_or_missing_expected_are_not_unsupported() {
    for source in [
        br#"[]"#.as_slice(),
        br#"[{"Operation":"x","Args":[],"Expected":null}]"#.as_slice(),
    ] {
        assert!(matches!(
            check_sources(source, b"[]"),
            Err(ReplayError::InvalidData(_)) | Err(ReplayError::LengthMismatch)
        ));
    }
    let missing = br#"[{"Operation":"x","Args":[]}]"#;
    assert!(matches!(
        parse_expectations(missing),
        Err(ReplayError::InvalidData(
            "required Expected field is missing"
        ))
    ));
    let incorrect = br#"[{"Operation":"x","Args":[],"Expected":false}]"#;
    assert!(
        check_sources(incorrect, b"[false]")
            .unwrap()
            .findings
            .is_empty()
    );
    let duplicate_last_null = br#"[{"Operation":"x","Args":[],"Expected":2,"expected":null}]"#;
    assert!(matches!(
        inputs_for_check_source(duplicate_last_null),
        Err(ReplayError::InvalidData(_))
    ));
    let duplicate_last_valid = br#"[{"Operation":"x","Args":[],"Expected":null,"expected":2}]"#;
    assert!(
        check_sources(duplicate_last_valid, b"[2]")
            .unwrap()
            .findings
            .is_empty()
    );
}

#[test]
fn operation_validation_matches_unicode_whitespace_but_not_inner_spaces() {
    for name in [r#""""#, r#"" \t\r\n ""#, r#""\u00A0\u2003""#] {
        let cases = format!(r#"[{{"Operation":{name},"Args":[],"Expected":1}}]"#);
        assert!(matches!(
            inputs_for_check_source(cases.as_bytes()),
            Err(ReplayError::InvalidData(_))
        ));
    }
    let cases = br#"[{"Operation":" a ","Args":[],"Expected":1}]"#;
    assert_eq!(inputs_for_check_source(cases).unwrap()[0].operation, " a ");
}

#[test]
fn signed_64bit_arguments_exact_and_strict() {
    let inputs = parse_inputs(
        br#"[{"operation":"fee","aRgs":[-9223372036854775808,9223372036854775807,0,-0]}]"#,
    )
    .unwrap();
    assert_eq!(inputs[0].args, vec![i64::MIN, i64::MAX, 0, 0]);
    for arg in [
        "9223372036854775808",
        "-9223372036854775809",
        "1.0",
        "1e0",
        "null",
        "\"1\"",
        "true",
    ] {
        let json = format!(r#"[{{"Operation":"a","Args":[{arg}]}}]"#);
        assert!(matches!(
            parse_inputs(json.as_bytes()),
            Err(ReplayError::InvalidData(_))
        ));
    }
    assert!(matches!(
        parse_inputs(br#"[{"Operation":"x","Args":null}]"#),
        Err(ReplayError::InvalidData(_))
    ));
    assert!(matches!(
        parse_inputs(br#"[null]"#),
        Err(ReplayError::InvalidData(_))
    ));
}

#[test]
fn parses_nested_values_without_loss_of_duplicate_properties() {
    assert!(equal(
        r#"{"a":[{"x":1,"x":2},[true,null,"c"]],"b":{}}"#,
        r#"{"b":{},"a":[{"x":1,"x":2},[true,null,"c"]]}"#
    ));
    assert!(equal(
        r#"{"a":1,"b":2,"a":3,"c":7}"#,
        r#"{"c":7,"a":1,"a":3,"b":2}"#
    ));
    assert!(!equal(r#"{"a":1,"a":2}"#, r#"{"a":2,"a":1}"#));
    assert!(!equal(r#"{"a":1,"a":1}"#, r#"{"a":1}"#));
    assert!(!equal("[1,2,3]", "[1,3,2]"));
    assert!(!equal(r#"{"x":1}"#, r#"{"X":1}"#));
    assert!(!equal("1", "\"1\""));
    assert!(!equal("true", "1"));
    assert!(!equal("null", "false"));
}

#[test]
fn decimal_exponent_and_large_integer_comparisons_are_exact() {
    for (a, b) in [
        ("1", "1.0"),
        ("100", "1e2"),
        ("0.012300", "123e-4"),
        ("-0.00e99", "0"),
        ("-1.2300e2", "-123"),
        ("9007199254740993", "90071992547409930e-1"),
        (
            "12345678901234567890123456789012345678901234567890",
            "1.2345678901234567890123456789012345678901234567890e49",
        ),
        ("1e-1000", "0.1e-999"),
        ("1e+1000", "10e999"),
        ("1E00000000000000000000000001", "10"),
    ] {
        assert!(equal(a, b), "should equal: {a} == {b}");
    }
    for (a, b) in [
        ("9007199254740993", "9007199254740992"),
        ("0.0000000000000000000001", "0"),
        ("-1e2", "1e2"),
        ("1e1000", "1e999"),
        ("1.00000000000000000001", "1"),
        ("1e2147483647", "1e2147483646"),
    ] {
        assert!(!equal(a, b), "should differ: {a} != {b}");
    }
    assert!(matches!(
        value("1e2147483648").deep_equals(&value("1e2147483648")),
        Err(ReplayError::ExponentOutOfRange)
    ));
}

#[test]
fn unicode_escapes_and_utf8_values_compare_by_decoded_text() {
    assert!(equal(r#""\u00E9""#, "\"é\""));
    assert!(equal(r#""\uD83D\uDE03""#, "\"😃\""));
    assert!(equal(r#"{"\u0061":1}"#, r#"{"a":1}"#));
    assert!(equal(r#""\\""#, r#""\u005c""#));
    assert!(equal(r#""\/""#, r#""/""#));
    assert!(!equal("\"résumé\"", "\"resume\""));
    // System.Text.Json accepts a lone escape during parsing but throws when
    // DeepEquals tries to materialize its string; our parser fails explicitly.
    for bad in [br#""\uD800""#.as_slice(), br#""\uDC00""#.as_slice()] {
        assert!(matches!(
            JsonValue::parse(bad),
            Err(ReplayError::InvalidJson { .. })
        ));
    }
}

#[test]
fn malformed_json_is_distinct_from_invalid_expected_fields() {
    for bad in [
        b"[1,]".as_slice(),
        b"{\"a\":1,}".as_slice(),
        b"01".as_slice(),
        b"1.".as_slice(),
        b"1e".as_slice(),
        b"NaN".as_slice(),
        b"true false".as_slice(),
        b"{foo:1}".as_slice(),
        b"[/*comment*/1]".as_slice(),
        b"[\"\x01\"]".as_slice(),
        b"\xFF".as_slice(),
    ] {
        assert!(
            matches!(JsonValue::parse(bad), Err(ReplayError::InvalidJson { .. })),
            "{bad:?}"
        );
    }
    assert!(matches!(
        parse_results(b"null"),
        Err(ReplayError::InvalidData(_))
    ));
    assert!(matches!(
        parse_expectations(br#"[{"Operation":"x","Args":[]}]"#),
        Err(ReplayError::InvalidData(_))
    ));
}

#[test]
fn exact_length_and_order_are_required() {
    let inputs = vec![ProbeInput {
        operation: "x".into(),
        args: vec![],
    }];
    let results = parse_results(b"[1]").unwrap();
    assert!(matches!(
        compare(&inputs, &results, &[]),
        Err(ReplayError::LengthMismatch)
    ));
    assert!(matches!(
        compare(&inputs, &[], &results),
        Err(ReplayError::LengthMismatch)
    ));
    assert!(matches!(
        compare_sources(br#"[]"#, b"[1]", b"[1]"),
        Err(ReplayError::LengthMismatch)
    ));
    assert!(compare_sources(b"[]", b"[]", b"[]").unwrap().is_empty());
    assert!(matches!(
        parse_inputs(br#"[{"Operation":" ","Args":[]}]"#)
            .and_then(|inputs| compare(&inputs, &results, &results)),
        Err(ReplayError::InvalidData(_))
    ));
}

#[test]
fn retained_evidence_has_explicit_non_truncating_caps() {
    let too_big = vec![b'0'; MAX_SOURCE_BYTES + 1];
    assert!(matches!(
        parse_results(&too_big),
        Err(ReplayError::Oversize {
            subject: "results",
            ..
        })
    ));
    assert!(matches!(
        parse_expectations(&too_big),
        Err(ReplayError::Oversize {
            subject: "cases",
            ..
        })
    ));

    // A few large, distinct payloads can exceed the findings byte cap even
    // though each individual source is within its own bound.
    let payload = "x".repeat(MAX_RETAINED_FINDING_BYTES / 2);
    let cases = format!(r#"[{{"Operation":"x","Args":[],"Expected":"{payload}"}}]"#);
    let actual = format!(r#"["{}"]"#, "y".repeat(MAX_RETAINED_FINDING_BYTES / 2));
    assert!(matches!(
        check_sources(cases.as_bytes(), actual.as_bytes()),
        Err(ReplayError::Oversize {
            subject: "retained findings bytes",
            ..
        })
    ));

    let count = MAX_FINDINGS + 1;
    let cases = format!(
        "[{}]",
        std::iter::repeat_n(r#"{"Operation":"x","Args":[],"Expected":1}"#, count)
            .collect::<Vec<_>>()
            .join(",")
    );
    let actual = format!(
        "[{}]",
        std::iter::repeat_n("2", count)
            .collect::<Vec<_>>()
            .join(",")
    );
    assert!(matches!(
        check_sources(cases.as_bytes(), actual.as_bytes()),
        Err(ReplayError::Oversize {
            subject: "findings count",
            ..
        })
    ));
}

#[test]
fn parser_depth_and_node_caps_are_explicit() {
    let deep = format!(
        "{}0{}",
        "[".repeat(MAX_JSON_DEPTH + 1),
        "]".repeat(MAX_JSON_DEPTH + 1)
    );
    assert!(matches!(
        JsonValue::parse(deep.as_bytes()),
        Err(ReplayError::Oversize {
            subject: "JSON depth",
            ..
        })
    ));
    let huge = format!(
        "[{}]",
        std::iter::repeat_n("0", MAX_JSON_NODES)
            .collect::<Vec<_>>()
            .join(",")
    );
    assert!(matches!(
        JsonValue::parse(huge.as_bytes()),
        Err(ReplayError::Oversize {
            subject: "JSON node count",
            ..
        })
    ));
}

#[test]
fn csharp_field_names_and_payload_json_can_be_consumed_by_json_tools() {
    let inputs = r#"[{"Operation":"α","Args":[1]},{"Operation":"β","Args":[]}]"#.as_bytes();
    let compared = compare_sources(inputs, b"[1,null]", b"[2,null]").unwrap();
    let json = operation_results_json(&compared);
    assert!(json.contains("\"Operation\":\"α\""));
    assert!(json.contains("\"Unsupported\":1"));
    assert!(JsonValue::parse(json.as_bytes()).is_ok());
    let cases = br#"[{"Operation":"x","Args":[-1],"Expected":{"a":1,"a":2}}]"#;
    let checked = check_sources(cases, br#"[{"a":2,"a":1}]"#).unwrap();
    let json = check_result_json(&checked);
    assert!(json.contains("\"Index\":0"));
    assert!(JsonValue::parse(json.as_bytes()).is_ok());
    assert_eq!(
        parse_expectations(cases).unwrap()[0].expected.raw_json(),
        r#"{"a":1,"a":2}"#
    );
}

#[test]
fn typed_check_and_input_parse_preserve_first_values_without_sorting() {
    let cases = parse_expectations(
        br#"[{"Operation":"z","Args":[2],"Expected":1},{"Operation":"a","Args":[1],"Expected":2}]"#,
    )
    .unwrap();
    let outputs = parse_results(b"[1,0]").unwrap();
    let result = check(&cases, &outputs).unwrap();
    assert_eq!(result.findings.len(), 1);
    assert_eq!(result.findings[0].input.operation, "a");
    assert_eq!(result.findings[0].index, 1);
}
