//! Opt-in production of declared import graphs from captured original bytes.
//!
//! The coordinator supplies a complete, uniquely mapped regular-package inventory
//! and attests its read permission, physical containment and lack of path aliases
//! or import-root shadowing. These declarations are NOT inferred from project
//! hints, a digest, a directory name or this crate's successful return value.
//! No filesystem, Python interpreter, loader, subprocess or discovery is used.
//! The output describes explicit syntactic dependencies, not runtime resolution.

use sha2::{Digest, Sha256};
use slophawk_python_project_context::import_layers::{
    AdmittedRoot, DirectImport, ImportSnapshot, ImportTarget, ImportUse, Module,
    SEMANTICS, SnapshotIdentity, Span, UnsupportedFact, UnsupportedKind,
};
use slophawk_python_syntax_provider::{self as python, imports};
use std::collections::{BTreeMap, BTreeSet};

pub const SCHEMA_VERSION: &str = "python-admitted-imports/v1";
pub const ADMISSION_SCHEMA: &str = "python-unique-regular-inventory/v1";
const MAX_NAME: usize = 256;
const MAX_PATH: usize = 1_024;
const MAX_ROOTS: usize = 128;
const MAX_MODULES: usize = 2_048;
const MAX_FACTS: usize = 16_384;
const MAX_SOURCE_BYTES: usize = 16 * 1024 * 1024;

/// Explicit coordinator declarations, with no permissive Default implementation.
/// The coordinator must recheck grants and capture every regular .py module in
/// these roots for this revision, including empty __init__.py files. It must rule
/// out symlinks, hard-link/path aliases, namespace portions and competing roots.
/// Digests identify that evidence; they do not authenticate these assertions.
#[derive(Clone, Copy, Debug)]
pub struct Admission<'a> {
    pub schema: &'a str,
    pub workspace_id: &'a str,
    pub revision: &'a str,
    /// Stable across the compared revisions: declared resolution environment.
    pub environment_sha256: &'a str,
    /// Owner's versioned root/read-admission policy, not the layer ordering.
    pub policy_sha256: &'a str,
    pub read_authorized: bool,
    pub complete_inventory: bool,
    pub unique_regular_roots: bool,
    pub no_path_aliases: bool,
}

#[derive(Clone, Copy, Debug)]
pub struct CapturedModule<'a> {
    pub name: &'a str,
    pub path: &'a str,
    pub bytes: &'a [u8],
    /// Independent capture expectation; verified against the original bytes.
    pub expected_sha256: &'a str,
    /// The package p is p/__init__.py, never a module named p.__init__.
    pub is_package: bool,
}

#[derive(Clone, Copy, Debug)]
pub struct Request<'a> {
    pub admission: Admission<'a>,
    pub roots: &'a [AdmittedRoot],
    pub modules: &'a [CapturedModule<'a>],
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Limits {
    pub max_modules: usize,
    /// Combined original source bytes in one snapshot, before any hashing.
    pub max_source_bytes: usize,
    /// Combined import occurrences and unsupported-statement records.
    pub max_facts: usize,
    pub per_file: python::Limits,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            max_modules: MAX_MODULES,
            max_source_bytes: MAX_SOURCE_BYTES,
            max_facts: MAX_FACTS,
            per_file: python::Limits {
                max_source_bytes: 512 * 1024,
                max_tokens: 65_536,
                max_facts: MAX_FACTS,
            },
        }
    }
}

impl Limits {
    fn valid(self) -> bool {
        (1..=MAX_MODULES).contains(&self.max_modules)
            && (1..=MAX_SOURCE_BYTES).contains(&self.max_source_bytes)
            && (1..=MAX_FACTS).contains(&self.max_facts)
            && (1..=python::HARD_MAX_SOURCE_BYTES).contains(&self.per_file.max_source_bytes)
            && (1..=131_072).contains(&self.per_file.max_tokens)
            && (1..=32_768).contains(&self.per_file.max_facts)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Requirement {
    ReadPermission,
    CompleteInventory,
    UniqueRegularRoots,
    NoPathAliases,
}

/// An Err is unavailable evidence, never an empty or complete graph. Syntax
/// frontiers that have valid source coordinates instead survive in Ok(snapshot).
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Error {
    InvalidLimits,
    LimitExceeded(&'static str),
    AdmissionRequired(Requirement),
    InvalidIdentity(&'static str),
    StaleRevision,
    InvalidRoot(String),
    AmbiguousRoot(String),
    InvalidModule(String),
    DuplicateModule(String),
    PathAlias(String),
    MissingPackage(String),
    HashMismatch(String),
    Extraction { path: String, outcome: python::Outcome },
    ProviderContract(String),
}

/// Produce a graph only for the caller's explicitly reviewed inventory. Python
/// 3.12 is the fixed reviewed grammar. `expected_revision` must come from the
/// coordinator's independent currentness expectation, not this result.
/// The later compare call still needs an independently admitted SnapshotPair.
/// No partial graph is returned on malformed, stale, denied or exhausted input.
pub fn produce(
    request: Request<'_>,
    expected_revision: &str,
    limits: Limits,
) -> Result<ImportSnapshot, Error> {
    if !limits.valid() {
        return Err(Error::InvalidLimits);
    }
    let admission = request.admission;
    validate_admission(admission, expected_revision)?;
    // Bound record counts and every field before constructing ordered maps,
    // derived paths, hashes or parser input. Caller-owned storage is not ours.
    if request.roots.is_empty() || request.roots.len() > MAX_ROOTS {
        return Err(Error::LimitExceeded("root count"));
    }
    if request.modules.len() > limits.max_modules {
        return Err(Error::LimitExceeded("module count"));
    }
    let mut source_bytes = 0usize;
    for root in request.roots {
        if root.package.len() > MAX_NAME || root.source_root.len() > MAX_PATH {
            return Err(Error::LimitExceeded("root fields"));
        }
    }
    for module in request.modules {
        if module.name.len() > MAX_NAME || module.path.len() > MAX_PATH
            || module.expected_sha256.len() > 64
        {
            return Err(Error::LimitExceeded("module fields"));
        }
        if module.bytes.len() > limits.per_file.max_source_bytes {
            return Err(Error::LimitExceeded("file bytes"));
        }
        source_bytes = source_bytes.saturating_add(module.bytes.len());
        if source_bytes > limits.max_source_bytes {
            return Err(Error::LimitExceeded("snapshot bytes"));
        }
    }

    let roots = validate_roots(request.roots)?;
    let modules = validate_modules(request.modules, &roots)?;
    let context_sha256 = context_digest(admission, &roots);
    let mut source_digest = Fields::new("slophawk/python/admitted-imports/source/v1");
    source_digest.append(&(modules.len() as u64).to_be_bytes());
    let mut snapshot = ImportSnapshot {
        identity: SnapshotIdentity {
            revision: admission.revision.into(),
            source_sha256: String::new(),
            context_sha256,
        },
        complete: true,
        roots: roots.values().map(|root| (**root).clone()).collect(),
        modules: Vec::new(),
        imports: Vec::new(),
        unsupported: Vec::new(),
    };

    for module in modules.values() {
        let actual = format!("{:x}", Sha256::digest(module.bytes));
        if actual != module.expected_sha256 {
            return Err(Error::HashMismatch(module.path.into()));
        }
        // Identity is path-qualified; graph module names are supplied separately.
        let source = python::SourceRequest {
            document_id: "admitted-import-module",
            path: module.path,
            bytes: module.bytes,
            target: python::PythonTarget { major: 3, minor: 12 },
            limits: limits.per_file,
        };
        let facts = imports::extract(source).map_err(|failure| Error::Extraction {
            path: module.path.into(), outcome: failure.outcome,
        })?;
        if facts.identity.provider != imports::PROVIDER_ID
            || facts.identity.schema != imports::SCHEMA_VERSION
            || facts.identity.parser_crate != python::PARSER_CRATE
            || facts.identity.parser_version != python::PARSER_VERSION
            || facts.identity.path != module.path
            || facts.identity.document_id != source.document_id
            || facts.identity.source_sha256 != actual
            || facts.identity.original_bytes != module.bytes.len()
            || facts.identity.kind != python::FileKind::Python
            || facts.identity.target != source.target
            || facts.identity.span_encoding != "utf8-original-byte-offsets-half-open"
            || !matches!(facts.identity.encoding, python::Encoding::Utf8 | python::Encoding::Utf8Bom)
        {
            return Err(Error::ProviderContract(module.path.into()));
        }
        let added_facts = facts.imports.len().saturating_add(facts.unsupported.len());
        if snapshot.imports.len().saturating_add(snapshot.unsupported.len())
            .saturating_add(added_facts) > limits.max_facts
        {
            return Err(Error::LimitExceeded("import/unsupported facts"));
        }
        let source_text = std::str::from_utf8(module.bytes)
            .map_err(|_| Error::ProviderContract(module.path.into()))?;
        for import in facts.imports {
            if !module_name(&import.name) || !valid_span(import.span, source_text) {
                return Err(Error::ProviderContract(module.path.into()));
            }
            let target = if modules.contains_key(import.name.as_str()) {
                ImportTarget::Internal(import.name)
            } else {
                snapshot.complete = false;
                ImportTarget::Unknown(UnsupportedKind::UnresolvedTarget)
            };
            snapshot.imports.push(DirectImport {
                importer: module.name.into(), source_sha256: actual.clone(),
                target, use_kind: ImportUse::Runtime, span: span(import.span),
            });
        }
        for unsupported in facts.unsupported {
            if !valid_span(unsupported.span, source_text) {
                return Err(Error::ProviderContract(module.path.into()));
            }
            let kind = match unsupported.kind {
                imports::UnsupportedKind::FromImport => UnsupportedKind::AmbiguousReexport,
                imports::UnsupportedKind::RelativeImport => UnsupportedKind::UnsupportedSyntax,
                imports::UnsupportedKind::UnresolvedGuard => UnsupportedKind::UnresolvedGuard,
                imports::UnsupportedKind::DynamicOrUnsupportedStatement => UnsupportedKind::UnsupportedSyntax,
                imports::UnsupportedKind::UnsupportedName => UnsupportedKind::UnsupportedName,
            };
            snapshot.complete = false;
            snapshot.unsupported.push(UnsupportedFact {
                module: Some(module.name.into()), span: Some(span(unsupported.span)), kind,
            });
        }
        source_digest.append(module.name.as_bytes());
        source_digest.append(module.path.as_bytes());
        source_digest.append(&[u8::from(module.is_package)]);
        source_digest.append(module.bytes);
        snapshot.modules.push(Module {
            name: module.name.into(), path: module.path.into(), source_sha256: actual,
            byte_len: module.bytes.len() as u32, is_package: module.is_package,
        });
    }
    snapshot.identity.source_sha256 = source_digest.finish();
    Ok(snapshot)
}

fn validate_admission(admission: Admission<'_>, expected_revision: &str) -> Result<(), Error> {
    if admission.schema != ADMISSION_SCHEMA {
        return Err(Error::InvalidIdentity("admission schema"));
    }
    for (accepted, requirement) in [
        (admission.read_authorized, Requirement::ReadPermission),
        (admission.complete_inventory, Requirement::CompleteInventory),
        (admission.unique_regular_roots, Requirement::UniqueRegularRoots),
        (admission.no_path_aliases, Requirement::NoPathAliases),
    ] {
        if !accepted { return Err(Error::AdmissionRequired(requirement)); }
    }
    for (text, field) in [
        (admission.workspace_id, "workspace identity"),
        (admission.revision, "revision"), (expected_revision, "expected revision"),
    ] {
        if !valid_text(text, 160) { return Err(Error::InvalidIdentity(field)); }
    }
    for (digest, field) in [
        (admission.environment_sha256, "environment digest"),
        (admission.policy_sha256, "admission policy digest"),
    ] {
        if !sha256(digest) { return Err(Error::InvalidIdentity(field)); }
    }
    if admission.revision != expected_revision { return Err(Error::StaleRevision); }
    Ok(())
}

fn validate_roots(roots: &[AdmittedRoot]) -> Result<BTreeMap<&str, &AdmittedRoot>, Error> {
    let mut found = BTreeMap::new();
    let mut names = BTreeSet::new();
    let mut directories = BTreeSet::<String>::new();
    for root in roots {
        if !identifier(&root.package) || root.package == "__init__" || !path(&root.source_root, true) {
            return Err(Error::InvalidRoot(root.package.clone()));
        }
        if !names.insert(root.package.to_ascii_lowercase()) {
            return Err(Error::AmbiguousRoot(root.package.clone()));
        }
        let directory = join(&root.source_root, &root.package).to_ascii_lowercase();
        if directories.iter().any(|other| within(&directory, other) || within(other, &directory)) {
            return Err(Error::AmbiguousRoot(root.package.clone()));
        }
        directories.insert(directory);
        found.insert(root.package.as_str(), root);
    }
    Ok(found)
}

fn validate_modules<'a>(
    input: &'a [CapturedModule<'a>],
    roots: &BTreeMap<&str, &AdmittedRoot>,
) -> Result<BTreeMap<&'a str, &'a CapturedModule<'a>>, Error> {
    let mut modules = BTreeMap::new();
    let mut names = BTreeSet::new();
    let mut paths = BTreeSet::new();
    for module in input {
        if !module_name(module.name) || !path(module.path, false) || !sha256(module.expected_sha256) {
            return Err(Error::InvalidModule(module.path.into()));
        }
        let top = module.name.split('.').next().unwrap_or("");
        let Some(root) = roots.get(top) else {
            return Err(Error::InvalidModule(module.path.into()));
        };
        let suffix = if module.is_package { "/__init__.py" } else { ".py" };
        let expected_path = join(&root.source_root, &format!("{}{suffix}", module.name.replace('.', "/")));
        if module.path != expected_path { return Err(Error::InvalidModule(module.path.into())); }
        if modules.insert(module.name, module).is_some() {
            return Err(Error::DuplicateModule(module.name.into()));
        }
        if !names.insert(module.name.to_ascii_lowercase()) || !paths.insert(module.path.to_ascii_lowercase()) {
            return Err(Error::PathAlias(module.path.into()));
        }
    }
    for package in roots.keys() {
        if !modules.get(package).is_some_and(|module| module.is_package) {
            return Err(Error::MissingPackage((*package).into()));
        }
    }
    for name in modules.keys() {
        let mut current = *name;
        while let Some((parent, _)) = current.rsplit_once('.') {
            if !modules.get(parent).is_some_and(|module| module.is_package) {
                return Err(Error::MissingPackage(parent.into()));
            }
            current = parent;
        }
    }
    Ok(modules)
}

fn valid_text(text: &str, max: usize) -> bool {
    !text.is_empty() && text.len() <= max && !text.chars().any(char::is_control)
}

fn sha256(text: &str) -> bool {
    text.len() == 64 && text.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

fn identifier(text: &str) -> bool {
    let mut bytes = text.bytes();
    bytes.next().is_some_and(|b| b.is_ascii_alphabetic() || b == b'_')
        && bytes.all(|b| b.is_ascii_alphanumeric() || b == b'_')
}

fn module_name(name: &str) -> bool {
    name.len() <= MAX_NAME && name.split('.').all(|part| identifier(part) && part != "__init__")
}

fn path(path: &str, directory: bool) -> bool {
    (directory && path == ".") || (valid_text(path, MAX_PATH) && path.is_ascii()
        && !path.contains('\\') && !path.contains(':') && !path.starts_with('/')
        && path.split('/').all(|part| !part.is_empty() && part != "." && part != ".."))
}

fn join(root: &str, suffix: &str) -> String {
    if root == "." { suffix.into() } else { format!("{root}/{suffix}") }
}

fn within(path: &str, root: &str) -> bool {
    path == root || path.strip_prefix(root).is_some_and(|rest| rest.starts_with('/'))
}

fn span(value: python::ByteSpan) -> Span {
    Span { start: value.start, end: value.end }
}

fn valid_span(span: python::ByteSpan, text: &str) -> bool {
    span.start < span.end && text.get(span.start as usize..span.end as usize).is_some()
}

fn context_digest(admission: Admission<'_>, roots: &BTreeMap<&str, &AdmittedRoot>) -> String {
    let mut digest = Fields::new("slophawk/python/admitted-imports/context/v1");
    for field in [SCHEMA_VERSION, ADMISSION_SCHEMA, imports::PROVIDER_ID,
        imports::SCHEMA_VERSION, python::PARSER_CRATE, python::PARSER_VERSION, SEMANTICS,
        admission.workspace_id, admission.environment_sha256, admission.policy_sha256]
    {
        digest.append(field.as_bytes());
    }
    digest.append(&[3, 12]);
    digest.append(&(roots.len() as u64).to_be_bytes());
    for root in roots.values() {
        digest.append(root.package.as_bytes());
        digest.append(root.source_root.as_bytes());
    }
    digest.finish()
}

// Streaming, length-prefixed fields avoid ambiguity and copying the source
// inventory into a second aggregate buffer. Hashes bind; they do not authorize.
struct Fields(Sha256);

impl Fields {
    fn new(domain: &str) -> Self {
        let mut result = Self(Sha256::new());
        result.append(domain.as_bytes());
        result
    }

    fn append(&mut self, bytes: &[u8]) {
        self.0.update((bytes.len() as u64).to_be_bytes());
        self.0.update(bytes);
    }

    fn finish(self) -> String {
        format!("{:x}", self.0.finalize())
    }
}
