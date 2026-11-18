//! Pytest 9.0.2 + pytest-json-report 1.5.0: authoritative collection and
//! setup/call/teardown classification without a first-party Python plugin.
use crate::protocol::{self, Executable, Job};
use crate::runner;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ArtifactWire {
    pub sha256: String,
    pub byte_count: u64,
    pub modified_ms: u64,
    pub excerpt: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CollectionWire {
    pub exit: i32,
    pub finished: bool,
    pub full_project_scope: bool,
    pub filtered: bool,
    pub collection_errors: u32,
    pub node_ids: Vec<String>,
    pub artifact: ArtifactWire,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CaseWire {
    pub node_id: String,
    pub outcome: String,
    pub failure_excerpt: Option<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RunWire {
    pub exit: i32,
    pub finished: bool,
    pub cases: Vec<CaseWire>,
    pub artifact: ArtifactWire,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Evidence {
    pub effective_config_sha256: String,
    pub collection: CollectionWire,
    pub run: Option<RunWire>,
}

fn object<'a>(value: &'a Value, name: &str) -> Result<&'a serde_json::Map<String, Value>, String> {
    value.as_object().ok_or_else(|| format!("{name} must be an object"))
}
fn array<'a>(obj: &'a serde_json::Map<String, Value>, name: &str) -> Result<&'a Vec<Value>, String> {
    obj.get(name).and_then(Value::as_array).ok_or_else(|| format!("{name} array missing"))
}
fn string<'a>(obj: &'a serde_json::Map<String, Value>, name: &str) -> Result<&'a str, String> {
    obj.get(name).and_then(Value::as_str).ok_or_else(|| format!("{name} string missing"))
}
fn report_root(doc: &Value, cwd: &Path, code: i32) -> Result<(), String> {
    let top = object(doc, "pytest report")?;
    let reported = top.get("root").and_then(Value::as_str);
    let expected = cwd.to_str();
    #[cfg(windows)]
    let same_root = reported.zip(expected).is_some_and(|(reported, expected)| {
        use slophawk_rust_project_context::platform_path_to_wire;
        reported == expected || platform_path_to_wire(reported)
            .is_some_and(|path| Some(path) == platform_path_to_wire(expected))
    });
    #[cfg(not(windows))]
    let same_root = reported.is_some() && reported == expected;
    if top.get("exitcode").and_then(Value::as_i64) != Some(i64::from(code))
        || !same_root {
        return Err("pytest exit or effective root differs from admitted scope".into());
    }
    object(top.get("summary").ok_or("pytest summary missing")?, "pytest summary")?;
    Ok(())
}

/// JSON collectors are authoritative; filenames and terminal output never
/// introduce IDs. Unknown custom item classes make collection partial.
fn collector_ids(doc: &Value) -> Result<(Vec<String>, bool, u32, bool), String> {
    let top = object(doc, "pytest report")?;
    let summary = object(top.get("summary").ok_or("summary absent")?, "summary")?;
    // The plugin documents both `collected` and `total`; complete runs can
    // report only the latter, while collect-only commonly reports the former.
    let collected = summary.get("collected").and_then(Value::as_u64)
        .or_else(|| summary.get("total").and_then(Value::as_u64))
        .ok_or("pytest collected/total count missing")?;
    if collected > 20_000 { return Err("collected ID budget exceeded".into()); }
    let deselected = summary.get("deselected").and_then(Value::as_u64).unwrap_or(0);
    let collectors = array(top, "collectors")?;
    if collectors.len() > 20_000 { return Err("collector budget exceeded".into()); }
    let mut ids = Vec::new();
    let mut unique = BTreeSet::new();
    let mut errors = 0u32;
    let mut unknown = false;
    for item in collectors {
        let collector = object(item, "collector")?;
        if string(collector, "outcome")? == "failed" {
            errors = errors.checked_add(1).ok_or("collector error count overflow")?;
        }
        for node in array(collector, "result")? {
            let node = object(node, "collected node")?;
            let kind = string(node, "type")?;
            if matches!(kind, "Function" | "TestCaseFunction" | "DoctestItem" | "Item") {
                let id = string(node, "nodeid")?;
                if id.is_empty() || id.len() > 512 || id.bytes().any(|b| b.is_ascii_control())
                    || !unique.insert(id.to_owned()) { return Err("invalid/duplicate pytest item ID".into()); }
                ids.push(id.to_owned());
                if node.get("deselected").and_then(Value::as_bool) == Some(true) { unknown = true; }
            } else if !matches!(kind, "Session" | "Dir" | "Directory" | "Package" | "Module" | "Class") {
                unknown = true;
            }
        }
    }
    // Collection items are in reporter traversal order (not lexically sorted).
    if ids.len() as u64 != collected { unknown = true; }
    if deselected > 0 { unknown = true; }
    Ok((ids, deselected > 0, errors, unknown))
}

pub(crate) fn interpret_collection(doc: &Value, cwd: &Path, code: i32, artifact: ArtifactWire) -> Result<CollectionWire, String> {
    report_root(doc, cwd, code)?;
    let (ids, filtered, errors, unknown) = collector_ids(doc)?;
    Ok(CollectionWire {
        exit: code, finished: true, full_project_scope: !unknown,
        filtered, collection_errors: errors, node_ids: ids, artifact,
    })
}

fn failure_excerpt(stages: &[&Value]) -> Option<String> {
    stages.iter().filter_map(|stage| stage.get("longrepr").and_then(Value::as_str))
        .find(|x| !x.is_empty())
        .map(|text| text.chars().filter(|c| !c.is_control() || matches!(c, '\n' | '\r' | '\t'))
            .take(256).collect())
}
fn stage<'a>(value: &'a Value, name: &str) -> Result<Option<&'a Value>, String> {
    match value.get(name) {
        None => Ok(None),
        Some(v) if v.is_object() && v.get("outcome").and_then(Value::as_str).is_some() => Ok(Some(v)),
        _ => Err(format!("malformed pytest {name} stage")),
    }
}

fn case_outcome(value: &Value) -> Result<CaseWire, String> {
    let record = object(value, "pytest case")?;
    let id = string(record, "nodeid")?;
    if id.is_empty() || id.len() > 512 { return Err("invalid pytest case ID".into()); }
    let setup = stage(value, "setup")?;
    let call = stage(value, "call")?;
    let teardown = stage(value, "teardown")?;
    fn stage_outcome(stage: Option<&Value>) -> Option<&str> {
        stage.and_then(|s| s.get("outcome")).and_then(Value::as_str)
    }
    let setup_o = stage_outcome(setup);
    let call_o = stage_outcome(call);
    let teardown_o = stage_outcome(teardown);
    // An error at setup or teardown overrides even a passing call. A skipped
    // setup has no call. A missing teardown cannot be normalized as a pass.
    let overall = string(record, "outcome")?;
    let outcome = if setup_o == Some("failed") || teardown_o == Some("failed") || overall == "error" {
        "error"
    } else {
        match overall {
            "passed" if setup_o == Some("passed") && call_o == Some("passed") && teardown_o == Some("passed") => "passed",
            "failed" if call_o == Some("failed") => "failed",
            "skipped" if [setup_o, call_o, teardown_o].into_iter().any(|x| x == Some("skipped")) => "skipped",
            "xfailed" => "xfailed",
            "xpassed" => "xpassed",
            _ => return Err("incomplete or contradictory pytest stage outcomes".into()),
        }
    };
    let error_stages = [setup, teardown, call].into_iter().flatten().collect::<Vec<_>>();
    let failure_excerpt = if matches!(outcome, "failed" | "error" | "xpassed") {
        failure_excerpt(&error_stages)
    } else { None };
    Ok(CaseWire { node_id: id.into(), outcome: outcome.into(), failure_excerpt })
}

pub(crate) fn interpret_run(doc: &Value, cwd: &Path, code: i32,
    collected: &CollectionWire, artifact: ArtifactWire) -> Result<RunWire, String> {
    report_root(doc, cwd, code)?;
    let (ids, filtered, errors, unknown) = collector_ids(doc)?;
    if errors > 0 || filtered || unknown || ids != collected.node_ids {
        return Err("pytest run recollection differed from admitted collection".into());
    }
    let top = object(doc, "pytest report")?;
    let mut cases = Vec::new();
    let mut seen = BTreeSet::new();
    for raw in array(top, "tests")? {
        let item = case_outcome(raw)?;
        if !seen.insert(item.node_id.clone()) { return Err("duplicate pytest case outcome".into()); }
        cases.push(item);
    }
    // Distinct missing cases are represented as incomplete by the classifier;
    // unknown/uncollected cases are rejected immediately.
    if cases.iter().any(|case| !ids.contains(&case.node_id)) {
        return Err("case has no collected node ID".into());
    }
    Ok(RunWire { exit: code, finished: true, cases, artifact })
}

fn safe_project(root: &str, relative: &str) -> Result<PathBuf, String> {
    if relative != "." && (relative.is_empty() || relative.starts_with('/') || relative.len() > 1024
        || relative.contains('\\') || relative.contains(':')
        || relative.split('/').any(|part| part.is_empty() || part == "." || part == "..")) {
        return Err("invalid Python project root".into());
    }
    let path = if relative == "." { PathBuf::from(root) } else { Path::new(root).join(relative) };
    if !path.is_dir() || !crate::admission::physical(&path).map_err(|_| "Python project root missing")? {
        return Err("Python project root is not a canonical directory".into());
    }
    Ok(path)
}

pub fn run_python(job: &Job, directory: &Path, interpreter: &Executable, project_root: &str,
    effective_config: Option<&str>) -> Result<Value, String> {
    let config = effective_config.filter(|x| protocol::is_sha(x))
        .ok_or("effective pytest config identity not attested; collection is unresolved")?;
    let root = safe_project(&job.identity.root, project_root)?;
    runner::probe_python(interpreter, &root, &job.budget)?;
    let collect_path = directory.join("collection.json");
    let run_path = directory.join("run.json");
    let collection = runner::invoke_python(interpreter, &root,
        &runner::python_args(&collect_path, true), &job.budget)?;
    let (collect_bytes, collect_artifact) = runner::artifact(&collect_path, &job.budget,
        &String::from_utf8_lossy(&collection.stdout))?;
    let report = serde_json::from_slice::<Value>(&collect_bytes)
        .map_err(|_| "malformed pytest collection artifact")?;
    let collected = interpret_collection(&report, &root, collection.exit, collect_artifact)?;
    let run = if collected.collection_errors > 0 || !collected.full_project_scope
        || collected.filtered || collected.node_ids.is_empty() || collected.exit != 0 {
        None
    } else {
        let execution = runner::invoke_python(interpreter, &root,
            &runner::python_args(&run_path, false), &job.budget)?;
        let (run_bytes, run_artifact) = runner::artifact(&run_path, &job.budget,
            &String::from_utf8_lossy(&execution.stdout))?;
        let report = serde_json::from_slice::<Value>(&run_bytes)
            .map_err(|_| "malformed pytest execution artifact")?;
        Some(interpret_run(&report, &root, execution.exit, &collected, run_artifact)?)
    };
    serde_json::to_value(Evidence { effective_config_sha256: config.into(),
        collection: collected, run }).map_err(|_| "Python evidence serialization failed".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    fn artifact() -> ArtifactWire {
        ArtifactWire { sha256: "f".repeat(64), byte_count: 100, modified_ms: 500,
            excerpt: String::new() }
    }
    fn project() -> &'static Path { Path::new("/tmp/pytest-project") }
    #[test]
    fn collector_nodes_are_authoritative_and_deselection_is_unresolved() {
        let data = json!({
            "root":"/tmp/pytest-project", "exitcode":0,
            "summary":{"collected":2},
            "collectors":[
                {"nodeid":"", "outcome":"passed", "result":[{"nodeid":"test_a.py", "type":"Module"}]},
                {"nodeid":"test_a.py", "outcome":"passed", "result":[
                    {"nodeid":"test_a.py::test_first[0]", "type":"Function"},
                    {"nodeid":"test_a.py::test_second", "type":"Function"}
                ]}
            ]
        });
        let result = interpret_collection(&data, project(), 0, artifact()).unwrap();
        assert_eq!(result.node_ids, vec!["test_a.py::test_first[0]".to_string(), "test_a.py::test_second".to_string()]);
        assert!(result.full_project_scope);
        let mut deselected = data;
        deselected["summary"]["deselected"] = json!(1);
        let result = interpret_collection(&deselected, project(), 0, artifact()).unwrap();
        assert!(result.filtered);
        assert!(!result.full_project_scope);
    }
    #[test]
    fn zero_collection_and_collection_error_are_not_passes() {
        let empty = json!({"root":"/tmp/pytest-project", "exitcode":5,
            "summary":{"collected":0}, "collectors":[]});
        let receipt = interpret_collection(&empty, project(), 5, artifact()).unwrap();
        assert!(receipt.node_ids.is_empty() && receipt.finished);
        let failure = json!({"root":"/tmp/pytest-project", "exitcode":2,
            "summary":{"collected":0},
            "collectors":[{"nodeid":"test_b.py", "outcome":"failed", "result":[], "longrepr":"syntax error"}]});
        let receipt = interpret_collection(&failure, project(), 2, artifact()).unwrap();
        assert_eq!(receipt.collection_errors, 1);
    }
    #[test]
    fn setup_teardown_xfail_xpass_and_call_failures_do_not_collapse() {
        let examples = [
            (json!({"nodeid":"a.py::pass", "outcome":"passed",
                "setup":{"outcome":"passed"}, "call":{"outcome":"passed"}, "teardown":{"outcome":"passed"}}), "passed"),
            (json!({"nodeid":"a.py::setup_error", "outcome":"error",
                "setup":{"outcome":"failed","longrepr":"setup crashed"}}), "error"),
            (json!({"nodeid":"a.py::teardown_error", "outcome":"error",
                "setup":{"outcome":"passed"}, "call":{"outcome":"passed"},
                "teardown":{"outcome":"failed","longrepr":"teardown crashed"}}), "error"),
            (json!({"nodeid":"a.py::fail", "outcome":"failed",
                "setup":{"outcome":"passed"}, "call":{"outcome":"failed","longrepr":"assertion"},
                "teardown":{"outcome":"passed"}}), "failed"),
            (json!({"nodeid":"a.py::skip", "outcome":"skipped",
                "setup":{"outcome":"skipped"}, "teardown":{"outcome":"passed"}}), "skipped"),
            (json!({"nodeid":"a.py::xfail", "outcome":"xfailed", "call":{"outcome":"skipped"}}), "xfailed"),
            (json!({"nodeid":"a.py::xpass", "outcome":"xpassed",
                "setup":{"outcome":"passed"}, "call":{"outcome":"passed"}, "teardown":{"outcome":"passed"}}), "xpassed"),
        ];
        for (value, expected) in examples {
            assert_eq!(case_outcome(&value).unwrap().outcome, expected);
        }
        assert!(case_outcome(&json!({"nodeid":"a.py::missing_teardown", "outcome":"passed",
            "setup":{"outcome":"passed"}, "call":{"outcome":"passed"}})).is_err());
    }
}
