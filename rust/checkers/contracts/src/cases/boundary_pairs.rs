use std::collections::HashSet;

use super::{ContractCases, expectation};
use crate::ProbeExpectation;

impl ContractCases {
    /// Combine two parameters' authored basis extrema, holding other inputs at a
    /// published example. Only qualified, in-domain cases with an expected value
    /// are returned. Work is bounded by four examples and four endpoint
    /// orientations per pair of parameters.
    pub fn boundary_pair_cases(&self, operation: &str, limit: usize) -> Vec<ProbeExpectation> {
        let Some(rule) = self
            .rules
            .iter()
            .find(|rule| rule.definition.operation == operation)
        else {
            return Vec::new();
        };
        let extrema: Vec<_> = rule
            .definition
            .parameters
            .iter()
            .enumerate()
            .filter_map(|(index, parameter)| {
                let (_, values) = rule
                    .definition
                    .basis
                    .iter()
                    .find(|(name, _)| name == parameter)?;
                let (low, high) = (*values.iter().min()?, *values.iter().max()?);
                (low < high).then_some((index, [low, high]))
            })
            .collect();
        let mut cases = Vec::new();
        let mut seen: HashSet<_> = rule.ordinary.iter().map(|case| case.args.clone()).collect();
        for (first_end, second_end) in [(0, 1), (1, 0), (0, 0), (1, 1)] {
            for example in rule.ordinary.iter().take(4) {
                for (first, &(first_index, first_values)) in extrema.iter().enumerate() {
                    for &(second_index, second_values) in &extrema[first + 1..] {
                        if cases.len() == limit {
                            return cases;
                        }
                        let mut args = example.args.clone();
                        args[first_index] = first_values[first_end];
                        args[second_index] = second_values[second_end];
                        if seen.insert(args.clone())
                            && let Some(value) = rule.try_expected(&args)
                        {
                            cases.push(expectation(operation, args, value));
                        }
                    }
                }
            }
        }
        cases
    }
}
