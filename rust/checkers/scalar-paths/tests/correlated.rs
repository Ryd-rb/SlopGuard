use slophawk_scalar_paths::{
    SearchPolicy, parse_wire, result_to_wire, search, search_with_policy,
};
use std::fmt::Write;

// Synthetic wire fixtures, hand-authored for planner regression only. These are not
// Roslyn exports or evidence of original-build execution. The owner must reconcile
// the projections with the equivalent C# bodies before accepting behavioral claims.
fn node(wire: &mut String, block: usize, id: usize, fields: &str, children: &str) {
    writeln!(
        wire,
        "node\t{block}\t{id}\t{fields}\t0\t0\t0\t1\t{children}\t-\t00"
    )
    .unwrap();
}

fn branch(wire: &mut String, block: usize, next: usize, exit: usize) {
    writeln!(wire, "branch\t{block}\t0").unwrap();
    writeln!(wire, "edge\t{block}\tfallThrough\t{next}\tRegular\t0\t-\t-").unwrap();
    writeln!(wire, "edge\t{block}\tconditional\t{exit}\tRegular\t0\t-\t-").unwrap();
}

// Each guard is p[i % 2] + offsets[i] == p[i % 2 + 1]. The last guard
// compares a sum containing `copies` occurrences of each of x, y, z to target.
// All arithmetic nodes are unchecked Add; there is no multiplication fact.
fn fixture(
    work: usize,
    cases: usize,
    seed: &[i32],
    offsets: &[i32],
    copies: usize,
    target: i32,
) -> String {
    let mut wire = format!("scalar-path-facts\t1\nlimits\t{work}\t{cases}\nbound\t1\t1\t-\n");
    for id in 0..seed.len() {
        writeln!(wire, "param\t{id}\tint").unwrap();
    }
    wire.push_str(concat!(
        "block\t0\tEntry\tNone\nbranch\t0\t-\n",
        "edge\t0\tfallThrough\t1\tRegular\t0\t-\t-\n"
    ));
    let exit = offsets.len() + 2;
    for (index, offset) in offsets.iter().enumerate() {
        let block = index + 1;
        let first = index % 2;
        writeln!(wire, "block\t{block}\tBlock\tWhenFalse").unwrap();
        node(&mut wire, block, 0, "binary\tbool\t-\t-\tEquals", "1,4");
        node(&mut wire, block, 1, "binary\tint\t-\t-\tAdd", "2,3");
        node(&mut wire, block, 2, &format!("slot\tint\t{first}\t-\t-"), "");
        node(
            &mut wire, block, 3,
            &format!("constant\tint\t-\t{offset}\t-"), "",
        );
        node(
            &mut wire, block, 4,
            &format!("slot\tint\t{}\t-\t-", first + 1), "",
        );
        branch(&mut wire, block, block + 1, exit);
    }
    let block = offsets.len() + 1;
    let terms: Vec<_> = (0..3).flat_map(|parameter| vec![parameter; copies]).collect();
    let constant = 2 * terms.len();
    writeln!(wire, "block\t{block}\tBlock\tWhenFalse").unwrap();
    node(
        &mut wire, block, 0, "binary\tbool\t-\t-\tEquals",
        &format!("1,{constant}"),
    );
    for (index, parameter) in terms.iter().enumerate() {
        let id = 1 + 2 * index;
        if index + 1 < terms.len() {
            node(
                &mut wire, block, id, "binary\tint\t-\t-\tAdd",
                &format!("{},{}", id + 1, id + 2),
            );
            node(
                &mut wire, block, id + 1,
                &format!("slot\tint\t{parameter}\t-\t-"), "",
            );
        } else {
            node(&mut wire, block, id, &format!("slot\tint\t{parameter}\t-\t-"), "");
        }
    }
    node(
        &mut wire, block, constant,
        &format!("constant\tint\t-\t{target}\t-"), "",
    );
    branch(&mut wire, block, exit, exit);
    writeln!(wire, "block\t{exit}\tExit\tNone\nbranch\t{exit}\t-").unwrap();
    let seed_text = seed.iter().map(i32::to_string).collect::<Vec<_>>().join(",");
    writeln!(wire, "seed\t{seed_text}\nend").unwrap();
    wire
}

#[test]
fn three_correlated_inputs_reach_the_literal_target_with_two_interpretations() {
    let problem = parse_wire(&fixture(2, 1, &[0, 0, 0], &[0, 0], 1, 3)).unwrap();
    let legacy = search(&problem);
    assert!(
        legacy.candidates.iter().all(|c| c.predicted_goal != "b3:fallThrough")
    );
    let correlated = search_with_policy(&problem, SearchPolicy::CorrelatedEqualitiesV1);
    assert_eq!(correlated.work, 2);
    assert_eq!(correlated.candidates.len(), 1);
    assert_eq!(correlated.candidates[0].args, vec![1, 1, 1]);
    assert_eq!(correlated.candidates[0].predicted_goal, "b3:fallThrough");
}

#[test]
fn translated_relations_preserve_their_affine_offsets() {
    let problem = parse_wire(&fixture(2, 1, &[0, 2, 5], &[2, 3], 1, 10)).unwrap();
    let result = search_with_policy(&problem, SearchPolicy::CorrelatedEqualitiesV1);
    assert_eq!(result.candidates[0].args, vec![1, 3, 6]);
    assert_eq!(result.candidates[0].predicted_goal, "b3:fallThrough");
}

#[test]
fn wrapping_extrema_use_literal_full_width_expectations() {
    // 3*MIN wraps to MIN; 3*MAX wraps to MAX-2. These expectations do not
    // call the planner's modular inverse or coefficient helpers.
    for (target, expected) in [(i32::MIN, i32::MIN), (i32::MAX - 2, i32::MAX), (-3, -1)] {
        let problem = parse_wire(&fixture(2, 1, &[0, 0, 0], &[0, 0], 1, target)).unwrap();
        let result = search_with_policy(&problem, SearchPolicy::CorrelatedEqualitiesV1);
        assert_eq!(result.candidates[0].args, vec![i64::from(expected); 3]);
        assert_eq!(result.candidates[0].predicted_goal, "b3:fallThrough");
    }
}

#[test]
fn odd_target_for_an_even_sum_remains_unresolved() {
    // Even modulo-2^32 sums cannot equal 3, including at overflow boundaries.
    let problem = parse_wire(&fixture(128, 128, &[0, 0, 0], &[0, 0], 2, 3)).unwrap();
    let result = search_with_policy(&problem, SearchPolicy::CorrelatedEqualitiesV1);
    assert!(
        result.candidates.iter().all(|c| c.predicted_goal != "b3:fallThrough")
    );
    assert!(
        result.unknown.iter().any(|u| u == "No supported input explored b3:fallThrough.")
    );
}

#[test]
fn explicit_legacy_preserves_every_pinned_fixture_result_and_wire() {
    for wire in [
        include_str!("../fixtures/trace.facts"),
        include_str!("../fixtures/checked.facts"),
        include_str!("../fixtures/opaque.facts"),
        include_str!("../fixtures/assignment.facts"),
        include_str!("../fixtures/loop.facts"),
        include_str!("../fixtures/contradiction.facts"),
    ] {
        let problem = parse_wire(wire).unwrap();
        let default = search(&problem);
        let explicit = search_with_policy(&problem, SearchPolicy::Legacy);
        assert_eq!(default, explicit);
        assert_eq!(result_to_wire(&default), result_to_wire(&explicit));
    }
}

#[test]
fn correlated_results_are_deterministic_and_early_disabled_results_are_unchanged() {
    let problem = parse_wire(&fixture(32, 4, &[0, 0, 0], &[0, 0], 1, 3)).unwrap();
    let first = search_with_policy(&problem, SearchPolicy::CorrelatedEqualitiesV1);
    assert_eq!(
        first,
        search_with_policy(&problem, SearchPolicy::CorrelatedEqualitiesV1)
    );
    assert!(first.work <= 32 && first.candidates.len() <= 4);
    for (work, cases) in [(32, 0), (0, 4)] {
        let problem = parse_wire(&fixture(work, cases, &[0, 0, 0], &[0, 0], 1, 3)).unwrap();
        assert_eq!(
            search(&problem),
            search_with_policy(&problem, SearchPolicy::CorrelatedEqualitiesV1)
        );
    }
    for wire in [
        "scalar-path-facts\t1\nlimits\t32\t4\nbound\t0\t0\t756e626f756e64\nend\n"
            .to_owned(),
        fixture(32, 4, &[0, 0, 0], &[0, 0], 1, 3)
            .replacen("bound\t1\t1\t-", "bound\t1\t0\t-", 1),
    ] {
        let problem = parse_wire(&wire).unwrap();
        assert_eq!(
            search(&problem),
            search_with_policy(&problem, SearchPolicy::CorrelatedEqualitiesV1)
        );
    }
}

#[test]
fn work_exhaustion_never_returns_an_unevaluated_correlated_tuple() {
    let problem = parse_wire(&fixture(1, 4, &[0, 0, 0], &[0, 0], 1, 3)).unwrap();
    let result = search_with_policy(&problem, SearchPolicy::CorrelatedEqualitiesV1);
    assert_eq!(result.work, 1);
    assert!(result.candidates.is_empty());
    assert!(result.unknown.iter().any(|u| u.contains("work limit")));
}

#[test]
fn proposal_and_case_exhaustion_remain_visible() {
    let problem = parse_wire(&fixture(1, 4, &[0, 0, 0], &[0; 8], 1, 3)).unwrap();
    let result = search_with_policy(&problem, SearchPolicy::CorrelatedEqualitiesV1);
    assert_eq!(result.work, 1);
    assert!(result.unknown.iter().any(|u| u.contains("proposal limit 64")));

    let problem = parse_wire(&fixture(8, 1, &[0, 0, 0], &[0, 0], 1, 3)).unwrap();
    let result = search_with_policy(&problem, SearchPolicy::CorrelatedEqualitiesV1);
    assert!(result.work <= 8);
    assert_eq!(result.candidates.len(), 1);
    assert!(result.unknown.iter().any(|u| u.contains("candidate case limit 1")));
}

#[test]
fn unsupported_rows_and_resource_caps_are_visible() {
    let non_unit = fixture(32, 4, &[0, 0, 0], &[0, 0], 1, 3).replacen(
        "node\t1\t3\tconstant\tint\t-\t0\t-",
        "node\t1\t3\tslot\tint\t0\t-\t-",
        1,
    );
    for (wire, reason) in [
        (non_unit, "non-unit"),
        (fixture(32, 4, &[0; 17], &[0, 0], 1, 3), "parameter limit"),
        (fixture(128, 4, &[0, 0, 0], &[0; 33], 1, 3), "equality limit"),
    ] {
        let problem = parse_wire(&wire).unwrap();
        let result = search_with_policy(&problem, SearchPolicy::CorrelatedEqualitiesV1);
        assert!(
            result.unknown.iter().any(|u| u.contains(reason)),
            "missing {reason}"
        );
    }
}

#[test]
fn checked_opaque_and_loop_frontiers_keep_their_existing_unknowns() {
    for (wire, reason) in [
        (include_str!("../fixtures/checked.facts"), "Add checked"),
        (include_str!("../fixtures/opaque.facts"), "Divide"),
        (include_str!("../fixtures/loop.facts"), "Loop frontier"),
    ] {
        let problem = parse_wire(wire).unwrap();
        let result = search_with_policy(&problem, SearchPolicy::CorrelatedEqualitiesV1);
        assert!(
            result.unknown.iter().any(|u| u.contains(reason)),
            "missing {reason}"
        );
    }
}

#[test]
fn opt_in_short_circuit_keeps_an_opaque_rhs_unvisited_until_required() {
    let wire = include_str!("../fixtures/opaque.facts")
        .replacen("limits\t128\t32", "limits\t1\t32", 1);
    let problem = parse_wire(&wire).unwrap();
    let result = search_with_policy(&problem, SearchPolicy::CorrelatedEqualitiesV1);
    assert!(!result.unknown.iter().any(|u| u.contains("Divide")));

    let wire = wire.replacen("seed\t0,0", "seed\t1,1", 1);
    let problem = parse_wire(&wire).unwrap();
    let result = search_with_policy(&problem, SearchPolicy::CorrelatedEqualitiesV1);
    assert!(result.unknown.iter().any(|u| u.contains("Divide")));
}


// The interactive transport accepts supplied facts, including malformed or costly
// graphs that the pinned Roslyn exporter never needs to produce.
#[test]
fn interactive_planning_rejects_large_requested_work_and_expanded_expressions() {
    use slophawk_scalar_paths::plan_correlations;
    let large = fixture(257, 1, &[0, 0, 0], &[0, 0], 1, 3);
    assert!(plan_correlations(&large).unwrap_err().contains("Interactive scalar limits"));

    let mut repeated = String::from(concat!(
        "scalar-path-facts\t1\nlimits\t1\t1\nbound\t1\t1\t-\n",
        "block\t0\tEntry\tNone\nbranch\t0\t-\n",
        "edge\t0\tfallThrough\t1\tRegular\t0\t-\t-\n",
        "block\t1\tBlock\tNone\n"
    ));
    for id in 0..14 {
        node(&mut repeated, 1, id, "binary\tint\t-\t-\tAdd", &format!("{0},{0}", id + 1));
    }
    node(&mut repeated, 1, 14, "constant\tint\t-\t1\t-", "");
    repeated.push_str("stmt\t1\t0\nbranch\t1\t-\nend\n");
    assert!(plan_correlations(&repeated).unwrap_err().contains("expanded expression budget"));
}
