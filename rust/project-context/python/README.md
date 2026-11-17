# Python project context / pytest receipts (SG-PROVIDER-07)

Standalone Rust library `slophawk_python_project_context` with the **typed
`python-project-context/v1` boundary**. It does not depend on the daemon, the
syntax provider or any TOML/pytest library. It has no imports, file-system
reads, subprocesses, network access, build scripts or runtime dependencies.
It is not a test executor, Python semantic resolver or an endorsed replacement
for pytest's collection machinery.

## Coordinator → read-only context

1. The coordinator authenticates a revision and an explicit workspace identity,
   chooses one or more **nonoverlapping** project roots, supplies original
   file-path/byte pairs from the admitted snapshot, and declares whether the
   inventory is `Complete` or `Partial`. All paths are normalized,
   workspace-relative POSIX strings; `.` is the whole workspace. Do not supply
   symlinks or silently follow them: the **coordinator** must verify paths
   resolve inside the authorized roots. This slice does not acquire filesystem
   handles or independently prove inventory completeness.
2. Call `analyze(WorkspaceScope, Vec<SnapshotFile>)`. It rejects traversal,
   absolute paths, duplicates, ambiguous overlapping scopes, off-scope files
   and configured size limits. It produces stable SHA-256 `scope`, `source`
   and `configuration` identities using length-prefixed, domain-separated
   byte fields. The source hash binds every supplied file including original
   bytes and exact relative path, not a rewritten or decoded representation.
   The configuration hash binds `pyproject.toml`, `pytest.ini`, `tox.ini`,
   `setup.cfg` and `conftest.py`. The relevant environment and plugin digests
   are **not** inferred from these files.
3. Each `ProjectFacts` includes an optional simple `[project].name`, package
   roots proved by a physical `__init__.py` in a root or `src/` layout, literal
   `testpaths` where the narrow `pyproject.toml` reader can safely identify
   them, default-name test-file hints and explicit `Gap`s. No imports happen.
   The TOML reader is intentionally **not a full TOML implementation**;
   multiline/escaped/complex values and custom collection are unresolved.
   Namespace packages, dynamic import/collection hooks and unreadable Python
   bytes are marked. The existing separately pinned Python syntax provider
   remains authoritative for syntax-derived facts, not test discovery.
4. Call `select_tests(context, baseline_source_sha256, changes)` with before
   and after paths (including deletes/renames). It selects **whole affected
   project roots**; no guessed reverse dependency edges or narrow node IDs are
   treated as exhaustive. `SelectionStatus::NeedsAdmittedCollection` requests
   worker collection, while `UnresolvedScope` blocks completeness whenever
   the snapshot was partial or a changed path leaves the admitted roots. Test
   hints may be empty while collection remains required. The baseline digest
   is supplied by the coordinator, not independently looked up here.

Budgets: 128 project roots, 4,096 files, 4 MiB per file, 32 MiB aggregate,
4,096 changes and 1,024 UTF-8 bytes per logical path. Snapshots are supplied
in memory; these are input-size limits, **not process RSS/latency bounds**.
A versioned, supervised worker is still required for production source parsing.

## Coordinator → admitted pytest collection → admitted execution

`classify_receipt(&SelectionContext, &Admission, &Observation)` validates a
single affected project root. Its `ReceiptBinding` includes an attempt nonce,
project root, scope/source/configuration/selection SHA-256 hashes, an
independently observed effective pytest configuration SHA-256, interpreter
binary SHA-256, environment SHA-256, pytest distribution SHA-256, plugin-set
SHA-256, and **ordered** `node_ids_digest` of exactly the IDs observed during
collection. The coordinator must compute these from the actual admitted
interpreter/venv, installed plugins and effective pytest configuration; never
replace unknown values with a constant or a filename guess. Digest equality is
**binding, not cryptographic authentication** of an untrusted worker.

The approved worker, **not this library**, is responsible for:

- Obtaining an execution grant and lease/fence for an immutable source
  revision; supervising CPU, memory, time and output; using an exclusive empty
  results directory and retaining provenance of the process and artifacts.
- Running pytest collection without narrowing `-k`, `-m`, `--ignore`, or a
  guessed list of test files. For the current boundary `Collected` must certify
  `full_project_scope = true`, `filtered = false`, final collection callbacks,
  and no collection errors. Any custom discovery, plugin, dynamic import or
  collection failure remains a gap. Worker collection may execute arbitrary
  `conftest.py`/plugin imports, so it **must never run during source analysis**.
- Capturing actual ordered node IDs from an admitted collection hook, not
  from parsed terminal output. Store the chosen pytest/pluggy version and
  hook adapter version in the upstream evidence protocol. Apply the approved
  filtering/selection policy *after* authoritative collection; for this first
  version, only complete project-root execution can be classified as passed.
- Running an admitted test session and folding each item's setup/call/teardown
  into one terminal `PytestCase`. A teardown/setup error dominates an earlier
  pass; unexpected pass (`XPassed`) is negative evidence, `XFailed` is treated
  as skipped-like, and statuses must be represented explicitly. The worker
  must not infer outcomes from aggregate stdout; it must observe hook results.
- Parsing bounded structured artifacts safely and supplying `Artifact`
  digests/byte counts/last-write times and normalized typed facts. This crate
  validates **internal consistency and provenance comparisons**; it does not
  parse untrusted JSON/XML bytes or prove that a supplied checksum matches a
  report that it was never given. A later wire parser needs separate tests.

For admitted collection, `pytest` exit `0` with nonempty collected IDs is
accepted; exit `5` with zero IDs and a complete unfiltered collection proves
`ZeroDiscovered` **for that admitted configuration**. A source-only empty hint
array, incomplete collection or a filtered run cannot prove it. Pytest exit
`1` with observed failed/error/XPASS terminal cases classifies `Failed`;
exit `0` with at least one pass and no negative outcomes classifies `Passed`;
exit `0` with all skips/expected failures classifies `AllSkipped`. Interrupted
or missing runners, startup/timeout/cancellation, other exits, missing case
outcomes and inconsistent counts have distinct nonpassing dispositions.
A successful classification is never synthesized from a process exit alone.

Admission and artifacts must agree on the attempt identity, source/config/env,
collection IDs and owned result directory. Artifacts must be modified within
the admitted process interval (plus a 2-second end tolerance) and execution
artifacts cannot precede collection artifacts. The adapter rejects duplicate,
escaping or malformed node IDs; uncollected or missing results; inconsistent
exit codes; and over-budget observations. Limits: 20,000 collected IDs,
512 bytes per ID, 2 MiB of ID/case text per stage, 2 MiB per artifact,
4,096 bytes of sanitized output excerpt and 256 bytes per failure excerpt.
The coordinator is responsible for pre-reading and bounding larger raw
reports, checking hashes against raw bytes and ensuring the worker is trusted.

**No production cutover:** The existing C# scheduler, approved execution
profile, VSTest classifier and source provider remain unchanged. This adapter
has no coordinator wiring or shared durable wire-format implementation. Any
Windows/Linux rollout is gated on coordinator parity, fault-injection and
matched full-process-tree PSS/RSS and latency measurements. The first revision
has no claim to equivalence with custom pytest collectors, arbitrary TOML,
namespace-package membership, dynamic imports, coverage, or test selection
from reverse call graphs. It reports those gaps instead of asserting complete
coverage.

## Offline build and fixtures

```sh
manifest=rust/project-context/python/Cargo.toml
export CARGO_TARGET_DIR="$(mktemp -d)"
cargo +1.99.1 fmt --manifest-path "$manifest" -- --check
cargo +1.99.1 build --locked --offline --manifest-path "$manifest"
cargo +1.99.1 test --locked --offline --manifest-path "$manifest"
cargo +1.99.1 clippy --locked --offline --manifest-path "$manifest" --all-targets -- -D warnings
```

No dependencies are needed in the lock; the required input is the pinned Rust
1.99.1 compiler/Cargo/rustfmt/clippy, native linker and libc development files.
The fixture suite exercises source identity, project layouts, multi-project
renames/deletes, partial inventory, dynamic imports, custom collection, missing
runners, zero discovery, all skipped, failures, inconsistent/stale evidence and
output limits. A separately admitted sandbox pytest 9.0.2 synthetic run
observed one pass, one skip, one failure; its outcomes are represented in the
Rust fixtures. That Python execution does **not** certify this Rust crate.

During integration on 17 November 2026, all 16 Rust tests passed on Windows
x64 with Rust 1.99.1. Formatting, locked offline all-target builds and Clippy
with warnings denied also passed. These checks exercise the typed pilot;
they do not establish a supervised pytest runner or release qualification.

## Primary rationale

- [Python `pyproject.toml` specification](https://packaging.python.org/en/latest/specifications/pyproject-toml/) — static metadata vs tool-owned tables.
- [pytest import mechanics](https://docs.pytest.org/en/stable/explanation/pythonpath.html) — collection can import source and mutate `sys.path`.
- [pytest exit codes](https://docs.pytest.org/en/stable/reference/exit-codes.html) — 0/1/2/3/4/5/6 are materially distinct.
- [pytest collection hooks](https://docs.pytest.org/en/stable/reference/reference.html) — hooks, plugins and modification/deselection make static discovery nonauthoritative.
- `docs/rust-checker-migration.md`, `rust/providers/python/README.md`, and
  `src/SlopHawk.Daemon/{TestRunReceiptDetails,VstestTrxClassifier}.cs` at pinned
  `a4399a0a199cd173a5b9d2048faaabfb093d1a96` — migration ownership and
  strict attempt/artifact fencing.
