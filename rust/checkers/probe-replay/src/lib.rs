//! Pure, ordered counterparts of the pinned `ProbeReplay.InputsForCheck`, `Check`,
//! and `Compare` operations. No adapter, CLI, receipt, or probe generation lives here.
//!
//! JSON is parsed into ordered property lists and exact decimal lexemes. In
//! particular, `serde_json::Value` is deliberately *not* used: its usual object
//! map drops duplicate keys, and its ordinary numeric representation can round.
//! All lengths and retained evidence are explicitly bounded. The public `parse_*`
//! entry points accept UTF-8 bytes, keeping encoding failures distinguishable.

use std::collections::HashMap;
use std::error::Error;
use std::fmt;

/// Per source JSON document, in bytes. The pinned foreground check also uses
/// an 8 MiB file/output budget; this library applies the cap to all entry points.
pub const MAX_SOURCE_BYTES: usize = 8 * 1024 * 1024;
/// Maximum combined payload retained in check findings or first differences.
pub const MAX_RETAINED_FINDING_BYTES: usize = 8 * 1024 * 1024;
/// Maximum number of distinct findings; never truncate a result silently.
pub const MAX_FINDINGS: usize = 16_384;
/// Prevent tiny JSON tokens from causing unbounded allocation amplification.
pub const MAX_JSON_NODES: usize = 500_000;
/// Match the default maximum depth in System.Text.Json's JsonDocument parser.
pub const MAX_JSON_DEPTH: usize = 64;

pub type ReplayResult<T> = Result<T, ReplayError>;

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ReplayError {
    /// Any input, node count, or retained-finding limit is exceeded.
    Oversize {
        subject: &'static str,
        limit: usize,
        actual: usize,
    },
    /// Malformed JSON syntax, invalid UTF-8, or invalid surrogate text.
    InvalidJson {
        subject: &'static str,
        offset: usize,
        reason: &'static str,
    },
    /// Valid JSON whose shape or required field is invalid for replay.
    InvalidData(&'static str),
    /// Result arrays must have one entry for each input, in the same order.
    LengthMismatch,
    /// System.Text.Json.DeepEquals throws on exponents outside its Int32 range.
    /// Do not silently compare or round such valid JSON numbers.
    ExponentOutOfRange,
}

impl fmt::Display for ReplayError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Oversize {
                subject,
                limit,
                actual,
            } => write!(f, "oversize {subject}: {actual} exceeds limit {limit}"),
            Self::InvalidJson {
                subject,
                offset,
                reason,
            } => write!(f, "invalid JSON in {subject} at byte {offset}: {reason}"),
            Self::InvalidData(reason) => write!(f, "invalid replay data: {reason}"),
            Self::LengthMismatch => {
                f.write_str("result arrays must have exactly one item per input, in input order")
            }
            Self::ExponentOutOfRange => {
                f.write_str("JSON exponent exceeds .NET DeepEquals Int32 range")
            }
        }
    }
}

impl Error for ReplayError {}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProbeInput {
    pub operation: String,
    pub args: Vec<i64>,
}

#[derive(Clone, Debug)]
pub struct ProbeExpectation {
    pub operation: String,
    pub args: Vec<i64>,
    pub expected: JsonValue,
}

/// One parsed JSON value, retaining both its original source lexeme and a
/// structural representation with duplicate properties in their input order.
#[derive(Clone, Debug)]
pub struct JsonValue {
    raw: String,
    node: JsonNode,
}

impl JsonValue {
    pub fn parse(source: &[u8]) -> ReplayResult<Self> {
        let (text, node) = parse_document(source, "value")?;
        Ok(Self::from_node(text, node))
    }

    pub fn raw_json(&self) -> &str {
        &self.raw
    }

    pub fn is_null(&self) -> bool {
        matches!(self.node.kind, JsonKind::Null)
    }

    pub fn deep_equals(&self, other: &Self) -> ReplayResult<bool> {
        deep_equals(&self.node, &other.node)
    }

    fn from_node(source: &str, node: JsonNode) -> Self {
        let raw = source[node.start..node.end].to_owned();
        Self { raw, node }
    }
}

#[derive(Clone, Debug)]
pub struct ProbeDifference {
    pub index: usize,
    pub input: ProbeInput,
    pub candidate: JsonValue,
    pub reference: JsonValue,
}

#[derive(Clone, Debug)]
pub struct ProbeOperationResult {
    pub operation: String,
    pub compared: usize,
    pub unsupported: usize,
    pub one_sided: usize,
    pub different: usize,
    pub first_difference: Option<ProbeDifference>,
}

#[derive(Clone, Debug)]
pub struct ProbeCheckResult {
    pub cases: usize,
    pub compared: usize,
    pub unsupported: usize,
    pub findings: Vec<ProbeDifference>,
}

/// `ProbeReplay.InputsForCheck`: requires at least one case, an operation that
/// is not entirely Unicode whitespace, an arguments array, and non-null
/// expected JSON. Missing `Expected` is rejected while parsing the cases.
pub fn inputs_for_check(cases: &[ProbeExpectation]) -> ReplayResult<Vec<ProbeInput>> {
    if cases.is_empty() {
        return Err(ReplayError::InvalidData("provide at least one check case"));
    }
    cases
        .iter()
        .map(|case| {
            if is_blank(&case.operation) || case.expected.is_null() {
                return Err(ReplayError::InvalidData(
                    "each check needs an operation, arguments and a non-null expected result",
                ));
            }
            Ok(ProbeInput {
                operation: case.operation.clone(),
                args: case.args.clone(),
            })
        })
        .collect()
}

/// `ProbeReplay.Check`: collect *all* non-null mismatches in global input
/// order, excluding adapter-null results from the comparison count.
pub fn check(cases: &[ProbeExpectation], actual: &[JsonValue]) -> ReplayResult<ProbeCheckResult> {
    if cases.len() != actual.len() {
        return Err(ReplayError::LengthMismatch);
    }
    let inputs = inputs_for_check(cases)?;
    let mut unsupported = 0usize;
    let mut findings = Vec::new();
    let mut retained = 0usize;
    for (index, (input, (case, observed))) in inputs
        .into_iter()
        .zip(cases.iter().zip(actual.iter()))
        .enumerate()
    {
        if observed.is_null() {
            unsupported += 1;
            continue;
        }
        if !observed.deep_equals(&case.expected)? {
            reserve_finding(
                &mut retained,
                findings.len(),
                &input,
                observed,
                &case.expected,
            )?;
            findings.push(ProbeDifference {
                index,
                input,
                candidate: observed.clone(),
                reference: case.expected.clone(),
            });
        }
    }
    Ok(ProbeCheckResult {
        cases: cases.len(),
        compared: cases.len() - unsupported,
        unsupported,
        findings,
    })
}

/// `ProbeReplay.Compare`: group by first occurrence of the *exact* operation
/// name, but keep every comparison and first difference at its global index.
/// Both-null and one-sided null cases count as unsupported, not as values.
pub fn compare(
    inputs: &[ProbeInput],
    candidate: &[JsonValue],
    reference: &[JsonValue],
) -> ReplayResult<Vec<ProbeOperationResult>> {
    if inputs.len() != candidate.len() || inputs.len() != reference.len() {
        return Err(ReplayError::LengthMismatch);
    }
    let mut positions = HashMap::<&str, usize>::new();
    let mut output = Vec::<ProbeOperationResult>::new();
    let mut retained = 0usize;
    let mut finding_count = 0usize;
    for (index, ((input, left), right)) in inputs
        .iter()
        .zip(candidate.iter())
        .zip(reference.iter())
        .enumerate()
    {
        if is_blank(&input.operation) {
            return Err(ReplayError::InvalidData(
                "an input is missing its operation or arguments",
            ));
        }
        let pos = if let Some(&pos) = positions.get(input.operation.as_str()) {
            pos
        } else {
            let pos = output.len();
            output.push(ProbeOperationResult {
                operation: input.operation.clone(),
                compared: 0,
                unsupported: 0,
                one_sided: 0,
                different: 0,
                first_difference: None,
            });
            positions.insert(input.operation.as_str(), pos);
            pos
        };
        let state = &mut output[pos];
        if left.is_null() || right.is_null() {
            state.unsupported += 1;
            if left.is_null() != right.is_null() {
                state.one_sided += 1;
            }
            continue;
        }
        state.compared += 1;
        if !left.deep_equals(right)? {
            state.different += 1;
            if state.first_difference.is_none() {
                reserve_finding(&mut retained, finding_count, input, left, right)?;
                finding_count += 1;
                state.first_difference = Some(ProbeDifference {
                    index,
                    input: input.clone(),
                    candidate: left.clone(),
                    reference: right.clone(),
                });
            }
        }
    }
    Ok(output)
}

fn reserve_finding(
    retained: &mut usize,
    findings: usize,
    input: &ProbeInput,
    candidate: &JsonValue,
    reference: &JsonValue,
) -> ReplayResult<()> {
    if findings >= MAX_FINDINGS {
        return Err(ReplayError::Oversize {
            subject: "findings count",
            limit: MAX_FINDINGS,
            actual: findings.saturating_add(1),
        });
    }
    let extra = input
        .operation
        .len()
        .saturating_add(input.args.len().saturating_mul(8))
        .saturating_add(candidate.raw.len())
        .saturating_add(reference.raw.len());
    let total = retained.saturating_add(extra);
    if total > MAX_RETAINED_FINDING_BYTES {
        return Err(ReplayError::Oversize {
            subject: "retained findings bytes",
            limit: MAX_RETAINED_FINDING_BYTES,
            actual: total,
        });
    }
    *retained = total;
    Ok(())
}

fn is_blank(name: &str) -> bool {
    name.chars().all(char::is_whitespace)
}

/// Deserialize an input JSON array like the case-insensitive .NET CLI path.
/// An empty array is allowed by `Compare`, but not by `InputsForCheck`.
pub fn parse_inputs(source: &[u8]) -> ReplayResult<Vec<ProbeInput>> {
    let (text, root) = parse_document(source, "inputs")?;
    let JsonKind::Array(values) = root.kind else {
        return Err(ReplayError::InvalidData(
            "a JSON array of inputs was expected",
        ));
    };
    values.iter().map(|node| decode_input(text, node)).collect()
}

/// Deserialize check cases, with required `Expected` distinguished from JSON
/// null and from a missing field. Unknown object properties are ignored.
pub fn parse_expectations(source: &[u8]) -> ReplayResult<Vec<ProbeExpectation>> {
    let (text, root) = parse_document(source, "cases")?;
    let JsonKind::Array(values) = root.kind else {
        return Err(ReplayError::InvalidData(
            "a JSON array of check cases was expected",
        ));
    };
    values
        .iter()
        .map(|node| {
            let input = decode_input(text, node)?;
            let JsonKind::Object(fields) = &node.kind else {
                return Err(ReplayError::InvalidData("a check case must be an object"));
            };
            let expected = field(fields, "Expected").ok_or(ReplayError::InvalidData(
                "required Expected field is missing",
            ))?;
            Ok(ProbeExpectation {
                operation: input.operation,
                args: input.args,
                expected: JsonValue::from_node(text, expected.clone()),
            })
        })
        .collect()
}

/// Deserialize a JSON result array without losing numbers, duplicate keys,
/// or original value text. JSON null remains a distinct support marker.
pub fn parse_results(source: &[u8]) -> ReplayResult<Vec<JsonValue>> {
    let (text, root) = parse_document(source, "results")?;
    let JsonKind::Array(values) = root.kind else {
        return Err(ReplayError::InvalidData(
            "a JSON array of results was expected",
        ));
    };
    Ok(values
        .into_iter()
        .map(|node| JsonValue::from_node(text, node))
        .collect())
}

/// Convenience boundary for future routing; no CLI behavior is changed.
pub fn compare_sources(
    inputs: &[u8],
    candidate: &[u8],
    reference: &[u8],
) -> ReplayResult<Vec<ProbeOperationResult>> {
    compare(
        &parse_inputs(inputs)?,
        &parse_results(candidate)?,
        &parse_results(reference)?,
    )
}

/// Convenience boundary for future routing; no adapter execution is performed.
pub fn check_sources(cases: &[u8], actual: &[u8]) -> ReplayResult<ProbeCheckResult> {
    check(&parse_expectations(cases)?, &parse_results(actual)?)
}

/// Boundary counterpart to `InputsForCheck` for an incoming JSON case file.
pub fn inputs_for_check_source(cases: &[u8]) -> ReplayResult<Vec<ProbeInput>> {
    inputs_for_check(&parse_expectations(cases)?)
}

fn decode_input(source: &str, node: &JsonNode) -> ReplayResult<ProbeInput> {
    let JsonKind::Object(fields) = &node.kind else {
        return Err(ReplayError::InvalidData("an input must be an object"));
    };
    let operation =
        field(fields, "Operation").ok_or(ReplayError::InvalidData("operation is required"))?;
    let JsonKind::String(name) = &operation.kind else {
        return Err(ReplayError::InvalidData("operation must be a string"));
    };
    let args = field(fields, "Args").ok_or(ReplayError::InvalidData("arguments are required"))?;
    let JsonKind::Array(args) = &args.kind else {
        return Err(ReplayError::InvalidData("arguments must be an array"));
    };
    let mut parsed = Vec::with_capacity(args.len());
    for arg in args {
        if !matches!(arg.kind, JsonKind::Number(_)) {
            return Err(ReplayError::InvalidData(
                "each argument must be a signed 64-bit integer",
            ));
        }
        let raw = &source[arg.start..arg.end];
        let value = raw.parse::<i64>().map_err(|_| {
            ReplayError::InvalidData("each argument must be a signed 64-bit integer")
        })?;
        parsed.push(value);
    }
    Ok(ProbeInput {
        operation: name.clone(),
        args: parsed,
    })
}

fn field<'a>(fields: &'a [(String, JsonNode)], name: &str) -> Option<&'a JsonNode> {
    // System.Text.Json's case-insensitive property binding takes the last
    // occurrence when a DTO field appears more than once.
    fields
        .iter()
        .rev()
        .find(|(key, _)| key.eq_ignore_ascii_case(name))
        .map(|(_, value)| value)
}

#[derive(Clone, Debug)]
struct JsonNode {
    kind: JsonKind,
    start: usize,
    end: usize,
}

#[derive(Clone, Debug)]
enum JsonKind {
    Null,
    Bool(bool),
    Number(String),
    String(String),
    Array(Vec<JsonNode>),
    // Not a map: DeepEquals counts duplicate properties, and checks the
    // ordered sequence of values *within each property name*.
    Object(Vec<(String, JsonNode)>),
}

fn deep_equals(left: &JsonNode, right: &JsonNode) -> ReplayResult<bool> {
    match (&left.kind, &right.kind) {
        (JsonKind::Null, JsonKind::Null) => Ok(true),
        (JsonKind::Bool(a), JsonKind::Bool(b)) => Ok(a == b),
        (JsonKind::String(a), JsonKind::String(b)) => Ok(a == b),
        (JsonKind::Number(a), JsonKind::Number(b)) => {
            Ok(DecimalKey::from_json(a)? == DecimalKey::from_json(b)?)
        }
        (JsonKind::Array(a), JsonKind::Array(b)) => {
            if a.len() != b.len() {
                return Ok(false);
            }
            for (x, y) in a.iter().zip(b.iter()) {
                if !deep_equals(x, y)? {
                    return Ok(false);
                }
            }
            Ok(true)
        }
        (JsonKind::Object(a), JsonKind::Object(b)) => {
            if a.len() != b.len() {
                return Ok(false);
            }
            let mut groups: HashMap<&str, Vec<&JsonNode>> = HashMap::new();
            for (name, value) in b {
                groups.entry(name.as_str()).or_default().push(value);
            }
            let mut seen: HashMap<&str, usize> = HashMap::new();
            for (name, value) in a {
                let position = seen.entry(name.as_str()).or_default();
                let Some(sequence) = groups.get(name.as_str()) else {
                    return Ok(false);
                };
                let Some(other) = sequence.get(*position) else {
                    return Ok(false);
                };
                if !deep_equals(value, other)? {
                    return Ok(false);
                }
                *position += 1;
            }
            Ok(seen.len() == groups.len()
                && seen.iter().all(|(name, count)| {
                    groups
                        .get(name)
                        .is_some_and(|sequence| sequence.len() == *count)
                }))
        }
        _ => Ok(false),
    }
}

#[derive(Debug, Eq, PartialEq)]
struct DecimalKey {
    negative: bool,
    digits: String,
    /// Power of ten for the unscaled significant digits; never uses floats.
    power: i64,
}

impl DecimalKey {
    fn from_json(raw: &str) -> ReplayResult<Self> {
        // The JSON grammar was already validated by Parser::number.
        let unsigned = raw.strip_prefix('-').unwrap_or(raw);
        let (mantissa, exponent) = match unsigned.find(['e', 'E']) {
            Some(i) => (&unsigned[..i], &unsigned[i + 1..]),
            None => (unsigned, "0"),
        };
        // JsonElement.DeepEquals in .NET 10 uses an Int32 exponent. Values
        // outside that range throw; returning `false` would hide a blocker.
        let exponent: i32 = exponent
            .parse()
            .map_err(|_| ReplayError::ExponentOutOfRange)?;
        let (integral, fraction) = mantissa.split_once('.').unwrap_or((mantissa, ""));
        let all = format!("{integral}{fraction}");
        let significant = all.trim_start_matches('0');
        if significant.is_empty() {
            return Ok(Self {
                negative: false,
                digits: "0".to_owned(),
                power: 0,
            });
        }
        let trimmed = significant.trim_end_matches('0');
        let trailing = significant.len() - trimmed.len();
        Ok(Self {
            negative: raw.starts_with('-'),
            digits: trimmed.to_owned(),
            power: i64::from(exponent) - fraction.len() as i64 + trailing as i64,
        })
    }
}

fn parse_document<'a>(
    source: &'a [u8],
    subject: &'static str,
) -> ReplayResult<(&'a str, JsonNode)> {
    if source.len() > MAX_SOURCE_BYTES {
        return Err(ReplayError::Oversize {
            subject,
            limit: MAX_SOURCE_BYTES,
            actual: source.len(),
        });
    }
    let text = std::str::from_utf8(source).map_err(|err| ReplayError::InvalidJson {
        subject,
        offset: err.valid_up_to(),
        reason: "invalid UTF-8",
    })?;
    let mut parser = Parser {
        source: text.as_bytes(),
        subject,
        offset: 0,
        nodes: 0,
    };
    parser.whitespace();
    let node = parser.value(0)?;
    parser.whitespace();
    if parser.offset != parser.source.len() {
        return Err(parser.invalid("text after the JSON value"));
    }
    Ok((text, node))
}

struct Parser<'a> {
    source: &'a [u8],
    subject: &'static str,
    offset: usize,
    nodes: usize,
}

impl Parser<'_> {
    fn invalid(&self, reason: &'static str) -> ReplayError {
        ReplayError::InvalidJson {
            subject: self.subject,
            offset: self.offset,
            reason,
        }
    }

    fn whitespace(&mut self) {
        while matches!(self.peek(), Some(b' ' | b'\n' | b'\r' | b'\t')) {
            self.offset += 1;
        }
    }

    fn peek(&self) -> Option<u8> {
        self.source.get(self.offset).copied()
    }

    fn consume(&mut self, expected: u8) -> ReplayResult<()> {
        if self.peek() != Some(expected) {
            return Err(self.invalid("unexpected JSON token"));
        }
        self.offset += 1;
        Ok(())
    }

    fn literal(&mut self, text: &[u8]) -> ReplayResult<()> {
        if !self.source[self.offset..].starts_with(text) {
            return Err(self.invalid("invalid JSON literal"));
        }
        self.offset += text.len();
        Ok(())
    }

    fn value(&mut self, depth: usize) -> ReplayResult<JsonNode> {
        if depth > MAX_JSON_DEPTH {
            return Err(ReplayError::Oversize {
                subject: "JSON depth",
                limit: MAX_JSON_DEPTH,
                actual: depth,
            });
        }
        self.nodes += 1;
        if self.nodes > MAX_JSON_NODES {
            return Err(ReplayError::Oversize {
                subject: "JSON node count",
                limit: MAX_JSON_NODES,
                actual: self.nodes,
            });
        }
        self.whitespace();
        let start = self.offset;
        let kind = match self.peek() {
            Some(b'n') => {
                self.literal(b"null")?;
                JsonKind::Null
            }
            Some(b't') => {
                self.literal(b"true")?;
                JsonKind::Bool(true)
            }
            Some(b'f') => {
                self.literal(b"false")?;
                JsonKind::Bool(false)
            }
            Some(b'"') => JsonKind::String(self.string()?),
            Some(b'[') => JsonKind::Array(self.array(depth)?),
            Some(b'{') => JsonKind::Object(self.object(depth)?),
            Some(b'-' | b'0'..=b'9') => JsonKind::Number(self.number()?),
            _ => return Err(self.invalid("expected a JSON value")),
        };
        Ok(JsonNode {
            kind,
            start,
            end: self.offset,
        })
    }

    fn array(&mut self, depth: usize) -> ReplayResult<Vec<JsonNode>> {
        self.consume(b'[')?;
        self.whitespace();
        let mut elements = Vec::new();
        if self.peek() == Some(b']') {
            self.offset += 1;
            return Ok(elements);
        }
        loop {
            elements.push(self.value(depth + 1)?);
            self.whitespace();
            match self.peek() {
                Some(b',') => {
                    self.offset += 1;
                    self.whitespace();
                }
                Some(b']') => {
                    self.offset += 1;
                    return Ok(elements);
                }
                _ => return Err(self.invalid("expected comma or array end")),
            }
        }
    }

    fn object(&mut self, depth: usize) -> ReplayResult<Vec<(String, JsonNode)>> {
        self.consume(b'{')?;
        self.whitespace();
        let mut fields = Vec::new();
        if self.peek() == Some(b'}') {
            self.offset += 1;
            return Ok(fields);
        }
        loop {
            if self.peek() != Some(b'"') {
                return Err(self.invalid("object property name must be a string"));
            }
            let key = self.string()?;
            self.whitespace();
            self.consume(b':')?;
            self.whitespace();
            fields.push((key, self.value(depth + 1)?));
            self.whitespace();
            match self.peek() {
                Some(b',') => {
                    self.offset += 1;
                    self.whitespace();
                }
                Some(b'}') => {
                    self.offset += 1;
                    return Ok(fields);
                }
                _ => return Err(self.invalid("expected comma or object end")),
            }
        }
    }

    fn number(&mut self) -> ReplayResult<String> {
        let start = self.offset;
        if self.peek() == Some(b'-') {
            self.offset += 1;
        }
        match self.peek() {
            Some(b'0') => {
                self.offset += 1;
                if matches!(self.peek(), Some(b'0'..=b'9')) {
                    return Err(self.invalid("leading zero in number"));
                }
            }
            Some(b'1'..=b'9') => self.digits(),
            _ => return Err(self.invalid("invalid JSON integer part")),
        }
        if self.peek() == Some(b'.') {
            self.offset += 1;
            if !matches!(self.peek(), Some(b'0'..=b'9')) {
                return Err(self.invalid("missing fractional digits"));
            }
            self.digits();
        }
        if matches!(self.peek(), Some(b'e' | b'E')) {
            self.offset += 1;
            if matches!(self.peek(), Some(b'+' | b'-')) {
                self.offset += 1;
            }
            if !matches!(self.peek(), Some(b'0'..=b'9')) {
                return Err(self.invalid("missing exponent digits"));
            }
            self.digits();
        }
        // All boundaries in a JSON number are ASCII and thus valid UTF-8.
        Ok(std::str::from_utf8(&self.source[start..self.offset])
            .expect("number is ASCII")
            .to_owned())
    }

    fn digits(&mut self) {
        while matches!(self.peek(), Some(b'0'..=b'9')) {
            self.offset += 1;
        }
    }

    fn hex_quad(&mut self) -> ReplayResult<u16> {
        let mut value = 0u16;
        for _ in 0..4 {
            let Some(ch) = self.peek() else {
                return Err(self.invalid("incomplete Unicode escape"));
            };
            let nibble = match ch {
                b'0'..=b'9' => ch - b'0',
                b'a'..=b'f' => ch - b'a' + 10,
                b'A'..=b'F' => ch - b'A' + 10,
                _ => return Err(self.invalid("invalid Unicode hex escape")),
            };
            value = (value << 4) | u16::from(nibble);
            self.offset += 1;
        }
        Ok(value)
    }

    fn string(&mut self) -> ReplayResult<String> {
        self.consume(b'"')?;
        let mut result = String::new();
        let mut run = self.offset;
        loop {
            let Some(ch) = self.peek() else {
                return Err(self.invalid("unterminated JSON string"));
            };
            if ch == b'"' || ch == b'\\' {
                // Input was verified as UTF-8 before lexical scanning.
                let raw = std::str::from_utf8(&self.source[run..self.offset])
                    .expect("string run is UTF-8");
                result.push_str(raw);
                if ch == b'"' {
                    self.offset += 1;
                    return Ok(result);
                }
                self.offset += 1;
                let Some(escaped) = self.peek() else {
                    return Err(self.invalid("incomplete JSON escape"));
                };
                self.offset += 1;
                match escaped {
                    b'"' => result.push('"'),
                    b'\\' => result.push('\\'),
                    b'/' => result.push('/'),
                    b'b' => result.push('\u{0008}'),
                    b'f' => result.push('\u{000C}'),
                    b'n' => result.push('\n'),
                    b'r' => result.push('\r'),
                    b't' => result.push('\t'),
                    b'u' => {
                        let first = self.hex_quad()?;
                        let codepoint = if (0xD800..=0xDBFF).contains(&first) {
                            if !self.source[self.offset..].starts_with(b"\\u") {
                                return Err(self.invalid("unpaired high surrogate"));
                            }
                            self.offset += 2;
                            let second = self.hex_quad()?;
                            if !(0xDC00..=0xDFFF).contains(&second) {
                                return Err(self.invalid("invalid low surrogate"));
                            }
                            0x10000
                                + ((u32::from(first) - 0xD800) << 10)
                                + (u32::from(second) - 0xDC00)
                        } else if (0xDC00..=0xDFFF).contains(&first) {
                            return Err(self.invalid("unpaired low surrogate"));
                        } else {
                            u32::from(first)
                        };
                        let Some(ch) = char::from_u32(codepoint) else {
                            return Err(self.invalid("invalid Unicode scalar"));
                        };
                        result.push(ch);
                    }
                    _ => return Err(self.invalid("invalid JSON string escape")),
                }
                run = self.offset;
            } else if ch < 0x20 {
                return Err(self.invalid("unescaped control character"));
            } else {
                self.offset += 1;
            }
        }
    }
}

/// Emit the public C# `ProbeOperationResult[]` JSON property names. Raw JSON
/// payloads are retained instead of being rounded through `f64`.
pub fn operation_results_json(results: &[ProbeOperationResult]) -> String {
    let mut output = String::from("[");
    for (index, result) in results.iter().enumerate() {
        if index != 0 {
            output.push(',');
        }
        output.push_str("{\"Operation\":");
        escape_json(&mut output, &result.operation);
        output.push_str(&format!(
            ",\"Compared\":{},\"Unsupported\":{},\"OneSided\":{},\"Different\":{},\"FirstDifference\":",
            result.compared, result.unsupported, result.one_sided, result.different
        ));
        if let Some(finding) = &result.first_difference {
            append_difference(&mut output, finding);
        } else {
            output.push_str("null");
        }
        output.push('}');
    }
    output.push(']');
    output
}

/// Emit the public C# `ProbeCheckResult` JSON property names.
pub fn check_result_json(result: &ProbeCheckResult) -> String {
    let mut output = format!(
        "{{\"Cases\":{},\"Compared\":{},\"Unsupported\":{},\"Findings\":[",
        result.cases, result.compared, result.unsupported
    );
    for (index, finding) in result.findings.iter().enumerate() {
        if index != 0 {
            output.push(',');
        }
        append_difference(&mut output, finding);
    }
    output.push_str("]}");
    output
}

fn append_difference(output: &mut String, value: &ProbeDifference) {
    output.push_str(&format!(
        "{{\"Index\":{},\"Input\":{{\"Operation\":",
        value.index
    ));
    escape_json(output, &value.input.operation);
    output.push_str(",\"Args\":[");
    for (index, arg) in value.input.args.iter().enumerate() {
        if index != 0 {
            output.push(',');
        }
        output.push_str(&arg.to_string());
    }
    output.push_str("]},\"Candidate\":");
    output.push_str(value.candidate.raw_json());
    output.push_str(",\"Reference\":");
    output.push_str(value.reference.raw_json());
    output.push('}');
}

fn escape_json(output: &mut String, value: &str) {
    output.push('"');
    for ch in value.chars() {
        match ch {
            '"' => output.push_str("\\\""),
            '\\' => output.push_str("\\\\"),
            '\n' => output.push_str("\\n"),
            '\r' => output.push_str("\\r"),
            '\t' => output.push_str("\\t"),
            '\u{0008}' => output.push_str("\\b"),
            '\u{000C}' => output.push_str("\\f"),
            c if c <= '\u{001F}' => output.push_str(&format!("\\u{:04x}", u32::from(c))),
            c => output.push(c),
        }
    }
    output.push('"');
}
