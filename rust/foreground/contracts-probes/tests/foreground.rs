//! Source-only differential cases against pinned C# ContractCasesTests,
//! ProbeReplayTests and the Rust checkers' captured .NET 10 golden vectors.
//! These tests are authored for integration; they are NOT executed here.
use slophawk_contracts::{ContractCases, IntegerContract, ProbeExpectation, ProbeInput, plan_inputs};
use slophawk_contracts_probes::{
    CheckPeer, Observation, Publication, qualify, read_literal_hints, read_rules,
    replay_changes_json, search, sha256_hex, expectations_json, inputs_json,
    parse_peer_evidence,
};
use slophawk_probe_replay::{
    JsonValue, check, check_result_json, compare_sources, parse_expectations, parse_inputs,
    parse_results,
};

fn expected(operation: &str, args: &[i64], number: i64) -> ProbeExpectation {
    ProbeExpectation { operation: operation.into(), args: args.into(),
        expected: JsonValue::parse(number.to_string().as_bytes()).unwrap() }
}
fn catalog(ops: &[IntegerContract], samples: &[ProbeExpectation]) -> ContractCases {
    ContractCases::new(ops, samples).unwrap()
}
fn rule(name: &str, params: &[&str], domain: &str, expression: &str) -> IntegerContract {
    IntegerContract::new(name, params, domain, expression, "contract.md")
}

#[test]
fn qualification_preserves_ordered_lets_basis_big_integers_and_lazy_arithmetic() {
    let rules = br#"[
        {"Operation":"Window","Parameters":["first","last","period","gate"],
         "Domain":"period > 0 && first <= last && gate >= 0 && gate <= 1",
         "Expected":"gate == 0 ? 0 : tail + 2 * span", "Source":"windows.md",
         "Let":{"span":"last - first", "tail":"mod(first,period)"},
         "Basis":{"gate":[1,0],"period":[1,7]}},
        {"Operation":"Huge","Parameters":["x"], "Domain":"true",
         "Expected":"(x * x * x * x) / (x * x * x)","Source":"big.md"},
        {"Operation":"Guarded","Parameters":["value","slot"],
         "Domain":"slot == 0 || value / slot >= 0",
         "Expected":"slot == 0 ? (value * value) / value : popcount((value >> slot) & 255)",
         "Source":"guards.md"}
    ]"#;
    let ordinary = br#"[
        {"Operation":"Window","Args":[-2,4,5,1],"Expected":15},
        {"Operation":"Huge","Args":[9223372036854775807],"Expected":9223372036854775807},
        {"Operation":"Guarded","Args":[9223372036854775807,0],"Expected":9223372036854775807}
    ]"#;
    let parsed = read_rules(rules).unwrap();
    assert_eq!(parsed[0].lets, [("span".into(), "last - first".into()),
        ("tail".into(), "mod(first,period)".into())]);
    assert_eq!(parsed[0].basis[0], ("gate".into(), vec![1,0]));
    let cat = qualify(rules, ordinary).unwrap();
    assert!(cat.qualification().iter().all(|q| q.error.is_none()));
    assert_eq!(cat.evaluate("Window", &[-2,4,5,0]).unwrap().expected.raw_json(), "0");
    assert!(cat.evaluate("Window", &[-2,4,0,1]).is_none());
    assert_eq!(cat.evaluate("Huge", &[i64::MIN].as_slice()).unwrap().expected.raw_json(),
        i64::MIN.to_string());
    assert_eq!(cat.evaluate("Guarded", &[i64::MAX,0].as_slice()).unwrap().expected.raw_json(),
        i64::MAX.to_string());
    assert!(cat.evaluate("Guarded", &[0,0].as_slice()).is_none());
}

#[test]
fn global_invalid_rules_and_local_qualification_errors_are_distinct() {
    let duplicates = br#"[{"Operation":"A","Parameters":["x"],"Domain":"true","Expected":"x","Source":"a"},
         {"Operation":"A","Parameters":["x"],"Domain":"true","Expected":"x","Source":"a"}]"#;
    assert!(qualify(duplicates, br#"[{"Operation":"A","Args":[1],"Expected":1}]"#).is_err());
    let rules = br#"[{"Operation":"Guard","Parameters":["x"],"Domain":"x > 0",
         "Expected":"x+1","Source":"a"},
         {"Operation":"Invalid","Parameters":["x"],"Domain":"true",
          "Expected":"x << 128","Source":"a"},
         {"Operation":"Unpublished","Parameters":["x"],"Domain":"true",
          "Expected":"x","Source":"a"}]"#;
    let cat = qualify(rules, br#"[{"Operation":"Guard","Args":[0],"Expected":1},
        {"Operation":"Invalid","Args":[1],"Expected":1},
        {"Operation":"Unknown","Args":[1],"Expected":1}]"#).unwrap();
    assert_eq!(cat.qualification().iter().map(|r| r.operation.as_str()).collect::<Vec<_>>(),
        ["Guard","Invalid","Unknown","Unpublished"]);
    assert!(cat.qualification().iter().all(|r| r.error.is_some()));
    assert!(cat.generate(10,0,None,false,&[]).unwrap().is_empty());
    assert!(read_rules(br#"[{"Operation":"A","Parameters":["x"],"Domain":"true",
         "Expected":"x","Source":"s","Basis":{"x":[18446744073709551615]}}]"#).is_err());
    assert!(qualify(rules, br#"[{"Operation":"Guard","Args":[],"Expected":1}]"#)
        .unwrap().qualification()[0].error.is_some());
    // Null/missing rule members are per-operation qualification failures in
    // the C# positional-record model, not a fatal JSON type error.
    let malformed = qualify(br#"[{"Operation":"Incomplete","Parameters":null,
        "Domain":null,"Expected":"x","Source":null}]"#,
        br#"[{"Operation":"Incomplete","Args":[],"Expected":1}]"#).unwrap();
    assert!(malformed.qualification()[0].error.is_some());
}

#[test]
fn seeded_mixed_arities_scales_and_signed_extremes_match_pinned_csharp_order() {
    let seeds = [ProbeInput {operation:"b".into(), args: vec![1]},
        ProbeInput {operation:"a".into(), args: vec![]},
        ProbeInput {operation:"b".into(), args: vec![10,20]},
        ProbeInput {operation:"b".into(), args: vec![1]}];
    let result = plan_inputs(&seeds, 12, 1, &[]).unwrap();
    let expected: Vec<Vec<i64>> = vec![vec![1],vec![10,20],vec![9,20],vec![0],vec![10,10],
        vec![20,0],vec![10,19],vec![2],vec![20,20],vec![4,11],vec![4],vec![11,20]];
    assert_eq!(result.iter().take(12).map(|x| x.args.clone()).collect::<Vec<_>>(), expected);
    assert_eq!(result.last().unwrap().operation, "a");
    let extreme = plan_inputs(&[ProbeInput {operation:"Ext".into(),
        args:vec![i64::MIN, i64::MAX]}], 24,-1,&[i64::MAX]).unwrap();
    assert_eq!(extreme[0].args, vec![i64::MIN,i64::MAX]);
    assert_eq!(extreme[1].args, vec![i64::MIN,i64::MIN]);
    assert_eq!(extreme[2].args, vec![i64::MIN,0]);
    let paired = plan_inputs(&[ProbeInput{operation:"scale".into(),args:vec![5,71,8]}],
        100,0,&[37]).unwrap();
    assert!(paired.iter().any(|x| x.args == [4,108,8]));
    assert!(paired.iter().any(|x| x.args == [5,34,9]));
    assert!(plan_inputs(&seeds, 12, 0, &[1]).is_err());
    assert!(plan_inputs(&seeds, 8193, 0, &[]).is_err());
    assert_eq!(parse_inputs(inputs_json(&result).unwrap().as_bytes()).unwrap(),result);
}

#[test]
fn compare_preserves_first_mismatch_and_one_sided_support_changes() {
    let result = compare_sources(
        br#"[{"Operation":"fee","Args":[2]},{"Operation":"fee","Args":[3]},
           {"Operation":"fee","Args":[4]},{"Operation":"status","Args":[]}]"#,
        br#"[{"cents":4},null,0,{"ready":true,"code":2}]"#,
        br#"[{"cents":5},null,null,{"code":2,"ready":true}]"#,
    ).unwrap();
    assert_eq!(result.len(),2);
    assert_eq!((result[0].compared,result[0].unsupported,result[0].one_sided,result[0].different),
        (1,2,1,1));
    assert_eq!(result[0].first_difference.as_ref().unwrap().index,0);
    assert!(result[1].first_difference.is_none());
    assert!(compare_sources(b"[]",b"[]",b"[null]").is_err());
    assert_eq!(JsonValue::parse(b"9.000").unwrap().deep_equals(&JsonValue::parse(b"9").unwrap()).unwrap(),true);
    assert!(!JsonValue::parse(b"{\"a\":1,\"a\":2}").unwrap()
        .deep_equals(&JsonValue::parse(b"{\"a\":2,\"a\":1}").unwrap()).unwrap());
}

/// Supplies every expected value except where the callback deliberately
/// changes observed bytes. This is a peer *model*, not a launched adapter.
struct Scripted<F> {
    calls: usize,
    responses: F,
    batches: Vec<Vec<ProbeExpectation>>,
}
impl<F> Scripted<F> where F: FnMut(usize, &[ProbeExpectation]) -> Vec<JsonValue> {
    fn new(responses: F) -> Self { Self {calls:0, responses, batches: Vec::new()} }
}
impl<F> CheckPeer for Scripted<F> where F: FnMut(usize, &[ProbeExpectation]) -> Vec<JsonValue> {
    fn check(&mut self, cases: &[ProbeExpectation]) -> std::result::Result<Observation,String> {
        let observed = (self.responses)(self.calls, cases);
        self.calls += 1;
        self.batches.push(cases.to_vec());
        let result = check(cases, &observed).map_err(|e| e.to_string())?;
        Ok(Observation {run:format!("run-{}", self.calls), candidate:"sha256:candidate".into(),
            report:format!("stage/{}.json",self.calls), artifacts:format!("stage/{}",self.calls),
            observed, result})
    }
}
fn number(n: i64) -> JsonValue { JsonValue::parse(n.to_string().as_bytes()).unwrap() }
fn all_expected(cases: &[ProbeExpectation]) -> Vec<JsonValue> {
    cases.iter().map(|c| c.expected.clone()).collect()
}

#[test]
fn stable_discrepancy_requires_a_second_peer_execution() {
    let cat = catalog(&[rule("Fee", &["n"], "true", "n + 1")], &[expected("Fee", &[1], 2)]);
    let mut peer = Scripted::new(|_, cases: &[ProbeExpectation]| cases.iter().map(|c| {
        number(c.expected.raw_json().parse::<i64>().unwrap()+1)
    }).collect());
    let out = search(&cat,1,&[],&mut peer).unwrap();
    assert_eq!(peer.calls,2);
    assert_eq!((out.compared,out.unsupported,out.observed_discrepancies), (1,0,1));
    assert!(out.replay_changes.is_empty());
    assert!(matches!(out.publication, Publication::Promote(_)));
    assert!(!out.success(true,1));
}

#[test]
fn vanished_and_new_mismatches_survive_even_when_replay_has_no_findings() {
    let cat = catalog(&[
        rule("A", &["n"], "true", "n"), rule("B", &["n"], "true", "n")],
        &[expected("A", &[1], 1), expected("B", &[2], 2)]);
    let mut peer = Scripted::new(|turn, cases: &[ProbeExpectation]| {
        let mut values = all_expected(cases);
        if turn == 0 { values[0] = number(1234); }
        else { values[1] = number(-999); }
        values
    });
    let out = search(&cat,1,&[],&mut peer).unwrap();
    assert_eq!(peer.calls,2);
    assert_eq!(out.observed_discrepancies,1);
    assert_eq!(out.replay_changes.len(),2);
    assert!(out.replay_changes[0].before.raw_json()=="1234"
        && out.replay_changes[0].after.raw_json()!="1234");
    assert_eq!(out.replay_changes[1].before.raw_json(),out.replay_changes[1].expected.raw_json());
    assert_eq!(out.replay_changes[1].after.raw_json(),"-999");
    assert!(matches!(out.publication,Publication::None));
    assert!(!out.success(false,0));
}

#[test]
fn unstable_null_support_and_raw_duplicate_json_are_preserved() {
    let cat = catalog(&[
        rule("A", &["n"], "true", "n"), rule("B", &["n"], "true", "n")],
        &[expected("A", &[1], 1), expected("B", &[2], 2)]);
    let mut peer = Scripted::new(|turn, cases: &[ProbeExpectation]| {
        if turn == 0 {
            let mut actual = all_expected(cases);
            actual[0] = number(13);
            actual
        } else { vec![JsonValue::parse(b"null").unwrap(), JsonValue::parse(b"null").unwrap()] }
    });
    let out = search(&cat,1,&[],&mut peer).unwrap();
    assert_eq!(out.replay_unsupported,2);
    assert_eq!(out.replay_changes.len(),2);
    assert!(out.replay_changes.iter().all(|r| r.after.is_null()));
    let text = replay_changes_json(&out.replay_changes).unwrap();
    assert!(text.contains("\"Before\":13") && text.contains("\"After\":null"));
    assert!(matches!(out.publication, Publication::None));
}

#[test]
fn subset_keeps_stable_difference_and_does_not_publish_unstable_match() {
    let cat = catalog(&[
        rule("A", &["n"], "true", "n"), rule("B", &["n"], "true", "n")],
        &[expected("A", &[1], 1), expected("B", &[2], 2)]);
    let mut peer = Scripted::new(|turn, cases: &[ProbeExpectation]| {
        let mut values = all_expected(cases);
        values[0] = number(123);
        if turn != 0 {values[1] = number(99);}
        values
    });
    let out = search(&cat,1,&[],&mut peer).unwrap();
    assert_eq!(out.replay_changes.len(),1);
    let Publication::Subset {cases, observed, candidate: _} = out.publication else {
        panic!("Expected only stable replay to qualify for publication");
    };
    assert_eq!(cases.len(),1);
    assert_eq!(cases[0].operation,"A");
    assert_eq!(observed[0].raw_json(),"123");
    assert_eq!(check(&cases,&observed).unwrap().findings.len(),1);
}

#[test]
fn literal_hints_guide_search_but_never_change_rule_authority() {
    let mut rule = rule("Route", &["a","b","c","d"],
        "min(min(a,b),min(c,d)) >= 0 && max(max(a,b),max(c,d)) <= 100",
        "a == c && b == d ? 0 : 1");
    rule.basis.push(("a".into(),vec![2,4]));
    let cat = catalog(&[rule], &[expected("Route", &[2,4,6,8],1)]);
    let bound = br#"{"Format":"slophawk-il-literals-v1","AssemblySha256":"ABCD",
        "Operations":[{"Operation":"Route","Arity":4,"Literals":[31,67,31]}]}"#;
    let hints = read_literal_hints(bound,cat.parameter_counts(),"ABCD").unwrap();
    assert!(read_literal_hints(bound,cat.parameter_counts(),"WRONG").is_err());
    assert!(read_literal_hints(br#"{"Format":"slophawk-il-literals-v1",
        "AssemblySha256":"ABCD", "Operations":[{"Operation":"Unknown","Arity":4,
        "Literals":[31]}]}"#,cat.parameter_counts(),"ABCD").is_err());
    assert!(read_literal_hints(br#"{"Format":"slophawk-il-literals-v1",
        "AssemblySha256":"ABCD", "Operations":[{"Operation":"Route","Arity":3,
        "Literals":[31]}]}"#,cat.parameter_counts(),"ABCD").is_err());
    let generated = cat.generate(128,1,None,true,&hints).unwrap();
    // The implementation literals guide correlated collision candidates, but
    // all expectations are independently derived from supplied public rules.
    assert!(generated.iter().all(|x| cat.evaluate(&x.operation,&x.args)
        .unwrap().expected.deep_equals(&x.expected).unwrap()));
    assert!(generated.iter().any(|x| x.args.contains(&31) || x.args.contains(&30)));
    assert_eq!(cat.evaluate("Route", &[2,4,6,8]).unwrap().expected.raw_json(),"1");
}

#[test]
fn raw_hashes_and_case_json_roundtrip_without_normalizing_values() {
    assert_eq!(sha256_hex(b"abc"),
        "BA7816BF8F01CFEA414140DE5DAE2223B00361A396177A9CB410FF61F20015AD");
    assert_ne!(sha256_hex(b"abc"),sha256_hex(b"\xef\xbb\xbfabc"));
    let original = vec![expected("long", &[i64::MIN,i64::MAX],i64::MAX)];
    let encoded = expectations_json(&original).unwrap();
    let roundtrip = parse_expectations(encoded.as_bytes()).unwrap();
    assert_eq!(roundtrip[0].args, original[0].args);
    assert!(roundtrip[0].expected.deep_equals(&original[0].expected).unwrap());
    assert_eq!(parse_results(b"[1,1.0,1e0]").unwrap().len(),3);
}

#[test]
fn published_peer_terminal_rejects_forgery_and_preserves_signed_numbers() {
    let cases = vec![expected("Run", &[i64::MAX],i64::MAX)];
    let requests = inputs_json(&[ProbeInput {operation:"Run".into(), args:vec![i64::MAX]}]).unwrap();
    let observed = "[9223372036854775807]";
    let result = check(&cases,&parse_results(observed.as_bytes()).unwrap()).unwrap();
    let report = format!(r#"{{"Format":"slophawk-check-v1","Run":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        "Candidate":"DEADBEEF","RunVerb":null,"PrepareVerb":null,
        "Result":{},"Expected":[9223372036854775807]}}"#,check_result_json(&result));
    let receipt = format!(r#"{{"Format":"slophawk-check-v1","Run":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        "Report":"stage/report.json","Artifacts":"stage","ObserveOnly":false,"Result":{}}}"#,
        check_result_json(&result));
    assert!(parse_peer_evidence(receipt.as_bytes(),report.as_bytes(),requests.as_bytes(),
        observed.as_bytes(),&cases,"DEADBEEF",None,None).is_ok());
    assert!(parse_peer_evidence(receipt.as_bytes(),report.replace("DEADBEEF","CHANGED").as_bytes(),
        requests.as_bytes(),observed.as_bytes(),&cases,"DEADBEEF",None,None).is_err());
    assert!(parse_peer_evidence(receipt.as_bytes(),report.as_bytes(),requests.as_bytes(),
        b"[null]",&cases,"DEADBEEF",None,None).is_err());
    assert!(parse_peer_evidence(receipt.replace("slophawk-check-v1","wrong-format").as_bytes(),
        report.as_bytes(),requests.as_bytes(),observed.as_bytes(),&cases,"DEADBEEF",None,None).is_err());
}
