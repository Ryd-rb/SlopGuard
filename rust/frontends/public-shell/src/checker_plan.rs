//! Explicit, read-only planning over supplied evidence.
use crate::{Result, failure};
use serde::Deserialize;
use serde_json::{Value, json};
use std::{fs::File, io::Read, path::Path};

const INPUT_BYTES: u64 = 256 * 1024;
const OUTPUT_BYTES: usize = 256 * 1024;

#[derive(Deserialize)]
#[serde(tag = "checker", rename_all = "snake_case", deny_unknown_fields)]
enum Request {
    ScalarCorrelated {
        facts: String,
    },
    ContractDiversity {
        rules: Vec<Value>,
        published: Vec<Value>,
        pool: Vec<Value>,
        #[serde(rename = "perOperation")]
        per_operation: usize,
    },
}

pub fn schema() -> Value {
    json!({"type": "object", "oneOf": [
        {"type": "object", "additionalProperties": false, "required": ["checker", "facts"],
         "properties": {"checker": {"const": "scalar_correlated"}, "facts": {"type": "string"}}},
        {"type": "object", "additionalProperties": false,
         "required": ["checker", "rules", "published", "pool", "perOperation"],
         "properties": {
            "checker": {"const": "contract_diversity"},
            "rules": {"type": "array", "items": {"type": "object"}, "maxItems": 16},
            "published": {"type": "array", "items": {"type": "object"}, "maxItems": 256},
            "pool": {"type": "array", "items": {"type": "object"}, "maxItems": 1024},
            "perOperation": {"type": "integer", "minimum": 1, "maximum": 128}
        }}
    ]})
}

pub fn read(path: &Path) -> Result<Value> {
    let metadata = std::fs::symlink_metadata(path).map_err(|e| failure("checker_plan_input", e))?;
    if !metadata.is_file() || metadata.len() > INPUT_BYTES {
        return Err(failure(
            "checker_plan_input",
            "Expected an ordinary input file of at most 256 KiB.",
        ));
    }
    let mut bytes = Vec::new();
    File::open(path)
        .and_then(|file| file.take(INPUT_BYTES + 1).read_to_end(&mut bytes))
        .map_err(|e| failure("checker_plan_input", e))?;
    if bytes.len() as u64 > INPUT_BYTES {
        return Err(failure(
            "checker_plan_input",
            "Checker plan input exceeds 256 KiB.",
        ));
    }
    evaluate(&serde_json::from_slice(&bytes).map_err(|e| failure("checker_plan_input", e))?)
}

pub fn evaluate(input: &Value) -> Result<Value> {
    let bytes = serde_json::to_vec(input).map_err(|e| failure("checker_plan_input", e))?;
    if bytes.len() as u64 > INPUT_BYTES {
        return Err(failure(
            "checker_plan_input",
            "Checker plan input exceeds 256 KiB.",
        ));
    }
    let input: Request =
        serde_json::from_slice(&bytes).map_err(|e| failure("checker_plan_input", e))?;
    let (checker, policy, result) = match input {
        Request::ScalarCorrelated { facts } => {
            let plan = slophawk_scalar_paths::plan_correlations(&facts)
                .map_err(|e| failure("checker_plan_input", e))?;
            let candidates: Vec<_> = plan.candidates.into_iter().map(|candidate| {
                json!({"args": candidate.args, "predictedEdge": candidate.predicted_goal})
            }).collect();
            (
                "scalar_correlated",
                slophawk_scalar_foreground::CORRELATED_MODE,
                json!({"bound": plan.bound, "work": plan.work, "candidates": candidates,
                       "unknown": plan.unknown}),
            )
        }
        Request::ContractDiversity {
            rules,
            published,
            pool,
            per_operation,
        } => {
            if rules.len() > 16
                || published.len() > 256
                || pool.len() > 1024
                || !(1..=128).contains(&per_operation)
            {
                return Err(failure(
                    "checker_plan_input",
                    "Interactive contract limits exceeded.",
                ));
            }
            let encode = |value: &[Value]| {
                serde_json::to_vec(value).map_err(|e| failure("checker_plan_input", e))
            };
            let plan = slophawk_contracts_probes::diversify_cases(
                &encode(&rules)?,
                &encode(&published)?,
                &encode(&pool)?,
                per_operation,
            )
            .map_err(|e| failure("checker_plan_input", e))?;
            ("contract_diversity", "diverse-v1", plan)
        }
    };
    let output = json!({
        "schema": "slophawk.checker-plan/1", "checker": checker, "policy": policy,
        "evidence": "caller_supplied", "result": result,
    });
    if serde_json::to_vec(&output)
        .map_err(|e| failure("checker_plan_output", e))?
        .len()
        > OUTPUT_BYTES
    {
        return Err(failure(
            "checker_plan_output",
            "Checker plan output exceeds 256 KiB.",
        ));
    }
    Ok(output)
}
