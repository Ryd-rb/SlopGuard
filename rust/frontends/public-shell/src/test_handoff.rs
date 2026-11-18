//! Projection of admitted native test facts into the schema-v5 writer snapshot.
use super::{Cell, Snapshot, hash, hex, invalid};
use serde_json::Value;
use std::collections::BTreeSet;
use std::io;

const MAX_CAUSES: usize = 128;
const HISTORY: usize = 64;

fn number(cell: &Cell) -> io::Result<i64> {
    cell.value
        .as_deref()
        .filter(|_| cell.kind == "integer")
        .and_then(|value| value.parse().ok())
        .ok_or_else(|| invalid("invalid integer in test inventory"))
}

fn text(value: &str) -> Cell {
    Cell::text(value.to_owned())
}

fn null() -> Cell {
    Cell::nullable(String::new())
}

fn detail_id(row: &[Cell]) -> io::Result<Option<String>> {
    let paths = [&row[2].value, &row[4].value];
    if !paths.iter().any(|path| path.is_some())
        || !paths
            .iter()
            .filter_map(|path| path.as_deref())
            .all(|path| path.to_ascii_lowercase().ends_with(".cs"))
    {
        return Ok(None);
    }
    let key = row[1]
        .value
        .as_deref()
        .ok_or_else(|| invalid("test intent omitted its path key"))?;
    Ok(Some(hash(format!("{}:{key}", number(&row[0])?).as_bytes())))
}

fn fallback_id(row: &[Cell]) -> io::Result<String> {
    Ok(hash(format!("{}:", number(&row[2])?).as_bytes()))
}

/// Schema v5 tracks C# causes. Other providers can contribute history, but
/// their receipts cannot serve as evidence of an evaluated C# test target.
pub(super) fn causes(state: &Snapshot) -> io::Result<Vec<String>> {
    let mut selected = state.tables[4]
        .rows
        .iter()
        .map(|row| fallback_id(row))
        .collect::<io::Result<Vec<_>>>()?;
    for row in &state.tables[3].rows {
        if selected.len() == MAX_CAUSES {
            break;
        }
        if let Some(id) = detail_id(row)? {
            selected.push(id);
        }
    }
    selected.sort_unstable();
    Ok(selected)
}

fn batch_prefix(epoch: &str, batch: u64) -> io::Result<String> {
    if !hex(epoch, 32) || batch == 0 {
        return Err(invalid("invalid native test batch"));
    }
    Ok(format!("native/{epoch}/{batch}/"))
}

pub(super) fn needs_projection(state: &Snapshot, epoch: &str, batch: u64) -> io::Result<bool> {
    let prefix = batch_prefix(epoch, batch)?;
    Ok(!state.tables[5].rows.iter().any(|row| {
        row[7]
            .value
            .as_deref()
            .is_some_and(|key| key.starts_with(&prefix))
    }))
}

fn outcome(value: &Value) -> &'static str {
    match value.as_str() {
        Some("Passed" | "passed") => "passed",
        Some("Failed" | "failed") => "failed",
        _ => "blocked",
    }
}

fn guid(epoch: &str) -> String {
    format!(
        "{}-{}-{}-{}-{}",
        &epoch[..8],
        &epoch[8..12],
        &epoch[12..16],
        &epoch[16..20],
        &epoch[20..]
    )
}

fn field<'a>(value: &'a Value, name: &str, limit: usize) -> io::Result<&'a str> {
    value[name]
        .as_str()
        .filter(|value| {
            !value.is_empty() && value.len() <= limit && !value.chars().any(char::is_control)
        })
        .ok_or_else(|| invalid("invalid text in native test receipt"))
}

fn value_cell(value: &Value) -> io::Result<Cell> {
    match value {
        Value::Null => Ok(null()),
        Value::String(value)
            if !value.is_empty() && value.len() <= 160 && !value.chars().any(char::is_control) =>
        {
            Ok(text(value))
        }
        Value::Number(value) => value
            .as_i64()
            .map(Cell::integer)
            .ok_or_else(|| invalid("invalid number in native test receipt")),
        _ => Err(invalid("invalid native test receipt storage value")),
    }
}

fn named(value: &Value, names: &[&str]) -> io::Result<Cell> {
    value
        .as_u64()
        .and_then(|index| usize::try_from(index).ok())
        .and_then(|index| names.get(index))
        .map(|name| text(name))
        .ok_or_else(|| invalid("unknown native VSTest disposition"))
}

fn trx_row(
    state: &Snapshot,
    epoch: &str,
    batch: u64,
    revision: i64,
    id: i64,
    target: &str,
    receipt: &Value,
) -> io::Result<Vec<Cell>> {
    if receipt["revision"].as_i64() != Some(revision)
        || receipt["epoch"] != epoch
        || receipt["fenceSequence"].as_u64() != Some(batch)
        || receipt["rootIdentity"] != state.root_identity
        || receipt["target"] != target
        || !receipt["attemptId"]
            .as_u64()
            .is_some_and(|value| value != 0)
    {
        return Err(invalid(
            "native VSTest receipt no longer binds the inventory",
        ));
    }
    let mut row = vec![
        Cell::integer(id),
        Cell::integer(1),
        Cell::integer(revision),
        text(epoch),
    ];
    for name in [
        "fenceSequence",
        "gitFingerprint",
        "rootIdentity",
        "target",
        "inputFingerprint",
        "profileRevision",
        "leaseId",
        "buildDomainVersion",
        "buildSha256",
        "runnerSha256",
        "artifactSha256",
        "artifactBytes",
        "runId",
    ] {
        row.push(value_cell(&receipt[name])?);
    }
    row.push(named(
        &receipt["platform"],
        &["Passed", "Failed", "ZeroDiscovered", "AllSkipped"],
    )?);
    row.push(named(
        &receipt["runner"],
        &[
            "ZeroExit",
            "NonzeroExit",
            "Incomplete",
            "TimedOut",
            "Cancelled",
            "StartupOrBuildFailure",
        ],
    )?);
    row.push(named(
        &receipt["artifact"],
        &[
            "Complete",
            "Missing",
            "Oversized",
            "Malformed",
            "Inconsistent",
            "Stale",
        ],
    )?);
    row.push(named(
        &receipt["execution"],
        &[
            "Unknown",
            "Passed",
            "Failed",
            "ZeroDiscovered",
            "AllSkipped",
        ],
    )?);
    for name in ["total", "executed", "passed", "failed", "skipped", "other"] {
        row.push(value_cell(&receipt["counters"][name])?);
    }
    row.push(text(field(receipt, "reasonCode", 64)?));
    Ok(row)
}

pub(super) fn project(
    state: &mut Snapshot,
    epoch: &str,
    batch: u64,
    revision: i64,
    scope: &str,
    planned: &[String],
    batches: &[Value],
) -> io::Result<bool> {
    let prefix = batch_prefix(epoch, batch)?;
    if !needs_projection(state, epoch, batch)? {
        return Ok(false);
    }
    if revision < 0 || revision > state.revision || !hex(scope, 64) || planned != causes(state)? {
        return Err(invalid("native test batch differs from its pending causes"));
    }
    let epoch = guid(epoch);
    let mut max_id = state.tables[5]
        .rows
        .iter()
        .map(|row| number(&row[0]))
        .collect::<io::Result<Vec<_>>>()?
        .into_iter()
        .max()
        .unwrap_or(0);
    let mut attempts = Vec::new();
    let mut details = Vec::new();
    let mut resolved = false;
    let mut providers = BTreeSet::new();
    for provider in batches {
        let Some(receipts) = provider["receipts"]
            .as_array()
            .filter(|rows| !rows.is_empty())
        else {
            continue;
        };
        let name = field(provider, "provider", 8)?;
        if !matches!(name, "csharp" | "python" | "rust" | "fluxr")
            || receipts.len() > 16
            || !providers.insert(name)
        {
            return Err(invalid("unsupported native test provider batch"));
        }
        let mut targets = BTreeSet::new();
        let mut accounted = name == "csharp";
        for receipt in receipts {
            let target = field(receipt, "target", 4096)?;
            if !targets.insert(target) {
                return Err(invalid("duplicate native test target receipt"));
            }
            max_id = max_id
                .checked_add(1)
                .ok_or_else(|| invalid("test attempt sequence exhausted"))?;
            let disposition = if name == "fluxr" {
                &receipt["receipt"]["outcome"]
            } else {
                &receipt["outcome"]
            };
            let status = outcome(disposition);
            let attempt_key = format!("{prefix}{}", hash(format!("{name}\0{target}").as_bytes()));
            let (target, fingerprint, grant, lease, cause_json) = if name == "csharp" {
                let raw = &receipt["receipt"];
                let echoed = serde_json::from_value::<Vec<String>>(receipt["causeIds"].clone())
                    .map_err(io::Error::other)?;
                if echoed != planned {
                    return Err(invalid(
                        "native VSTest receipt echoed another set of causes",
                    ));
                }
                accounted &=
                    receipt["selectionComplete"] == true && matches!(status, "passed" | "failed");
                details.push(trx_row(state, &epoch, batch, revision, max_id, target, raw)?);
                (
                    text(field(receipt, "target", 128)?),
                    text(field(raw, "inputFingerprint", 128)?),
                    text(field(raw, "profileRevision", 128)?),
                    value_cell(&raw["leaseId"])?,
                    Cell::text(serde_json::to_string(planned).map_err(io::Error::other)?),
                )
            } else {
                let label = format!("{name}:{target}");
                let label = if label.len() > 128 {
                    format!("{name}:{}", hash(target.as_bytes()))
                } else {
                    label
                };
                let payload = serde_json::to_vec(receipt).map_err(io::Error::other)?;
                (
                    Cell::text(label),
                    Cell::text(hash(&payload)),
                    text(scope),
                    null(),
                    text("[]"),
                )
            };
            attempts.push(vec![
                Cell::integer(max_id),
                Cell::integer(1),
                Cell::integer(revision),
                text(&epoch),
                target,
                fingerprint,
                cause_json,
                Cell::text(attempt_key),
                text(status),
                text(status),
                null(),
                lease,
                grant,
            ]);
        }
        resolved |= accounted;
    }
    if attempts.is_empty() {
        return Ok(false);
    }

    if resolved {
        // No other provider can retire a v5 C# cause. A full C# graph and
        // terminal results for every target are required before clearing it.
        let planned = planned.iter().map(String::as_str).collect::<BTreeSet<_>>();
        state.tables[3].rows = state.tables[3]
            .rows
            .iter()
            .filter_map(|row| match detail_id(row) {
                Ok(Some(id)) if planned.contains(id.as_str()) => None,
                Ok(_) => Some(Ok(row.clone())),
                Err(error) => Some(Err(error)),
            })
            .collect::<io::Result<Vec<_>>>()?;
        state.tables[4].rows = state.tables[4]
            .rows
            .iter()
            .filter_map(|row| match fallback_id(row) {
                Ok(id) if planned.contains(id.as_str()) => None,
                Ok(_) => Some(Ok(row.clone())),
                Err(error) => Some(Err(error)),
            })
            .collect::<io::Result<Vec<_>>>()?;
        state.tables[6].rows.retain(|row| {
            !row[0]
                .value
                .as_deref()
                .is_some_and(|id| planned.contains(id))
        });
    }
    state.tables[5].rows.extend(attempts);
    let rows = &mut state.tables[5].rows;
    rows.drain(..rows.len().saturating_sub(HISTORY));
    let retained = rows
        .iter()
        .map(|row| row[0].value.clone())
        .collect::<BTreeSet<_>>();
    state.tables[8].rows.extend(details);
    state.tables[8]
        .rows
        .retain(|row| retained.contains(&row[0].value));
    Ok(true)
}
