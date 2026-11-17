# Admitted language execution (SG-MIG-12)

This is a **separate, opt-in migration worker**, not a scheduler registration, a
new daemon IPC revision, or a public command. The passive Python/Cargo project
context libraries still never launch repository code. Rust 1.99.1 and Linux are
the initial supervised target. The owner must compile and qualify this code
before enabling it from an authenticated coordinator. Existing .NET, MCP,
Roslyn-frame-v1, public argv and artifact contracts are unchanged.

## Coordinator contract

A trusted coordinator constructs `Job` with `format =
"slophawk.migration-job/1"`, a canonical physical root, immutable epoch and
revision, scoped source/input/config/tool/policy digests, owner-issued grant and
budget references, bounded input file ledger, and a typed operation. A **private
OS-user-owned profile outside the checkout** must attest all these values,
including SHA-256 of canonical JSON serializations of `inputs` and `operation`.
Neither the request nor a repository file can supply a grant. The owner
acquires its worker slot/output lease, commits the attempt, checks its root and
sources, and uses `run_admitted(profile_path, job, &cancelled)`. The method
verifies 0600 OS-user profile ownership under a private directory, hashes the
approved worker and tool binaries, verifies every declared file without
following symlinks, and launches a fresh worker process group in a 0700
exclusive attempt directory. Only a successful child, matching single evidence
frame plus complete terminal, unchanged profile, unchanged inputs and still
valid tool pins produce `Verified`. A refused, stale, missing or malformed
terminal discards staged output. `discard_staged` releases even successful
scratch artifacts. The caller **must revalidate lease, graph, policy and source
revision immediately before daemon publication**, and atomically publish the
result/invalidation or discard it. This crate does not publish daemon results.

The profile's JSON is authenticated by ownership, not by a user-provided
signature. Required field names are `root`, `context_sha256`, `epoch`,
`committed_revision`, `fence_sequence`, `input_sha256`, `source_sha256`,
`input_files_sha256`, `operation_sha256`, `config_sha256`,
`environment_sha256`, `policy_sha256`, `grant_ref`, `budget_ref`, `operations`
(list of permitted operation names), `tool`, `worker`, `artifacts_root`,
`budget`, `allow_build`, `allow_build_scripts`, `allow_proc_macros`. Executable
objects have `{ "path": "/canonical/exe", "sha256": "64 lowercase hex" }`.
The deadline is in milliseconds and byte limits are counts. **Cargo test**
requires explicit owner grants for build, scripts and proc macros, because the
command can execute them; metadata is a separate, non-build operation. Do not
infer these grants from a workspace manifest. The process environment is
cleared and network access is not requested (`--locked --offline`,
`CARGO_NET_OFFLINE=true`). The coordinator must compute the variant environment with
`cargo_environment_digest(&cargo, &rustc)` over the exact controlled
key/value recipe; the per-attempt Cargo scratch path is a normalized token.
The installer must provide an approved, directly
invocable Cargo/rustc pair; the pinned `CargoVariant.rustc` binary and
`rustc -Vv` bytes are separately verified. All implicit ancestor Cargo config
must be absent; root-local `.cargo/config[.toml]` must be included in the
source ledger if present. Unlisted inputs, toolchain config or dependency
registries yield an unresolved scope rather than complete evidence.

The wire is one **little-endian u32 length followed by UTF-8 JSON** per frame;
size is checked before allocating, with a 2,400,000-byte frame cap, 3-frame
and 5,200,000-byte total cap, one evidence and one terminal required on success.
Both frames echo the full `Identity` including test fence sequence. `complete`,
`partial`, `unsupported`, `deferred`, `cancelled`, `stale`, and `failed` are
explicit terminal statuses, with bounded reasons and omissions. A non-complete
terminal cannot satisfy a test or verification claim. Cargo failed tests and
pytest failed cases are valid *observed outcomes*, not successful checks.

## Language-specific use

- **Python:** Select project scopes with the published
  `slophawk_python_project_context::select_tests`. Collect the **entire**
  admitted root with CPython >=3.10, pytest **9.0.2** and
  pytest-json-report **1.5.0** in an owner-verified offline virtualenv.
  `-I`, `PYTEST_DISABLE_PLUGIN_AUTOLOAD=1`, a fixed explicit plugin, and
  disabled cache prevent implicit user-installed plugins or pytest cache writes.
  The owner binds interpreter bytes, installed wheel provenance, environment,
  `pytest_sha256`, `plugins_sha256`, source/config, effective pytest settings,
  authoritative collected node IDs and attempt. `classify_python` validates
  the context and forwards JSON collector and setup/call/teardown evidence to
  the existing conservative classifier. Missing/unknown fields, deselection,
  setup/teardown errors, skip, xfail, xpass, zero, and collection errors are
  not inferred from exit text. The plugin is third-party; no first-party
  SlopHawk Python app or plugin is installed.
- **Cargo:** For metadata use `context_sha256 = Binding::digest()`; for test
  use the existing graph's `context_sha256`. In both cases set
  `identity.source_sha256 = identity.input_sha256 =
  sha256(serde_json::to_vec(&job.inputs))`. Include Cargo.toml, Cargo.lock,
  all member manifests and relevant source/config in the owner ledger.
  `classify_cargo_metadata(verified, manifests, lock)` rejects a snapshot not
  in that ledger; `classify_cargo_test(verified, graph, job)` checks the exact
  graph, member, target, feature, epoch, attempt and **fence sequence** before
  using the existing Cargo JSON and libtest receipt classifier. No build
  scripts or macros run during passive interpretation. Metadata and test runs
  are separate, explicitly admitted worker actions; the test action may build.
- **FluxR:** Only `canonical_checked`, portable VM execution and pinned
  `slophawk-fluxr-checked-worker` are eligible. The source digest is
  `sha256(serde_json::to_vec(&modules))`; the input digest is SHA-256 of
  serialized ordered `cases`. Source hashes, compiler tree/revision and
  worker hash are enforced by the published checked harness. `check`,
  `observe`, or `replay` are supported. Compact/native codecs and native
  execution remain unsupported. The outer process-group deadline covers
  reads, checked compilation, the direct-child deadline and replay passes.

**Boundaries:** A process group is not a hard security sandbox: a child can
attempt to daemonize, create another process group, access files, or make
network requests. Production admission of untrusted repositories requires a
separate owner-configured OS isolation/network boundary and a supervisor that
contains the *entire* process tree; this crate does not authorize unattended
scheduling. Mutable local worktrees also need publication-time revalidation.
Symlinked input files, incomplete Cargo source/lock/config input inventories,
non-CPython runtimes, other pytest/plugin versions, unsupported/custom JSON
shapes, unknown generated macro inputs, and non-Linux targets remain unresolved
until separately qualified. Full daemon integration remains owner work.

## Verification (source only in this delivery)

Tests in `tests/protocol_contract.rs` and `src/python.rs` cover frame caps,
matching terminals, collection, deselection, and setup/call/teardown parsing.
Ignored `tests/real_workers.rs` contains real Python, two-package offline Cargo
and checked FluxR workers, with malformed/missing terminal, stale source,
codec and failure-neighbor fixtures. The owner must prepare pinned tools,
execute those tests under an admitted Linux worker, and review the resulting
receipts before enabling automatic routing. No tests or builds were executed
by the SG-MIG-12 recipient.
