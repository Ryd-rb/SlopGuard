//! Deterministic `ProbeReplay.Plan` candidate ordering and bounded mutation streams.
use crate::{ContractError, rng::CompatRandom};
use slophawk_probe_replay::ProbeInput;
use std::collections::{HashSet, VecDeque};

pub const MAX_ARITY: usize = 32;
pub const MAX_OPERATION_LIMIT: usize = 8192;
pub const MAX_TOTAL_PLAN: usize = 65_536;
pub const MAX_SEEDS_PER_OPERATION: usize = 256;
pub const MAX_SCALE_COUNT: usize = 16;
const MAX_PATHS_PER_SEED: usize = 128_000;
const MAX_ATTEMPTS_PER_SHAPE: usize = 1_000_000;

fn scope(what: &str) -> ContractError {
    ContractError::Scope(what.into())
}

pub fn plan_inputs(
    seeds: &[ProbeInput],
    limit: usize,
    random_seed: i32,
    scales: &[i64],
) -> Result<Vec<ProbeInput>, ContractError> {
    if !(1..=MAX_OPERATION_LIMIT).contains(&limit) {
        return Err(scope("probe count must be 1..=8192"));
    }
    let mut units = Vec::new();
    for &scale in scales {
        if scale <= 1 {
            return Err(ContractError::Invalid(
                "Probe scales must be integers greater than one.".into(),
            ));
        }
        if !units.contains(&scale) {
            units.push(scale);
        }
    }
    if units.len() > MAX_SCALE_COUNT {
        return Err(scope("more than 16 distinct scales"));
    }
    let mut groups: Vec<(String, Vec<Vec<i64>>)> = Vec::new();
    for seed in seeds {
        if seed.operation.trim().is_empty() {
            return Err(ContractError::Invalid(
                "Each probe needs an operation name and an argument array.".into(),
            ));
        }
        if seed.args.len() > MAX_ARITY {
            return Err(scope("probe arity exceeds 32"));
        }
        if let Some((_, items)) = groups.iter_mut().find(|(name, _)| *name == seed.operation) {
            items.push(seed.args.clone());
            if items.len() > MAX_SEEDS_PER_OPERATION {
                return Err(scope("more than 256 seeds for one operation"));
            }
        } else {
            if groups.len() >= 64 {
                return Err(scope("more than 64 operations"));
            }
            groups.push((seed.operation.clone(), vec![seed.args.clone()]));
        }
    }
    if groups
        .len()
        .checked_mul(limit)
        .is_none_or(|n| n > MAX_TOTAL_PLAN)
    {
        return Err(scope("generated plan exceeds 65536 cases"));
    }
    let mut result = Vec::new();
    for (operation, examples) in groups {
        let mut shapes: Vec<(usize, Vec<Vec<i64>>)> = Vec::new();
        for args in examples {
            if let Some((_, items)) = shapes.iter_mut().find(|(length, _)| *length == args.len()) {
                items.push(args);
            } else {
                shapes.push((args.len(), vec![args]));
            }
        }
        let mut iterators: VecDeque<ShapeIterator> = shapes
            .into_iter()
            .map(|(_, examples)| ShapeIterator::new(examples, random_seed, limit, units.clone()))
            .collect();
        let mut seen = HashSet::<Vec<i64>>::new();
        while let Some(mut source) = iterators.pop_front() {
            if let Some(args) = source.next()? {
                if seen.insert(args.clone()) {
                    result.push(ProbeInput {
                        operation: operation.clone(),
                        args,
                    });
                    if seen.len() == limit {
                        break;
                    }
                }
                iterators.push_back(source);
            }
        }
    }
    Ok(result)
}

fn add(value: i64, delta: i64) -> i64 {
    (i128::from(value) + i128::from(delta)).clamp(i128::from(i64::MIN), i128::from(i64::MAX)) as i64
}

fn replace(seed: &[i64], index: usize, value: i64) -> Vec<i64> {
    let mut result = seed.to_vec();
    result[index] = value;
    result
}

fn axes(seed: &[i64]) -> Vec<Vec<i64>> {
    let n = seed.len();
    let mut output = Vec::new();
    for delta in [-1, 1, -2, 2] {
        for index in 0..n {
            output.push(replace(seed, index, add(seed[index], delta)));
        }
    }
    for value in [0, 1, -1, 2, 3, 4, 7, 8, 15, 16] {
        for index in 0..n {
            output.push(replace(seed, index, value));
        }
    }
    for index in 0..n {
        for &value in seed {
            output.push(replace(seed, index, value));
            output.push(replace(seed, index, add(value, -1)));
            output.push(replace(seed, index, add(value, 1)));
        }
    }
    output
}

fn relations(seed: &[i64]) -> Vec<Vec<i64>> {
    let n = seed.len();
    let mut output = Vec::new();
    for &value in seed.iter().chain([0, 1, -1, 2, 3].iter()) {
        output.push(vec![value; n]);
    }
    for first in 0..n {
        for second in first + 1..n {
            let mut swapped = seed.to_vec();
            swapped.swap(first, second);
            output.push(swapped);
            for tie in [seed[first], seed[second], 0, 1] {
                for delta in [0, -1, 1] {
                    let mut point = replace(seed, first, tie);
                    point[second] = add(tie, delta);
                    output.push(point);
                }
            }
        }
    }
    output
}

fn scaled_offsets(seed: &[i64], scales: &[i64]) -> Vec<Vec<i64>> {
    let mut output = Vec::new();
    for first in 0..seed.len() {
        for second in 0..seed.len() {
            if first == second {
                continue;
            }
            for &scale in scales {
                let mut lower = replace(seed, first, add(seed[first], -1));
                lower[second] = add(seed[second], scale);
                output.push(lower);
                let mut upper = replace(seed, first, add(seed[first], 1));
                upper[second] = add(seed[second], -scale);
                output.push(upper);
            }
        }
    }
    output
}

fn scatter(seed: &[i64], random: &mut CompatRandom) -> Vec<i64> {
    let n = seed.len();
    let mut point = seed.to_vec();
    let shared = if random.next(2) == 0 {
        seed[random.next(n)]
    } else {
        i64::from(random.range(-1, 9))
    };
    let changes = match random.next(5) {
        0 => 1,
        1 => 2,
        2 => 3,
        3 => n,
        _ => random.range(1, n as i32 + 1) as usize,
    };
    for change in 0..changes {
        let index = if changes == n { change } else { random.next(n) };
        point[index] = match random.next(10) {
            0 => 0,
            1 => 1,
            2 => shared,
            3 => add(shared, i64::from(random.range(-1, 2))),
            4 => add(seed[index], i64::from(random.range(-4, 5))),
            5 => seed[random.next(n)],
            6 => i64::from(random.range(0, 16)),
            7 => seed[index] ^ (1_i64 << random.range(0, 5)),
            8 => seed[index] / 2,
            _ => add(seed[index], seed[random.next(n)]),
        };
    }
    point
}

struct Paths {
    axes: Vec<Vec<i64>>,
    relations: Vec<Vec<i64>>,
    scaled: Vec<Vec<i64>>,
    a: usize,
    r: usize,
    s: usize,
}

impl Paths {
    fn new(seed: &[i64], scales: &[i64]) -> Result<Self, ContractError> {
        let a = axes(seed);
        let r = relations(seed);
        let s = scaled_offsets(seed, scales);
        if a.len() + r.len() + s.len() > MAX_PATHS_PER_SEED {
            return Err(scope("correlated candidate paths exceed 128000 per seed"));
        }
        Ok(Self {
            axes: a,
            relations: r,
            scaled: s,
            a: 0,
            r: 0,
            s: 0,
        })
    }
    fn take(&mut self) -> Vec<Vec<i64>> {
        let mut group = Vec::with_capacity(3);
        if let Some(args) = self.axes.get(self.a) {
            group.push(args.clone());
            self.a += 1;
        }
        if let Some(args) = self.relations.get(self.r) {
            group.push(args.clone());
            self.r += 1;
        }
        if let Some(args) = self.scaled.get(self.s) {
            group.push(args.clone());
            self.s += 1;
        }
        group
    }
}

struct ShapeIterator {
    examples: Vec<Vec<i64>>,
    initial: usize,
    current: usize,
    paths: Option<Paths>,
    pending: VecDeque<Vec<i64>>,
    random: CompatRandom,
    fallback_trials: usize,
    limit: usize,
    scales: Vec<i64>,
    attempts: usize,
}

impl ShapeIterator {
    fn new(examples: Vec<Vec<i64>>, seed: i32, limit: usize, scales: Vec<i64>) -> Self {
        Self {
            examples,
            initial: 0,
            current: 0,
            paths: None,
            pending: VecDeque::new(),
            random: CompatRandom::new(seed),
            fallback_trials: 0,
            limit,
            scales,
            attempts: 0,
        }
    }

    fn next(&mut self) -> Result<Option<Vec<i64>>, ContractError> {
        loop {
            if self.attempts >= MAX_ATTEMPTS_PER_SHAPE {
                return Err(scope("more than 1000000 candidates in an arity stream"));
            }
            if self.initial < self.examples.len() {
                let output = self.examples[self.initial].clone();
                self.initial += 1;
                self.attempts += 1;
                return Ok(Some(output));
            }
            if self.examples[0].is_empty() {
                return Ok(None);
            }
            if let Some(output) = self.pending.pop_front() {
                self.attempts += 1;
                return Ok(Some(output));
            }
            if self.current < self.examples.len() {
                if self.paths.is_none() {
                    self.paths = Some(Paths::new(&self.examples[self.current], &self.scales)?);
                }
                let group = self.paths.as_mut().expect("initialized").take();
                if group.is_empty() {
                    self.current += 1;
                    self.paths = None;
                    continue;
                }
                self.pending.extend(group);
                self.pending
                    .push_back(scatter(&self.examples[self.current], &mut self.random));
                continue;
            }
            if self.fallback_trials < 8 * self.limit {
                self.fallback_trials += 1;
                let selected = self.random.next(self.examples.len());
                self.attempts += 1;
                return Ok(Some(scatter(&self.examples[selected], &mut self.random)));
            }
            return Ok(None);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::plan_inputs;
    use slophawk_probe_replay::ProbeInput;
    fn input(op: &str, args: &[i64]) -> ProbeInput {
        ProbeInput {
            operation: op.into(),
            args: args.into(),
        }
    }
    #[test]
    fn grouping_arity_dedup_and_limits() {
        let seeds = [
            input("b", &[1]),
            input("a", &[]),
            input("b", &[1, 2]),
            input("b", &[1]),
        ];
        let first = plan_inputs(&seeds, 3, 1, &[]).unwrap();
        assert_eq!(first[0].operation, "b");
        assert_eq!(first[0].args, vec![1]);
        assert!(
            first
                .iter()
                .any(|item| item.operation == "b" && item.args.len() == 2)
        );
        assert_eq!(first.last().unwrap().operation, "a");
        assert_eq!(plan_inputs(&seeds, 3, 1, &[]).unwrap().len(), first.len());
        assert!(plan_inputs(&seeds, 1, 0, &[1]).is_err());
        assert!(plan_inputs(&seeds, 8193, 0, &[]).is_err());
    }
    #[test]
    fn scaling_clamps_extreme_arguments_and_seed_is_reproducible() {
        let seeds = [input("op", &[i64::MIN, i64::MAX])];
        let first = plan_inputs(&seeds, 120, 17, &[i64::MAX]).unwrap();
        let again = plan_inputs(&seeds, 120, 17, &[i64::MAX]).unwrap();
        assert_eq!(
            first
                .iter()
                .map(|i| (&i.operation, &i.args))
                .collect::<Vec<_>>(),
            again
                .iter()
                .map(|i| (&i.operation, &i.args))
                .collect::<Vec<_>>()
        );
        assert!(first.iter().all(|point| point.args.len() == 2));
        assert!(first.iter().any(|point| point.args.contains(&i64::MAX)));
    }
}
