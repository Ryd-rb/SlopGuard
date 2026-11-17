//! Pinned SlopHawk integer-contract qualification, evaluation and pure probe planning.
//!
//! The library owns no target process, IL scanning, mutable global state, CLI
//! or publication. Callers supply ordered typed public examples, authored rules,
//! optional seed and optional implementation literals. All output order is stable.
//! Limits return a `Scope` error rather than silently sampling or truncating.
mod cases;
mod expression;
mod integer;
mod plan;
mod rng;

pub use cases::ContractCases;
pub use plan::{
    MAX_ARITY, MAX_OPERATION_LIMIT, MAX_SCALE_COUNT, MAX_SEEDS_PER_OPERATION, MAX_TOTAL_PLAN,
    plan_inputs,
};
pub use slophawk_probe_replay::{ProbeExpectation, ProbeInput};

#[derive(Clone, Debug)]
pub struct IntegerContract {
    pub operation: String,
    pub parameters: Vec<String>,
    pub domain: String,
    pub expected: String,
    pub source: String,
    /// Ordered locals: names can refer to parameters and previously declared locals.
    pub lets: Vec<(String, String)>,
    /// Optional authored inputs per parameter, in authored order.
    pub basis: Vec<(String, Vec<i64>)>,
}

impl IntegerContract {
    pub fn new(
        operation: &str,
        parameters: &[&str],
        domain: &str,
        expected: &str,
        source: &str,
    ) -> Self {
        Self {
            operation: operation.into(),
            parameters: parameters.iter().map(|s| (*s).into()).collect(),
            domain: domain.into(),
            expected: expected.into(),
            source: source.into(),
            lets: Vec::new(),
            basis: Vec::new(),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ContractQualification {
    pub operation: String,
    pub source: Option<String>,
    pub error: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ContractError {
    /// Invalid globally supplied typed inputs or unsupported options.
    Invalid(String),
    /// A documented deterministic memory/work envelope was exceeded.
    Scope(String),
}

impl std::fmt::Display for ContractError {
    fn fmt(&self, writer: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Invalid(message) => write!(writer, "invalid contract input: {message}"),
            Self::Scope(message) => write!(writer, "contract scope exceeded: {message}"),
        }
    }
}
impl std::error::Error for ContractError {}
