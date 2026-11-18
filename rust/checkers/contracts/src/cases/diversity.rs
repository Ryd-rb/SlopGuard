//! Opt-in allocation over a caller-supplied pool; no new candidate synthesis.
use super::{ContractCases, Rule, expectation, fill_frame};
use crate::expression::Comparison;
use crate::integer::Integer;
use crate::plan::{MAX_ARITY, MAX_OPERATION_LIMIT, MAX_SEEDS_PER_OPERATION, MAX_TOTAL_PLAN};
use crate::{ContractError, ProbeExpectation, ProbeInput};
use std::cmp::Ordering;
use std::collections::BTreeSet;

const MAX_CONDITIONS: usize = 64;
const PARTITIONS: usize = 5;
const UNKNOWN: u8 = 5;

impl ContractCases {
    /// Select at most `per_operation` new cases from an ordered input pool.
    ///
    /// This opt-in allocator leaves `generate` and `plan_inputs` unchanged. It
    /// retains qualified-operation order and excludes published inputs and
    /// duplicates. Unknown/unqualified operations, wrong arities, illegal
    /// domains and unavailable expectations produce no cases. Domain filtering
    /// still precedes local and expected-output evaluation.
    ///
    /// Zero-based even output slots take the earliest remaining admissible pool entry.
    /// Odd slots rotate through explicit comparisons, preferring an unrepresented
    /// partition of left minus right: `< -1`, `-1`, `0`, `1`, or `> 1`.
    /// Published inputs and every selection credit all their known partitions.
    /// Exhausted partitions fall back to pool order. Thus at least the first
    /// `ceil(min(per_operation, admissible_count) / 2)` pool entries survive.
    /// This is not a prefix guarantee for a separately generated smaller pool.
    ///
    /// Comparisons follow domain, ordered locals, then expected expression;
    /// operands are visited in left-to-right postorder. Structurally identical
    /// parsed comparisons are coalesced. Constant comparisons and implicit
    /// function branches are not targets. Off-path errors mean unknown partitions, never
    /// rejected otherwise-valid cases. These features do not establish target
    /// branch coverage, feasibility, or oracle correctness.
    ///
    /// Raw pools are limited to 65536 rows total and 8192 per qualified
    /// operation, with arity at most 32. An active rule may have at most 256
    /// published inputs and 64 distinct input-dependent comparisons. Limits
    /// return `Scope`, without partial output; `per_operation` must be 1..=8192.
    /// Candidate exhaustion may return fewer cases. After expression evaluation,
    /// scheduling takes O(P*(C+1) + B*C) work per operation for P raw pool rows,
    /// C comparisons and budget B. Storage is O(P*(A+C)) for arity A, plus
    /// published anchors, expression frames, output and operation-name bytes.
    /// Pool production and oracle work are separate costs; callers must account
    /// for them when comparing budgets.
    pub fn select_diverse(
        &self,
        candidates: &[ProbeInput],
        per_operation: usize,
    ) -> Result<Vec<ProbeExpectation>, ContractError> {
        if !(1..=MAX_OPERATION_LIMIT).contains(&per_operation) {
            return Err(ContractError::Invalid(
                "per-operation count must be from 1 through 8192".into(),
            ));
        }
        if candidates.len() > MAX_TOTAL_PLAN {
            return Err(ContractError::Scope(
                "diversity pool exceeds 65536 inputs".into(),
            ));
        }
        if candidates.iter().any(|input| input.args.len() > MAX_ARITY) {
            return Err(ContractError::Scope("diversity input arity exceeds 32".into()));
        }
        if candidates.iter().any(|input| input.operation.trim().is_empty()) {
            return Err(ContractError::Invalid(
                "Each probe needs an operation name.".into(),
            ));
        }
        let counts: Vec<usize> = self
            .rules
            .iter()
            .map(|rule| {
                candidates
                    .iter()
                    .filter(|input| input.operation == rule.definition.operation)
                    .count()
            })
            .collect();
        if counts.iter().any(|&count| count > MAX_OPERATION_LIMIT) {
            return Err(ContractError::Scope(
                "diversity pool exceeds 8192 inputs for one qualified operation".into(),
            ));
        }
        if counts
            .iter()
            .filter(|&&count| count != 0)
            .count()
            .checked_mul(per_operation)
            .is_none_or(|total| total > MAX_TOTAL_PLAN)
        {
            return Err(ContractError::Scope(
                "contract case plan exceeds 65536 results".into(),
            ));
        }
        let mut output = Vec::new();
        for (rule, count) in self.rules.iter().zip(counts) {
            if count == 0 {
                continue;
            }
            if rule.ordinary.len() > MAX_SEEDS_PER_OPERATION {
                return Err(ContractError::Scope(
                    "more than 256 published inputs for one operation".into(),
                ));
            }
            let mut conditions = Vec::new();
            for expression in std::iter::once(&rule.domain)
                .chain(rule.locals.iter())
                .chain(std::iter::once(&rule.expected))
            {
                expression
                    .collect_comparisons(&mut conditions, MAX_CONDITIONS)
                    .map_err(|message| ContractError::Scope(message.into()))?;
            }
            let mut covered = vec![[false; PARTITIONS]; conditions.len()];
            let mut seen = BTreeSet::new();
            for example in &rule.ordinary {
                if seen.insert(example.args.clone()) {
                    credit(&mut covered, &partitions(rule, &conditions, &example.args));
                }
            }
            let mut pool = Vec::new();
            for input in candidates
                .iter()
                .filter(|input| input.operation == rule.definition.operation)
            {
                if !seen.insert(input.args.clone()) {
                    continue;
                }
                // Qualification is already established. Reuse its exact
                // domain-first oracle path before computing selection features.
                let Some(value) = rule.try_expected(&input.args) else {
                    continue;
                };
                pool.push(Candidate {
                    case: expectation(&rule.definition.operation, input.args.clone(), value),
                    partitions: partitions(rule, &conditions, &input.args),
                });
            }
            output.extend(select(&pool, per_operation, covered));
        }
        Ok(output)
    }
}

struct Candidate {
    case: ProbeExpectation,
    partitions: Vec<u8>,
}

// Only qualified published inputs or inputs accepted by try_expected arrive
// here. A comparison in an unvisited lazy arm can still fail independently.
fn partitions(rule: &Rule, conditions: &[Comparison], args: &[i64]) -> Vec<u8> {
    let Ok(frame) = fill_frame(args, &rule.locals) else {
        return vec![UNKNOWN; conditions.len()];
    };
    conditions
        .iter()
        .map(|condition| {
            let Ok(difference) = condition.difference(&frame) else {
                return UNKNOWN;
            };
            match difference.compare(&Integer::from_i64(-1)) {
                Ordering::Less => 0,
                Ordering::Equal => 1,
                Ordering::Greater if difference.is_zero() => 2,
                Ordering::Greater if difference.compare(&Integer::one()) == Ordering::Equal => 3,
                Ordering::Greater => 4,
            }
        })
        .collect()
}

fn credit(covered: &mut [[bool; PARTITIONS]], partitions: &[u8]) {
    for (condition, &partition) in covered.iter_mut().zip(partitions) {
        if let Some(slot) = condition.get_mut(usize::from(partition)) {
            *slot = true;
        }
    }
}

fn select(
    pool: &[Candidate],
    budget: usize,
    mut covered: Vec<[bool; PARTITIONS]>,
) -> Vec<ProbeExpectation> {
    let mut output = Vec::new();
    let mut selected = vec![false; pool.len()];
    let mut cursors = vec![0; covered.len()];
    let mut condition = 0;
    let mut first = 0;
    while output.len() < budget {
        while first < pool.len() && selected[first] {
            first += 1;
        }
        if first == pool.len() {
            break;
        }
        let mut novel = None;
        if output.len() % 2 == 1 {
            for _ in 0..covered.len() {
                let target = condition;
                condition = (condition + 1) % covered.len();
                // Cursors only move forward: a selected row or an already
                // credited partition can never become novel again.
                while cursors[target] < pool.len() {
                    let index = cursors[target];
                    cursors[target] += 1;
                    let partition = usize::from(pool[index].partitions[target]);
                    if !selected[index] && partition < PARTITIONS && !covered[target][partition] {
                        novel = Some(index);
                        break;
                    }
                }
                if novel.is_some() {
                    break;
                }
            }
        }
        let index = novel.unwrap_or(first);
        selected[index] = true;
        credit(&mut covered, &pool[index].partitions);
        output.push(pool[index].case.clone());
    }
    output
}
