//! Pure bounded scalar-path search over a Roslyn-authored, typed, versioned fact stream.
//! Structural and predicted edges are **not** invocation-scoped observed edges.
//! No compilation, replay, artifact verification or user-facing cutover lives here.

use std::cmp::Ordering;
use std::collections::{BTreeSet, BinaryHeap, HashMap, HashSet};

mod correlated;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Ty {
    Int,
    Bool,
    Other,
}

impl Ty {
    fn read(s: &str) -> Result<Self, String> {
        match s {
            "int" => Ok(Self::Int),
            "bool" => Ok(Self::Bool),
            "other" => Ok(Self::Other),
            _ => Err(format!("Unknown fact type: {s}")),
        }
    }
}

#[derive(Clone, Debug)]
struct Node {
    kind: String,
    ty: Ty,
    slot: Option<usize>,
    constant: Option<i32>,
    operator: Option<String>,
    checked: bool,
    lifted: bool,
    postfix: bool,
    writable: bool,
    children: Vec<usize>,
    hole: Option<String>,
    diagnostic: String,
}

#[derive(Clone, Debug)]
struct Edge {
    to: Option<usize>,
    semantics: String,
    has_finally: bool,
    clear_locals: Vec<usize>,
    clear_captures: Vec<usize>,
}

#[derive(Clone, Debug)]
struct Block {
    kind: String,
    condition: String,
    nodes: Vec<Node>,
    statements: Vec<usize>,
    branch: Option<usize>,
    fall_through: Option<Edge>,
    conditional: Option<Edge>,
}

#[derive(Clone, Debug)]
pub struct Problem {
    bound: bool,
    eligible: bool,
    reason: Option<String>,
    params: Vec<Ty>,
    blocks: Vec<Block>,
    seeds: Vec<Option<Vec<i64>>>,
    work_limit: usize,
    case_limit: usize,
}

fn fields<'a>(parts: &'a [&'a str], n: usize, line: usize) -> Result<&'a [&'a str], String> {
    if parts.len() == n {
        Ok(parts)
    } else {
        Err(format!(
            "Line {line}: expected {n} fields, got {}",
            parts.len()
        ))
    }
}

fn num<T: std::str::FromStr>(field: &str, line: usize) -> Result<T, String> {
    field
        .parse::<T>()
        .map_err(|_| format!("Line {line}: invalid integer '{field}'"))
}

fn opt_num<T: std::str::FromStr>(field: &str, line: usize) -> Result<Option<T>, String> {
    if field == "-" {
        Ok(None)
    } else {
        num(field, line).map(Some)
    }
}

fn flag(field: &str, line: usize) -> Result<bool, String> {
    match field {
        "0" => Ok(false),
        "1" => Ok(true),
        _ => Err(format!("Line {line}: invalid boolean flag '{field}'")),
    }
}

fn csv<T: std::str::FromStr>(field: &str, line: usize) -> Result<Vec<T>, String> {
    if field.is_empty() || field == "-" {
        Ok(Vec::new())
    } else {
        field.split(',').map(|s| num(s, line)).collect()
    }
}

fn unhex(field: &str, line: usize) -> Result<Option<String>, String> {
    if field == "-" {
        return Ok(None);
    }
    if !field.len().is_multiple_of(2) {
        return Err(format!("Line {line}: odd-sized hex text"));
    }
    let mut bytes = Vec::with_capacity(field.len() / 2);
    for pair in field.as_bytes().as_chunks::<2>().0 {
        let part = std::str::from_utf8(pair).map_err(|_| format!("Line {line}: invalid hex"))?;
        bytes.push(u8::from_str_radix(part, 16).map_err(|_| format!("Line {line}: invalid hex"))?);
    }
    String::from_utf8(bytes)
        .map(Some)
        .map_err(|_| format!("Line {line}: text is not UTF-8"))
}

/// Parse a whole versioned projection. All IDs are checked before search, including the
/// unexecuted halves of short-circuit expressions, to avoid treating malformed facts as holes.
pub fn parse_wire(input: &str) -> Result<Problem, String> {
    if input.len() > 16 * 1024 * 1024 {
        return Err("Scalar fact stream exceeds 16 MiB".into());
    }
    let mut problem = Problem {
        bound: false,
        eligible: false,
        reason: None,
        params: Vec::new(),
        blocks: Vec::new(),
        seeds: Vec::new(),
        work_limit: 0,
        case_limit: 0,
    };
    let mut started = false;
    let mut has_bound = false;
    let mut has_limits = false;
    let mut ended = false;
    for (i, raw_line) in input.lines().enumerate() {
        let line = i + 1;
        let parts: Vec<_> = raw_line.trim_end_matches('\r').split('\t').collect();
        if ended {
            return Err(format!("Line {line}: content after end"));
        }
        if !started {
            fields(&parts, 2, line)?;
            if parts != ["scalar-path-facts", "1"] {
                return Err("Expected scalar-path-facts schema 1".into());
            }
            started = true;
            continue;
        }
        match parts[0] {
            "limits" => {
                fields(&parts, 3, line)?;
                if has_limits {
                    return Err("Duplicate limits".into());
                }
                problem.work_limit = num(parts[1], line)?;
                problem.case_limit = num(parts[2], line)?;
                has_limits = true;
            }
            "bound" => {
                fields(&parts, 4, line)?;
                if has_bound {
                    return Err("Duplicate bound".into());
                }
                problem.bound = flag(parts[1], line)?;
                problem.eligible = flag(parts[2], line)?;
                problem.reason = unhex(parts[3], line)?;
                has_bound = true;
            }
            "param" => {
                fields(&parts, 3, line)?;
                let ordinal: usize = num(parts[1], line)?;
                if ordinal != problem.params.len() {
                    return Err(format!("Line {line}: parameters must be ordinal-ordered"));
                }
                problem.params.push(Ty::read(parts[2])?);
            }
            "block" => {
                fields(&parts, 4, line)?;
                let id: usize = num(parts[1], line)?;
                if id != problem.blocks.len() || id >= 4096 {
                    return Err(format!("Line {line}: blocks must be consecutive (<= 4096)"));
                }
                problem.blocks.push(Block {
                    kind: parts[2].into(),
                    condition: parts[3].into(),
                    nodes: Vec::new(),
                    statements: Vec::new(),
                    branch: None,
                    fall_through: None,
                    conditional: None,
                });
            }
            "node" => {
                fields(&parts, 15, line)?;
                let id: usize = num(parts[1], line)?;
                let node_id: usize = num(parts[2], line)?;
                let block_count = problem.blocks.len();
                let b = problem
                    .blocks
                    .get_mut(id)
                    .ok_or_else(|| format!("Line {line}: no block"))?;
                if id + 1 != block_count || node_id != b.nodes.len() || node_id >= 65536 {
                    return Err(format!("Line {line}: non-consecutive node"));
                }
                b.nodes.push(Node {
                    kind: parts[3].into(),
                    ty: Ty::read(parts[4])?,
                    slot: opt_num(parts[5], line)?,
                    constant: opt_num(parts[6], line)?,
                    operator: if parts[7] == "-" {
                        None
                    } else {
                        Some(parts[7].into())
                    },
                    checked: flag(parts[8], line)?,
                    lifted: flag(parts[9], line)?,
                    postfix: flag(parts[10], line)?,
                    writable: flag(parts[11], line)?,
                    children: csv(parts[12], line)?,
                    hole: unhex(parts[13], line)?,
                    diagnostic: unhex(parts[14], line)?
                        .ok_or_else(|| format!("Line {line}: missing diagnostic"))?,
                });
            }
            "stmt" => {
                fields(&parts, 3, line)?;
                let id: usize = num(parts[1], line)?;
                let node_id: usize = num(parts[2], line)?;
                let b = problem
                    .blocks
                    .get_mut(id)
                    .ok_or_else(|| format!("Line {line}: no block"))?;
                b.statements.push(node_id);
            }
            "branch" => {
                fields(&parts, 3, line)?;
                let id: usize = num(parts[1], line)?;
                let b = problem
                    .blocks
                    .get_mut(id)
                    .ok_or_else(|| format!("Line {line}: no block"))?;
                b.branch = opt_num(parts[2], line)?;
            }
            "edge" => {
                fields(&parts, 8, line)?;
                let id: usize = num(parts[1], line)?;
                let edge = Edge {
                    to: opt_num(parts[3], line)?,
                    semantics: parts[4].into(),
                    has_finally: flag(parts[5], line)?,
                    clear_locals: csv(parts[6], line)?,
                    clear_captures: csv(parts[7], line)?,
                };
                let b = problem
                    .blocks
                    .get_mut(id)
                    .ok_or_else(|| format!("Line {line}: no block"))?;
                match parts[2] {
                    "fallThrough" if b.fall_through.is_none() => b.fall_through = Some(edge),
                    "conditional" if b.conditional.is_none() => b.conditional = Some(edge),
                    _ => return Err(format!("Line {line}: invalid or duplicate edge role")),
                }
            }
            "seed" => {
                fields(&parts, 2, line)?;
                problem.seeds.push(if parts[1] == "-" {
                    None
                } else {
                    Some(csv(parts[1], line)?)
                });
            }
            "end" => {
                fields(&parts, 1, line)?;
                ended = true;
            }
            other => return Err(format!("Line {line}: unknown record {other}")),
        }
    }
    if !started || !ended || !has_bound || !has_limits {
        return Err("Incomplete scalar fact stream".into());
    }
    if !problem.bound && (!problem.params.is_empty() || !problem.blocks.is_empty()) {
        return Err("Unbound projection must not have executable facts".into());
    }
    if problem.bound {
        if problem.blocks.is_empty() || problem.blocks[0].kind != "Entry" {
            return Err("Bound projection needs a Roslyn entry block".into());
        }
        for (id, b) in problem.blocks.iter().enumerate() {
            for (node_id, n) in b.nodes.iter().enumerate() {
                if n.children
                    .iter()
                    .any(|c| *c >= b.nodes.len() || *c <= node_id)
                {
                    return Err(format!("Block {id}: unbound child expression"));
                }
            }
            // Forward-only child IDs let us bound recursion without walking
            // the graph recursively or accepting a stack-overflowing stream.
            let mut depths = vec![1usize; b.nodes.len()];
            for node_id in (0..b.nodes.len()).rev() {
                let depth = b.nodes[node_id]
                    .children
                    .iter()
                    .map(|child| depths[*child] + 1)
                    .max()
                    .unwrap_or(1);
                if depth > 256 {
                    return Err(format!("Block {id}: expression depth exceeds 256"));
                }
                depths[node_id] = depth;
            }
            if b.statements.iter().any(|s| *s >= b.nodes.len())
                || b.branch.is_some_and(|v| v >= b.nodes.len())
            {
                return Err(format!("Block {id}: invalid expression root"));
            }
            for edge in [&b.fall_through, &b.conditional].into_iter().flatten() {
                if edge.to.is_some_and(|to| to >= problem.blocks.len()) {
                    return Err(format!("Block {id}: invalid edge target"));
                }
            }
        }
    }
    Ok(problem)
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct Scalar {
    value: i32,
    ty: Ty,
    coefficients: Option<Vec<(usize, i32)>>,
    comparisons: Vec<Comparison>,
}

impl Scalar {
    fn new(value: i32, ty: Ty) -> Self {
        Self {
            value,
            ty,
            coefficients: None,
            comparisons: Vec::new(),
        }
    }

    fn boolean(&self) -> bool {
        self.value != 0
    }

    fn factor(&self, index: usize) -> i32 {
        self.coefficients
            .as_ref()
            .and_then(|c| c.iter().find(|(i, _)| *i == index))
            .map_or(0, |(_, v)| *v)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct Comparison {
    operator: String,
    left: Scalar,
    right: Scalar,
}

#[derive(Clone, Debug)]
struct Guard {
    taken: String,
    alternate: String,
    comparisons: Vec<Comparison>,
}

struct Interpreter<'a> {
    problem: &'a Problem,
    vars: HashMap<usize, Scalar>,
    values: HashMap<usize, Scalar>,
    slots: HashMap<usize, usize>,
    guards: Vec<Guard>,
}

impl<'a> Interpreter<'a> {
    fn new(problem: &'a Problem, args: &[i64]) -> Self {
        let mut vars = HashMap::new();
        for (i, ty) in problem.params.iter().enumerate() {
            let value = args[i] as i32;
            let a = Scalar {
                value,
                ty: Ty::Int,
                coefficients: Some(vec![(i, 1)]),
                comparisons: Vec::new(),
            };
            let v = if *ty == Ty::Int {
                a
            } else {
                Scalar {
                    value,
                    ty: Ty::Bool,
                    coefficients: None,
                    comparisons: vec![Comparison {
                        operator: "NotEquals".into(),
                        left: a,
                        right: Scalar::new(0, Ty::Int),
                    }],
                }
            };
            vars.insert(i, v);
        }
        Self {
            problem,
            vars,
            values: HashMap::new(),
            slots: HashMap::new(),
            guards: Vec::new(),
        }
    }

    fn run(mut self) -> (Vec<Guard>, Option<String>) {
        let mut current = 0;
        let mut visited = HashSet::new();
        loop {
            // Clone the current immutable block so nested evaluation can mutate interpreter state.
            let b = self.problem.blocks[current].clone();
            if b.kind == "Exit" {
                return (self.guards, None);
            }
            if !visited.insert(current) {
                return (
                    self.guards,
                    Some(format!("Loop frontier at block {current}.")),
                );
            }
            let result: Result<usize, String> = (|| {
                for id in &b.statements {
                    self.eval(&b, *id)?;
                }
                let branch_value = b.branch.map(|id| self.eval(&b, id)).transpose()?;
                let conditional = match b.condition.as_str() {
                    "None" => false,
                    "WhenTrue" if branch_value.as_ref().is_some_and(|v| v.ty == Ty::Bool) => {
                        branch_value.as_ref().is_some_and(Scalar::boolean)
                    }
                    "WhenFalse" if branch_value.as_ref().is_some_and(|v| v.ty == Ty::Bool) => {
                        !branch_value.as_ref().is_some_and(Scalar::boolean)
                    }
                    _ => return Err(format!("Unsupported branch condition at block {current}.")),
                };
                let role = if conditional {
                    "conditional"
                } else {
                    "fallThrough"
                };
                let edge = if conditional {
                    &b.conditional
                } else {
                    &b.fall_through
                };
                let semantics = edge.as_ref().map_or("missing", |e| e.semantics.as_str());
                let edge = edge
                    .as_ref()
                    .filter(|e| {
                        !e.has_finally
                            && matches!(e.semantics.as_str(), "Regular" | "Return")
                            && e.to.is_some()
                    })
                    .ok_or_else(|| {
                        format!("Unsupported transition on b{current}:{role} ({semantics}).")
                    })?;
                if b.condition != "None" {
                    self.guards.push(Guard {
                        taken: format!("b{current}:{role}"),
                        alternate: format!(
                            "b{current}:{}",
                            if conditional {
                                "fallThrough"
                            } else {
                                "conditional"
                            }
                        ),
                        comparisons: branch_value.map_or(Vec::new(), |v| v.comparisons),
                    });
                }
                for id in &edge.clear_locals {
                    self.vars.remove(id);
                }
                for id in &edge.clear_captures {
                    self.values.remove(id);
                    self.slots.remove(id);
                }
                Ok(edge.to.expect("filtered non-null edge"))
            })();
            match result {
                Ok(next) => current = next,
                Err(err) => return (self.guards, Some(err)),
            }
        }
    }

    fn slot(&self, b: &Block, id: usize) -> Result<usize, String> {
        let n = &b.nodes[id];
        match n.kind.as_str() {
            "slot" if n.writable => n.slot.ok_or_else(|| n.diagnostic.clone()),
            "capture-ref" => n
                .slot
                .and_then(|c| self.slots.get(&c).copied())
                .ok_or_else(|| n.diagnostic.clone()),
            _ => Err(n.diagnostic.clone()),
        }
    }

    fn eval(&mut self, b: &Block, id: usize) -> Result<Scalar, String> {
        let n = &b.nodes[id];
        if let Some(hole) = &n.hole {
            return Err(hole.clone());
        }
        if n.checked || n.lifted {
            return Err("Checked or lifted scalar node was not modeled".into());
        }
        let child = |ix: usize| -> Result<usize, String> {
            n.children
                .get(ix)
                .copied()
                .ok_or_else(|| "Missing expression child".into())
        };
        match n.kind.as_str() {
            "constant" => Ok(Scalar::new(
                n.constant.ok_or("Constant has no value")?,
                n.ty,
            )),
            "slot" => self
                .vars
                .get(&n.slot.ok_or("Unbound slot ID")?)
                .cloned()
                .ok_or_else(|| n.diagnostic.clone()),
            "capture-ref" => {
                let c = n.slot.ok_or("Unbound capture ID")?;
                self.values
                    .get(&c)
                    .cloned()
                    .or_else(|| {
                        self.slots
                            .get(&c)
                            .and_then(|slot| self.vars.get(slot))
                            .cloned()
                    })
                    .ok_or_else(|| n.diagnostic.clone())
            }
            "capture-slot" => {
                let c = n.slot.ok_or("Unbound capture ID")?;
                let slot = self.slot(b, child(0)?)?;
                self.slots.insert(c, slot);
                Ok(Scalar::new(0, Ty::Other))
            }
            "capture" => {
                let value = self.eval(b, child(0)?)?;
                self.values
                    .insert(n.slot.ok_or("Unbound capture ID")?, value.clone());
                Ok(value)
            }
            "identity" => self.eval(b, child(0)?),
            "assign" => {
                let value = self.eval(b, child(1)?)?;
                let target = child(0)?;
                if b.nodes[target].ty != value.ty {
                    return Err(b.nodes[target].diagnostic.clone());
                }
                let slot = self.slot(b, target)?;
                self.vars.insert(slot, value.clone());
                Ok(value)
            }
            "compound" => {
                let target = child(0)?;
                let left = self.eval(b, target)?;
                let right = self.eval(b, child(1)?)?;
                let result = binary(n.operator.as_deref().unwrap_or(""), left, right)
                    .map_err(|_| n.diagnostic.clone())?;
                if b.nodes[target].ty != result.ty {
                    return Err(b.nodes[target].diagnostic.clone());
                }
                let slot = self.slot(b, target)?;
                self.vars.insert(slot, result.clone());
                Ok(result)
            }
            "step" => {
                let target = child(0)?;
                let current = self.eval(b, target)?;
                let op = if n.operator.as_deref() == Some("Increment") {
                    "Add"
                } else {
                    "Subtract"
                };
                let updated = binary(op, current.clone(), Scalar::new(1, Ty::Int))
                    .map_err(|_| n.diagnostic.clone())?;
                if b.nodes[target].ty != updated.ty {
                    return Err(b.nodes[target].diagnostic.clone());
                }
                let slot = self.slot(b, target)?;
                self.vars.insert(slot, updated.clone());
                Ok(if n.postfix { current } else { updated })
            }
            "unary" => {
                let mut value = self.eval(b, child(0)?)?;
                match (n.operator.as_deref(), value.ty) {
                    (Some("Not"), Ty::Bool) => value.value = i32::from(!value.boolean()),
                    (Some("Plus"), Ty::Int) => (),
                    (Some("Minus"), Ty::Int) => {
                        value.value = value.value.wrapping_neg();
                        value.coefficients = combine(None, value.coefficients.as_deref(), true);
                    }
                    _ => return Err(n.diagnostic.clone()),
                }
                Ok(value)
            }
            "binary" => {
                let left = self.eval(b, child(0)?)?;
                let short_circuits = match n.operator.as_deref() {
                    Some("ConditionalAnd") => !left.boolean(),
                    Some("ConditionalOr") => left.boolean(),
                    _ => false,
                };
                if left.ty == Ty::Bool && short_circuits {
                    return Ok(left);
                }
                let right = self.eval(b, child(1)?)?;
                binary(n.operator.as_deref().unwrap_or(""), left, right)
                    .map_err(|_| n.diagnostic.clone())
            }
            "hole" => Err(n.diagnostic.clone()),
            _ => Err(n.diagnostic.clone()),
        }
    }
}

fn combine(
    left: Option<&[(usize, i32)]>,
    right: Option<&[(usize, i32)]>,
    subtract: bool,
) -> Option<Vec<(usize, i32)>> {
    let Some(right) = right else {
        return left.map(|v| v.to_vec());
    };
    if left.is_none() && !subtract {
        return Some(right.to_vec());
    }
    let mut result = left.unwrap_or_default().to_vec();
    for &(index, factor) in right {
        let value = result
            .iter()
            .find(|(i, _)| *i == index)
            .map_or(0, |(_, v)| *v);
        let combined = value.wrapping_add(if subtract {
            factor.wrapping_neg()
        } else {
            factor
        });
        if let Some(position) = result.iter().position(|(i, _)| *i == index) {
            if combined == 0 {
                result.remove(position);
            } else {
                result[position].1 = combined;
            }
        } else if combined != 0 {
            result.push((index, combined));
        }
    }
    Some(result)
}

fn compare(operator: &str, left: i32, right: i32) -> bool {
    match operator {
        "Equals" => left == right,
        "NotEquals" => left != right,
        "LessThan" => left < right,
        "LessThanOrEqual" => left <= right,
        "GreaterThan" => left > right,
        "GreaterThanOrEqual" => left >= right,
        _ => false, // Rejected as unsupported before a model comparison can be proposed.
    }
}

fn binary(operator: &str, left: Scalar, right: Scalar) -> Result<Scalar, String> {
    if left.ty != right.ty {
        return Err("Scalar operands have different types".into());
    }
    match left.ty {
        Ty::Int => match operator {
            "Add" | "Subtract" => {
                let subtract = operator == "Subtract";
                Ok(Scalar {
                    value: if subtract {
                        left.value.wrapping_sub(right.value)
                    } else {
                        left.value.wrapping_add(right.value)
                    },
                    ty: Ty::Int,
                    coefficients: combine(
                        left.coefficients.as_deref(),
                        right.coefficients.as_deref(),
                        subtract,
                    ),
                    comparisons: Vec::new(),
                })
            }
            "Equals" | "NotEquals" | "LessThan" | "LessThanOrEqual" | "GreaterThan"
            | "GreaterThanOrEqual" => Ok(Scalar {
                value: i32::from(compare(operator, left.value, right.value)),
                ty: Ty::Bool,
                coefficients: None,
                comparisons: vec![Comparison {
                    operator: operator.into(),
                    left,
                    right,
                }],
            }),
            _ => Err(format!("Unsupported integer binary operator {operator}")),
        },
        Ty::Bool => {
            let result = match operator {
                "Equals" => left.boolean() == right.boolean(),
                "NotEquals" => left.boolean() != right.boolean(),
                "ConditionalAnd" | "And" => left.boolean() && right.boolean(),
                "ConditionalOr" | "Or" => left.boolean() || right.boolean(),
                _ => return Err(format!("Unsupported Boolean binary operator {operator}")),
            };
            let mut comparisons = left.comparisons;
            for c in right.comparisons {
                if !comparisons.contains(&c) {
                    comparisons.push(c);
                }
            }
            Ok(Scalar {
                value: i32::from(result),
                ty: Ty::Bool,
                coefficients: None,
                comparisons,
            })
        }
        Ty::Other => Err("Unsupported non-scalar operands".into()),
    }
}

/// All solutions of factor * change == delta modulo 2^32, at most two representatives.
/// This matches the C# worklist's 2-adic inverse and its tie-to-negative representative.
fn changes(factor: i32, delta: i32) -> Vec<i32> {
    if factor == 0 {
        return Vec::new();
    }
    let shift = factor.trailing_zeros();
    let divisor = 1_u32 << shift;
    if (delta as u32) & (divisor - 1) != 0 {
        return Vec::new();
    }
    let odd = (factor as u32) >> shift;
    let mut inverse = odd;
    for _ in 0..5 {
        inverse = inverse.wrapping_mul(2_u32.wrapping_sub(odd.wrapping_mul(inverse)));
    }
    let period = 1_u64 << (32 - shift);
    let residue = ((delta as u32 >> shift).wrapping_mul(inverse) as u64) & (period - 1);
    let mut nearest = residue as i64;
    if residue >= period / 2 {
        nearest -= period as i64;
    }
    let mut result = vec![nearest as i32];
    if shift != 0 {
        result.push(
            (nearest
                + if nearest < 0 {
                    period as i64
                } else {
                    -(period as i64)
                }) as i32,
        );
    }
    result
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Candidate {
    pub args: Vec<i64>,
    /// A modeled, predicted edge ID only; never a proof of execution.
    pub predicted_goal: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SearchResult {
    pub bound: bool,
    pub work: usize,
    pub candidates: Vec<Candidate>,
    pub unknown: Vec<String>,
}

/// Candidate ordering is policy-dependent. The caller must bind an opted-in policy
/// and planner revision to its replay identity; neither scalar wire schema changes.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SearchPolicy {
    /// The existing ordered search, also used by [`search`].
    Legacy,
    /// Try common wrapping shifts of three or more linked int parameters before
    /// legacy proposals. Limits and unsupported correlations remain explicit.
    CorrelatedEqualitiesV1,
}

#[derive(Clone, Debug)]
struct Input {
    args: Vec<i64>,
    key: String,
    goal: Option<String>,
    seed: bool,
}

#[derive(Clone, Debug)]
struct Output {
    candidate: Candidate,
    novel: bool,
    targeted: bool,
    depth: usize,
    order: usize,
}

struct Queued {
    priority: i32,
    order: u64,
    input: Input,
}

impl PartialEq for Queued {
    fn eq(&self, other: &Self) -> bool {
        self.priority == other.priority && self.order == other.order
    }
}
impl Eq for Queued {}
impl PartialOrd for Queued {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}
impl Ord for Queued {
    fn cmp(&self, other: &Self) -> Ordering {
        other
            .priority
            .cmp(&self.priority)
            .then_with(|| other.order.cmp(&self.order))
    }
}

struct Search<'a> {
    p: &'a Problem,
    policy: SearchPolicy,
    seed_keys: HashSet<String>,
    simulated: HashSet<String>,
    covered: HashSet<String>,
    unknown: BTreeSet<String>,
    expansions: HashMap<String, usize>,
    pending: HashMap<String, i32>,
    queue: BinaryHeap<Queued>,
    outputs: Vec<Output>,
    proposal_order: u64,
    proposal_work: u64,
    work: usize,
}

impl<'a> Search<'a> {
    fn new(p: &'a Problem) -> Self {
        Self {
            p,
            policy: SearchPolicy::Legacy,
            seed_keys: HashSet::new(),
            simulated: HashSet::new(),
            covered: HashSet::new(),
            unknown: BTreeSet::new(),
            expansions: HashMap::new(),
            pending: HashMap::new(),
            queue: BinaryHeap::new(),
            outputs: Vec::new(),
            proposal_order: 0,
            proposal_work: 0,
            work: 0,
        }
    }

    fn key(args: &[i64]) -> String {
        args.iter()
            .map(i64::to_string)
            .collect::<Vec<_>>()
            .join(",")
    }

    fn compatible(&self, args: &[i64]) -> bool {
        args.len() == self.p.params.len()
            && args.iter().zip(&self.p.params).all(|(n, ty)| {
                (i32::MIN as i64..=i32::MAX as i64).contains(n)
                    && (*ty != Ty::Bool || *n == 0 || *n == 1)
            })
    }

    fn run(mut self) -> SearchResult {
        if !self.p.bound {
            return SearchResult {
                bound: false,
                work: 0,
                candidates: Vec::new(),
                unknown: vec![
                    self.p
                        .reason
                        .clone()
                        .unwrap_or_else(|| "Unbound facts".into()),
                ],
            };
        }
        if self.p.case_limit == 0 {
            return SearchResult {
                bound: true,
                work: 0,
                candidates: Vec::new(),
                unknown: vec!["Scalar search was not requested.".into()],
            };
        }
        if !self.p.eligible {
            return SearchResult { bound: true, work: 0, candidates: Vec::new(), unknown: vec![
                "Scalar search requires a synchronous method with by-value int or bool parameters and return.".into(),
            ] };
        }
        if self.p.work_limit == 0 {
            return SearchResult {
                bound: true,
                work: 0,
                candidates: Vec::new(),
                unknown: vec!["Scalar work limit reached before search.".into()],
            };
        }
        let mut initial = Vec::new();
        let seed_limit = (self.p.work_limit / 4).max(1);
        for seed in &self.p.seeds {
            let Some(seed) = seed else {
                self.unknown.insert(
                    "A seed is incompatible with the bound int and bool parameters.".into(),
                );
                continue;
            };
            if !self.compatible(seed) {
                self.unknown.insert(
                    "A seed is incompatible with the bound int and bool parameters.".into(),
                );
                continue;
            }
            let key = Self::key(seed);
            if self.seed_keys.insert(key.clone()) && initial.len() < seed_limit {
                initial.push(Input {
                    args: seed.clone(),
                    key,
                    goal: None,
                    seed: true,
                });
            }
        }
        if self.seed_keys.len() > initial.len() {
            self.unknown.insert(
                "Some seed paths were skipped to reserve work for generated inputs.".into(),
            );
        }
        if initial.is_empty() {
            let args = vec![0; self.p.params.len()];
            initial.push(Input {
                key: Self::key(&args),
                args,
                goal: None,
                seed: false,
            });
        }
        for input in initial {
            self.simulate(input);
        }
        while self.work < self.p.work_limit {
            let Some(q) = self.queue.pop() else {
                break;
            };
            if self.pending.get(&q.input.key) != Some(&q.priority) {
                continue;
            }
            self.pending.remove(&q.input.key);
            self.simulate(q.input);
        }
        if !self.pending.is_empty() {
            self.unknown
                .insert(format!("Scalar work limit {} reached.", self.p.work_limit));
        }
        if self.outputs.len() > self.p.case_limit {
            self.unknown.insert(format!(
                "Scalar candidate case limit {} reached.",
                self.p.case_limit
            ));
        }
        if self.proposal_work >= 64_u64.saturating_mul(self.p.work_limit as u64) {
            self.unknown.insert(format!(
                "Scalar proposal limit {} reached.",
                64_u64.saturating_mul(self.p.work_limit as u64)
            ));
        }
        for (id, b) in self.p.blocks.iter().enumerate() {
            if b.condition != "None" {
                for (role, present) in [
                    ("fallThrough", b.fall_through.is_some()),
                    ("conditional", b.conditional.is_some()),
                ] {
                    let name = format!("b{id}:{role}");
                    if present && !self.covered.contains(&name) {
                        self.unknown
                            .insert(format!("No supported input explored {name}."));
                    }
                }
            }
        }
        self.outputs.sort_by(|a, b| {
            b.novel
                .cmp(&a.novel)
                .then_with(|| b.targeted.cmp(&a.targeted))
                .then_with(|| b.depth.cmp(&a.depth))
                .then_with(|| a.order.cmp(&b.order))
        });
        SearchResult {
            bound: true,
            work: self.work,
            candidates: self
                .outputs
                .into_iter()
                .take(self.p.case_limit)
                .map(|o| o.candidate)
                .collect(),
            unknown: self.unknown.into_iter().collect(),
        }
    }

    fn simulate(&mut self, input: Input) {
        if !self.simulated.insert(input.key) {
            return;
        }
        self.work += 1;
        let (guards, reason) = Interpreter::new(self.p, &input.args).run();
        if let Some(reason) = reason {
            self.unknown.insert(reason);
        }
        let mut novel = None;
        let mut targeted = false;
        let mut prefix = String::new();
        let mut preserved: Vec<Comparison> = Vec::new();
        for (index, guard) in guards.iter().enumerate() {
            if self.covered.insert(guard.taken.clone()) {
                novel = Some(guard.taken.clone());
            }
            targeted |= input.goal.as_ref() == Some(&guard.taken);
            prefix.push('/');
            prefix.push_str(&guard.taken);
            let attempts = self.expansions.entry(prefix.clone()).or_default();
            if *attempts < 4 {
                *attempts += 1;
                for comparison in &guard.comparisons {
                    if self.policy == SearchPolicy::CorrelatedEqualitiesV1 {
                        self.propose_correlated(
                            &input.args,
                            comparison,
                            &guard.alternate,
                            &guard.taken,
                            index,
                            &preserved,
                        );
                    }
                    self.propose(
                        &input.args,
                        comparison,
                        &guard.alternate,
                        &guard.taken,
                        index,
                        &preserved,
                    );
                }
            }
            if guard.comparisons.len() == 1 {
                let c = &guard.comparisons[0];
                if matches!(c.operator.as_str(), "Equals" | "NotEquals")
                    && c.left.value == c.right.value
                {
                    preserved.push(c.clone());
                }
            }
        }
        if input.seed || guards.is_empty() {
            return;
        }
        let goal = novel
            .clone()
            .or_else(|| if targeted { input.goal.clone() } else { None })
            .unwrap_or_else(|| guards.last().expect("nonempty guards").taken.clone());
        self.outputs.push(Output {
            candidate: Candidate {
                args: input.args,
                predicted_goal: goal,
            },
            novel: novel.is_some(),
            targeted,
            depth: guards.len(),
            order: self.work,
        });
    }

    fn propose(
        &mut self,
        args: &[i64],
        c: &Comparison,
        alternate: &str,
        taken: &str,
        depth: usize,
        preserved: &[Comparison],
    ) {
        let mut seen = HashSet::new();
        let parameters = c
            .left
            .coefficients
            .as_ref()
            .into_iter()
            .flatten()
            .chain(c.right.coefficients.as_ref().into_iter().flatten())
            .map(|(id, _)| *id)
            .collect::<Vec<_>>();
        for parameter in parameters {
            if !seen.insert(parameter) {
                continue;
            }
            let lf = c.left.factor(parameter);
            let rf = c.right.factor(parameter);
            let diff = c.left.value.wrapping_sub(c.right.value);
            let diff_factor = lf.wrapping_sub(rf);
            for target in [0_i32, -1, 1] {
                let delta = target.wrapping_sub(diff);
                for change in changes(diff_factor, delta) {
                    self.consider(args, c, alternate, taken, depth, parameter, change, None);
                }
                for prior in preserved {
                    self.propose_pair(
                        args,
                        c,
                        alternate,
                        taken,
                        depth,
                        prior,
                        parameter,
                        diff_factor,
                        delta,
                        &c.left,
                        Some(&c.right),
                    );
                }
            }
            for boundary in [i32::MIN, i32::MAX] {
                let delta_left = boundary.wrapping_sub(c.left.value);
                let delta_right = boundary.wrapping_sub(c.right.value);
                for change in changes(lf, delta_left) {
                    self.consider(args, c, alternate, taken, depth, parameter, change, None);
                }
                for change in changes(rf, delta_right) {
                    self.consider(args, c, alternate, taken, depth, parameter, change, None);
                }
                for prior in preserved {
                    self.propose_pair(
                        args, c, alternate, taken, depth, prior, parameter, lf, delta_left,
                        &c.left, None,
                    );
                    self.propose_pair(
                        args,
                        c,
                        alternate,
                        taken,
                        depth,
                        prior,
                        parameter,
                        rf,
                        delta_right,
                        &c.right,
                        None,
                    );
                }
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn propose_pair(
        &mut self,
        args: &[i64],
        c: &Comparison,
        alternate: &str,
        taken: &str,
        depth: usize,
        prior: &Comparison,
        parameter: usize,
        target_factor: i32,
        delta: i32,
        first_operand: &Scalar,
        second_operand: Option<&Scalar>,
    ) {
        let prior_factor = prior
            .left
            .factor(parameter)
            .wrapping_sub(prior.right.factor(parameter));
        if prior_factor == 0 || self.proposal_work >= self.proposal_limit() {
            return;
        }
        let mut seen = HashSet::new();
        let partners = prior
            .left
            .coefficients
            .as_ref()
            .into_iter()
            .flatten()
            .chain(prior.right.coefficients.as_ref().into_iter().flatten())
            .map(|(id, _)| *id)
            .collect::<Vec<_>>();
        for partner in partners {
            if !seen.insert(partner) || partner == parameter {
                continue;
            }
            let partner_prior = prior
                .left
                .factor(partner)
                .wrapping_sub(prior.right.factor(partner));
            if partner_prior == 0 {
                continue;
            }
            let mut partner_target = first_operand.factor(partner);
            if let Some(second) = second_operand {
                partner_target = partner_target.wrapping_sub(second.factor(partner));
            }
            let determinant = target_factor
                .wrapping_mul(partner_prior)
                .wrapping_sub(partner_target.wrapping_mul(prior_factor));
            for first in changes(determinant, delta.wrapping_mul(partner_prior)) {
                for second in changes(
                    partner_prior,
                    prior_factor.wrapping_neg().wrapping_mul(first),
                ) {
                    if target_factor
                        .wrapping_mul(first)
                        .wrapping_add(partner_target.wrapping_mul(second))
                        == delta
                    {
                        self.consider(
                            args,
                            c,
                            alternate,
                            taken,
                            depth,
                            parameter,
                            first,
                            Some((partner, second)),
                        );
                    }
                }
            }
        }
    }

    fn proposal_limit(&self) -> u64 {
        64_u64.saturating_mul(self.p.work_limit as u64)
    }

    #[allow(clippy::too_many_arguments)]
    fn consider(
        &mut self,
        args: &[i64],
        c: &Comparison,
        alternate: &str,
        taken: &str,
        depth: usize,
        parameter: usize,
        change: i32,
        partner: Option<(usize, i32)>,
    ) {
        if self.proposal_work >= self.proposal_limit() {
            return;
        }
        self.proposal_work += 1;
        if parameter >= args.len() {
            return;
        }
        let next = (args[parameter] as i32).wrapping_add(change);
        if change == 0 && partner.is_none_or(|(_, n)| n == 0) || !self.valid_param(parameter, next)
        {
            return;
        }
        let mut copy = args.to_vec();
        copy[parameter] = next as i64;
        let mut left = c
            .left
            .value
            .wrapping_add(c.left.factor(parameter).wrapping_mul(change));
        let mut right = c
            .right
            .value
            .wrapping_add(c.right.factor(parameter).wrapping_mul(change));
        if let Some((other, partner_change)) = partner {
            if other >= args.len() {
                return;
            }
            let p = (args[other] as i32).wrapping_add(partner_change);
            if !self.valid_param(other, p) {
                return;
            }
            copy[other] = p as i64;
            left = left.wrapping_add(c.left.factor(other).wrapping_mul(partner_change));
            right = right.wrapping_add(c.right.factor(other).wrapping_mul(partner_change));
        }
        let flips =
            compare(&c.operator, c.left.value, c.right.value) != compare(&c.operator, left, right);
        let score = -(2 * depth as i32) - i32::from(flips);
        self.enqueue(copy, if flips { alternate } else { taken }, score);
    }

    fn valid_param(&self, index: usize, value: i32) -> bool {
        self.p.params[index] != Ty::Bool || value == 0 || value == 1
    }

    fn enqueue(&mut self, args: Vec<i64>, goal: &str, priority: i32) {
        let key = Self::key(&args);
        if self.seed_keys.contains(&key)
            || self.simulated.contains(&key)
            || self.pending.get(&key).is_some_and(|p| *p <= priority)
        {
            return;
        }
        self.pending.insert(key.clone(), priority);
        self.queue.push(Queued {
            priority,
            order: self.proposal_order,
            input: Input {
                args,
                key,
                goal: Some(goal.into()),
                seed: false,
            },
        });
        self.proposal_order += 1;
    }
}

/// Run only the pure graph search. The caller must use its own compiler/artifact verifier,
/// original-build execution and replay before treating any result as observed behavior.
pub fn search(problem: &Problem) -> SearchResult {
    Search::new(problem).run()
}

/// Explicit policy selection for owner-side evaluation. Existing callers remain
/// on [`search`]; opting in can change candidate order, work and unknown reasons.
/// This produces predictions only and does not verify original-build behavior.
pub fn search_with_policy(problem: &Problem, policy: SearchPolicy) -> SearchResult {
    let mut search = Search::new(problem);
    search.policy = policy;
    search.run()
}

/// Run a small interactive search on caller-supplied facts. Bound expanded
/// expression work as well as ordinary search work because the wire format
/// permits multiple references to the same child node.
pub fn plan_correlations(input: &str) -> Result<SearchResult, String> {
    let problem = parse_wire(input)?;
    if problem.work_limit > 256 || problem.case_limit > 128 || problem.params.len() > 32 {
        return Err("Interactive scalar limits exceeded (256 work, 128 cases, 32 parameters).".into());
    }
    let mut expanded = 0usize;
    for block in &problem.blocks {
        let mut costs = vec![0usize; block.nodes.len()];
        for (id, node) in block.nodes.iter().enumerate().rev() {
            costs[id] = node.children.iter().fold(1usize, |cost, &child| {
                cost.saturating_add(costs[child])
            });
        }
        for &root in block.statements.iter().chain(block.branch.iter()) {
            expanded = expanded.saturating_add(costs[root]);
            if expanded > 8192 {
                return Err("Interactive scalar facts exceed the expanded expression budget.".into());
            }
        }
    }
    Ok(search_with_policy(&problem, SearchPolicy::CorrelatedEqualitiesV1))
}

/// Machine-readable output; the wire type `candidate` means predicted, not measured.
pub fn result_to_wire(result: &SearchResult) -> String {
    let mut text = format!(
        "scalar-path-results\t1\nresult\t{}\t{}\n",
        u8::from(result.bound),
        result.work
    );
    for candidate in &result.candidates {
        text.push_str(&format!(
            "candidate\t{}\t{}\n",
            candidate.predicted_goal,
            candidate
                .args
                .iter()
                .map(i64::to_string)
                .collect::<Vec<_>>()
                .join(",")
        ));
    }
    for unknown in &result.unknown {
        let hex = unknown
            .as_bytes()
            .iter()
            .map(|b| format!("{b:02X}"))
            .collect::<String>();
        text.push_str(&format!("unknown\t{hex}\n"));
    }
    text.push_str("end\n");
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn modular_changes_match_wrapping_and_tie_rules() {
        assert_eq!(changes(1, i32::MIN), vec![i32::MIN]);
        assert_eq!(changes(2, 2), vec![1, -2147483647]);
        assert!(changes(2, 1).is_empty());
        assert_eq!(changes(i32::MIN, i32::MIN), vec![-1, 1]);
    }

    #[test]
    fn coefficients_preserve_order_after_removal() {
        let l = vec![(0, 1), (1, -1)];
        let r = vec![(0, 1), (2, 3)];
        assert_eq!(
            combine(Some(&l), Some(&r), true),
            Some(vec![(1, -1), (2, -3)])
        );
    }
}
