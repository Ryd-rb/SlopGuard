//! Passive, bounded managed-PE metadata and IL comparison.
//!
//! This standalone migration checker never executes or reflection-loads an
//! assembly and does not change SlopHawk's existing C# CLI routing.
//! C# reference: ManagedBodyComparison at upstream a367ef58076f7e29d2ae475a3b593daad04c4998.
mod budget;
mod il;
mod metadata;

use budget::{Budget, JsonOut};
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::fmt;
use std::fs::{self, File};
use std::io::Read;
use std::path::Path;

pub use metadata::MAX_ASSEMBLY_BYTES;

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum ErrorKind { NonManaged, Malformed, Unsupported, Limit, Io }
impl ErrorKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::NonManaged => "non-managed",
            Self::Malformed => "malformed",
            Self::Unsupported => "unsupported",
            Self::Limit => "limit",
            Self::Io => "io",
        }
    }
}
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct CheckError { pub kind: ErrorKind, pub message: String }
impl CheckError {
    pub(crate) fn new(kind: ErrorKind, reason: impl AsRef<str>) -> Self {
        // Diagnostics never echo unbounded untrusted input.
        let message: String = reason.as_ref().chars().take(256).collect();
        Self { kind, message }
    }
}
impl fmt::Display for CheckError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.kind.as_str(), self.message)
    }
}
impl std::error::Error for CheckError {}
pub type Result<T> = std::result::Result<T, CheckError>;

const MAX_METHODS: usize = 100_000;
const MAX_REQUEST_OPERATIONS: usize = 4_096;
const MAX_REQUEST_BYTES: usize = 256 * 1024;
const MAX_ACCUMULATED_INSTRUCTIONS: usize = 250_000;

fn limited(reason: &'static str) -> CheckError { CheckError::new(ErrorKind::Limit, reason) }

pub fn read_assembly(path: &Path) -> Result<Vec<u8>> {
    let metadata = fs::metadata(path).map_err(|error| CheckError::new(ErrorKind::Io, error.to_string()))?;
    if !metadata.is_file() || metadata.len() > MAX_ASSEMBLY_BYTES as u64 {
        return Err(limited("input is not a bounded ordinary file"));
    }
    let mut contents = Vec::new();
    contents.try_reserve_exact(metadata.len() as usize)
        .map_err(|_| limited("input buffer allocation failed"))?;
    File::open(path).map_err(|error| CheckError::new(ErrorKind::Io, error.to_string()))?
        .take(MAX_ASSEMBLY_BYTES as u64 + 1).read_to_end(&mut contents)
        .map_err(|error| CheckError::new(ErrorKind::Io, error.to_string()))?;
    if contents.len() > MAX_ASSEMBLY_BYTES {
        return Err(limited("input grew beyond size cap"));
    }
    Ok(contents)
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ManagedBodyDifference {
    pub method: String,
    pub name: String,
    pub change: &'static str,
    pub common_prefix_count: usize,
    pub candidate: Vec<String>,
    pub reference: Vec<String>,
    pub integer_literals: Vec<i64>,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LiteralEntry { pub operation: String, pub literals: Vec<i64> }
struct MethodBody {
    name: String,
    instructions: Vec<String>,
    literals: Vec<i64>,
    arity: Option<usize>,
    order: u32,
}

/// Diff by fully qualified type, overload signature, and normalized IL body.
/// Candidate appears on the left and reference on the right (C# convention).
/// The input slices are read-only; neither assembly is loaded or executed.
pub fn compare(candidate: &[u8], reference: &[u8]) -> Result<Vec<ManagedBodyDifference>> {
    let budget = Budget::new();
    let left = read(candidate, None, None, &budget)?;
    let right = read(reference, None, None, &budget)?;
    let mut keys = budget.vec::<&String>(left.len().saturating_add(right.len()))?;
    keys.extend(left.keys().chain(right.keys()));
    // .NET's StringComparer.Ordinal orders UTF-16 code units, not UTF-8 bytes.
    keys.sort_unstable_by(|a, b| a.encode_utf16().cmp(b.encode_utf16()));
    keys.dedup();
    let mut differences = Vec::new();
    for method in keys {
        let l = left.get(method);
        let r = right.get(method);
        let before: &[String] = r.map_or(&[], |x| x.instructions.as_slice());
        let after: &[String] = l.map_or(&[], |x| x.instructions.as_slice());
        if l.is_some() && r.is_some() && after == before { continue; }
        let prefix = before.iter().zip(after).take_while(|(a, b)| a == b).count();
        let suffix = before.iter().rev().zip(after.iter().rev())
            .take(before.len().min(after.len()).saturating_sub(prefix))
            .take_while(|(a, b)| a == b).count();
        let count = l.map_or(0, |m| m.literals.len()).saturating_add(r.map_or(0, |m| m.literals.len()));
        let mut literals = budget.vec::<i64>(count)?;
        literals.extend(l.into_iter().flat_map(|x| x.literals.iter())
            .chain(r.into_iter().flat_map(|x| x.literals.iter())).copied());
        literals.sort_unstable();
        literals.dedup();
        budget.charge_array::<ManagedBodyDifference>(2)?;
        differences.try_reserve(1).map_err(|_| limited("differences list allocation failed"))?;
        differences.push(ManagedBodyDifference {
            method: budget.owned(method)?,
            name: budget.owned(&l.or(r).expect("key from method maps").name)?,
            change: if l.is_none() { "removed" } else if r.is_none() { "added" } else { "changed" },
            common_prefix_count: prefix,
            candidate: excerpt(after, prefix, suffix, &budget)?,
            reference: excerpt(before, prefix, suffix, &budget)?,
            integer_literals: literals,
        });
    }
    Ok(differences)
}
fn excerpt(body: &[String], prefix: usize, suffix: usize, budget: &Budget) -> Result<Vec<String>> {
    let start = prefix.saturating_sub(2);
    let end = body.len().min(body.len().saturating_sub(suffix).saturating_add(2));
    let count = end.saturating_sub(start).min(24);
    let mut result = budget.vec::<String>(count)?;
    for value in body[start..end].iter().take(24) { result.push(budget.owned(value)?); }
    Ok(result)
}

/// Equivalent to `IntegerLiterals(Stream, ISet<string>)`. Compiler-generated
/// local functions/lambdas (`<Foo>g__`, `<Foo>b__`) belong to `Foo`.
pub fn integer_literals_for_names(assembly: &[u8], requested: &[String]) -> Result<Vec<LiteralEntry>> {
    let budget = Budget::new();
    let names = requests(requested.iter().map(String::as_str), &budget)?;
    let methods = read(assembly, Some(&names), None, &budget)?;
    let mut bodies = budget.vec::<&MethodBody>(methods.len())?;
    bodies.extend(methods.values());
    bodies.sort_unstable_by_key(|body| body.order);
    let mut grouped: Vec<(String, BTreeSet<i64>)> = Vec::new();
    let mut indexes: HashMap<&str, usize> = HashMap::new();
    for body in bodies {
        let idx = if let Some(&index) = indexes.get(body.name.as_str()) { index } else {
            budget.map_entries::<&str, usize>(1)?;
            indexes.try_reserve(1).map_err(|_| limited("literal index allocation failed"))?;
            budget.charge_array::<(String, BTreeSet<i64>)>(2)?;
            grouped.try_reserve(1).map_err(|_| limited("literal grouping allocation failed"))?;
            grouped.push((budget.owned(&body.name)?, BTreeSet::new()));
            let index = grouped.len() - 1;
            indexes.insert(body.name.as_str(), index);
            index
        };
        for &value in &body.literals {
            if !grouped[idx].1.contains(&value) {
                budget.map_entries::<i64, ()>(1)?;
                grouped[idx].1.insert(value);
            }
        }
    }
    let mut result = budget.vec::<LiteralEntry>(grouped.len())?;
    for (operation, values) in grouped {
        let mut literals = budget.vec::<i64>(values.len())?;
        literals.extend(values);
        result.push(LiteralEntry { operation, literals });
    }
    Ok(result)
}

/// Equivalent to the arity-guided overload used by C# `ContractCheck`.
/// Entries are operation names and numeric parameter arities in caller order.
pub fn integer_literals_for_operations(assembly: &[u8], operations: &[(String, usize)]) -> Result<Vec<LiteralEntry>> {
    let budget = Budget::new();
    let names = requests(operations.iter().map(|(name, _)| name.as_str()), &budget)?;
    if names.len() != operations.len() {
        return Err(CheckError::new(ErrorKind::Malformed, "duplicate contract operation name"));
    }
    let mut arities = HashSet::new();
    budget.map_entries::<usize, ()>(operations.len())?;
    arities.try_reserve(operations.len()).map_err(|_| limited("operation arity index allocation failed"))?;
    for (_, arity) in operations { arities.insert(*arity); }
    let methods = read(assembly, Some(&names), Some(&arities), &budget)?;
    let mut by_arity: HashMap<usize, HashMap<i64, usize>> = HashMap::new();
    let mut direct: HashMap<&str, BTreeSet<i64>> = HashMap::new();
    for body in methods.values() {
        if names.contains(body.name.as_str()) {
            if !direct.contains_key(body.name.as_str()) {
                budget.map_entries::<&str, BTreeSet<i64>>(1)?;
                direct.try_reserve(1).map_err(|_| limited("direct literal index allocation failed"))?;
            }
            let values = direct.entry(&body.name).or_default();
            for &value in &body.literals {
                if let Some(value) = useful(value) {
                    if !values.contains(&value) {
                        budget.map_entries::<i64, ()>(1)?;
                        values.insert(value);
                    }
                }
            }
        }
        if let Some(arity) = body.arity {
            if !by_arity.contains_key(&arity) {
                budget.map_entries::<usize, HashMap<i64, usize>>(1)?;
                by_arity.try_reserve(1).map_err(|_| limited("arity histogram allocation failed"))?;
            }
            let votes = by_arity.entry(arity).or_default();
            let mut unique = HashSet::new();
            for &value in &body.literals {
                if let Some(value) = useful(value) {
                    if !unique.contains(&value) {
                        budget.map_entries::<i64, ()>(1)?;
                        unique.try_reserve(1).map_err(|_| limited("literal uniqueness set allocation failed"))?;
                        unique.insert(value);
                        if !votes.contains_key(&value) {
                            budget.map_entries::<i64, usize>(1)?;
                            votes.try_reserve(1).map_err(|_| limited("literal vote allocation failed"))?;
                        }
                        *votes.entry(value).or_default() += 1;
                    }
                }
            }
        }
    }
    let mut fallbacks = HashMap::new();
    for (arity, votes) in by_arity {
        let mut values = budget.vec::<(i64, usize)>(votes.len())?;
        values.extend(votes);
        values.sort_by(|(v, n), (w, m)| n.cmp(m).then_with(|| w.cmp(v)));
        let mut selected = budget.vec::<i64>(values.len().min(16))?;
        selected.extend(values.into_iter().take(16).map(|(v, _)| v));
        budget.map_entries::<usize, Vec<i64>>(1)?;
        fallbacks.try_reserve(1).map_err(|_| limited("literal fallback allocation failed"))?;
        fallbacks.insert(arity, selected);
    }
    let mut result = budget.vec::<LiteralEntry>(operations.len())?;
    for (operation, arity) in operations {
        let values = if let Some(direct) = direct.get(operation.as_str()).filter(|v| !v.is_empty()) {
            let mut result = budget.vec::<i64>(direct.len())?;
            result.extend(direct.iter().rev().copied());
            result
        } else {
            let fallback = fallbacks.get(arity).map(Vec::as_slice).unwrap_or(&[]);
            let mut result = budget.vec::<i64>(fallback.len())?;
            result.extend_from_slice(fallback);
            result
        };
        result.push(LiteralEntry { operation: budget.owned(operation)?, literals: values });
    }
    Ok(result)
}
fn useful(value: i64) -> Option<i64> {
    if value == i64::MIN || (-1..=1).contains(&value) { None } else { Some(value.abs()) }
}
fn requests<'a>(entries: impl Iterator<Item = &'a str>, budget: &Budget) -> Result<HashSet<&'a str>> {
    let mut names = HashSet::new();
    let mut total = 0usize;
    for entry in entries {
        if names.len() >= MAX_REQUEST_OPERATIONS { return Err(limited("too many requested operations")); }
        total = total.checked_add(entry.len()).ok_or_else(|| limited("request text length overflow"))?;
        if total > MAX_REQUEST_BYTES { return Err(limited("requested operation names exceed cap")); }
        if !names.contains(entry) {
            budget.map_entries::<&str, ()>(1)?;
            names.try_reserve(1).map_err(|_| limited("operation name set allocation failed"))?;
            names.insert(entry);
        }
    }
    Ok(names)
}
fn requested_operation<'a>(name: &'a str, names: &HashSet<&str>) -> Option<&'a str> {
    if names.contains(name) { return Some(name); }
    let stem = name.strip_prefix('<')?;
    let (parent, suffix) = stem.split_once('>')?;
    (suffix.starts_with("g__") || suffix.starts_with("b__"))
        .then_some(parent).filter(|p| names.contains(*p))
}
fn read(bytes: &[u8], requested: Option<&HashSet<&str>>, arities: Option<&HashSet<usize>>,
        budget: &Budget) -> Result<BTreeMap<String, MethodBody>> {
    let pe = metadata::Metadata::parse(bytes)?;
    let count = pe.count(6) as usize;
    if count > MAX_METHODS { return Err(limited("method count exceeds cap")); }
    let mut names = metadata::Names::new(&pe, budget)?;
    let mut methods = BTreeMap::new();
    let mut instruction_count = 0usize;
    for row in 1..=count as u32 {
        let def = pe.row(6, row)?;
        let rva = def[0];
        let impl_flags = def[1];
        if rva == 0 || (impl_flags & 3) != 0 { continue; }
        let name = pe.string(def[3])?;
        let operation = requested.map_or(Some(name), |selected| requested_operation(name, selected));
        let arity = if let Some(target) = arities {
            let sig = names.method_signature(def[4])?;
            (target.contains(&sig.params.len()) && sig.return_integral && sig.params_integral)
                .then_some(sig.params.len())
        } else { None };
        if operation.is_none() && arity.is_none() { continue; }
        let parsed = il::body(&pe, &mut names, rva, budget)?;
        instruction_count = instruction_count.checked_add(parsed.instructions.len())
            .ok_or_else(|| limited("normalized IL instruction count overflow"))?;
        if instruction_count > MAX_ACCUMULATED_INSTRUCTIONS { return Err(limited("aggregate normalized IL exceeds cap")); }
        let symbol = names.entity((6u32 << 24) | row)?;
        if methods.contains_key(&symbol) {
            return Err(CheckError::new(ErrorKind::Malformed, "duplicate managed method identity"));
        }
        budget.charge(std::mem::size_of::<(String, MethodBody)>() + 128)?;
        methods.insert(symbol, MethodBody {
            name: budget.owned(operation.unwrap_or(name))?,
            instructions: parsed.instructions,
            literals: parsed.literals,
            arity, order: row,
        });
    }
    Ok(methods)
}

/// JSON matches the C# record's property names, order, and default encoding.
/// Every escape and append is size-checked before allocating the result text.
pub fn differences_json(differences: &[ManagedBodyDifference]) -> Result<String> {
    let mut output = JsonOut::new();
    output.push("[")?;
    for (i, item) in differences.iter().enumerate() {
        if i != 0 { output.push(",")?; }
        output.push("{\"Method\":")?; output.quoted(&item.method)?;
        output.push(",\"Name\":")?; output.quoted(&item.name)?;
        output.push(",\"Change\":")?; output.quoted(item.change)?;
        output.push(",\"CommonPrefixCount\":")?; output.number(item.common_prefix_count)?;
        output.push(",\"Candidate\":")?; json_array(&mut output, &item.candidate)?;
        output.push(",\"Reference\":")?; json_array(&mut output, &item.reference)?;
        output.push(",\"IntegerLiterals\":[")?;
        for (j, value) in item.integer_literals.iter().enumerate() {
            if j != 0 { output.push(",")?; }
            output.number(value)?;
        }
        output.push("]}")?;
    }
    output.push("]")?;
    Ok(output.finish())
}
pub fn literals_json(entries: &[LiteralEntry]) -> Result<String> {
    let mut output = JsonOut::new();
    output.push("{")?;
    for (i, item) in entries.iter().enumerate() {
        if i != 0 { output.push(",")?; }
        output.quoted(&item.operation)?;
        output.push(":[")?;
        for (j, value) in item.literals.iter().enumerate() {
            if j != 0 { output.push(",")?; }
            output.number(value)?;
        }
        output.push("]")?;
    }
    output.push("}")?;
    Ok(output.finish())
}
fn json_array(output: &mut JsonOut, values: &[String]) -> Result<()> {
    output.push("[")?;
    for (i, value) in values.iter().enumerate() {
        if i != 0 { output.push(",")?; }
        output.quoted(value)?;
    }
    output.push("]")
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn operation_names_and_useful_literals() {
        let requested = HashSet::from(["Add", "Divide"]);
        assert_eq!(requested_operation("<Add>g__check|12_0", &requested), Some("Add"));
        assert_eq!(requested_operation("<Add>b__1_0", &requested), Some("Add"));
        assert_eq!(requested_operation("<Add>g__not", &HashSet::from(["Other"])), None);
        assert_eq!(useful(i64::MIN), None);
        assert_eq!(useful(-123), Some(123));
    }
    #[test]
    fn output_json_has_csharp_casing_and_default_escaping() {
        let result = ManagedBodyDifference {
            method: "T::M():Int32[00:0:0]".into(), name: "M".into(), change: "changed",
            common_prefix_count: 1, candidate: vec!["ldstr \"\\u003C\\u002B\\u0026\"".into()],
            reference: vec!["ldc.i4.s 3".into()], integer_literals: vec![-3, 3],
        };
        assert_eq!(differences_json(&[result]).unwrap(),
            r#"[{"Method":"T::M():Int32[00:0:0]","Name":"M","Change":"changed","CommonPrefixCount":1,"Candidate":["ldstr \u0022\\u003C\\u002B\\u0026\u0022"],"Reference":["ldc.i4.s 3"],"IntegerLiterals":[-3,3]}]"#);
    }
    #[test]
    fn non_managed_file_is_distinct_from_malformed_metadata() {
        let err = compare(b"just text", b"just text").unwrap_err();
        assert_eq!(err.kind, ErrorKind::NonManaged);
    }
    #[test]
    fn input_request_budget_precedes_collection_growth() {
        let long = vec!["m".repeat(MAX_REQUEST_BYTES / 2); 3];
        assert_eq!(requests(long.iter().map(String::as_str), &Budget::new()).unwrap_err().kind, ErrorKind::Limit);
    }
}
