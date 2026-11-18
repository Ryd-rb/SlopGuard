//! A deliberately narrow affine move, not a general constraint solver.
//! Equalities with coefficient difference e_i - e_j admit a common wrapping
//! shift of every parameter in a connected component, including affine offsets.
//! All proposals still enter the ordinary queue and interpreter.

use super::{Comparison, Search, Ty, changes, compare};

const MAX_PARAMETERS: usize = 16;
const MAX_EQUALITIES: usize = 32;

fn root(parents: &[usize], mut index: usize) -> usize {
    while parents[index] != index {
        index = parents[index];
    }
    index
}

impl Search<'_> {
    // Preparation, every inspected row, every target equation (even unsolvable),
    // and every considered representative share the existing proposal cap.
    fn correlated_credit(&mut self) -> bool {
        if self.proposal_work >= self.proposal_limit() {
            self.unknown.insert(
                "Correlated-equalities-v1 exhausted the shared proposal budget.".into(),
            );
            return false;
        }
        self.proposal_work += 1;
        true
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn propose_correlated(
        &mut self,
        args: &[i64],
        comparison: &Comparison,
        alternate: &str,
        taken: &str,
        depth: usize,
        preserved: &[Comparison],
    ) {
        if preserved.len() < 2 || !self.correlated_credit() {
            return;
        }
        // Check bounds before allocating or scanning coefficient rows. The
        // caller supplies only compatible inputs from the existing search.
        if args.len() > MAX_PARAMETERS {
            self.unknown.insert(
                "Correlated-equalities-v1 parameter limit 16 exceeded; using legacy proposals."
                    .into(),
            );
            return;
        }
        if preserved.len() > MAX_EQUALITIES {
            self.unknown.insert(
                "Correlated-equalities-v1 equality limit 32 exceeded; using legacy proposals."
                    .into(),
            );
            return;
        }
        let mut parents = (0..args.len()).collect::<Vec<_>>();
        for prior in preserved {
            if !self.correlated_credit() {
                return;
            }
            let mut positive = None;
            let mut negative = None;
            let mut supported = prior.left.ty == Ty::Int && prior.right.ty == Ty::Int;
            for parameter in 0..args.len() {
                let factor = prior
                    .left
                    .factor(parameter)
                    .wrapping_sub(prior.right.factor(parameter));
                match factor {
                    0 => (),
                    1 if positive.is_none() && self.p.params[parameter] == Ty::Int => {
                        positive = Some(parameter);
                    }
                    -1 if negative.is_none() && self.p.params[parameter] == Ty::Int => {
                        negative = Some(parameter);
                    }
                    _ => supported = false,
                }
            }
            if !supported || positive.is_some() != negative.is_some() {
                self.unknown.insert(
                    "Correlated-equalities-v1 skipped a non-unit or non-int prefix equality; using legacy proposals."
                        .into(),
                );
                return;
            }
            // A zero coefficient row is invariant under every input change.
            if let (Some(a), Some(b)) = (positive, negative) {
                let a = root(&parents, a);
                let b = root(&parents, b);
                parents[a.max(b)] = a.min(b);
            }
        }
        // Roots and members are visited in ascending parameter ordinal. No hash
        // iteration controls ordering, and no Cartesian products are formed.
        for leader in 0..args.len() {
            let members = (0..args.len())
                .filter(|&parameter| root(&parents, parameter) == leader)
                .collect::<Vec<_>>();
            if members.len() < 3 {
                continue;
            }
            let left_factor = members.iter().fold(0_i32, |sum, &parameter| {
                sum.wrapping_add(comparison.left.factor(parameter))
            });
            let right_factor = members.iter().fold(0_i32, |sum, &parameter| {
                sum.wrapping_add(comparison.right.factor(parameter))
            });
            let difference = comparison.left.value.wrapping_sub(comparison.right.value);
            let factor = left_factor.wrapping_sub(right_factor);
            // Keep the baseline's equality/neighbour/extrema bias. Difference
            // targets do not imply signed-order outcomes across overflow.
            let equations = [
                (factor, 0_i32.wrapping_sub(difference)),
                (factor, (-1_i32).wrapping_sub(difference)),
                (factor, 1_i32.wrapping_sub(difference)),
                (left_factor, i32::MIN.wrapping_sub(comparison.left.value)),
                (right_factor, i32::MIN.wrapping_sub(comparison.right.value)),
                (left_factor, i32::MAX.wrapping_sub(comparison.left.value)),
                (right_factor, i32::MAX.wrapping_sub(comparison.right.value)),
            ];
            for (factor, delta) in equations {
                if !self.correlated_credit() {
                    return;
                }
                // changes() keeps its existing at-most-two representative rule;
                // no solution here is not evidence that a branch is infeasible.
                for change in changes(factor, delta) {
                    if !self.correlated_credit() {
                        return;
                    }
                    if change == 0 {
                        continue;
                    }
                    let mut copy = args.to_vec();
                    for &parameter in &members {
                        copy[parameter] = (copy[parameter] as i32).wrapping_add(change) as i64;
                    }
                    let left = comparison
                        .left
                        .value
                        .wrapping_add(left_factor.wrapping_mul(change));
                    let right = comparison
                        .right
                        .value
                        .wrapping_add(right_factor.wrapping_mul(change));
                    let flips = compare(
                        &comparison.operator,
                        comparison.left.value,
                        comparison.right.value,
                    ) != compare(&comparison.operator, left, right);
                    let priority = -(2 * depth as i32) - i32::from(flips);
                    // Queue priority is only a hint. Other guards, scope changes,
                    // holes and loops are re-evaluated by Search::simulate.
                    self.enqueue(copy, if flips { alternate } else { taken }, priority);
                }
            }
        }
    }
}
