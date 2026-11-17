//! The pinned ContractExpression grammar and lazy integer evaluation.
use crate::integer::Integer;
use std::{cmp::Ordering, sync::Arc};

pub(crate) const MAX_EXPRESSION_BYTES: usize = 4096;
pub(crate) const MAX_DEPTH: usize = 64;
pub(crate) const MAX_INTERMEDIATE_BITS: usize = 1024;

#[derive(Clone, Debug)]
pub(crate) struct Name {
    pub(crate) text: String,
    pub(crate) inputs: u32,
}

#[derive(Clone, Debug)]
pub(crate) struct Expr {
    node: Arc<Node>,
    pub(crate) inputs: u32,
    conditions: Option<Vec<Comparison>>,
}

#[derive(Clone, Debug)]
enum Node {
    Value(Integer),
    Parameter(usize),
    Unary(char, Expr),
    Binary(String, Expr, Expr),
    Conditional(Expr, Expr, Expr),
    Function(String, Vec<Expr>),
}

impl Expr {
    fn make(node: Node, inputs: u32, conditions: Option<Vec<Comparison>>) -> Self {
        Self {
            node: Arc::new(node),
            inputs,
            conditions,
        }
    }

    pub(crate) fn eval(&self, values: &[Integer]) -> Result<Integer, &'static str> {
        let result = match self.node.as_ref() {
            Node::Value(value) => value.clone(),
            Node::Parameter(index) => values
                .get(*index)
                .ok_or("missing expression value")?
                .clone(),
            Node::Unary(operator, inner) => {
                let value = inner.eval(values)?;
                match operator {
                    '-' => value.negated(),
                    '!' => truth(value.is_zero()),
                    '~' => value.not(),
                    '+' => value,
                    _ => unreachable!(),
                }
            }
            Node::Binary(op, left, right) if op == "&&" => {
                truth(!left.eval(values)?.is_zero() && !right.eval(values)?.is_zero())
            }
            Node::Binary(op, left, right) if op == "||" => {
                truth(!left.eval(values)?.is_zero() || !right.eval(values)?.is_zero())
            }
            Node::Binary(op, left, right) => {
                let l = left.eval(values)?;
                let r = right.eval(values)?;
                let answer = binary(op, &l, &r)?;
                if answer.twos_complement_bits() > MAX_INTERMEDIATE_BITS {
                    return Err("A contract intermediate exceeds the expression budget.");
                }
                answer
            }
            Node::Conditional(cond, yes, no) => {
                if cond.eval(values)?.is_zero() {
                    no.eval(values)?
                } else {
                    yes.eval(values)?
                }
            }
            Node::Function(name, args) => {
                let first = args[0].eval(values)?;
                match name.as_str() {
                    "abs" => first.abs(),
                    "popcount" => {
                        let unsigned = first
                            .to_u64()
                            .ok_or("popcount requires an unsigned 64-bit value")?;
                        Integer::from_i64(i64::from(unsigned.count_ones()))
                    }
                    "min" => {
                        let second = args[1].eval(values)?;
                        if first.compare(&second) == Ordering::Greater {
                            second
                        } else {
                            first
                        }
                    }
                    "max" => {
                        let second = args[1].eval(values)?;
                        if first.compare(&second) == Ordering::Less {
                            second
                        } else {
                            first
                        }
                    }
                    "mod" => {
                        let second = args[1].eval(values)?;
                        if second.is_zero() || second.negative() {
                            return Err("Mathematical modulo requires a positive period.");
                        }
                        let rem = first.div_rem(&second)?.1;
                        if rem.negative() {
                            rem.add(&second)
                        } else {
                            rem
                        }
                    }
                    "clamp" => {
                        let second = args[1].eval(values)?;
                        let third = args[2].eval(values)?;
                        let max = if first.compare(&second) == Ordering::Less {
                            second
                        } else {
                            first
                        };
                        if max.compare(&third) == Ordering::Greater {
                            third
                        } else {
                            max
                        }
                    }
                    _ => unreachable!(),
                }
            }
        };
        // Limits are checked after C# binary operators; the expression parser
        // also restricts shifts and constants, so unary/function output is bounded.
        Ok(result)
    }
}

fn truth(value: bool) -> Integer {
    if value {
        Integer::one()
    } else {
        Integer::zero()
    }
}

fn binary(op: &str, left: &Integer, right: &Integer) -> Result<Integer, &'static str> {
    Ok(match op {
        "+" => left.add(right),
        "-" => left.sub(right),
        "*" => left.multiply(right),
        "/" => left.div_rem(right)?.0,
        "%" => left.div_rem(right)?.1,
        "<<" | ">>" => {
            let shift = right
                .to_i64()
                .filter(|count| (0..=127).contains(count))
                .ok_or("A contract shift must be between 0 and 127.")?
                as usize;
            if op == "<<" {
                left.left(shift)
            } else {
                left.right(shift)
            }
        }
        "&" | "|" | "^" => left.bit_op(right, op),
        "==" => truth(left.compare(right) == Ordering::Equal),
        "!=" => truth(left.compare(right) != Ordering::Equal),
        "<" => truth(left.compare(right) == Ordering::Less),
        "<=" => truth(left.compare(right) != Ordering::Greater),
        ">" => truth(left.compare(right) == Ordering::Greater),
        ">=" => truth(left.compare(right) != Ordering::Less),
        _ => return Err("unknown binary operator"),
    })
}

#[derive(Clone, Debug)]
pub(crate) struct Comparison {
    pub(crate) op: String,
    pub(crate) inputs: u32,
    left: Expr,
    right: Expr,
}

impl Comparison {
    pub(crate) fn difference(&self, values: &[Integer]) -> Result<Integer, &'static str> {
        let result = self.left.eval(values)?.sub(&self.right.eval(values)?);
        if result.twos_complement_bits() > MAX_INTERMEDIATE_BITS + 1 {
            return Err("A contract comparison exceeds the expression budget.");
        }
        Ok(result)
    }
    pub(crate) fn matches(&self, value: &Integer) -> bool {
        match self.op.as_str() {
            "==" => value.is_zero(),
            "!=" => !value.is_zero(),
            "<" => value.negative(),
            "<=" => value.negative() || value.is_zero(),
            ">" => !value.negative() && !value.is_zero(),
            ">=" => !value.negative(),
            _ => unreachable!(),
        }
    }
    pub(crate) fn boundary(&self) -> i64 {
        match self.op.as_str() {
            "<" => -1,
            ">" | "!=" => 1,
            _ => 0,
        }
    }
}

#[derive(Debug)]
pub(crate) struct Compiled {
    pub(crate) expr: Expr,
    pub(crate) groups: Vec<Vec<Comparison>>,
}

pub(crate) fn is_name(text: &str) -> bool {
    let mut chars = text.bytes();
    let Some(first) = chars.next() else {
        return false;
    };
    (first.is_ascii_alphabetic() || first == b'_')
        && chars.all(|b| b.is_ascii_alphanumeric() || b == b'_')
        && text != "true"
        && text != "false"
}

pub(crate) fn compile(text: &str, names: &[Name]) -> Result<Compiled, String> {
    if text.trim().is_empty() || text.len() > MAX_EXPRESSION_BYTES || text.contains('\0') {
        return Err("Provide a contract expression of at most 4096 characters.".into());
    }
    let mut parser = Parser {
        text,
        position: 0,
        depth: 0,
        names,
        groups: Vec::new(),
    };
    let expr = parser.parse(0)?;
    if parser.peek().is_some() {
        return Err(parser.error("Unexpected token"));
    }
    Ok(Compiled {
        expr,
        groups: parser.groups,
    })
}

struct Parser<'a> {
    text: &'a str,
    position: usize,
    depth: usize,
    names: &'a [Name],
    groups: Vec<Vec<Comparison>>,
}

impl<'a> Parser<'a> {
    fn error(&self, reason: &str) -> String {
        format!("{reason} at column {}: {}", self.position + 1, self.text)
    }

    fn peek(&mut self) -> Option<u8> {
        while let Some(ch) = self.text[self.position..].chars().next() {
            if !ch.is_whitespace() {
                break;
            }
            self.position += ch.len_utf8();
        }
        self.text.as_bytes().get(self.position).copied()
    }

    fn take(&mut self, token: u8) -> bool {
        if self.peek() == Some(token) {
            self.position += 1;
            true
        } else {
            false
        }
    }

    fn require(&mut self, token: u8) -> Result<(), String> {
        if self.take(token) {
            Ok(())
        } else {
            Err(self.error(&format!("Expected '{}'", token as char)))
        }
    }

    fn operator(&mut self) -> Option<(&'static str, u8)> {
        let remain = &self.text[self.position..];
        for (op, priority) in [
            ("||", 1),
            ("&&", 2),
            ("==", 6),
            ("!=", 6),
            ("<=", 7),
            (">=", 7),
            ("<<", 8),
            (">>", 8),
            ("|", 3),
            ("^", 4),
            ("&", 5),
            ("<", 7),
            (">", 7),
            ("+", 9),
            ("-", 9),
            ("*", 10),
            ("/", 10),
            ("%", 10),
        ] {
            if remain.starts_with(op) {
                return Some((op, priority));
            }
        }
        None
    }

    fn parse(&mut self, minimum: u8) -> Result<Expr, String> {
        self.depth += 1;
        if self.depth > MAX_DEPTH {
            return Err(self.error("Expression nesting is too deep"));
        }
        let result = self.parse_body(minimum);
        self.depth -= 1;
        result
    }

    fn parse_body(&mut self, minimum: u8) -> Result<Expr, String> {
        let mut left = self.operand()?;
        loop {
            let _ = self.peek();
            let Some((op, priority)) = self.operator() else {
                break;
            };
            if priority < minimum {
                break;
            }
            self.position += op.len();
            let right = self.parse(priority + 1)?;
            let dependencies = left.inputs | right.inputs;
            let conditions = match op {
                "&&" => match (&left.conditions, &right.conditions) {
                    (Some(a), Some(b)) if a.len() + b.len() <= 32 => {
                        let mut group = a.clone();
                        group.extend(b.clone());
                        Some(group)
                    }
                    _ => None,
                },
                "==" | "!=" | "<" | "<=" | ">" | ">=" => Some(vec![Comparison {
                    op: op.into(),
                    inputs: dependencies,
                    left: left.clone(),
                    right: right.clone(),
                }]),
                _ => None,
            };
            let result = Expr::make(
                Node::Binary(op.into(), left, right),
                dependencies,
                conditions,
            );
            if let Some(group) = &result.conditions {
                self.groups.push(group.clone());
            }
            left = result;
        }
        if minimum == 0 && self.take(b'?') {
            let yes = self.parse(0)?;
            self.require(b':')?;
            let no = self.parse(0)?;
            left = Expr::make(
                Node::Conditional(left.clone(), yes.clone(), no.clone()),
                left.inputs | yes.inputs | no.inputs,
                None,
            );
        }
        Ok(left)
    }

    fn operand(&mut self) -> Result<Expr, String> {
        if let Some(op @ (b'-' | b'!' | b'~' | b'+')) = self.peek() {
            self.position += 1;
            let rhs = self.parse(11)?;
            return Ok(Expr::make(
                Node::Unary(op as char, rhs.clone()),
                rhs.inputs,
                None,
            ));
        }
        if self.take(b'(') {
            let inner = self.parse(0)?;
            self.require(b')')?;
            return Ok(inner);
        }
        let next = self.peek();
        let start = self.position;
        match next {
            Some(ch) if ch.is_ascii_digit() => {
                while self
                    .text
                    .as_bytes()
                    .get(self.position)
                    .is_some_and(u8::is_ascii_digit)
                {
                    self.position += 1;
                }
                if self.position - start > 39 {
                    return Err(self.error("Integer constant is too large"));
                }
                let value = Integer::decimal(&self.text[start..self.position])
                    .map_err(|msg| self.error(msg))?;
                Ok(Expr::make(Node::Value(value), 0, None))
            }
            Some(ch) if ch.is_ascii_alphabetic() || ch == b'_' => {
                self.position += 1;
                while self
                    .text
                    .as_bytes()
                    .get(self.position)
                    .is_some_and(|ch| ch.is_ascii_alphanumeric() || *ch == b'_')
                {
                    self.position += 1;
                }
                let name = self.text[start..self.position].to_owned();
                if self.take(b'(') {
                    return self.function(&name);
                }
                if name == "true" || name == "false" {
                    return Ok(Expr::make(Node::Value(truth(name == "true")), 0, None));
                }
                let Some((index, binding)) = self
                    .names
                    .iter()
                    .enumerate()
                    .find(|(_, item)| item.text == name)
                else {
                    return Err(self.error(&format!("Unknown name '{name}'")));
                };
                Ok(Expr::make(Node::Parameter(index), binding.inputs, None))
            }
            _ => Err(self.error("Expected a constant, name, or parenthesized expression")),
        }
    }

    fn function(&mut self, name: &str) -> Result<Expr, String> {
        let first = self.parse(0)?;
        if name == "abs" || name == "popcount" {
            self.require(b')')?;
            return Ok(Expr::make(
                Node::Function(name.into(), vec![first.clone()]),
                first.inputs,
                None,
            ));
        }
        self.require(b',')?;
        let second = self.parse(0)?;
        if name == "clamp" {
            self.require(b',')?;
            let third = self.parse(0)?;
            self.require(b')')?;
            let mask = first.inputs | second.inputs | third.inputs;
            return Ok(Expr::make(
                Node::Function(name.into(), vec![first, second, third]),
                mask,
                None,
            ));
        }
        self.require(b')')?;
        let mask = first.inputs | second.inputs;
        match name {
            "min" | "max" | "mod" => Ok(Expr::make(
                Node::Function(name.into(), vec![first, second]),
                mask,
                None,
            )),
            _ => Err(self.error(&format!("Unknown function '{name}'"))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Name, compile};
    use crate::integer::Integer;
    fn evaluate(source: &str, values: &[i64]) -> Option<i64> {
        let names = [
            Name {
                text: "a".into(),
                inputs: 1,
            },
            Name {
                text: "b".into(),
                inputs: 2,
            },
        ];
        compile(source, &names)
            .ok()?
            .expr
            .eval(
                &values
                    .iter()
                    .copied()
                    .map(Integer::from_i64)
                    .collect::<Vec<_>>(),
            )
            .ok()?
            .to_i64()
    }
    #[test]
    fn arithmetic_eager_and_short_circuit() {
        assert_eq!(evaluate("a == 0 || b/a > 0", &[0, 1]), Some(1));
        assert_eq!(evaluate("a != 0 && b/a > 0", &[0, 1]), Some(0));
        assert_eq!(evaluate("mod(-a, 5)", &[2, 1]), Some(3));
        assert_eq!(evaluate("(a * a) / a", &[i64::MAX, 0]), Some(i64::MAX));
        assert_eq!(evaluate("a / 0", &[1, 0]), None);
        assert_eq!(evaluate("a << 128", &[1, 0]), None);
        assert_eq!(evaluate("-3 >> 1", &[1, 0]), Some(-2));
        assert_eq!(evaluate("popcount(255)", &[1, 0]), Some(8));
        assert_eq!(evaluate("popcount(-1)", &[1, 0]), None);
        assert_eq!(evaluate("a < b ? 10 : 20", &[1, 2]), Some(10));
        assert_eq!(evaluate("-1 & 255", &[1, 0]), Some(255));
        assert_eq!(evaluate("~0", &[1, 0]), Some(-1));
    }
    #[test]
    fn grammar_and_depth_limits() {
        let names = [];
        assert!(compile("1+2*3", &names).is_ok());
        assert!(compile("missing", &names).is_err());
        assert!(compile("1 << -1", &names).is_ok());
        assert!(compile(&format!("{}1{}", "(".repeat(70), ")".repeat(70)), &names).is_err());
        assert!(compile("1".repeat(4100).as_str(), &names).is_err());
    }
}
