//! Public v1 foreground evidence records. `RawValue` prevents silently rounding
//! candidate/expected JSON numbers or dropping duplicate JSON object keys.
use serde::{Deserialize, Serialize};
use serde_json::value::RawValue;
use slophawk_probe_replay::{JsonValue, ProbeDifference, ProbeInput};

pub(crate) const CHECK_FORMAT: &str = "slophawk-check-v1";
pub(crate) const REVIEW_FORMAT: &str = "slophawk-check-review-v1";
pub(crate) const IDENTITY_FORMAT: &str = "slophawk-check-identity-v1";

#[derive(Clone, Debug, Serialize, Deserialize, Eq, PartialEq, Hash)]
#[serde(rename_all = "PascalCase")]
pub(crate) struct Input {
    #[serde(alias = "operation")]
    pub operation: String,
    #[serde(alias = "args")]
    pub args: Vec<i64>,
}

impl From<&ProbeInput> for Input {
    fn from(value: &ProbeInput) -> Self {
        Self { operation: value.operation.clone(), args: value.args.clone() }
    }
}
impl From<&Input> for ProbeInput {
    fn from(value: &Input) -> Self {
        Self { operation: value.operation.clone(), args: value.args.clone() }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub(crate) struct CheckRun {
    pub format: String,
    pub run: String,
    pub candidate: String,
    pub run_verb: Option<String>,
    pub prepare_verb: Option<String>,
    pub result: Box<RawValue>,
    pub expected: Option<Vec<Box<RawValue>>>,
}

#[derive(Serialize)]
#[serde(rename_all = "PascalCase")]
pub(crate) struct CheckReceipt {
    pub format: String,
    pub run: String,
    pub report: String,
    pub artifacts: String,
    pub observe_only: bool,
    pub result: Box<RawValue>,
}

#[derive(Clone, Debug, Serialize, Deserialize, Eq, PartialEq, Hash)]
#[serde(rename_all = "PascalCase")]
pub(crate) struct Adapter {
    pub candidate: String,
    pub run_verb: Option<String>,
    pub prepare_verb: Option<String>,
}
impl From<&CheckRun> for Adapter {
    fn from(v: &CheckRun) -> Self {
        Self { candidate: v.candidate.clone(), run_verb: v.run_verb.clone(), prepare_verb: v.prepare_verb.clone() }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub(crate) struct Acknowledgment {
    #[serde(alias = "run")]
    pub run: String,
    #[serde(alias = "index")]
    pub index: usize,
    #[serde(alias = "retestRun")]
    pub retest_run: String,
    #[serde(alias = "retestIndex")]
    pub retest_index: usize,
    #[serde(alias = "disposition")]
    pub disposition: String,
    #[serde(alias = "reason")]
    pub reason: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub(crate) struct Decision {
    #[serde(alias = "index")]
    pub index: usize,
    #[serde(alias = "reason")]
    pub reason: String,
    #[serde(default, alias = "expected")]
    pub expected: Option<Box<RawValue>>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "PascalCase")]
pub(crate) struct Source {
    #[serde(alias = "operation")]
    pub operation: String,
    #[serde(alias = "component")]
    pub component: String,
    #[serde(alias = "contract")]
    pub contract: String,
    #[serde(alias = "implementation")]
    pub implementation: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub(crate) struct Scope {
    #[serde(alias = "operations")]
    pub operations: Vec<String>,
    #[serde(alias = "ordinary")]
    pub ordinary: Vec<Input>,
    #[serde(default = "one", alias = "minimumOutsideOrdinary")]
    pub minimum_outside_ordinary: usize,
    #[serde(default, alias = "sources")]
    pub sources: Option<Vec<Source>>,
}
fn one() -> usize { 1 }

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub(crate) struct Identity {
    pub format: String,
    pub run: String,
    pub root: String,
    pub mode: String,
    pub input_sha256: String,
    pub requests_sha256: String,
    pub prepared_sha256: Option<String>,
    pub observed_sha256: String,
    pub candidate: String,
    pub run_verb: Option<String>,
    pub prepare_verb: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "PascalCase")]
pub(crate) struct Difference {
    pub index: usize,
    pub input: Input,
    pub candidate: Box<RawValue>,
    pub reference: Box<RawValue>,
}
impl From<&ProbeDifference> for Difference {
    fn from(v: &ProbeDifference) -> Self {
        Self {
            index: v.index,
            input: Input::from(&v.input),
            candidate: raw(v.candidate.raw_json()),
            reference: raw(v.reference.raw_json()),
        }
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "PascalCase")]
pub(crate) struct Finding {
    pub run: String,
    pub difference: Difference,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "PascalCase")]
pub(crate) struct Observation {
    pub run: String,
    pub observed: Box<RawValue>,
}

pub(crate) fn raw(source: &str) -> Box<RawValue> {
    // Callers pass only JSON values verified by the lossless replay parser or
    // by serde_json before this point.
    RawValue::from_string(source.to_owned()).expect("validated raw JSON")
}

pub(crate) fn equal(a: &JsonValue, b: &JsonValue) -> Result<bool, String> {
    a.deep_equals(b).map_err(|e| e.to_string())
}

pub(crate) fn input_valid(input: &Input) -> bool {
    !input.operation.trim().is_empty() && !input.operation.chars().all(char::is_whitespace)
}

pub(crate) fn same_input(a: &ProbeInput, b: &ProbeInput) -> bool {
    a.operation == b.operation && a.args == b.args
}
