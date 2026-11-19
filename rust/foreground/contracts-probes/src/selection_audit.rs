//! Bounded accounting for diverse search, separate from witness publication.
//! Nominations are the generator's emitted legal, non-published cases; rejected
//! generation attempts and input-partition or program coverage are not measured.
//! Executed means a returned, validated observation, not a failed adapter attempt.

use serde::Serialize;
use slophawk_contracts::{ProbeExpectation, ProbeInput};

use crate::{Observation, Result, inputs_json, sha256_hex};

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct SelectionAudit {
    pub format: &'static str,
    pub generator_basis: &'static str,
    pub execution_basis: &'static str,
    pub passes: Vec<PassAudit>,
    pub batches: Vec<BatchAudit>,
    pub replay: Option<ObservationAudit>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct PassAudit {
    pub operation: String,
    pub pass: i32,
    pub budget: usize,
    pub pool_limit: usize,
    pub generation: &'static str,
    pub nominated: usize,
    pub nominated_inputs_sha256: Option<String>,
    pub previously_executed: usize,
    pub eligible: usize,
    pub selected: usize,
    pub selected_inputs_sha256: Option<String>,
    pub not_selected: usize,
    pub skipped_after_finding: usize,
    pub skipped_duplicate: usize,
    pub executed: usize,
    pub compared: usize,
    pub unsupported: usize,
    pub unused_budget: usize,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct BatchAudit {
    pub pass: i32,
    #[serde(flatten)]
    pub observation: ObservationAudit,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct ObservationAudit {
    pub run: String,
    pub report: String,
    pub inputs_sha256: String,
    pub executed: usize,
    pub compared: usize,
    pub unsupported: usize,
}

impl SelectionAudit {
    pub(crate) fn new() -> Self {
        Self {
            format: "slophawk-contract-selection-audit-v1",
            generator_basis: "emitted-legal-nonpublished-cases",
            execution_basis: "validated-observations",
            passes: Vec::new(), batches: Vec::new(), replay: None,
        }
    }

    /// Record selection before any flush can stop later work for this operation.
    pub(crate) fn nominate(&mut self, operation: &str, pass: i32, budget: usize,
        pool_limit: usize, generated: &[ProbeExpectation], previously_executed: usize,
        selected: &[ProbeExpectation]) -> Result<()> {
        let eligible = generated.len().checked_sub(previously_executed)
            .ok_or("Previously executed cases exceed the nominated pool.")?;
        let not_selected = eligible.checked_sub(selected.len())
            .ok_or("Selected cases exceed the eligible pool.")?;
        if selected.len() > budget {
            return Err("Selected cases exceed the pass budget.".into());
        }
        self.passes.push(PassAudit {
            operation: operation.into(), pass, budget, pool_limit,
            generation: "completed", nominated: generated.len(),
            nominated_inputs_sha256: Some(inputs_digest(generated)?),
            previously_executed, eligible, selected: selected.len(),
            selected_inputs_sha256: Some(inputs_digest(selected)?), not_selected,
            skipped_after_finding: 0, skipped_duplicate: 0, executed: 0,
            compared: 0, unsupported: 0, unused_budget: budget,
        });
        Ok(())
    }

    /// No pool exists for an operation whose earlier finding stopped this pass.
    pub(crate) fn skip_pass(&mut self, operation: &str, pass: i32, budget: usize,
        pool_limit: usize) {
        self.passes.push(PassAudit {
            operation: operation.into(), pass, budget, pool_limit,
            generation: "skipped-after-finding", nominated: 0,
            nominated_inputs_sha256: None, previously_executed: 0, eligible: 0,
            selected: 0, selected_inputs_sha256: None, not_selected: 0,
            skipped_after_finding: 0, skipped_duplicate: 0, executed: 0,
            compared: 0, unsupported: 0, unused_budget: budget,
        });
    }

    pub(crate) fn skip_case(&mut self, pass: i32, operation: &str, after_finding: bool) {
        let row = self.pass_mut(pass, operation);
        if after_finding { row.skipped_after_finding += 1; }
        else { row.skipped_duplicate += 1; }
    }

    /// The caller must validate the full receipt before recording it. Every
    /// returned row consumes budget, including rows after its first mismatch. Null responses
    /// consume execution budget but do not count as supported comparisons.
    pub(crate) fn record_batch(&mut self, pass: i32, cases: &[ProbeExpectation],
        receipt: &Observation) -> Result<()> {
        let observation = observation_audit(cases, receipt)?;
        for (case, actual) in cases.iter().zip(&receipt.observed) {
            let row = self.pass_mut(pass, &case.operation);
            row.executed += 1;
            if actual.is_null() { row.unsupported += 1; }
            else { row.compared += 1; }
            row.unused_budget = row.budget.checked_sub(row.executed)
                .expect("selected search cases cannot exceed the pass budget");
        }
        self.batches.push(BatchAudit { pass, observation });
        Ok(())
    }

    /// A separately validated witness execution is never charged to search.
    pub(crate) fn record_replay(&mut self, cases: &[ProbeExpectation],
        receipt: &Observation) -> Result<()> {
        self.replay = Some(observation_audit(cases, receipt)?);
        Ok(())
    }

    fn pass_mut(&mut self, pass: i32, operation: &str) -> &mut PassAudit {
        self.passes.iter_mut().find(|row| row.pass == pass && row.operation == operation)
            .expect("search records a nomination before flushing its cases")
    }
}

fn inputs_digest(cases: &[ProbeExpectation]) -> Result<String> {
    let inputs: Vec<_> = cases.iter().map(|case| ProbeInput {
        operation: case.operation.clone(), args: case.args.clone(),
    }).collect();
    Ok(sha256_hex(inputs_json(&inputs)?.as_bytes()))
}

fn observation_audit(cases: &[ProbeExpectation], receipt: &Observation)
    -> Result<ObservationAudit> {
    Ok(ObservationAudit {
        run: receipt.run.clone(), report: receipt.report.clone(),
        inputs_sha256: inputs_digest(cases)?, executed: cases.len(),
        compared: receipt.result.compared, unsupported: receipt.result.unsupported,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use slophawk_probe_replay::{JsonValue, ProbeCheckResult, ProbeDifference};

    fn value(raw: &str) -> JsonValue { JsonValue::parse(raw.as_bytes()).unwrap() }
    fn case(operation: &str, arg: i64) -> ProbeExpectation {
        ProbeExpectation { operation: operation.into(), args: vec![arg],
            expected: value(&arg.to_string()) }
    }

    #[test]
    fn mixed_batch_accounts_for_later_rows_and_separates_witness_replay() {
        let a: Vec<_> = (0..=4).map(|arg| case("A", arg)).collect();
        let b = [case("B", 7), case("B", 8)];
        let mut audit = SelectionAudit::new();
        audit.nominate("A", 0, 3, 12, &a, 1, &a[1..4]).unwrap();
        audit.nominate("B", 0, 3, 12, &b, 0, &b).unwrap();
        let cases = [a[1].clone(), a[2].clone(), b[0].clone()];
        // Hand-authored observations: the first A differs, the second A is
        // unsupported, and B still completes in the same batch after the finding.
        let receipt = Observation {
            run: "search-run".into(), candidate: "candidate-sha".into(),
            artifacts: "stage/search".into(), report: "stage/search/report.json".into(),
            observed: vec![value("99"), value("null"), value("7")],
            result: ProbeCheckResult { cases: 3, compared: 2, unsupported: 1,
                findings: vec![ProbeDifference { index: 0,
                    input: ProbeInput {operation: "A".into(), args: vec![1]},
                    candidate: value("99"), reference: value("1") }] },
        };
        audit.record_batch(0, &cases, &receipt).unwrap();
        audit.skip_case(0, "A", true);
        audit.skip_case(0, "B", false);
        let row = &audit.passes[0];
        assert_eq!((row.nominated, row.previously_executed, row.eligible,
            row.selected, row.not_selected), (5, 1, 4, 3, 1));
        assert_eq!((row.executed, row.compared, row.unsupported,
            row.skipped_after_finding, row.skipped_duplicate, row.unused_budget),
            (2, 1, 1, 1, 0, 1));
        let row = &audit.passes[1];
        assert_eq!((row.nominated, row.selected, row.executed, row.compared,
            row.unsupported, row.skipped_duplicate, row.unused_budget),
            (2, 2, 1, 1, 0, 1, 2));
        for row in &audit.passes {
            assert_eq!(row.nominated, row.previously_executed + row.eligible);
            assert_eq!(row.eligible, row.selected + row.not_selected);
            assert_eq!(row.selected,
                row.executed + row.skipped_after_finding + row.skipped_duplicate);
            assert_eq!(row.executed, row.compared + row.unsupported);
            assert_eq!(row.budget, row.executed + row.unused_budget);
        }
        let before = serde_json::to_value(&audit.passes).unwrap();
        let replay = Observation {
            run: "replay-run".into(), candidate: "candidate-sha".into(),
            artifacts: "stage/replay".into(), report: "stage/replay/report.json".into(),
            observed: vec![value("99"), value("7")],
            result: ProbeCheckResult { cases: 2, compared: 2, unsupported: 0,
                findings: receipt.result.findings.clone() },
        };
        audit.record_replay(&[a[1].clone(), b[0].clone()], &replay).unwrap();
        assert_eq!(serde_json::to_value(&audit.passes).unwrap(), before);
        assert_eq!(audit.batches.len(), 1);
        let batch = &audit.batches[0].observation;
        assert_eq!((batch.executed, batch.compared, batch.unsupported), (3, 2, 1));
        assert_eq!(batch.run, "search-run");
        let replay = audit.replay.as_ref().unwrap();
        assert_eq!((replay.executed, replay.compared, replay.unsupported), (2, 2, 0));
        assert_eq!(replay.run, "replay-run");
        assert_eq!(replay.report, "stage/replay/report.json");
        let json = serde_json::to_value(&audit).unwrap();
        assert_eq!(json["Format"], "slophawk-contract-selection-audit-v1");
        assert_eq!(json["Batches"][0]["Pass"], 0);
        assert_eq!(json["Batches"][0]["Executed"], 3);
        assert!(json["Batches"][0].get("Observation").is_none());
    }

    #[test]
    fn exhausted_pool_is_distinct_from_a_pass_skipped_after_a_finding() {
        let mut audit = SelectionAudit::new();
        audit.nominate("A", 0, 1, 4, &[], 0, &[]).unwrap();
        audit.skip_pass("A", 1, 1, 4);
        let empty = &audit.passes[0];
        assert_eq!(empty.generation, "completed");
        assert_eq!((empty.nominated, empty.eligible, empty.selected,
            empty.executed, empty.unused_budget), (0, 0, 0, 0, 1));
        let digest = "4F53CDA18C2BAA0C0354BB5F9A3ECBE5ED12AB4D8E11BA873C2F11161202B945";
        assert_eq!(empty.nominated_inputs_sha256.as_deref(), Some(digest));
        assert_eq!(empty.selected_inputs_sha256.as_deref(), Some(digest));
        let skipped = &audit.passes[1];
        assert_eq!(skipped.generation, "skipped-after-finding");
        assert_eq!((skipped.nominated, skipped.selected, skipped.executed,
            skipped.skipped_after_finding, skipped.unused_budget), (0, 0, 0, 0, 1));
        assert!(skipped.nominated_inputs_sha256.is_none());
        assert!(skipped.selected_inputs_sha256.is_none());
        assert!(audit.batches.is_empty());
        assert!(audit.replay.is_none());
    }

    #[test]
    fn digests_bind_ordered_requests_without_including_expectations() {
        let cases = [case("A", 1), case("B", 2)];
        let expected = sha256_hex(br#"[{"Operation":"A","Args":[1]},{"Operation":"B","Args":[2]}]"#);
        assert_eq!(inputs_digest(&cases).unwrap(), expected);
        let mut changed_expectation = cases.clone();
        changed_expectation[0].expected = value("999");
        assert_eq!(inputs_digest(&changed_expectation).unwrap(), expected);
        assert_ne!(inputs_digest(&[cases[1].clone(), cases[0].clone()]).unwrap(), expected);
    }
}
