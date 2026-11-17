//! Public contract and probe commands that need no executable adapter.
use crate::{Result, SOURCE_LIMIT, inputs_json, qualifications_json, qualify, read_scales};
use slophawk_contracts::plan_inputs;
use slophawk_managed_methods::{compare, differences_json, read_assembly};
use slophawk_probe_replay::{compare_sources, operation_results_json, parse_inputs};
use std::fs::File;
use std::io::Read;
use std::path::Path;

pub fn run(args: &[String]) -> Result<(String, u8)> {
    let parts = args.iter().map(String::as_str).collect::<Vec<_>>();
    match parts.as_slice() {
        ["contracts", "qualify", rules, published] => {
            let catalog = qualify(&read_limited(rules)?, &read_limited(published)?)?;
            let rows = catalog.qualification();
            Ok((
                qualifications_json(rows).to_string(),
                u8::from(rows.iter().any(|row| row.error.is_some())),
            ))
        }
        ["probes", "plan", seeds, count, rest @ ..] if rest.len() <= 2 => {
            let count = positive_count(count)?;
            let seed = rest
                .first()
                .map_or(Ok(0), |s| s.trim().parse::<i32>())
                .map_err(|_| "The optional random seed must be a signed 32-bit integer.")?;
            let scales = match rest.get(1) {
                Some(path) => read_scales(&read_limited(path)?)?,
                None => Vec::new(),
            };
            let inputs = parse_inputs(&read_limited(seeds)?).map_err(|error| error.to_string())?;
            let result =
                plan_inputs(&inputs, count, seed, &scales).map_err(|error| error.to_string())?;
            Ok((inputs_json(&result)?, 0))
        }
        ["probes", "compare", inputs, candidate, reference] => {
            let result = compare_sources(
                &read_limited(inputs)?,
                &read_limited(candidate)?,
                &read_limited(reference)?,
            )
            .map_err(|error| error.to_string())?;
            Ok((operation_results_json(&result), 0))
        }
        ["probes", "methods", candidate, reference] => {
            let read =
                |path: &str| read_assembly(Path::new(path)).map_err(|error| error.to_string());
            let result =
                compare(&read(candidate)?, &read(reference)?).map_err(|error| error.to_string())?;
            Ok((
                differences_json(&result).map_err(|error| error.to_string())?,
                0,
            ))
        }
        _ => Err("Expected contracts qualify or probes plan, compare or methods.".into()),
    }
}

pub fn positive_count(text: &str) -> Result<usize> {
    if text.is_empty() || !text.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err("The probe count must be a positive integer.".into());
    }
    text.parse::<i32>()
        .ok()
        .filter(|count| *count > 0)
        .map(|count| count as usize)
        .ok_or_else(|| "The probe count must be a positive 32-bit integer.".into())
}

pub fn read_limited(path: impl AsRef<Path>) -> Result<Vec<u8>> {
    let path = path.as_ref();
    let file = File::open(path).map_err(|error| format!("{}: {error}", path.display()))?;
    let metadata = file
        .metadata()
        .map_err(|error| format!("{}: {error}", path.display()))?;
    if !metadata.is_file() || metadata.len() > SOURCE_LIMIT as u64 {
        return Err(format!("{} is not a bounded input file", path.display()));
    }
    let mut bytes = Vec::with_capacity(metadata.len() as usize);
    file.take(SOURCE_LIMIT as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| format!("{}: {error}", path.display()))?;
    if bytes.len() > SOURCE_LIMIT {
        return Err("File grew beyond the input budget.".into());
    }
    Ok(bytes)
}
