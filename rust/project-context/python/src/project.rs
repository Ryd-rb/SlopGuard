use crate::digest::{DigestInput, valid_sha256};
use std::collections::{BTreeMap, BTreeSet};

const MAX_FILES: usize = 4_096;
const MAX_FILE_BYTES: usize = 4 * 1024 * 1024;
const MAX_SNAPSHOT_BYTES: usize = 32 * 1024 * 1024;
const MAX_PATH: usize = 1_024;
const MAX_PROJECTS: usize = 128;
const MAX_CHANGES: usize = 4_096;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Inventory {
    /// The caller checked all files in the explicitly admitted roots. This does not
    /// imply that static discovery is complete or that pytest collected any test.
    Complete,
    /// The caller was unable to observe every path or every original byte.
    Partial,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WorkspaceScope {
    pub workspace_id: String,
    pub revision: String,
    /// POSIX-style, workspace-relative, non-overlapping roots; `.` is the root.
    pub project_roots: Vec<String>,
    pub inventory: Inventory,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SnapshotFile {
    pub path: String,
    /// The original bytes. No text decoding is needed for identity binding.
    pub bytes: Vec<u8>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ContextIdentity {
    pub schema: &'static str,
    pub workspace_id: String,
    pub revision: String,
    pub scope_sha256: String,
    pub source_sha256: String,
    pub configuration_sha256: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ContextStatus {
    StaticHintsOnly,
    IncompleteInventory,
}

/// Gaps always qualify selection and are never represented as an empty test set.
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum Gap {
    MissingPyproject,
    UnsupportedPyproject,
    UnsupportedTestDiscovery,
    BuildBackendOpaque,
    CustomCollection,
    DynamicImport,
    NamespacePackage,
    UnreadablePythonFile,
    NoStaticTestHints,
    PartialInventory,
    StaticImportsNotSemantic,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PackageRoot {
    pub import_name: String,
    /// Path of the root containing the package (e.g. `service/src`).
    pub source_root: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProjectFacts {
    pub root: String,
    pub project_name: Option<String>,
    pub package_roots: Vec<PackageRoot>,
    pub pytest_testpaths: Vec<String>,
    /// Only lexical candidates, *not* pytest-collected node IDs.
    pub static_test_hints: Vec<String>,
    pub gaps: Vec<Gap>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProjectContext {
    pub identity: ContextIdentity,
    pub status: ContextStatus,
    pub projects: Vec<ProjectFacts>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Change {
    /// Before an edit/delete/rename; may not exist in the new snapshot.
    pub before: Option<String>,
    /// After an edit/add/rename; must be in the new snapshot if inventory is complete.
    pub after: Option<String>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SelectionStatus {
    /// Collection is required, even if all static candidate arrays are empty.
    NeedsAdmittedCollection,
    /// Missing inventory or out-of-scope changes forbid a completeness verdict.
    UnresolvedScope,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProjectSelection {
    pub root: String,
    pub collect_entire_root: bool,
    pub changed_paths: Vec<String>,
    pub static_test_hints: Vec<String>,
    pub gaps: Vec<Gap>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SelectionContext {
    pub identity: ContextIdentity,
    pub baseline_source_sha256: String,
    pub selection_sha256: String,
    pub status: SelectionStatus,
    pub impacted: Vec<ProjectSelection>,
    pub unowned_changes: Vec<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ContextError {
    EmptyScope,
    InvalidPath(String),
    OverlappingRoots,
    DuplicatePath(String),
    FileOutsideScope(String),
    LimitExceeded(&'static str),
    InvalidIdentity(&'static str),
    InvalidChange,
}

/// The `scope` represents an already authorised snapshot; this function never
/// enumerates the host filesystem, imports Python or evaluates `pyproject.toml`.
pub fn analyze(
    scope: WorkspaceScope,
    files: Vec<SnapshotFile>,
) -> Result<ProjectContext, ContextError> {
    validate_identity(&scope.workspace_id, "workspace_id")?;
    validate_identity(&scope.revision, "revision")?;
    if scope.project_roots.is_empty() {
        return Err(ContextError::EmptyScope);
    }
    if scope.project_roots.len() > MAX_PROJECTS || files.len() > MAX_FILES {
        return Err(ContextError::LimitExceeded("project/file count"));
    }
    let mut roots = BTreeSet::new();
    for root in &scope.project_roots {
        valid_path(root, true)?;
        if !roots.insert(root.clone()) {
            return Err(ContextError::DuplicatePath(root.clone()));
        }
    }
    for a in &roots {
        for b in &roots {
            if a != b && contains(a, b) {
                return Err(ContextError::OverlappingRoots);
            }
        }
    }
    let mut source = BTreeMap::new();
    let mut total = 0usize;
    for file in files {
        valid_path(&file.path, false)?;
        if !roots.iter().any(|root| contains(root, &file.path)) {
            return Err(ContextError::FileOutsideScope(file.path));
        }
        if file.bytes.len() > MAX_FILE_BYTES {
            return Err(ContextError::LimitExceeded("file bytes"));
        }
        total = total.saturating_add(file.bytes.len());
        if total > MAX_SNAPSHOT_BYTES {
            return Err(ContextError::LimitExceeded("snapshot bytes"));
        }
        if source.insert(file.path.clone(), file.bytes).is_some() {
            return Err(ContextError::DuplicatePath(file.path));
        }
    }

    let mut scope_digest = DigestInput::new("slophawk/python/scope/v1");
    scope_digest.append(scope.workspace_id.as_bytes());
    scope_digest.append(scope.revision.as_bytes());
    scope_digest.append(&[u8::from(scope.inventory == Inventory::Complete)]);
    for root in &roots {
        scope_digest.append(root.as_bytes());
    }
    let mut source_digest = DigestInput::new("slophawk/python/source/v1");
    let mut config_digest = DigestInput::new("slophawk/python/config/v1");
    for (path, bytes) in &source {
        source_digest.append(path.as_bytes());
        source_digest.append(bytes);
        if is_pytest_config(path) {
            config_digest.append(path.as_bytes());
            config_digest.append(bytes);
        }
    }
    let identity = ContextIdentity {
        schema: "python-project-context/v1",
        workspace_id: scope.workspace_id,
        revision: scope.revision,
        scope_sha256: scope_digest.finish(),
        source_sha256: source_digest.finish(),
        configuration_sha256: config_digest.finish(),
    };
    let mut projects = Vec::new();
    for root in roots {
        let project_files: Vec<_> = source
            .iter()
            .filter(|(path, _)| contains(&root, path))
            .collect();
        let pyproject_path = join(&root, "pyproject.toml");
        let mut name = None;
        let mut testpaths = Vec::new();
        let mut gaps = BTreeSet::from([Gap::StaticImportsNotSemantic]);
        match source.get(&pyproject_path) {
            Some(bytes) => parse_pyproject(bytes, &mut name, &mut testpaths, &mut gaps),
            None => {
                gaps.insert(Gap::MissingPyproject);
            }
        }
        if scope.inventory == Inventory::Partial {
            gaps.insert(Gap::PartialInventory);
        }
        let mut packages = BTreeSet::new();
        let mut tests = BTreeSet::new();
        for (path, bytes) in &project_files {
            let relative = remove_root(&root, path);
            if path.ends_with("/conftest.py")
                || relative == "conftest.py"
                || relative == "pytest.ini"
                || relative == "tox.ini"
                || relative == "setup.cfg"
            {
                gaps.insert(Gap::CustomCollection);
            }
            if !path.ends_with(".py") && !path.ends_with(".pyi") {
                continue;
            }
            if path.ends_with(".py") && is_default_test_file(path) {
                tests.insert((**path).clone());
            }
            if let Some(prefix) = relative.strip_suffix("/__init__.py") {
                let mut parts = prefix.split('/');
                let first = parts.next().unwrap_or("");
                let (source_root, import_name) = if first == "src" {
                    (join(&root, "src"), parts.next().unwrap_or(""))
                } else {
                    (root.clone(), first)
                };
                if !import_name.is_empty() && !import_name.starts_with('.') {
                    packages.insert((source_root, import_name.to_string()));
                }
            }
            match std::str::from_utf8(bytes) {
                Ok(text) => {
                    if text.contains("__import__(")
                        || text.contains("importlib.")
                        || text.contains("pytest_generate_tests")
                        || text.contains("pytest_collect_file")
                        || text.contains("pytest_collect_directory")
                        || text.contains("pytest_plugins")
                        || text.contains("exec(")
                        || text.contains("eval(")
                    {
                        gaps.insert(Gap::DynamicImport);
                    }
                }
                Err(_) => {
                    gaps.insert(Gap::UnreadablePythonFile);
                }
            }
            if relative.starts_with("src/")
                && relative.ends_with(".py")
                && !relative.ends_with("/__init__.py")
                && !relative.contains("/test_")
            {
                let prefix = relative.strip_prefix("src/").unwrap_or(relative);
                if let Some(package) = prefix.split('/').next() {
                    let marker = join(&join(&root, "src"), &format!("{package}/__init__.py"));
                    if !source.contains_key(&marker) {
                        gaps.insert(Gap::NamespacePackage);
                    }
                }
            }
        }
        if tests.is_empty() {
            gaps.insert(Gap::NoStaticTestHints);
        }
        projects.push(ProjectFacts {
            root,
            project_name: name,
            package_roots: packages
                .into_iter()
                .map(|(source_root, import_name)| PackageRoot {
                    import_name,
                    source_root,
                })
                .collect(),
            pytest_testpaths: testpaths,
            static_test_hints: tests.into_iter().collect(),
            gaps: gaps.into_iter().collect(),
        });
    }
    Ok(ProjectContext {
        identity,
        status: if scope.inventory == Inventory::Complete {
            ContextStatus::StaticHintsOnly
        } else {
            ContextStatus::IncompleteInventory
        },
        projects,
    })
}

/// Select whole affected project collection scopes. Static test files are hints
/// only; the coordinator must collect in an approved worker before execution.
pub fn select_tests(
    context: &ProjectContext,
    baseline_source_sha256: &str,
    changes: &[Change],
) -> Result<SelectionContext, ContextError> {
    if !valid_sha256(baseline_source_sha256) {
        return Err(ContextError::InvalidIdentity("baseline_source_sha256"));
    }
    if changes.is_empty() || changes.len() > MAX_CHANGES {
        return Err(ContextError::InvalidChange);
    }
    let mut affected: BTreeMap<&str, BTreeSet<String>> = BTreeMap::new();
    let mut unowned = BTreeSet::new();
    let mut normalized = BTreeSet::new();
    for change in changes {
        if change.before.is_none() && change.after.is_none() {
            return Err(ContextError::InvalidChange);
        }
        for path in [&change.before, &change.after].into_iter().flatten() {
            valid_path(path, false)?;
            let mut owner = false;
            for project in &context.projects {
                if contains(&project.root, path) {
                    affected
                        .entry(&project.root)
                        .or_default()
                        .insert(path.clone());
                    owner = true;
                }
            }
            if !owner {
                unowned.insert(path.clone());
            }
        }
        normalized.insert((change.before.clone(), change.after.clone()));
    }
    let impacted: Vec<_> = context
        .projects
        .iter()
        .filter_map(|project| {
            affected
                .get(project.root.as_str())
                .map(|paths| ProjectSelection {
                    root: project.root.clone(),
                    collect_entire_root: true,
                    changed_paths: paths.iter().cloned().collect(),
                    static_test_hints: project.static_test_hints.clone(),
                    gaps: project.gaps.clone(),
                })
        })
        .collect();
    let status = if context.status == ContextStatus::IncompleteInventory || !unowned.is_empty() {
        SelectionStatus::UnresolvedScope
    } else {
        SelectionStatus::NeedsAdmittedCollection
    };
    let mut digest = DigestInput::new("slophawk/python/selection/v1");
    for value in [
        &context.identity.scope_sha256,
        &context.identity.source_sha256,
        &context.identity.configuration_sha256,
        baseline_source_sha256,
    ] {
        digest.append(value.as_bytes());
    }
    digest.append(&[u8::from(status == SelectionStatus::NeedsAdmittedCollection)]);
    for (before, after) in normalized {
        digest.append(&[u8::from(before.is_some())]);
        if let Some(path) = before {
            digest.append(path.as_bytes());
        }
        digest.append(&[u8::from(after.is_some())]);
        if let Some(path) = after {
            digest.append(path.as_bytes());
        }
    }
    for project in &impacted {
        digest.append(project.root.as_bytes());
    }
    for path in &unowned {
        digest.append(path.as_bytes());
    }
    Ok(SelectionContext {
        identity: context.identity.clone(),
        baseline_source_sha256: baseline_source_sha256.to_string(),
        selection_sha256: digest.finish(),
        status,
        impacted,
        unowned_changes: unowned.into_iter().collect(),
    })
}

fn validate_identity(text: &str, name: &'static str) -> Result<(), ContextError> {
    if text.is_empty() || text.len() > 160 || text.chars().any(char::is_control) {
        Err(ContextError::InvalidIdentity(name))
    } else {
        Ok(())
    }
}

fn valid_path(path: &str, root: bool) -> Result<(), ContextError> {
    if root && path == "." {
        return Ok(());
    }
    if path.is_empty()
        || path.len() > MAX_PATH
        || path.starts_with('/')
        || path.contains('\\')
        || path.chars().any(char::is_control)
        || path
            .split('/')
            .any(|part| part.is_empty() || part == "." || part == "..")
        || path.contains(':')
    {
        Err(ContextError::InvalidPath(path.to_string()))
    } else {
        Ok(())
    }
}

fn contains(root: &str, path: &str) -> bool {
    root == "."
        || path
            .strip_prefix(root)
            .is_some_and(|rest| rest.starts_with('/'))
}

fn join(root: &str, leaf: &str) -> String {
    if root == "." {
        leaf.to_string()
    } else {
        format!("{root}/{leaf}")
    }
}

fn remove_root<'a>(root: &str, path: &'a str) -> &'a str {
    if root == "." {
        path
    } else {
        &path[root.len() + 1..]
    }
}

fn is_pytest_config(path: &str) -> bool {
    let base = path.rsplit('/').next().unwrap_or(path);
    matches!(
        base,
        "pyproject.toml" | "pytest.ini" | "tox.ini" | "setup.cfg" | "conftest.py"
    )
}

fn is_default_test_file(path: &str) -> bool {
    let base = path.rsplit('/').next().unwrap_or(path);
    (base.starts_with("test_") || base.ends_with("_test.py")) && base.ends_with(".py")
}

/// A deliberately narrow TOML *hint* reader, not a TOML interpreter. No paths
/// or execution settings are honored when parsing is ambiguous. Entire original
/// file bytes still participate in both source and configuration identities.
fn parse_pyproject(
    bytes: &[u8],
    name: &mut Option<String>,
    testpaths: &mut Vec<String>,
    gaps: &mut BTreeSet<Gap>,
) {
    let Ok(content) = std::str::from_utf8(bytes) else {
        gaps.insert(Gap::UnsupportedPyproject);
        return;
    };
    let mut table = "";
    for raw in content.lines() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if line.starts_with('[') {
            if !line.ends_with(']') || line.starts_with("[[") || line.contains('#') {
                gaps.insert(Gap::UnsupportedPyproject);
                table = "";
            } else {
                table = &line[1..line.len() - 1];
                if table.starts_with("tool.pytest.") && table != "tool.pytest.ini_options" {
                    gaps.insert(Gap::UnsupportedTestDiscovery);
                }
                if table == "build-system" {
                    gaps.insert(Gap::BuildBackendOpaque);
                }
            }
            continue;
        }
        let Some((key, raw_value)) = line.split_once('=') else {
            gaps.insert(Gap::UnsupportedPyproject);
            continue;
        };
        let key = key.trim();
        let value = strip_simple_comment(raw_value.trim());
        match (table, key) {
            ("project", "name") => {
                if let Some(found) = parse_string(value) {
                    *name = Some(found.to_string());
                } else {
                    gaps.insert(Gap::UnsupportedPyproject);
                }
            }
            ("tool.pytest.ini_options", "testpaths") => {
                if let Some(paths) = parse_string_array(value) {
                    if paths.iter().all(|p| valid_path(p, false).is_ok()) {
                        *testpaths = paths;
                    } else {
                        gaps.insert(Gap::UnsupportedTestDiscovery);
                    }
                } else {
                    gaps.insert(Gap::UnsupportedTestDiscovery);
                }
            }
            ("tool.pytest.ini_options", "python_files") => {
                if parse_string_array(value).is_none() {
                    gaps.insert(Gap::UnsupportedTestDiscovery);
                } else {
                    // Non-default patterns can change collection; never use them
                    // to exclude a static hint or to assert an empty collection.
                    gaps.insert(Gap::UnsupportedTestDiscovery);
                }
            }
            (
                "tool.pytest.ini_options",
                "addopts"
                | "python_classes"
                | "python_functions"
                | "norecursedirs"
                | "consider_namespace_packages"
                | "confcutdir",
            ) => {
                gaps.insert(Gap::UnsupportedTestDiscovery);
            }
            ("tool.setuptools.packages.find", "where") => {
                // The hints use only physical __init__.py files; configured
                // namespace packages and custom finders remain unresolved.
                if parse_string_array(value).is_none() {
                    gaps.insert(Gap::UnsupportedPyproject);
                }
            }
            ("tool.poetry", "packages") | ("tool.hatch.build.targets.wheel", "packages") => {
                gaps.insert(Gap::UnsupportedPyproject);
            }
            _ => {}
        }
    }
}

fn strip_simple_comment(raw: &str) -> &str {
    let mut quote = None;
    let mut escaped = false;
    for (index, ch) in raw.char_indices() {
        if escaped {
            escaped = false;
            continue;
        }
        if ch == '\\' && quote == Some('"') {
            escaped = true;
            continue;
        }
        if quote == Some(ch) {
            quote = None;
            continue;
        }
        if quote.is_none() && (ch == '"' || ch == '\'') {
            quote = Some(ch);
            continue;
        }
        if quote.is_none() && ch == '#' {
            return raw[..index].trim_end();
        }
    }
    raw
}

fn parse_string(value: &str) -> Option<&str> {
    if value.len() < 2 {
        return None;
    }
    let first = value.as_bytes()[0];
    let last = value.as_bytes()[value.len() - 1];
    if first != last || (first != b'"' && first != b'\'') {
        return None;
    }
    let inner = &value[1..value.len() - 1];
    if inner
        .chars()
        .any(|ch| matches!(ch, '"' | '\'' | '\\' | '\n' | '\r'))
        || inner.len() > 256
    {
        None
    } else {
        Some(inner)
    }
}

fn parse_string_array(value: &str) -> Option<Vec<String>> {
    let content = value.strip_prefix('[')?.strip_suffix(']')?;
    if content.is_empty() {
        return Some(Vec::new());
    }
    if content.len() > 2_048 {
        return None;
    }
    let items: Vec<String> = content
        .split(',')
        .map(|part| parse_string(part.trim()).map(str::to_string))
        .collect::<Option<_>>()?;
    if items.len() > 64 { None } else { Some(items) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_traversal_and_overlapping_roots() {
        assert!(valid_path("../x.py", false).is_err());
        assert!(valid_path("C:\\x.py", false).is_err());
        assert!(valid_path("src//x.py", false).is_err());
        assert!(valid_path("/etc/passwd", false).is_err());
        let scope = WorkspaceScope {
            workspace_id: "w".into(),
            revision: "r".into(),
            project_roots: vec![".".into(), "packages/a".into()],
            inventory: Inventory::Complete,
        };
        assert_eq!(analyze(scope, vec![]), Err(ContextError::OverlappingRoots));
    }

    #[test]
    fn comments_are_not_pyproject_values() {
        let mut name = None;
        let mut tests = Vec::new();
        let mut gaps = BTreeSet::new();
        parse_pyproject(
            b"[project]\nname = 'a#b' # comment\n[tool.pytest.ini_options]\ntestpaths = ['tests']",
            &mut name,
            &mut tests,
            &mut gaps,
        );
        assert_eq!(name.as_deref(), Some("a#b"));
        assert_eq!(tests, vec!["tests"]);
    }
}
