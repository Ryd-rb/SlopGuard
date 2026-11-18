//! Public-rule qualification, interpreted expectations and ordered seeded generation.
use crate::expression::{self, Comparison, Expr, Name};
use crate::integer::Integer;
use crate::plan::{self, MAX_OPERATION_LIMIT, MAX_TOTAL_PLAN};
use crate::rng::CompatRandom;
use crate::{ContractError, ContractQualification, IntegerContract};
use slophawk_probe_replay::{JsonValue, ProbeExpectation, ProbeInput};
use std::cmp::Reverse;
use std::collections::HashSet;

const MAX_GUIDED_POINTS: usize = 250_000;

pub struct ContractCases {
    rules: Vec<Rule>,
    qualification: Vec<ContractQualification>,
    parameter_counts: Vec<(String, usize)>,
}

impl ContractCases {
    pub fn new(
        contracts: &[IntegerContract],
        ordinary: &[ProbeExpectation],
    ) -> Result<Self, ContractError> {
        slophawk_probe_replay::inputs_for_check(ordinary)
            .map_err(|error| ContractError::Invalid(format!("{error:?}")))?;
        if contracts.len() > 64 {
            return Err(ContractError::Scope("more than 64 contracts".into()));
        }
        let mut used = HashSet::new();
        for definition in contracts {
            if definition.operation.trim().is_empty() || !used.insert(&definition.operation) {
                return Err(ContractError::Invalid(
                    "Contract operation names must be present and unique.".into(),
                ));
            }
        }
        let mut groups: Vec<(String, Vec<ProbeExpectation>)> = Vec::new();
        for example in ordinary {
            if let Some((_, items)) = groups
                .iter_mut()
                .find(|(name, _)| *name == example.operation)
            {
                items.push(example.clone());
            } else {
                groups.push((example.operation.clone(), vec![example.clone()]));
            }
        }
        let mut qualification = Vec::new();
        let mut rules = Vec::new();
        let mut parameter_counts = Vec::new();
        for (operation, examples) in &groups {
            let Some(definition) = contracts.iter().find(|def| def.operation == *operation) else {
                qualification.push(ContractQualification {
                    operation: operation.clone(),
                    source: None,
                    error: Some("No executable public rule.".into()),
                });
                continue;
            };
            let mut maybe = Rule::new(definition, examples);
            if let Ok(rule) = maybe.as_ref() {
                for example in examples {
                    let published = example.expected.raw_json().parse::<i64>().ok();
                    let derived = rule.try_expected(&example.args);
                    if published.is_none() || published != derived {
                        maybe = Err(
                            "The rule does not reproduce a published example or its legal domain."
                                .into(),
                        );
                        break;
                    }
                }
            }
            match maybe {
                Ok(rule) => {
                    qualification.push(ContractQualification {
                        operation: operation.clone(),
                        source: Some(definition.source.clone()),
                        error: None,
                    });
                    parameter_counts.push((operation.clone(), definition.parameters.len()));
                    rules.push(rule);
                }
                Err(message) => qualification.push(ContractQualification {
                    operation: operation.clone(),
                    source: Some(definition.source.clone()),
                    error: Some(message),
                }),
            }
        }
        for definition in contracts {
            if groups.iter().all(|(name, _)| name != &definition.operation) {
                qualification.push(ContractQualification {
                    operation: definition.operation.clone(),
                    source: Some(definition.source.clone()),
                    error: Some("No published example to qualify the rule.".into()),
                });
            }
        }
        Ok(Self {
            rules,
            qualification,
            parameter_counts,
        })
    }

    pub fn qualification(&self) -> &[ContractQualification] {
        &self.qualification
    }
    pub fn parameter_counts(&self) -> &[(String, usize)] {
        &self.parameter_counts
    }

    pub fn evaluate(&self, operation: &str, args: &[i64]) -> Option<ProbeExpectation> {
        let rule = self
            .rules
            .iter()
            .find(|rule| rule.definition.operation == operation)?;
        let value = rule.try_expected(args)?;
        Some(expectation(operation, args.to_vec(), value))
    }

    pub fn generate(
        &self,
        per_operation: usize,
        seed: i32,
        operations: Option<&HashSet<String>>,
        guide_with_boundaries: bool,
        implementation_literals: &[(String, Vec<i64>)],
    ) -> Result<Vec<ProbeExpectation>, ContractError> {
        if !(1..=MAX_OPERATION_LIMIT).contains(&per_operation) {
            return Err(ContractError::Invalid(
                "per-operation count must be from 1 through 8192".into(),
            ));
        }
        let count = self
            .rules
            .iter()
            .filter(|rule| operations.is_none_or(|ops| ops.contains(&rule.definition.operation)))
            .count();
        if count
            .checked_mul(per_operation)
            .is_none_or(|total| total > MAX_TOTAL_PLAN)
        {
            return Err(ContractError::Scope(
                "contract case plan exceeds 65536 results".into(),
            ));
        }
        let mut output = Vec::new();
        for rule in &self.rules {
            if operations.is_some_and(|ops| !ops.contains(&rule.definition.operation)) {
                continue;
            }
            let anchors = rule.ordinary.len().min(32);
            let mut legal: Vec<Vec<i64>> = rule
                .ordinary
                .iter()
                .take(anchors)
                .map(|case| case.args.clone())
                .collect();
            let mut seen: HashSet<Vec<i64>> =
                rule.ordinary.iter().map(|case| case.args.clone()).collect();
            let mut random = CompatRandom::new(seed);
            let probes = plan::plan_inputs(
                &rule
                    .ordinary
                    .iter()
                    .map(|case| ProbeInput {
                        operation: case.operation.clone(),
                        args: case.args.clone(),
                    })
                    .collect::<Vec<_>>(),
                per_operation.max(32),
                seed,
                &[],
            )?;
            let mut probe_index = 0;
            let boundaries = if guide_with_boundaries {
                rule.boundary_points()?
            } else {
                Vec::new()
            };
            let equalities = if guide_with_boundaries {
                rule.equality_points()?
            } else {
                Vec::new()
            };
            let combined = if guide_with_boundaries {
                rule.combined_points()?
            } else {
                Vec::new()
            };
            let literals = implementation_literals
                .iter()
                .find(|(name, _)| name == &rule.definition.operation)
                .map_or(&[][..], |(_, values)| values.as_slice());
            let implementation = if literals.is_empty() {
                Vec::new()
            } else {
                rule.implementation_points(literals)?
            };
            let (mut boundary_index, mut equality_index, mut combined_index, mut impl_index) =
                (0, 0, 0, 0);
            let mut emitted = 0;
            for attempt in 0..per_operation * 48 {
                if emitted >= per_operation {
                    break;
                }
                let point = if attempt % 4 == 1 && impl_index < implementation.len() {
                    let next = implementation[impl_index].clone();
                    impl_index += 1;
                    next
                } else if guide_with_boundaries
                    && attempt % 4 == 0
                    && boundary_index < boundaries.len()
                {
                    let next = boundaries[boundary_index].clone();
                    boundary_index += 1;
                    next
                } else if guide_with_boundaries
                    && attempt % 4 == 2
                    && equality_index < equalities.len()
                {
                    let next = equalities[equality_index].clone();
                    equality_index += 1;
                    next
                } else if guide_with_boundaries
                    && attempt % 8 == 3
                    && combined_index < combined.len()
                {
                    let next = combined[combined_index].clone();
                    combined_index += 1;
                    next
                } else if attempt % 3 == 0 && probe_index < probes.len() {
                    let next = probes[probe_index].args.clone();
                    probe_index += 1;
                    next
                } else {
                    rule.mutate(&legal, &mut random, guide_with_boundaries)
                };
                if seen.contains(&point) {
                    continue;
                }
                let Some(value) = rule.try_expected(&point) else {
                    continue;
                };
                seen.insert(point.clone());
                if legal.len() < 64 {
                    legal.push(point.clone());
                } else {
                    let index = random.range(anchors as i32, legal.len() as i32) as usize;
                    legal[index] = point.clone();
                }
                emitted += 1;
                output.push(expectation(&rule.definition.operation, point, value));
            }
        }
        Ok(output)
    }
}

fn expectation(operation: &str, args: Vec<i64>, value: i64) -> ProbeExpectation {
    ProbeExpectation {
        operation: operation.into(),
        args,
        expected: JsonValue::parse(value.to_string().as_bytes())
            .expect("valid signed integer JSON"),
    }
}

#[derive(Clone)]
struct Rule {
    definition: IntegerContract,
    ordinary: Vec<ProbeExpectation>,
    domain: Expr,
    locals: Vec<Expr>,
    expected: Expr,
    atoms: Vec<Vec<i64>>,
    landmarks: Vec<Vec<i64>>,
    guided_atoms: Vec<Vec<i64>>,
    equality_pairs: Vec<(usize, usize)>,
    combined: Vec<Vec<Comparison>>,
}

impl Rule {
    fn new(definition: &IntegerContract, ordinary: &[ProbeExpectation]) -> Result<Self, String> {
        let params = &definition.parameters;
        if definition.source.trim().is_empty()
            || params.is_empty()
            || params.len() > 32
            || params.iter().any(|name| !expression::is_name(name))
            || params.iter().collect::<HashSet<_>>().len() != params.len()
            || ordinary.iter().any(|case| case.args.len() != params.len())
            || definition.lets.len() > 64
            || definition
                .basis
                .iter()
                .any(|(name, values)| !params.contains(name) || values.len() > 32)
            || definition
                .basis
                .iter()
                .map(|(name, _)| name)
                .collect::<HashSet<_>>()
                .len()
                != definition.basis.len()
        {
            return Err(
                "Invalid contract source, parameter names, arity, locals, or input basis.".into(),
            );
        }
        let mut names: Vec<Name> = params
            .iter()
            .enumerate()
            .map(|(index, name)| Name {
                text: name.clone(),
                inputs: 1_u32 << index,
            })
            .collect();
        let domain = expression::compile(&definition.domain, &names)?;
        let mut locals = Vec::new();
        let mut result_groups = Vec::new();
        for (name, text) in &definition.lets {
            if !expression::is_name(name) || names.iter().any(|existing| existing.text == *name) {
                return Err(format!("Invalid or repeated local '{name}'."));
            }
            let compiled = expression::compile(text, &names)?;
            result_groups.extend(compiled.groups);
            names.push(Name {
                text: name.clone(),
                inputs: compiled.expr.inputs,
            });
            locals.push(compiled.expr);
        }
        let result = expression::compile(&definition.expected, &names)?;
        result_groups.extend(result.groups);
        result_groups.sort_by_key(|group| Reverse(group.len()));
        let mut domain_groups = domain.groups;
        domain_groups.sort_by_key(|group| Reverse(group.len()));
        let combined = result_groups
            .into_iter()
            .chain(domain_groups)
            .filter(|group| {
                group
                    .iter()
                    .fold(0_u32, |mask, cmp| mask | cmp.inputs)
                    .count_ones()
                    > 1
            })
            .take(24)
            .collect();
        let expressions = std::iter::once(definition.domain.as_str())
            .chain(std::iter::once(definition.expected.as_str()))
            .chain(definition.lets.iter().map(|(_, value)| value.as_str()))
            .collect::<Vec<_>>()
            .join("\n");
        let (bounds, equality_pairs) = scan_relations(&expressions, params);
        let mut landmarks = Vec::new();
        let mut atoms = Vec::new();
        let mut guided_atoms = Vec::new();
        for (index, name) in params.iter().enumerate() {
            let basis = definition
                .basis
                .iter()
                .find(|(key, _)| key == name)
                .map_or(&[][..], |(_, list)| list.as_slice());
            let mut landmark = Vec::new();
            for &value in bounds[index].iter().chain(basis.iter()) {
                for candidate in [value, offset(value, -1), offset(value, 1)] {
                    if !landmark.contains(&candidate) && landmark.len() < 64 {
                        landmark.push(candidate);
                    }
                }
            }
            let mut atom = Vec::new();
            for value in ordinary
                .iter()
                .map(|case| case.args[index])
                .chain(basis.iter().copied())
            {
                for candidate in [value, offset(value, -1), offset(value, 1)] {
                    insert_unique(&mut atom, candidate);
                }
            }
            for value in [-1, 0, 1, 2, 3, 4, 5, 7, 8, 10, 15, 16, 20] {
                insert_unique(&mut atom, value);
            }
            let mut guided = atom.clone();
            for &value in &landmark {
                insert_unique(&mut guided, value);
            }
            landmarks.push(landmark);
            atoms.push(atom);
            guided_atoms.push(guided);
        }
        Ok(Self {
            definition: definition.clone(),
            ordinary: ordinary.to_vec(),
            domain: domain.expr,
            locals,
            expected: result.expr,
            atoms,
            landmarks,
            guided_atoms,
            equality_pairs,
            combined,
        })
    }

    fn try_expected(&self, args: &[i64]) -> Option<i64> {
        if args.len() != self.definition.parameters.len() {
            return None;
        }
        let mut frame: Vec<Integer> = args.iter().copied().map(Integer::from_i64).collect();
        frame.resize(frame.len() + self.locals.len(), Integer::zero());
        if self.domain.eval(&frame).ok()?.is_zero() {
            return None;
        }
        for (index, local) in self.locals.iter().enumerate() {
            frame[args.len() + index] = local.eval(&frame).ok()?;
        }
        self.expected.eval(&frame).ok()?.to_i64()
    }

    fn boundary_points(&self) -> Result<Vec<Vec<i64>>, ContractError> {
        let mut output = Vec::new();
        for example in &self.ordinary {
            for (field, values) in self.landmarks.iter().enumerate() {
                for &value in values {
                    let mut point = example.args.clone();
                    point[field] = value;
                    push_point(&mut output, point)?;
                }
            }
        }
        Ok(output)
    }

    fn equality_points(&self) -> Result<Vec<Vec<i64>>, ContractError> {
        let mut output = Vec::new();
        for example in &self.ordinary {
            for first in 0..self.equality_pairs.len() {
                for second in first..self.equality_pairs.len() {
                    let directions = if first == second { 2 } else { 4 };
                    for direction in 0..directions {
                        let mut point = example.args.clone();
                        apply_pair(&mut point, self.equality_pairs[first], direction & 1 != 0);
                        if second != first {
                            apply_pair(&mut point, self.equality_pairs[second], direction & 2 != 0);
                        }
                        push_point(&mut output, point)?;
                    }
                }
            }
        }
        Ok(output)
    }

    fn implementation_points(&self, literals: &[i64]) -> Result<Vec<Vec<i64>>, ContractError> {
        let mut scales: Vec<i64> = Vec::new();
        for value in literals.iter().copied() {
            if value == i64::MIN || (-1..=1).contains(&value) {
                continue;
            }
            let scale = value.abs();
            if !scales.contains(&scale) {
                scales.push(scale);
            }
        }
        scales.sort_unstable_by(|a, b| b.cmp(a));
        scales.truncate(16);
        let mut output = Vec::new();
        for scale in scales {
            for example in &self.ordinary {
                let base = &example.args;
                for field in 0..base.len() {
                    for delta in [0, -1, 1] {
                        let mut point = base.clone();
                        point[field] = offset(scale, delta);
                        push_point(&mut output, point)?;
                    }
                }
                for stride in 1..base.len() {
                    for source in 0..base.len() - stride {
                        for target in 0..base.len() - stride {
                            if source == target
                                || source + stride == target
                                || target + stride == source
                            {
                                continue;
                            }
                            for direction in [-1, 1] {
                                let mut point = base.clone();
                                point[target] = offset(base[source], direction);
                                point[target + stride] =
                                    offset(base[source + stride], -direction * scale);
                                push_point(&mut output, point)?;
                            }
                        }
                    }
                }
            }
        }
        Ok(output)
    }

    fn combined_points(&self) -> Result<Vec<Vec<i64>>, ContractError> {
        let mut output = Vec::new();
        for example in self.ordinary.iter().take(4) {
            for group in &self.combined {
                for reverse in [false, true] {
                    let mut point = example.args.clone();
                    if !project(group, &mut point, &self.locals, reverse) || point == example.args {
                        continue;
                    }
                    push_point(&mut output, point.clone())?;
                    for field in 0..point.len() {
                        if point[field] == example.args[field] {
                            continue;
                        }
                        for delta in [-1, 1] {
                            let value = i128::from(point[field]) + delta;
                            if !(i128::from(i64::MIN)..=i128::from(i64::MAX)).contains(&value) {
                                continue;
                            }
                            let mut neighbor = point.clone();
                            neighbor[field] = value as i64;
                            push_point(&mut output, neighbor)?;
                        }
                    }
                }
            }
        }
        Ok(output)
    }

    fn mutate(&self, legal: &[Vec<i64>], random: &mut CompatRandom, guided: bool) -> Vec<i64> {
        let mut point = legal[random.next(legal.len())].clone();
        let palette = if guided {
            &self.guided_atoms
        } else {
            &self.atoms
        };
        let changes = match random.next(5) {
            0 => 1,
            1 => 2,
            2 => point.len().min(3),
            3 => point.len(),
            _ => random.range(1, point.len() as i32 + 1) as usize,
        };
        let target = point[random.next(point.len())];
        for n in 0..changes {
            let index = if changes == point.len() {
                n
            } else {
                random.next(point.len())
            };
            point[index] = match random.next(5) {
                0 => offset(target, i64::from(random.range(-1, 2))),
                1 => offset(point[index], i64::from(random.range(-3, 4))),
                2 => {
                    let source = point[random.next(point.len())];
                    offset(source, i64::from(random.range(-1, 2)))
                }
                _ => palette[index][random.next(palette[index].len())],
            };
        }
        point
    }
}

fn push_point(output: &mut Vec<Vec<i64>>, item: Vec<i64>) -> Result<(), ContractError> {
    if output.len() >= MAX_GUIDED_POINTS {
        return Err(ContractError::Scope(
            "more than 250000 guided candidates".into(),
        ));
    }
    output.push(item);
    Ok(())
}

fn apply_pair(point: &mut [i64], pair: (usize, usize), reverse: bool) {
    if reverse {
        point[pair.0] = point[pair.1];
    } else {
        point[pair.1] = point[pair.0];
    }
}

fn insert_unique(items: &mut Vec<i64>, value: i64) {
    if !items.contains(&value) {
        items.push(value);
    }
}

fn offset(value: i64, delta: i64) -> i64 {
    (i128::from(value) + i128::from(delta)).clamp(i128::from(i64::MIN), i128::from(i64::MAX)) as i64
}

fn fill_frame(point: &[i64], locals: &[Expr]) -> Result<Vec<Integer>, &'static str> {
    let mut frame: Vec<Integer> = point.iter().copied().map(Integer::from_i64).collect();
    frame.resize(frame.len() + locals.len(), Integer::zero());
    for (index, local) in locals.iter().enumerate() {
        frame[point.len() + index] = local.eval(&frame)?;
    }
    Ok(frame)
}

fn cmp_difference(
    comparison: &Comparison,
    point: &[i64],
    locals: &[Expr],
) -> Result<Integer, &'static str> {
    let frame = fill_frame(point, locals)?;
    comparison.difference(&frame)
}

fn project(group: &[Comparison], point: &mut [i64], locals: &[Expr], reverse: bool) -> bool {
    // Anchor periodic equalities before projecting coupled arithmetic.
    // Otherwise an earlier linear projection can pin the only periodic input.
    let mut ordered: Vec<_> = group.iter().map(|comparison| {
        (comparison, comparison.modulo_anchor().filter(|(field, _, _)| *field < point.len()))
    }).collect();
    ordered.sort_by_key(|(_, anchor)| anchor.is_none());
    let mut run = || -> Result<bool, &'static str> {
        for (index, &(comparison, anchor)) in ordered.iter().enumerate() {
            let current = cmp_difference(comparison, point, locals)?;
            if comparison.matches(&current) {
                continue;
            }
            let mut adjusted = false;
            for step in 0..point.len() {
                let field = if reverse {
                    point.len() - 1 - step
                } else {
                    step
                };
                if comparison.inputs & (1_u32 << field) == 0 {
                    continue;
                }
                let origin = point[field];
                let mut candidate = point.to_vec();
                let preserves = |candidate: &[i64]| -> Result<bool, &'static str> {
                    if !comparison.matches(&cmp_difference(comparison, candidate, locals)?) {
                        return Ok(false);
                    }
                    for &(earlier, _) in &ordered[..index] {
                        if !earlier.matches(&cmp_difference(earlier, candidate, locals)?) {
                            return Ok(false);
                        }
                    }
                    Ok(true)
                };
                if let Some((periodic_field, period, target)) = anchor {
                    if field != periodic_field {
                        continue;
                    }
                    let (origin, period, target) =
                        (i128::from(origin), i128::from(period), i128::from(target));
                    let lower = origin - (origin - target).rem_euclid(period);
                    let mut choices = [lower, lower + period];
                    choices.sort_by_key(|value| (value - origin).abs());
                    for proposed in choices {
                        let Ok(value) = i64::try_from(proposed) else {
                            continue;
                        };
                        candidate[field] = value;
                        if preserves(&candidate)? {
                            point[field] = value;
                            adjusted = true;
                            break;
                        }
                    }
                    break;
                }
                let unit = if origin == i64::MAX { -1_i64 } else { 1_i64 };
                candidate[field] = origin + unit;
                let slope = cmp_difference(comparison, &candidate, locals)?
                    .sub(&current)
                    .multiply(&Integer::from_i64(unit));
                if !slope.is_zero() {
                    let boundary = Integer::from_i64(comparison.boundary());
                    let div = boundary.sub(&current).div_rem(&slope)?.0;
                    let center = Integer::from_i64(origin).add(&div);
                    for adjustment in [0, -1, 1] {
                        let proposed = center.add(&Integer::from_i64(adjustment));
                        let Some(value) = proposed.to_i64() else {
                            continue;
                        };
                        if value == origin {
                            continue;
                        }
                        candidate[field] = value;
                        if preserves(&candidate)? {
                            point[field] = value;
                            adjusted = true;
                            break;
                        }
                    }
                }
                if adjusted {
                    break;
                }
            }
            if !adjusted {
                return Ok(false);
            }
        }
        Ok(true)
    };
    run().unwrap_or(false)
}

#[derive(Debug)]
enum Token {
    Name(String),
    Number(i64),
    Cmp(String),
    Other,
}

fn tokens(text: &str) -> Vec<Token> {
    let data = text.as_bytes();
    let mut index = 0;
    let mut result = Vec::new();
    while index < data.len() {
        let ch = data[index];
        if ch.is_ascii_whitespace() {
            index += 1;
            continue;
        }
        if ch.is_ascii_alphabetic() || ch == b'_' {
            let start = index;
            index += 1;
            while index < data.len() && (data[index].is_ascii_alphanumeric() || data[index] == b'_')
            {
                index += 1;
            }
            result.push(Token::Name(text[start..index].into()));
            continue;
        }
        if ch.is_ascii_digit()
            || (ch == b'-' && data.get(index + 1).is_some_and(u8::is_ascii_digit))
        {
            let start = index;
            if ch == b'-' {
                index += 1;
            }
            while index < data.len() && data[index].is_ascii_digit() {
                index += 1;
            }
            match text[start..index].parse::<i64>() {
                Ok(value) => result.push(Token::Number(value)),
                Err(_) => result.push(Token::Other),
            }
            continue;
        }
        let pair = data.get(index..index + 2);
        if pair.is_some_and(|a| a == b"<=" || a == b">=" || a == b"==" || a == b"!=") {
            result.push(Token::Cmp(text[index..index + 2].into()));
            index += 2;
            continue;
        }
        if ch == b'<' || ch == b'>' {
            result.push(Token::Cmp((ch as char).to_string()));
            index += 1;
            continue;
        }
        result.push(Token::Other);
        index += 1;
    }
    result
}

fn scan_relations(source: &str, parameters: &[String]) -> (Vec<Vec<i64>>, Vec<(usize, usize)>) {
    let sequence = tokens(source);
    let mut bounds = vec![Vec::new(); parameters.len()];
    let mut pairs = Vec::new();
    for window in sequence.windows(3) {
        if let [Token::Name(left), Token::Cmp(_), Token::Number(number)] = window
            && let Some(index) = parameters.iter().position(|item| item == left)
        {
            bounds[index].push(*number);
        }
    }
    for window in sequence.windows(3) {
        if let [Token::Number(number), Token::Cmp(_), Token::Name(right)] = window
            && let Some(index) = parameters.iter().position(|item| item == right)
        {
            bounds[index].push(*number);
        }
    }
    for window in sequence.windows(3) {
        if let [Token::Name(left), Token::Cmp(op), Token::Name(right)] = window {
            if op != "==" && op != "!=" {
                continue;
            }
            let Some(a) = parameters.iter().position(|item| item == left) else {
                continue;
            };
            let Some(b) = parameters.iter().position(|item| item == right) else {
                continue;
            };
            if a == b {
                continue;
            }
            let pair = (a.min(b), a.max(b));
            if !pairs.contains(&pair) && pairs.len() < 32 {
                pairs.push(pair);
            }
        }
    }
    (bounds, pairs)
}

#[cfg(test)]
mod tests {
    use super::scan_relations;
    #[test]
    fn direct_reverse_and_equality_boundaries() {
        let names = vec!["a".into(), "b".into()];
        let (bounds, pairs) = scan_relations("a <= 99 && 0 <= b && a != b && b >= -4", &names);
        assert_eq!(bounds, vec![vec![99], vec![-4, 0]]);
        assert_eq!(pairs, vec![(0, 1)]);
    }
}
