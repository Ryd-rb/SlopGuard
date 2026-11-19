//! Owner-run development fixtures; authored and reviewed as source only.
//! Admission metadata is asserted only for these controlled in-memory regular
//! packages. SHA-256 expectations are computed from the captured fixture bytes.
//! The comparison pair is pinned from the initial produced fixtures to isolate
//! graph semantics; this is NOT a production currentness or authorization route.

use sha2::{Digest, Sha256};
use slophawk_python_import_producer::{
    ADMISSION_SCHEMA, Admission, CapturedModule, Error, Limits, Request, Requirement, produce,
};
use slophawk_python_project_context::import_layers::{
    self as layers, AdmittedRoot, Comparison, ComparisonStatus, FindingKind, ImportSnapshot,
    ImportTarget, ImportUse, LayerPolicy, Reason, SnapshotPair, UnsupportedKind, compare,
};
use slophawk_python_syntax_provider::{self as python, Outcome};

const ENVIRONMENT: &str = "1111111111111111111111111111111111111111111111111111111111111111";
const POLICY: &str = "2222222222222222222222222222222222222222222222222222222222222222";
const A: &str = "pkg.domain.a";
const UI: &str = "pkg.ui.a";

#[derive(Clone)]
struct Source {
    name: &'static str,
    path: &'static str,
    bytes: Vec<u8>,
    expected: String,
    is_package: bool,
}

fn hash(bytes: &[u8]) -> String { format!("{:x}", Sha256::digest(bytes)) }

fn source(name: &'static str, path: &'static str, bytes: &[u8], is_package: bool) -> Source {
    Source { name, path, bytes: bytes.into(), expected: hash(bytes), is_package }
}

struct Fixture { roots: Vec<AdmittedRoot>, modules: Vec<Source> }

fn fixture(importer: &[u8]) -> Fixture {
    Fixture {
        roots: vec![AdmittedRoot { package: "pkg".into(), source_root: "src".into() }],
        modules: vec![
            source("pkg", "src/pkg/__init__.py", b"", true),
            source("pkg.domain", "src/pkg/domain/__init__.py", b"", true),
            source("pkg.ui", "src/pkg/ui/__init__.py", b"", true),
            source(A, "src/pkg/domain/a.py", importer, false),
            source(UI, "src/pkg/ui/a.py", b"", false),
        ],
    }
}

fn admission(revision: &str) -> Admission<'_> {
    Admission {
        schema: ADMISSION_SCHEMA, workspace_id: "controlled-import-fixture", revision,
        environment_sha256: ENVIRONMENT, policy_sha256: POLICY,
        read_authorized: true, complete_inventory: true,
        unique_regular_roots: true, no_path_aliases: true,
    }
}

fn capture(
    fixture: &Fixture, admission: Admission<'_>, expected: &str, limits: Limits,
) -> Result<ImportSnapshot, Error> {
    let modules: Vec<_> = fixture.modules.iter().map(|module| CapturedModule {
        name: module.name, path: module.path, bytes: &module.bytes,
        expected_sha256: &module.expected, is_package: module.is_package,
    }).collect();
    produce(Request { admission, roots: &fixture.roots, modules: &modules }, expected, limits)
}

fn snapshot(fixture: &Fixture, revision: &str) -> ImportSnapshot {
    capture(fixture, admission(revision), revision, Limits::default()).unwrap()
}

fn policy() -> LayerPolicy {
    LayerPolicy { layers: vec![vec!["pkg.ui".into()], vec!["pkg.domain".into()]] }
}

fn pair(before: &ImportSnapshot, after: &ImportSnapshot) -> SnapshotPair {
    // Fixture-only pinning: never use the candidate as its own production expectation.
    SnapshotPair { before: before.identity.clone(), after: after.identity.clone() }
}

fn compare_fixture(before: &ImportSnapshot, after: &ImportSnapshot) -> Comparison {
    compare(before, after, &pair(before, after), &policy(), layers::Limits::default())
}

fn no_regression(report: Comparison) {
    assert_eq!(report.status, ComparisonStatus::Complete);
    assert!(report.findings.is_empty());
    assert!(report.frontiers.is_empty());
}

fn unknown(report: &Comparison, reason: Reason) {
    assert_eq!(report.status, ComparisonStatus::Unknown);
    assert!(report.findings.is_empty());
    assert!(report.frontiers.iter().any(|frontier| frontier.reason == reason));
}

#[test]
fn a_new_forbidden_import_has_a_real_witness_even_when_its_alias_is_unused() {
    let text = b"import pkg.ui.a as never_used\n";
    let before = snapshot(&fixture(b""), "before");
    let after = snapshot(&fixture(text), "after");
    let report = compare_fixture(&before, &after);
    assert_eq!(report.status, ComparisonStatus::Complete);
    assert_eq!(report.findings.len(), 1);
    let finding = &report.findings[0];
    assert_eq!((finding.kind, finding.source.as_str(), finding.target.as_str()),
        (FindingKind::ForbiddenPath, A, UI));
    assert_eq!(finding.witness.len(), 1);
    let witness = &finding.witness[0];
    assert!(witness.added);
    assert_eq!(witness.path, "src/pkg/domain/a.py");
    assert_eq!(witness.source_sha256, hash(text));
    assert_eq!(&text[witness.span.start as usize..witness.span.end as usize],
        b"import pkg.ui.a as never_used");
    assert_eq!(after.imports[0].use_kind, ImportUse::Runtime);
}

#[test]
fn existing_debt_moved_spans_and_deleted_edges_do_not_become_new_findings() {
    let before = snapshot(&fixture(b"import pkg.ui.a\n"), "before");
    for text in [b"import pkg.ui.a as unused\n".as_slice(),
        b"# moved\r\n\r\nimport pkg.ui.a\r\n".as_slice(), b"".as_slice()]
    {
        let after = snapshot(&fixture(text), "after");
        assert_ne!(before.identity.source_sha256, after.identity.source_sha256);
        no_regression(compare_fixture(&before, &after));
    }
}

#[test]
fn newly_added_modules_only_create_debt_when_imported() {
    let before = snapshot(&fixture(b""), "before");
    for imported in [false, true] {
        let text = if imported { b"import pkg.ui.new\n".as_slice() } else { b"".as_slice() };
        let mut candidate = fixture(text);
        candidate.modules.push(source("pkg.ui.new", "src/pkg/ui/new.py", b"", false));
        let report = compare_fixture(&before, &snapshot(&candidate, "after"));
        if imported {
            assert_eq!(report.status, ComparisonStatus::Complete);
            assert_eq!(report.findings.len(), 1);
            assert_eq!(report.findings[0].target, "pkg.ui.new");
            assert!(report.findings[0].witness[0].added);
        } else { no_regression(report); }
    }
}

#[test]
fn package_initializer_bom_and_crlf_witnesses_use_original_bytes() {
    let text = b"\xef\xbb\xbf# header\r\nimport pkg.ui\r\n";
    let before = snapshot(&fixture(b""), "before");
    let mut candidate = fixture(b"");
    candidate.modules[1] = source("pkg.domain", "src/pkg/domain/__init__.py", text, true);
    let after = snapshot(&candidate, "after");
    let report = compare_fixture(&before, &after);
    assert_eq!(report.status, ComparisonStatus::Complete);
    assert_eq!(report.findings.len(), 1);
    let witness = &report.findings[0].witness[0];
    assert_eq!((witness.from.as_str(), witness.to.as_str()), ("pkg.domain", "pkg.ui"));
    assert_eq!(witness.path, "src/pkg/domain/__init__.py");
    assert_eq!(witness.span.start as usize, b"\xef\xbb\xbf# header\r\n".len());
    assert_eq!(&text[witness.span.start as usize..witness.span.end as usize], b"import pkg.ui");
    assert_eq!(witness.source_sha256, hash(text));
    let package = after.modules.iter().find(|module| module.name == "pkg.domain").unwrap();
    assert!(package.is_package);
    assert_eq!(package.byte_len as usize, text.len());
}

#[test]
fn inventory_order_does_not_change_output_or_identity() {
    let mut input = fixture(b"import pkg.ui.a\n");
    input.roots.push(AdmittedRoot { package: "util".into(), source_root: "lib".into() });
    input.modules.push(source("util", "lib/util/__init__.py", b"", true));
    let original = snapshot(&input, "same-revision");
    input.roots.reverse();
    input.modules.reverse();
    assert_eq!(snapshot(&input, "same-revision"), original);
}

#[test]
fn a_missing_target_blocks_even_an_unrelated_valid_violation() {
    let before = snapshot(&fixture(b""), "before");
    let after = snapshot(&fixture(b"import missing.module\nimport pkg.ui.a\n"), "after");
    assert!(!after.complete);
    assert_eq!(after.imports.len(), 2);
    assert_eq!(after.imports[0].target, ImportTarget::Unknown(UnsupportedKind::UnresolvedTarget));
    assert_eq!(after.imports[1].target, ImportTarget::Internal(UI.into()));
    let report = compare_fixture(&before, &after);
    unknown(&report, Reason::IncompleteSnapshot);
    unknown(&report, Reason::Unsupported(UnsupportedKind::UnresolvedTarget));
}

#[test]
fn from_relative_reexports_guards_and_dynamic_calls_retain_unknown_frontiers() {
    let before = snapshot(&fixture(b""), "before");
    for (text, expected) in [
        ("from pkg.ui.a import Service\n", UnsupportedKind::AmbiguousReexport),
        ("from pkg.ui import a as public_api\n", UnsupportedKind::AmbiguousReexport),
        ("from . import a\n", UnsupportedKind::UnsupportedSyntax),
        ("from ..ui import a\n", UnsupportedKind::UnsupportedSyntax),
        ("if TYPE_CHECKING:\n    import pkg.ui.a\n", UnsupportedKind::UnresolvedGuard),
        ("if TC:\n    import pkg.ui.a\n", UnsupportedKind::UnresolvedGuard),
        ("from typing import TYPE_CHECKING\nif TYPE_CHECKING:\n    import pkg.ui.a\n",
            UnsupportedKind::UnresolvedGuard),
        ("__import__('pkg.ui.a')\n", UnsupportedKind::UnsupportedSyntax),
        ("loader('pkg.ui.a')\n", UnsupportedKind::UnsupportedSyntax),
    ] {
        let after = snapshot(&fixture(text.as_bytes()), "after");
        assert!(!after.complete, "{text}");
        assert!(after.imports.is_empty(), "{text}");
        assert!(after.unsupported.iter().any(|fact| fact.kind == expected), "{text}");
        unknown(&compare_fixture(&before, &after), Reason::Unsupported(expected));
    }
}

#[test]
fn all_four_admissions_are_required_including_namespace_and_alias_exclusion() {
    let input = fixture(b"");
    for requirement in [Requirement::ReadPermission, Requirement::CompleteInventory,
        Requirement::UniqueRegularRoots, Requirement::NoPathAliases]
    {
        let mut declaration = admission("r");
        match requirement {
            Requirement::ReadPermission => declaration.read_authorized = false,
            Requirement::CompleteInventory => declaration.complete_inventory = false,
            Requirement::UniqueRegularRoots => declaration.unique_regular_roots = false,
            Requirement::NoPathAliases => declaration.no_path_aliases = false,
        }
        assert_eq!(capture(&input, declaration, "r", Limits::default()),
            Err(Error::AdmissionRequired(requirement)));
    }
}

#[test]
fn stale_revision_bytes_and_malformed_metadata_are_unavailable() {
    let mut input = fixture(b"import pkg.ui.a\n");
    assert_eq!(capture(&input, admission("old"), "current", Limits::default()), Err(Error::StaleRevision));
    input.modules[3].bytes.push(b' '); // Preserve the independent capture expectation.
    assert_eq!(capture(&input, admission("r"), "r", Limits::default()),
        Err(Error::HashMismatch("src/pkg/domain/a.py".into())));
    input.modules[3].expected = "a".repeat(63);
    assert_eq!(capture(&input, admission("r"), "r", Limits::default()),
        Err(Error::InvalidModule("src/pkg/domain/a.py".into())));
    let input = fixture(b"");
    let mut declaration = admission("r");
    declaration.schema = "python-unique-regular-inventory/v0";
    assert_eq!(capture(&input, declaration, "r", Limits::default()), Err(Error::InvalidIdentity("admission schema")));
    declaration = admission("r");
    declaration.environment_sha256 = "unverified";
    assert_eq!(capture(&input, declaration, "r", Limits::default()), Err(Error::InvalidIdentity("environment digest")));
}

#[test]
fn compare_still_requires_current_and_compatible_independent_expectations() {
    let before = snapshot(&fixture(b""), "before");
    let mut after = snapshot(&fixture(b"import pkg.ui.a\n"), "after");
    let pinned = pair(&before, &after);
    after.identity.revision = "stale-observation".into();
    unknown(&compare(&before, &after, &pinned, &policy(), layers::Limits::default()), Reason::StaleSnapshot);
    let mut declaration = admission("after");
    declaration.policy_sha256 = ENVIRONMENT;
    let changed = capture(&fixture(b"import pkg.ui.a\n"), declaration, "after", Limits::default()).unwrap();
    unknown(&compare_fixture(&before, &changed), Reason::IncompatibleContext);
}

#[test]
fn ambiguous_inventory_and_missing_regular_packages_are_rejected() {
    for case in 0..6 {
        let mut input = fixture(b"");
        let expected = match case {
            0 => {
                input.roots.push(AdmittedRoot { package: "pkg".into(), source_root: "vendor".into() });
                Error::AmbiguousRoot("pkg".into())
            }
            1 => { input.modules.push(input.modules[3].clone()); Error::DuplicateModule(A.into()) }
            2 => {
                input.modules.push(source("pkg.domain.A", "src/pkg/domain/A.py", b"", false));
                Error::PathAlias("src/pkg/domain/A.py".into())
            }
            3 => { input.modules.retain(|module| module.name != "pkg.domain"); Error::MissingPackage("pkg.domain".into()) }
            4 => { input.modules.retain(|module| module.name != "pkg"); Error::MissingPackage("pkg".into()) }
            5 => { input.modules[3].path = "src/pkg/domain/../a.py"; Error::InvalidModule("src/pkg/domain/../a.py".into()) }
            _ => unreachable!(),
        };
        assert_eq!(capture(&input, admission("r"), "r", Limits::default()), Err(expected));
    }
}

#[test]
fn byte_module_and_fact_caps_return_no_partial_snapshot() {
    let input = fixture(b"import pkg.ui\nimport pkg.ui.a\n");
    for (limits, expected) in [
        (Limits { max_modules: 4, ..Limits::default() }, Error::LimitExceeded("module count")),
        (Limits { max_source_bytes: 1, ..Limits::default() }, Error::LimitExceeded("snapshot bytes")),
        (Limits { max_facts: 1, ..Limits::default() }, Error::LimitExceeded("import/unsupported facts")),
        (Limits { per_file: python::Limits { max_source_bytes: 1, ..Limits::default().per_file }, ..Limits::default() },
            Error::LimitExceeded("file bytes")),
        (Limits { max_modules: 0, ..Limits::default() }, Error::InvalidLimits),
    ] {
        assert_eq!(capture(&input, admission("r"), "r", limits), Err(expected));
    }
    for per_file in [python::Limits { max_tokens: 1, ..Limits::default().per_file },
        python::Limits { max_facts: 1, ..Limits::default().per_file }]
    {
        assert!(matches!(capture(&input, admission("r"), "r", Limits { per_file, ..Limits::default() }),
            Err(Error::Extraction { outcome: Outcome::ResourceLimit, .. })));
    }
}

#[test]
fn malformed_syntax_and_encoding_are_unavailable_instead_of_clean() {
    for (bytes, outcome) in [
        (b"import\n".as_slice(), Outcome::InvalidSyntax),
        (b"\xff".as_slice(), Outcome::InvalidEncoding),
        (b"# coding: latin-1\npass\n".as_slice(), Outcome::UnsupportedEncoding),
    ] {
        assert_eq!(capture(&fixture(bytes), admission("r"), "r", Limits::default()),
            Err(Error::Extraction { path: "src/pkg/domain/a.py".into(), outcome }));
    }
}
