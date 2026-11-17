use slophawk_scalar_paths::{parse_wire, result_to_wire, search};

const TRACE: &str = include_str!("../fixtures/trace.facts");
const CHECKED: &str = include_str!("../fixtures/checked.facts");
const OPAQUE: &str = include_str!("../fixtures/opaque.facts");
const ASSIGNMENT: &str = include_str!("../fixtures/assignment.facts");
const LOOP: &str = include_str!("../fixtures/loop.facts");
const CONTRADICTION: &str = include_str!("../fixtures/contradiction.facts");

#[test]
fn trace_predictions_are_deterministic_and_attributed_to_roslyn_edges() {
    let facts = parse_wire(TRACE).expect("valid pinned Trace fact projection");
    let first = search(&facts);
    let second = search(&facts);
    assert_eq!(
        first, second,
        "identical projected inputs must yield identical ordered candidates"
    );
    assert!((1..=128).contains(&first.work));
    assert!(first.candidates.len() <= 32);
    assert!(
        first
            .candidates
            .iter()
            .any(|candidate| candidate.args.as_slice() == [i32::MAX as i64, i32::MAX as i64, 1])
    );
    assert!(first.candidates.iter().all(|candidate| {
        TRACE.lines().any(|line| {
            let columns = line.split('\t').collect::<Vec<_>>();
            columns.len() == 8
                && columns[0] == "edge"
                && format!("b{}:{}", columns[1], columns[2]) == candidate.predicted_goal
        })
    }));
    assert!(!result_to_wire(&first).contains("observed"));
}

#[test]
fn graph_off_and_zero_work_never_claim_executions() {
    let off = parse_wire(&TRACE.replacen("limits\t128\t32", "limits\t128\t0", 1)).unwrap();
    let result = search(&off);
    assert_eq!(result.work, 0);
    assert!(result.candidates.is_empty());
    assert_eq!(
        result.unknown,
        vec!["Scalar search was not requested.".to_owned()]
    );

    let empty = parse_wire(&TRACE.replacen("limits\t128\t32", "limits\t0\t32", 1)).unwrap();
    let result = search(&empty);
    assert_eq!(result.work, 0);
    assert!(result.candidates.is_empty());
    assert_eq!(
        result.unknown,
        vec!["Scalar work limit reached before search.".to_owned()]
    );
}

#[test]
fn checked_overflow_and_division_remain_explicit_semantic_holes() {
    let result = search(&parse_wire(CHECKED).unwrap());
    assert!(result.candidates.is_empty());
    assert!(result.unknown.iter().any(|u| u.contains("Add checked")));

    let result = search(&parse_wire(OPAQUE).unwrap());
    assert!(result.unknown.iter().any(|u| u.contains("Divide")));
    assert!(
        result
            .unknown
            .iter()
            .any(|u| u.contains("No supported input explored"))
    );
}

#[test]
fn short_circuit_skips_unsupported_rhs_until_its_guard_is_true() {
    let limit_one = OPAQUE.replacen("limits\t128\t32", "limits\t1\t32", 1);
    let result = search(&parse_wire(&limit_one).unwrap());
    assert_eq!(result.work, 1);
    assert!(!result.unknown.iter().any(|u| u.contains("Divide")));

    let reached = limit_one.replacen("seed\t0,0", "seed\t1,1", 1);
    let result = search(&parse_wire(&reached).unwrap());
    assert!(result.unknown.iter().any(|u| u.contains("Divide")));
}

#[test]
fn captured_assignment_can_generate_an_input_for_395() {
    let result = search(&parse_wire(ASSIGNMENT).unwrap());
    assert!(result.candidates.iter().any(|candidate| {
        let args = &candidate.args;
        let value = if args[2] == 1 {
            (args[0] as i32).wrapping_add(7)
        } else {
            args[1] as i32
        };
        value.wrapping_sub(5) == 395
    }));
}

#[test]
fn loops_and_unreachable_arms_defer_instead_of_claiming_coverage() {
    let loop_result = search(&parse_wire(LOOP).unwrap());
    assert!(
        loop_result
            .unknown
            .iter()
            .any(|u| u.starts_with("Loop frontier at block"))
    );
    let contradiction = search(&parse_wire(CONTRADICTION).unwrap());
    assert!(
        contradiction
            .unknown
            .iter()
            .any(|u| u.starts_with("No supported input explored"))
    );
}

#[test]
fn budget_exhaustion_is_reported_and_does_not_expand_beyond_work_limit() {
    // One enabled seed leaves generated proposals pending. The full seed set
    // instead reserves those inputs as skipped seeds, exactly as C# does.
    let one_seed = TRACE
        .lines()
        .filter(|line| !line.starts_with("seed\t"))
        .map(|line| {
            if line == "end" {
                "seed\t-1,-1,1\nend"
            } else {
                line
            }
        })
        .collect::<Vec<_>>()
        .join("\n");
    let limit_one = one_seed.replacen("limits\t128\t32", "limits\t1\t32", 1);
    let result = search(&parse_wire(&limit_one).unwrap());
    assert_eq!(result.work, 1);
    assert!(
        result
            .unknown
            .iter()
            .any(|u| u == "Scalar work limit 1 reached.")
    );
}

#[test]
fn rejects_cycles_and_invalid_fact_schema_before_search() {
    let mut lines = TRACE.lines().map(str::to_owned).collect::<Vec<_>>();
    let node = lines
        .iter_mut()
        .find(|l| l.starts_with("node\t1\t0\t"))
        .unwrap();
    let mut parts = node.split('\t').map(str::to_owned).collect::<Vec<_>>();
    parts[12] = "0".into();
    *node = parts.join("\t");
    assert!(
        parse_wire(&lines.join("\n"))
            .unwrap_err()
            .contains("unbound child expression")
    );
    assert!(
        parse_wire(&TRACE.replacen("scalar-path-facts\t1", "scalar-path-facts\t2", 1)).is_err()
    );
}

#[test]
fn unbound_and_incompatible_seeds_defer_with_explicit_reasons() {
    let unbound = "scalar-path-facts\t1\nlimits\t128\t32\nbound\t0\t0\t556E626F756E64\nend\n";
    let result = search(&parse_wire(unbound).expect("unbound facts are valid"));
    assert!(!result.bound);
    assert_eq!(result.work, 0);
    assert_eq!(result.unknown, vec!["Unbound".to_owned()]);

    let bad_seed = TRACE.replacen("seed\t-1,-1,0", "seed\t-1,-1,2", 1);
    let result = search(&parse_wire(&bad_seed).unwrap());
    assert!(
        result
            .unknown
            .iter()
            .any(|u| u.contains("seed is incompatible"))
    );
}

#[test]
fn malformed_graph_edge_cannot_produce_a_prediction() {
    let corrupt = TRACE.replacen(
        "edge\t0\tfallThrough\t1\tRegular",
        "edge\t0\tfallThrough\t9000\tRegular",
        1,
    );
    assert!(
        parse_wire(&corrupt)
            .unwrap_err()
            .contains("invalid edge target")
    );
}

#[test]
fn deeply_nested_facts_are_rejected_before_recursive_evaluation() {
    let mut wire =
        String::from("scalar-path-facts\t1\nlimits\t1\t1\nbound\t1\t1\t-\nblock\t0\tEntry\tNone\n");
    for id in 0..257 {
        wire.push_str(&format!(
            "node\t0\t{id}\tidentity\tint\t-\t-\t-\t0\t0\t0\t1\t{}\t-\t00\n",
            id + 1
        ));
    }
    wire.push_str("node\t0\t257\tconstant\tint\t-\t0\t-\t0\t0\t0\t1\t\t-\t00\nstmt\t0\t0\nbranch\t0\t-\nend\n");
    assert!(parse_wire(&wire).unwrap_err().contains("expression depth"));
}
