//! Draft regressions for opt-in allocation; authored and reviewed without execution.
use slophawk_contracts::{ContractCases, ContractError, IntegerContract, ProbeExpectation, ProbeInput};
use slophawk_probe_replay::JsonValue;

fn known(operation: &str, args: &[i64], value: i64) -> ProbeExpectation {
    ProbeExpectation {
        operation: operation.into(),
        args: args.into(),
        expected: JsonValue::parse(value.to_string().as_bytes()).unwrap(),
    }
}

fn input(operation: &str, args: &[i64]) -> ProbeInput {
    ProbeInput {
        operation: operation.into(),
        args: args.into(),
    }
}

fn rows(cases: &[ProbeExpectation]) -> Vec<(String, Vec<i64>, String)> {
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
fn correlated_inequalities_share_a_budget_without_repeated_condition_weight() {
    let pool = [
        input("Ordered", &[1, 10, 20]),
        input("Ordered", &[2, 10, 20]),
        input("Ordered", &[3, 10, 20]),
        input("Ordered", &[4, 10, 20]),
        input("Ordered", &[10, 10, 20]),
        input("Ordered", &[0, 20, 20]),
    ];
    let expected = [
        known("Ordered", &[1, 10, 20], 1),
        known("Ordered", &[10, 10, 20], 0),
        known("Ordered", &[2, 10, 20], 1),
        known("Ordered", &[0, 20, 20], 0),
    ];
    for predicate in ["a < b && b < c", "a < b && a < b && b < c"] {
        let rule = IntegerContract::new(
            "Ordered",
            &["a", "b", "c"],
            "true",
            &format!("{predicate} ? 1 : 0"),
            "ordered.md",
        );
        let catalog = ContractCases::new(&[rule], &[known("Ordered", &[0, 10, 20], 1)])
            .unwrap();
        // Both inequality boundaries appear at four calls; one call preserves the first row.
        for budget in 1..=4 {
            assert_eq!(
                rows(&catalog.select_diverse(&pool, budget).unwrap()),
                rows(&expected[..budget]),
            );
        }
    }
}

#[test]
fn ordinary_duplicates_and_unqualified_operations_do_not_change_operation_order() {
    let rules = [
        IntegerContract::new("A", &["x"], "true", "x", "a.md"),
        IntegerContract::new("Z", &["x"], "true", "x", "z.md"),
        IntegerContract::new("Wrong", &["x"], "true", "x + 1", "wrong.md"),
    ];
    let ordinary = [
        known("Z", &[0], 0),
        known("Z", &[0], 0),
        known("A", &[0], 0),
        known("Wrong", &[0], 0),
        known("Unmodeled", &[0], 0),
    ];
    let catalog = ContractCases::new(&rules, &ordinary).unwrap();
    let pool = [
        input("A", &[2]),
        input("Z", &[1]),
        input("Z", &[0]),
        input("Wrong", &[1]),
        input("Missing", &[2]),
        input("Z", &[1]),
        input("A", &[3]),
        input("Z", &[2]),
        input("Z", &[]),
        input("Unmodeled", &[1]),
    ];
    assert_eq!(
        rows(&catalog.select_diverse(&pool, 2).unwrap()),
        rows(&[
            known("Z", &[1], 1),
            known("Z", &[2], 2),
            known("A", &[2], 2),
            known("A", &[3], 3),
        ]),
    );
    assert!(catalog.select_diverse(&[], 2).unwrap().is_empty());
}

#[test]
fn rare_domain_and_expected_errors_exhaust_without_fabricating_cases() {
    let rule = IntegerContract::new("Rare", &["x", "d"], "x == 37", "x / d", "rare.md");
    let catalog = ContractCases::new(&[rule], &[known("Rare", &[37, 1], 37)]).unwrap();
    let rejected = [
        input("Rare", &[36, 0]),
        input("Rare", &[37, 0]),
        input("Rare", &[37]),
        input("Rare", &[37, 1]),
    ];
    assert!(catalog.select_diverse(&rejected, 8).unwrap().is_empty());
    let mut pool = rejected.to_vec();
    pool.extend([input("Rare", &[37, 2]), input("Rare", &[37, -1])]);
    assert_eq!(
        rows(&catalog.select_diverse(&pool, 8).unwrap()),
        rows(&[known("Rare", &[37, 2], 18), known("Rare", &[37, -1], -37)]),
    );

    let rule = IntegerContract::new(
        "Overflow",
        &["x"],
        "x >= 0",
        "9223372036854775807 + x",
        "overflow.md",
    );
    let catalog = ContractCases::new(&[rule], &[known("Overflow", &[0], i64::MAX)])
        .unwrap();
    // -1 has a representable output but violates the domain; +1 overflows the output.
    let pool = [input("Overflow", &[-1]), input("Overflow", &[1])];
    assert!(catalog.select_diverse(&pool, 2).unwrap().is_empty());
}

#[test]
fn signed_extreme_distances_are_not_narrowed_to_i64() {
    let rule = IntegerContract::new(
        "Order",
        &["a", "b"],
        "true",
        "a < b ? -1 : a > b ? 1 : 0",
        "order.md",
    );
    let catalog = ContractCases::new(&[rule], &[known("Order", &[0, 0], 0)]).unwrap();
    let pool = [
        input("Order", &[i64::MIN, i64::MAX]),
        input("Order", &[i64::MIN + 1, i64::MAX]),
        input("Order", &[i64::MAX, i64::MIN]),
    ];
    assert_eq!(
        rows(&catalog.select_diverse(&pool, 3).unwrap()),
        rows(&[
            known("Order", &[i64::MIN, i64::MAX], -1),
            known("Order", &[i64::MAX, i64::MIN], 1),
            known("Order", &[i64::MIN + 1, i64::MAX], -1),
        ]),
    );
}

#[test]
fn an_unvisited_failing_comparison_does_not_invalidate_a_lazy_expectation() {
    let rule = IntegerContract::new(
        "Lazy",
        &["gate", "x"],
        "true",
        "gate == 0 ? 7 : 10 / x < 2 ? 1 : 2",
        "lazy.md",
    );
    let catalog = ContractCases::new(&[rule], &[known("Lazy", &[0, 1], 7)]).unwrap();
    let pool = [
        input("Lazy", &[0, 0]),
        input("Lazy", &[0, 2]),
        input("Lazy", &[1, 10]),
        input("Lazy", &[1, 0]),
    ];
    assert_eq!(
        rows(&catalog.select_diverse(&pool, 4).unwrap()),
        rows(&[
            known("Lazy", &[0, 0], 7),
            known("Lazy", &[1, 10], 1),
            known("Lazy", &[0, 2], 7),
        ]),
    );
}

#[test]
fn implicit_builtin_boundaries_fall_back_to_input_order() {
    let rule = IntegerContract::new(
        "Clamp",
        &["x"],
        "true",
        "clamp(x, 0, 3)",
        "builtin.md",
    );
    let catalog = ContractCases::new(&[rule], &[known("Clamp", &[1], 1)]).unwrap();
    let pool = [
        input("Clamp", &[-10]),
        input("Clamp", &[-9]),
        input("Clamp", &[3]),
        input("Clamp", &[4]),
    ];
    assert_eq!(
        rows(&catalog.select_diverse(&pool, 4).unwrap()),
        rows(&[
            known("Clamp", &[-10], 0),
            known("Clamp", &[-9], 0),
            known("Clamp", &[3], 3),
            known("Clamp", &[4], 3),
        ]),
    );
}

#[test]
fn limits_count_raw_duplicate_rows_and_distinct_explicit_comparisons() {
    let rule = IntegerContract::new("Id", &["x"], "true", "x", "id.md");
    let catalog = ContractCases::new(&[rule], &[known("Id", &[0], 0)]).unwrap();
    for budget in [0, 8193] {
        assert!(matches!(
            catalog.select_diverse(&[], budget),
            Err(ContractError::Invalid(_)),
        ));
    }
    assert!(matches!(
        catalog.select_diverse(&[input("", &[1])], 1),
        Err(ContractError::Invalid(_)),
    ));
    assert!(matches!(
        catalog.select_diverse(&vec![input("Id", &[1]); 8193], 1),
        Err(ContractError::Scope(_)),
    ));
    assert!(matches!(
        catalog.select_diverse(&[input("Id", &[0; 33])], 1),
        Err(ContractError::Scope(_)),
    ));
    for local_count in [63, 64] {
        let mut rule = IntegerContract::new("Many", &["x"], "true", "x < 64 ? 1 : 0", "many.md");
        rule.lets = (0..local_count)
            .map(|index| (format!("c{index}"), format!("x < {index}")))
            .collect();
        let catalog = ContractCases::new(&[rule], &[known("Many", &[0], 1)]).unwrap();
        assert!(catalog.qualification()[0].error.is_none());
        let result = catalog.select_diverse(&[input("Many", &[65])], 1);
        if local_count == 63 {
            assert_eq!(rows(&result.unwrap()), rows(&[known("Many", &[65], 0)]));
        } else {
            assert!(matches!(result, Err(ContractError::Scope(_))));
        }
    }
}

#[test]
fn total_pool_anchor_and_output_envelopes_fail_before_selection() {
    let rule = IntegerContract::new("Id", &["x"], "true", "x", "id.md");
    let catalog = ContractCases::new(&[rule.clone()], &[known("Id", &[0], 0)]).unwrap();
    assert!(matches!(
        catalog.select_diverse(&vec![input("Unknown", &[]); 65_537], 1),
        Err(ContractError::Scope(_)),
    ));
    let catalog = ContractCases::new(&[rule], &vec![known("Id", &[0], 0); 257]).unwrap();
    assert!(matches!(
        catalog.select_diverse(&[input("Id", &[1])], 1),
        Err(ContractError::Scope(_)),
    ));
    let names: Vec<_> = (0..9).map(|index| format!("Op{index}")).collect();
    let rules: Vec<_> = names
        .iter()
        .map(|name| IntegerContract::new(name, &["x"], "true", "x", "many-operations.md"))
        .collect();
    let ordinary: Vec<_> = names.iter().map(|name| known(name, &[0], 0)).collect();
    let pool: Vec<_> = names.iter().map(|name| input(name, &[1])).collect();
    let catalog = ContractCases::new(&rules, &ordinary).unwrap();
    assert!(matches!(
        catalog.select_diverse(&pool, 8192),
        Err(ContractError::Scope(_)),
    ));
}

#[test]
fn explicit_clamp_agrees_with_literal_anchors_and_a_domain_restricted_translation() {
    let rule = IntegerContract::new(
        "Clamp",
        &["x", "lo", "hi"],
        "lo <= hi",
        "x < lo ? lo : x > hi ? hi : x",
        "clamp.md",
    );
    let catalog = ContractCases::new(&[rule], &[known("Clamp", &[4, 2, 7], 4)]).unwrap();
    // Independently hand-derived values; the relation alone is only a partial oracle.
    let fixtures: [(&[i64], i64); 6] = [
        (&[8, 2, 7], 7),
        (&[11, 5, 10], 10),
        (&[1, 2, 7], 2),
        (&[4, 5, 10], 5),
        (&[7, 2, 7], 7),
        (&[10, 5, 10], 10),
    ];
    let mut pool: Vec<_> = fixtures
        .iter()
        .map(|(args, _)| input("Clamp", args))
        .collect();
    pool.push(input("Clamp", &[4, 7, 2]));
    let selected = catalog.select_diverse(&pool, 6).unwrap();
    assert_eq!(selected.len(), 6);
    let value = |args: &[i64]| {
        selected
            .iter()
            .find(|case| case.args.as_slice() == args)
            .unwrap()
            .expected.raw_json().parse::<i64>().unwrap()
    };
    for (args, expected) in fixtures {
        assert_eq!(value(args), expected);
    }
    assert_eq!(value(&[11, 5, 10]), value(&[8, 2, 7]) + 3);
    assert_eq!(value(&[4, 5, 10]), value(&[1, 2, 7]) + 3);
    // Endpoint-only comparator changes are equivalent here, not required mutant kills.
}

#[test]
fn a_shared_wrong_oracle_can_agree_and_preserve_translation() {
    let wrong = IntegerContract::new(
        "Clamp",
        &["x", "lo", "hi"],
        "lo <= hi",
        "(x < lo ? lo : x > hi ? hi : x) + 1",
        "wrong-clamp.md",
    );
    // Corrupt the published seed too: qualification cannot establish a true specification.
    let left = ContractCases::new(&[wrong.clone()], &[known("Clamp", &[4, 2, 7], 5)]).unwrap();
    let right = ContractCases::new(&[wrong.clone()], &[known("Clamp", &[4, 2, 7], 5)]).unwrap();
    let pool = [input("Clamp", &[0, -1, 1]), input("Clamp", &[3, 2, 4])];
    let selected = left.select_diverse(&pool, 2).unwrap();
    let baseline: Vec<_> = pool
        .iter()
        .map(|case| right.evaluate(&case.operation, &case.args).unwrap())
        .collect();
    assert_eq!(rows(&selected), rows(&baseline));
    assert_eq!(
        rows(&selected),
        rows(&[
            known("Clamp", &[0, -1, 1], 1),
            known("Clamp", &[3, 2, 4], 4),
        ]),
    );
    let source = selected[0].expected.raw_json().parse::<i64>().unwrap();
    let follow_up = selected[1].expected.raw_json().parse::<i64>().unwrap();
    assert_eq!(follow_up, source + 3); // Both literal results are still wrong.
    assert_ne!(selected[0].expected.raw_json(), "0"); // Independent correct anchor.
    let independent = ContractCases::new(&[wrong], &[known("Clamp", &[4, 2, 7], 4)]).unwrap();
    assert!(independent.qualification()[0].error.is_some());
    assert!(independent.select_diverse(&pool, 2).unwrap().is_empty());
}
