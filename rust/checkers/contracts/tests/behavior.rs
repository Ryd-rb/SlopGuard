use slophawk_contracts::{
    ContractCases, IntegerContract, ProbeExpectation, ProbeInput, plan_inputs,
};
use slophawk_probe_replay::JsonValue;
use std::collections::HashSet;

fn known(name: &str, args: &[i64], value: i64) -> ProbeExpectation {
    ProbeExpectation {
        operation: name.into(),
        args: args.into(),
        expected: JsonValue::parse(value.to_string().as_bytes()).unwrap(),
    }
}
fn values(cases: &[ProbeExpectation]) -> Vec<(String, Vec<i64>, String)> {
    cases
        .iter()
        .map(|case| {
            (
                case.operation.clone(),
                case.args.clone(),
                case.expected.raw_json().into(),
            )
        })
        .collect()
}

#[test]
fn periodic_projection_reaches_a_coupled_branch_from_an_ordinary_input() {
    let rule = IntegerContract::new(
        "Carry",
        &["start", "end", "span", "part", "combined", "base"],
        "start >= 0 && start <= end && span > 0 && part > 0 && base >= 0",
        "end - start == span && span % 17 == 0 && combined == base + part ? end + part : end",
        "carry.md",
    );
    let catalog = ContractCases::new(
        &[rule],
        &[known("Carry", &[3, 144, 55, 7, 88, 62], 144)],
    )
    .unwrap();
    let probes = catalog.generate(64, 0, None, true, &[]).unwrap();
    assert!(probes.iter().any(|case| {
        let [start, end, span, part, combined, base] = case.args[..] else {
            return false;
        };
        end - start == span && span % 17 == 0 && combined == base + part
    }));
}

#[test]
fn public_example_validation_and_rule_absence() {
    let guarded = IntegerContract::new(
        "Guarded",
        &["value", "slot"],
        "slot == 0 || value / slot >= 0",
        "slot == 0 ? (value * value) / value : popcount((value >> slot) & 255)",
        "guards.md",
    );
    let wrong = IntegerContract::new("Wrong", &["a"], "true", "a + 1", "guards.md");
    let unpublished = IntegerContract::new("Unpublished", &["a"], "true", "a", "guards.md");
    let catalog = ContractCases::new(
        &[guarded, wrong, unpublished],
        &[
            known("Guarded", &[i64::MAX, 0], i64::MAX),
            known("Wrong", &[1], 1),
            known("Unmodeled", &[0], 0),
        ],
    )
    .unwrap();
    assert_eq!(
        catalog
            .qualification()
            .iter()
            .map(|q| q.operation.as_str())
            .collect::<Vec<_>>(),
        ["Guarded", "Wrong", "Unmodeled", "Unpublished"]
    );
    assert!(catalog.qualification()[0].error.is_none());
    assert!(
        catalog.qualification()[1]
            .error
            .as_ref()
            .unwrap()
            .contains("published example")
    );
    assert_eq!(
        catalog.qualification()[2].error.as_deref(),
        Some("No executable public rule.")
    );
    assert_eq!(
        catalog.qualification()[3].error.as_deref(),
        Some("No published example to qualify the rule.")
    );
    let result = catalog.evaluate("Guarded", &[i64::MAX, 0]).unwrap();
    assert_eq!(result.expected.raw_json(), i64::MAX.to_string());
    assert_eq!(
        catalog
            .evaluate("Guarded", &[i64::MIN, 0])
            .unwrap()
            .expected
            .raw_json(),
        i64::MIN.to_string()
    );
    assert!(catalog.evaluate("Guarded", &[0, 0]).is_none());
    assert!(catalog.evaluate("Wrong", &[1]).is_none());
}

#[test]
fn locals_mathematical_modulo_lazy_expected_and_domain() {
    let mut rule = IntegerContract::new(
        "Window",
        &["first", "last", "period", "gate"],
        "period > 0 && first <= last && gate >= 0 && gate <= 1",
        "gate == 0 ? 0 : tail + 2 * span",
        "windows.md",
    );
    rule.lets = vec![
        ("span".into(), "last - first".into()),
        ("tail".into(), "mod(first,period)".into()),
    ];
    let catalog = ContractCases::new(&[rule], &[known("Window", &[-2, 4, 5, 1], 15)]).unwrap();
    assert!(catalog.qualification()[0].error.is_none());
    assert_eq!(
        catalog
            .evaluate("Window", &[-2, 4, 5, 1])
            .unwrap()
            .expected
            .raw_json(),
        "15"
    );
    assert_eq!(
        catalog
            .evaluate("Window", &[-2, 4, 5, 0])
            .unwrap()
            .expected
            .raw_json(),
        "0"
    );
    assert!(catalog.evaluate("Window", &[-2, 4, 0, 1]).is_none());
    let cases = catalog.generate(80, 11, None, false, &[]).unwrap();
    assert_eq!(cases.len(), 80);
    assert_eq!(
        cases
            .iter()
            .map(|p| p.args.clone())
            .collect::<HashSet<_>>()
            .len(),
        80
    );
    assert!(
        cases
            .iter()
            .all(|p| p.args[0] <= p.args[1] && p.args[2] > 0 && (p.args[3] == 0 || p.args[3] == 1))
    );
    assert_eq!(
        values(&cases),
        values(&catalog.generate(80, 11, None, false, &[]).unwrap())
    );
}

#[test]
fn boundary_equality_and_multi_condition_guidance() {
    let rule = IntegerContract::new(
        "Threshold",
        &["units"],
        "0 <= units && units <= 910",
        "units >= 811 ? 2 : 1",
        "threshold.md",
    );
    let catalog = ContractCases::new(&[rule], &[known("Threshold", &[3], 1)]).unwrap();
    let cases = catalog.generate(40, 1, None, true, &[]).unwrap();
    assert!(
        cases
            .iter()
            .any(|x| x.args == [910] && x.expected.raw_json() == "2")
    );
    assert!(
        cases
            .iter()
            .any(|x| x.args == [810] && x.expected.raw_json() == "1")
    );
    assert!(
        cases
            .iter()
            .any(|x| x.args == [811] && x.expected.raw_json() == "2")
    );

    let mut combined = IntegerContract::new(
        "Combined",
        &["left", "padding", "total", "stride", "right", "gain"],
        "left >= 0 && padding >= 0 && total >= 0 && stride >= 0 && right >= 0 && gain >= 0",
        "sum == total && stride % 82 == 61 && right == gain * 7 ? 9 : 2",
        "combined.md",
    );
    combined.lets.push(("sum".into(), "left + padding".into()));
    let catalog = ContractCases::new(
        &[combined],
        &[known("Combined", &[52, 92, 1247, 422, 166, 97], 2)],
    )
    .unwrap();
    let cases = catalog.generate(48, 1, None, true, &[]).unwrap();
    assert!(cases.iter().any(|x| x.expected.raw_json() == "9"
        && x.args[0] + x.args[1] == x.args[2]
        && x.args[3] % 82 == 61
        && x.args[4] == x.args[5] * 7));
}

#[test]
fn signed_boundaries_and_invalid_arithmetic_rejected() {
    let div = IntegerContract::new("Division", &["a", "b"], "b != 0", "a / b", "bounds.md");
    let rem = IntegerContract::new("Remainder", &["a", "b"], "b != 0", "a % b", "bounds.md");
    let cat = ContractCases::new(
        &[div, rem],
        &[
            known("Division", &[i64::MIN, 1], i64::MIN),
            known("Remainder", &[-5, 2], -1),
        ],
    )
    .unwrap();
    assert!(cat.qualification().iter().all(|q| q.error.is_none()));
    assert!(cat.evaluate("Division", &[i64::MIN, -1]).is_none());
    assert!(cat.evaluate("Division", &[1, 0]).is_none());
    assert_eq!(
        cat.evaluate("Remainder", &[i64::MIN, -1])
            .unwrap()
            .expected
            .raw_json(),
        "0"
    );
    assert_eq!(
        cat.evaluate("Remainder", &[-5, 2])
            .unwrap()
            .expected
            .raw_json(),
        "-1"
    );
}

#[test]
fn operation_arity_round_robin_scaled_clamps_and_filter() {
    let seeds = vec![
        ProbeInput {
            operation: "b".into(),
            args: vec![1],
        },
        ProbeInput {
            operation: "a".into(),
            args: vec![],
        },
        ProbeInput {
            operation: "b".into(),
            args: vec![i64::MAX, i64::MIN],
        },
        ProbeInput {
            operation: "b".into(),
            args: vec![1],
        },
    ];
    let one = plan_inputs(&seeds, 120, 42, &[i64::MAX]).unwrap();
    let two = plan_inputs(&seeds, 120, 42, &[i64::MAX]).unwrap();
    assert_eq!(
        one.iter()
            .map(|p| (&p.operation, &p.args))
            .collect::<Vec<_>>(),
        two.iter()
            .map(|p| (&p.operation, &p.args))
            .collect::<Vec<_>>()
    );
    assert_eq!(one[0].operation, "b");
    assert_eq!(one.last().unwrap().operation, "a");
    assert!(one.iter().any(|p| p.args.len() == 2));
    assert!(one.iter().any(|p| p.args.contains(&i64::MIN)));
    assert!(plan_inputs(&seeds, 1, 42, &[0]).is_err());
}

#[test]
fn linked_equality_and_unqualified_noninteger_examples() {
    let rule = IntegerContract::new(
        "Shared",
        &["key1", "key2", "room1", "room2"],
        "key1 >= 0 && key2 >= 0 && room1 >= 0 && room2 >= 0 && (key1 != key2 || room1 == room2)",
        "key1 == key2 ? room1 : room1 + room2",
        "shared.md",
    );
    let invalid = IntegerContract::new("FloatExample", &["a"], "true", "a", "float.md");
    let ordinary = [
        known("Shared", &[400, 500, 89, 99], 188),
        ProbeExpectation {
            operation: "FloatExample".into(),
            args: vec![1],
            expected: JsonValue::parse(b"1.0").unwrap(),
        },
    ];
    let catalog = ContractCases::new(&[rule, invalid], &ordinary).unwrap();
    assert!(catalog.qualification()[0].error.is_none());
    assert!(catalog.qualification()[1].error.is_some());
    assert!(catalog.evaluate("FloatExample", &[1]).is_none());
    let cases = catalog.generate(24, 1, None, true, &[]).unwrap();
    assert!(
        cases
            .iter()
            .any(|case| case.args == [400, 400, 89, 89] && case.expected.raw_json() == "89")
    );
}

#[test]
fn parser_scope_overflow_lazy_branch_and_bigint_exactness() {
    let rules = [
        IntegerContract::new(
            "Huge",
            &["x"],
            "true",
            "(x * x * x * x) / (x * x * x)",
            "huge.md",
        ),
        IntegerContract::new("BadShift", &["x"], "true", "x << 128", "shift.md"),
        IntegerContract::new("BadExpression", &["x"], "true", "foo(x)", "syntax.md"),
        IntegerContract::new("Lazy", &["x"], "true", "x == 0 ? 1 : 11 / x", "lazy.md"),
    ];
    let catalog = ContractCases::new(
        &rules,
        &[
            known("Huge", &[i64::MAX], i64::MAX),
            known("BadShift", &[1], 1),
            known("BadExpression", &[2], 2),
            known("Lazy", &[0], 1),
        ],
    )
    .unwrap();
    assert!(catalog.qualification()[0].error.is_none());
    assert!(catalog.qualification()[1].error.is_some());
    assert!(catalog.qualification()[2].error.is_some());
    assert!(catalog.qualification()[3].error.is_none());
    assert_eq!(
        catalog
            .evaluate("Huge", &[i64::MAX])
            .unwrap()
            .expected
            .raw_json(),
        i64::MAX.to_string()
    );
    assert_eq!(
        catalog.evaluate("Lazy", &[0]).unwrap().expected.raw_json(),
        "1"
    );
    assert_eq!(
        catalog.evaluate("Lazy", &[-3]).unwrap().expected.raw_json(),
        "-3"
    );
}
