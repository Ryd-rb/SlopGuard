//! Source-only regressions for live allocation; owner execution is still required.
use super::*;

fn number(n: i64) -> JsonValue { JsonValue::parse(n.to_string().as_bytes()).unwrap() }
fn case(operation: &str, n: i64) -> ProbeExpectation {
    ProbeExpectation { operation: operation.into(), args: vec![n], expected: number(0) }
}
fn catalog(names: &[&str], domain: &str) -> ContractCases {
    let rules: Vec<_> = names.iter().map(|name|
        IntegerContract::new(name, &["x"], domain, "0", "zero.md")).collect();
    let published: Vec<_> = names.iter().map(|name| case(name, 0)).collect();
    ContractCases::new(&rules, &published).unwrap()
}

struct Peer<F> { respond: F, batches: Vec<Vec<ProbeExpectation>> }
impl<F> Peer<F> where F: FnMut(usize, &[ProbeExpectation]) -> Vec<JsonValue> {
    fn new(respond: F) -> Self { Self { respond, batches: Vec::new() } }
}
impl<F> CheckPeer for Peer<F> where F: FnMut(usize, &[ProbeExpectation]) -> Vec<JsonValue> {
    fn check(&mut self, cases: &[ProbeExpectation]) -> Result<Observation> {
        let observed = (self.respond)(self.batches.len(), cases);
        self.batches.push(cases.to_vec());
        Ok(Observation {
            run: format!("run-{}", self.batches.len()), candidate: "candidate".into(),
            report: format!("stage/{}/report.json", self.batches.len()), artifacts: "stage".into(),
            result: check(cases, &observed).map_err(|e| e.to_string())?, observed,
        })
    }
}

fn state(names: &[&str]) -> SearchState {
    SearchState {
        selected: Vec::new(), matched: Vec::new(), seen: HashSet::new(),
        remaining: names.iter().map(|name| (*name).into()).collect(),
        processed: names.iter().map(|name| ((*name).into(), 0)).collect(),
        compared: 0, unsupported: 0, candidate: None,
        selection_audit: Some(SelectionAudit::new()),
    }
}

#[test]
fn flush_counts_a_whole_mixed_batch_and_skips_only_future_cases() {
    let mut state = state(&["A", "B"]);
    let a: Vec<_> = (1..=3).map(|n| case("A", n)).collect();
    let b: Vec<_> = (1..=3).map(|n| case("B", n)).collect();
    let audit = state.selection_audit.as_mut().unwrap();
    audit.nominate("A", 0, 3, 12, &a, 0, &a).unwrap();
    audit.nominate("B", 0, 3, 12, &b, 0, &b).unwrap();
    let mut peer = Peer::new(|_, cases: &[ProbeExpectation]| cases.iter().map(|c| {
        match (c.operation.as_str(), c.args[0]) {
            ("A", 1) => number(9),
            ("B", 2) => JsonValue::parse(b"null").unwrap(),
            _ => number(0), // Independent public constant rule.
        }
    }).collect());
    flush(&mut vec![a[0].clone(), a[1].clone(), b[0].clone(), b[1].clone()],
        &mut peer, &mut state, 0).unwrap();
    flush(&mut vec![a[2].clone(), b[2].clone()], &mut peer, &mut state, 0).unwrap();
    assert_eq!(peer.batches.iter().map(Vec::len).collect::<Vec<_>>(), [4, 1]);
    assert_eq!(peer.batches[1][0].operation, "B");
    assert_eq!(peer.batches[1][0].args, [3]);
    assert_eq!((state.compared, state.unsupported), (4, 1));
    assert!(!state.seen.contains(&("A".into(), vec![3])));
    assert_eq!(state.selected[0].1.case.args, [1]);
    assert_eq!((state.selected[0].1.run.as_str(), state.selected[0].1.index), ("run-1", 0));
    let audit = state.selection_audit.unwrap();
    let a = &audit.passes[0];
    let b = &audit.passes[1];
    assert_eq!((a.selected, a.executed, a.compared, a.unsupported,
        a.skipped_after_finding, a.unused_budget), (3, 2, 2, 0, 1, 1));
    assert_eq!((b.selected, b.executed, b.compared, b.unsupported,
        b.skipped_after_finding, b.unused_budget), (3, 3, 2, 1, 0, 0));
    assert_eq!(audit.batches.len(), 2);
    assert!(audit.replay.is_none());
}

#[test]
fn tiny_odd_and_split_budgets_keep_replay_outside_search_allocation() {
    let catalog = catalog(&["A"], "true");
    for budget in [1, 2, 3, 63, 64, 65] {
        let mut peer = Peer::new(|_, cases: &[ProbeExpectation]| vec![number(0); cases.len()]);
        let outcome = search_with_selection(&catalog, budget, &[], &mut peer,
            CaseSelection::DiverseV1).unwrap();
        let audit = outcome.selection_audit.as_ref().unwrap();
        assert_eq!(audit.passes.len(), if budget > 64 { 2 } else { 1 });
        assert_eq!(audit.passes.iter().map(|p| p.budget).sum::<usize>(), budget);
        let executed: usize = audit.passes.iter().map(|p| p.executed).sum();
        assert!(executed > 0 && executed <= budget);
        assert_eq!(outcome.compared, executed);
        assert_eq!(audit.replay.as_ref().unwrap().executed, 1);
        assert_eq!(peer.batches.last().unwrap().len(), 1);
        let mut unique = HashSet::new();
        for batch in &peer.batches[..peer.batches.len() - 1] {
            for test in batch {
                assert_ne!(test.args, [0]); // Published anchor is never an execution.
                assert!(unique.insert((test.operation.clone(), test.args.clone())));
                assert_eq!(test.expected.raw_json(), "0");
            }
        }
        for p in &audit.passes {
            assert_eq!(p.pool_limit, (p.budget * 4).min(8192));
            assert_eq!(p.nominated, p.previously_executed + p.eligible);
            assert_eq!(p.eligible, p.selected + p.not_selected);
            assert_eq!(p.selected, p.executed + p.skipped_after_finding + p.skipped_duplicate);
            assert_eq!(p.budget, p.executed + p.unused_budget);
            assert_eq!(p.executed, p.compared + p.unsupported);
        }
        assert!(matches!(&outcome.publication, Publication::Promote(_)));
    }
    let mut peer = Peer::new(|_, cases: &[ProbeExpectation]| vec![number(0); cases.len()]);
    assert!(search(&catalog, 3, &[], &mut peer).unwrap().selection_audit.is_none());
}

#[test]
fn finite_domain_exhaustion_and_cross_pass_duplicates_leave_quota_unused() {
    // Only x=1 is new and legal. The published x=0 is an anchor, and all other
    // mutations are invalid. Independent expected value is the constant zero.
    let catalog = catalog(&["A"], "x >= 0 && x <= 1");
    let mut peer = Peer::new(|_, cases: &[ProbeExpectation]| vec![number(0); cases.len()]);
    let outcome = search_with_selection(&catalog, 65, &[], &mut peer,
        CaseSelection::DiverseV1).unwrap();
    let audit = outcome.selection_audit.unwrap();
    assert_eq!(peer.batches.len(), 2); // One search input and its separate witness.
    assert_eq!(peer.batches[0][0].args, [1]);
    assert_eq!(peer.batches[1][0].args, [1]);
    let first = &audit.passes[0];
    let second = &audit.passes[1];
    assert_eq!((first.budget, first.nominated, first.selected, first.executed,
        first.unused_budget), (64, 1, 1, 1, 63));
    assert_eq!((second.budget, second.nominated, second.previously_executed,
        second.eligible, second.selected, second.executed, second.unused_budget),
        (1, 1, 1, 0, 0, 0, 1));
    assert_eq!(first.nominated_inputs_sha256, second.nominated_inputs_sha256);
}

#[test]
fn a_finding_does_not_donate_the_stopped_operations_second_pass_quota() {
    for names in [&["A"][..], &["A", "B"][..]] {
        let catalog = catalog(names, "true");
        let mut peer = Peer::new(|_, cases: &[ProbeExpectation]| cases.iter().map(|c|
            number(if c.operation == "A" { 9 } else { 0 })).collect());
        let outcome = search_with_selection(&catalog, 65, &[], &mut peer,
            CaseSelection::DiverseV1).unwrap();
        let audit = outcome.selection_audit.unwrap();
        assert_eq!(audit.passes.len(), names.len() * 2);
        let stopped = audit.passes.iter().find(|p| p.operation == "A" && p.pass == 1).unwrap();
        assert_eq!(stopped.generation, "skipped-after-finding");
        assert_eq!((stopped.budget, stopped.nominated, stopped.executed, stopped.unused_budget),
            (1, 0, 0, 1));
        assert!(stopped.nominated_inputs_sha256.is_none());
        assert!(stopped.selected_inputs_sha256.is_none());
        assert!(audit.passes.iter().filter(|p| p.pass == 1).all(|p| p.budget == 1));
        assert_eq!(audit.replay.as_ref().unwrap().executed, names.len());
    }
}

#[test]
fn unsupported_exhausted_and_changed_qualification_lanes_remain_incomplete() {
    for (domain, unsupported) in [("true", true), ("x == 0", false), ("x > 0", false)] {
        let catalog = catalog(&["A"], domain);
        let mut peer = Peer::new(|_, cases: &[ProbeExpectation]| {
            assert!(unsupported, "Only the unsupported lane can reach the peer.");
            vec![JsonValue::parse(b"null").unwrap(); cases.len()]
        });
        let outcome = search_with_selection(&catalog, 3, &[], &mut peer,
            CaseSelection::DiverseV1).unwrap();
        assert!(!outcome.success(false, 0));
        assert!(matches!(&outcome.publication, Publication::None));
        assert_eq!(outcome.compared, 0);
        let audit = outcome.selection_audit.unwrap();
        assert!(audit.replay.is_none());
        if unsupported {
            assert_eq!((audit.passes[0].executed, audit.passes[0].compared,
                audit.passes[0].unsupported), (3, 0, 3));
        } else {
            assert!(peer.batches.is_empty());
            assert!(audit.batches.is_empty());
            if domain == "x > 0" { assert!(audit.passes.is_empty()); }
            else { assert_eq!(audit.passes[0].unused_budget, 3); }
        }
    }
}

#[test]
fn changing_witness_does_not_change_initial_counts_or_promote_its_identity() {
    let catalog = catalog(&["A", "B"], "true");
    let mut peer = Peer::new(|turn, cases: &[ProbeExpectation]| cases.iter().map(|c|
        number(if c.operation == "A" { 9 } else if turn == 0 { 0 } else { 7 })).collect());
    let outcome = search_with_selection(&catalog, 3, &[], &mut peer,
        CaseSelection::DiverseV1).unwrap();
    assert_eq!((outcome.compared, outcome.observed_discrepancies, outcome.replay_changes.len()), (6, 1, 1));
    let Publication::Subset { cases, observed, .. } = &outcome.publication else {
        panic!("Only A's separately repeated discrepancy is stable");
    };
    assert_eq!((cases.len(), cases[0].operation.as_str(), observed[0].raw_json()), (1, "A", "9"));
    let audit = outcome.selection_audit.unwrap();
    assert_eq!(audit.passes.iter().map(|p| p.executed).sum::<usize>(), 6);
    assert_eq!(audit.replay.as_ref().unwrap().executed, 2);
    assert_eq!(audit.replay.as_ref().unwrap().run, "run-2");
    assert_eq!(audit.replay.as_ref().unwrap().report, "stage/2/report.json");
}

#[test]
fn invalid_or_missing_observations_never_commit_execution_accounting() {
    struct BadPeer(u8);
    impl CheckPeer for BadPeer {
        fn check(&mut self, cases: &[ProbeExpectation]) -> Result<Observation> {
            if self.0 == 0 { return Err("missing adapter receipt".into()); }
            let mut observed = vec![number(0); cases.len()];
            let mut result = check(cases, &observed).unwrap();
            if self.0 == 1 { observed.clear(); }
            if self.0 == 2 { result.compared += 1; }
            Ok(Observation {
                run: if self.0 == 3 { String::new() } else { "run".into() },
                candidate: if self.0 == 4 { "changed".into() } else { "candidate".into() },
                artifacts: "stage".into(), report: "stage/report.json".into(), observed, result,
            })
        }
    }
    for failure in 0..=4 {
        let mut state = state(&["A"]);
        state.candidate = Some("candidate".into());
        let cases = [case("A", 1)];
        state.selection_audit.as_mut().unwrap().nominate("A", 0, 1, 4, &cases, 0, &cases).unwrap();
        assert!(flush(&mut cases.to_vec(), &mut BadPeer(failure), &mut state, 0).is_err());
        let audit = state.selection_audit.unwrap();
        assert_eq!((audit.passes[0].executed, audit.passes[0].compared,
            audit.passes[0].unsupported), (0, 0, 0));
        assert!(audit.batches.is_empty());
        assert!(audit.replay.is_none());
    }
}

#[test]
fn read_only_diversification_has_no_live_execution_audit() {
    let rules = br#"[{"Operation":"A","Parameters":["x"],"Domain":"x >= 0 && x <= 1",
        "Expected":"0","Source":"zero.md"}]"#;
    let published = br#"[{"Operation":"A","Args":[0],"Expected":0}]"#;
    let pool = br#"[{"Operation":"A","Args":[0]},{"Operation":"A","Args":[1]},
        {"Operation":"A","Args":[1]},{"Operation":"A","Args":[2]},
        {"Operation":"Unknown","Args":[1]}]"#;
    let result = diversify_cases(rules, published, pool, 3).unwrap();
    assert_eq!(result.as_object().unwrap().len(), 2);
    assert_eq!(result["cases"], serde_json::json!([{"Operation":"A","Args":[1],"Expected":0}]));
    assert!(result.get("SelectionAudit").is_none());
}
