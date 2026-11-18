# Rust migration: engine selection, state transfer and release qualification

SG-MIG-14 provides explicit state transfer to the installed Rust peer and a
rollback path. The Rust resident daemon serves the transferred workspace and
the public MCP tool projection through authenticated local IPC.
Default native promotion requires the compatible daemon and MCP surface, the
full parity matrix, and matched performance evidence below.

## Invocation and trust

An owner first builds the intended release, captures *exact-binary* evidence on each
OS, and supplies an **external**, physical, SHA-256-pinned profile and qualification
receipt. There is no ambient `SLOPHAWK_ENGINE`/repository-file override and no
silent fallback when an explicit native selection is unsupported.

```text
slophawk engine transfer /absolute/outside/checkout/native-profile.json PROFILE_SHA256 --root <checkout>
slophawk engine native /absolute/outside/checkout/native-profile.json PROFILE_SHA256 -- start --root <checkout>
slophawk engine native /absolute/outside/checkout/native-profile.json PROFILE_SHA256 -- status --root <checkout>
slophawk engine native /absolute/outside/checkout/native-profile.json PROFILE_SHA256 -- recheck --root <checkout>
slophawk engine native /absolute/outside/checkout/native-profile.json PROFILE_SHA256 -- mcp --root <checkout>
slophawk engine rollback /absolute/outside/checkout/native-profile.json PROFILE_SHA256 --root <checkout>
```

Profile example (all paths absolute, physical and outside the checkout):

```json
{
  "schemaVersion": 1,
  "platform": "windows",
  "binaryPath": "C:\\SlopHawk-release\\slophawk-native.exe",
  "binarySha256": "<64 uppercase hex digits>",
  "receiptPath": "C:\\SlopHawk-release\\qualification-receipt.json",
  "receiptSha256": "<64 uppercase hex digits>",
  "grantReference": "owner-reviewed-release-2026-11"
}
```

The receipt format is `schemaVersion`, `platform`, `binarySha256`, `legacyBinarySha256`,
`matrixComplete`, `compatibilityComplete`, `containmentComplete`,
`recoveryComplete`, `performanceComplete`, `qualifiedCommands`,
`qualifiedFormats`, `receiptId` (JSON camel case). Missing/false gates,
OS mismatch, a changed profile/receipt/executable, unqualified public commands,
linked file paths, or an unrecognized command are errors. These receipt bits
represent **owner-run evidence**, not evidence the selection code can invent.
The digest on the invocation is explicit operator intent; neither repository text
nor an unsigned receipt grants execution permissions. The daemon's existing
`ExecutionProfile` and worker admission checks remain independent authorities.

## Transition and rollback

The active daemon owns `.slophawk/results.json` (schema v2) and supplies the
MCP workspace tools. The state transfer has its own framed worker protocol.
During rollback the marker withdraws native authority, the resident daemon
closes its endpoint and releases its writer lock, and the export waits for
that lock before handing the snapshot back.

1. Request the current **matching legacy** daemon to Stop, then acquire the
   existing named single-writer mutex. If Stop fails, the lock does not drain,
   root identity changes, or worker admission cannot prove its prior process
   scope was drained, abort. An unrecognized local schema or SQL layout blocks
   before any irreversible operation.
2. Read schema **v5** in one SQLite read transaction. Export explicit ordered
   columns and typed SQLite cells for `workspace`, `source`, `scope`,
   `test_intent`, `intent_fallback`, `test_attempt`, `test_evidence`,
   `project_graph` and `test_run_receipt`. Running attempts become
   `interrupted` in **the export**, retaining lease/profile IDs and retry
   causes. Graph predecessors, revision, receipts, recoverable identities and
   uncertainty scopes retain their values. The snapshot limit is 16 MiB.
3. Persist an outside-checkout recovery snapshot and a fail-closed writer
   marker **before** admitting a native importer. The marker blocks accidental
   managed restart even if a peer fails mid-handoff. These files must be treated
   as retained recovery evidence; do not delete them while ownership is in doubt.
4. Start the exact pinned peer with `--slophawk-migration-job-v1`. Send bounded
   framed semantic snapshot records, require an identical terminal identity,
   `complete`, no omissions, verified digest, confirmed process drainage and
   native writer claim. **No directory copy or SQLite file copy** transfers
   semantic ownership. A peer lacking the protocol is incompatible.
5. Only after accepted import mark native authority. Rust `recheck` uses the
   bounded workspace scanner to commit source changes to the native snapshot.
   A C# source change extends the conservative workspace test fallback and
   preserves detailed prior causes, graph predecessors and receipts. An
   incomplete observation refuses reconciliation. On rollback, require the
   same qualified peer to `export_and_stop_v1` and attest process drainage and
   release of its native writer. Atomically import the validated semantic
   snapshot into the managed schema-v5 store under the named lock, checking
   root, committed revision and all required tables. Remove the writer marker
   only after the durable import commits. Restarting managed reconciliation
   creates a fresh publication epoch and rebuilds derived caches.

The marker records `transition-uncertain`, `native`, or `rollback-uncertain`.
Rollback can abort an interrupted import using the exact same peer and can
retry an interrupted export. The original backup is retained per epoch for
forensic recovery. Transitions to or from newer durable schemas require a new
interchange version.

### Scheduled source checks

To enable the bounded resident source checker, put `native-checkers.json` in
the private state directory that contains `migration-engine.json`. The
version-1 document has `version`, `rootIdentity`, `context` and optional
`worker` members. Copy `rootIdentity` from the current writer marker and use
the documented shadow checker context. If the source closure contains C#,
provide `worker.dotnet` and `worker.assembly` as `{path, sha256}` pins and an
external reference pack in `context.referencePack`. Keep the profile readable
and writable by the owner only. The other source checkers are built into Rust.

The scheduler binds all input paths and digests, the Git observation, profile
bytes and workspace revision before admitting an attempt. It checks this
binding again before publishing the receipt. An interrupted attempt becomes
eligible to rerun only after the writer and worker scopes are recovered. An
unchanged completed or partial receipt is reused after restart. View the
current attempt and report through `status` or MCP `workspace_status` under
`checkerScheduler`. The latest receipt in `native-<epoch>.checks.json` is
retained after rollback. Managed executable-test data continues to use its
schema-v5 intent and receipt tables.
On Linux, a bounded Git HEAD probe can wake the reconciliation loop on a
branch switch between full source scans. The full observation still validates
the Git context and source digests before publishing.

This scheduler covers the existing shadow checker's bounded source syntax
and clone rules. Larger or incomplete source closures are explicitly blocked.
Executable test selection, scalar replay and test receipts have their own
qualification requirements.

### Scheduled executable tests

The native resident writer reads `native-tests.json` in the same private
state directory as the migration marker. The version-1 registration contains
`version`, `rootIdentity`, the pinned language `worker` with its `path` and
`sha256`, a worker `budget`, and a `providers` array.
The [live scheduler fixture](../rust/frontends/public-shell/src/scheduled_tests.rs)
shows a registration for Python, Cargo and FluxR.

Python selects the registered project and collects its full pytest scope.
Cargo evaluates the root workspace manifest and conservatively selects up
to sixteen workspace packages. Use a variant valid for every selected
package. FluxR executes the registered checked modules and ordered cases.
Pin the worker and tools and set the provider-specific grants as described
in the [worker contract](../rust/workers/language-execution/README.md).

The writer persists a running attempt before launching a worker. Restart
recovers the worker scope first and marks an unfinished attempt interrupted.
It records provider receipts only for the same root, Git context, source
inventory and grant. View the latest batch under `testScheduler` in daemon
status. Its ledger lives in `native-<epoch>.tests.json` and provider captures
remain in private attempt directories. This registration does not resolve
managed schema-v5 causes or establish C# executable-test parity.

### Migration job frame contract

Protocol `slophawk.migration-job/1`: every frame is little-endian `int32`
length (1..131072) followed by UTF-8 JSON. Request identity has `protocol`,
`operation`, `job`, `root`, `rootIdentity`, `epoch`, `committedRevision`,
`inputSha256`, `toolSha256`, `policySha256`, `budgetRef`, `grantRef`, `stateDirectory`.
First `kind=request` binds input byte count and digest. For export
(`export_and_stop_v1`) the input is empty and the job input digest is the
SHA-256 of empty bytes, while the retained managed pre-handoff snapshot is
independently checked against the backup digest in the writer marker. Ordered `kind=chunk`
frames carry base64-encoded 72-KiB raw chunks; `kind=end` seals the input.
The peer may stream ordered export chunks, then exactly one `kind=terminal` with
**the same full identity**, `status`, `reason`, `omissions`, `payloadSha256`,
`drained` and `writerClaimed`. The terminal must precede a zero process exit;
absent/duplicate/malformed/mismatched terminals discard staged output.
`status` is one of `complete`, `partial`, `unsupported`, `deferred`,
`cancelled`, `stale`, `failed`; reasons <=240 characters and omissions <=32
bounded entries. `import_state_v1` claims native authority, `inspect_state_v1`
and `reconcile_state_v1` require that authority, and `export_and_stop_v1` and
`abort_import_v1` seal the epoch before returning authority to managed. Each
operation is fenced by the current writer marker and exact executable digest.

## Comparison matrix

Use `tools/SlopHawk.RustMigrationQualification` on a **pair** of comparable,
owner-built exact binaries. Every matrix cell has a distinct `id`, command,
provider, workflow, format, legacy/native artifact paths and explicit statuses.
Both statuses must be `complete` and results must compare exactly after removing
only `$.timing.startedUtc`, `$.timing.elapsedMs`, `$.telemetry.pid`, and
`$.telemetry.sampleUtc`. MCP comparisons decode JSON returned as tool text so
key order and escaping do not affect the comparison. In TRX the `Times` element's run-level
`creation`, `queuing`, `start` and `finish` attributes are the only XML
normalizations. All other XML content is compared structurally. The normalizer
**never** drops durable run IDs, receipt
provenance, source spans, finding order, comparison ordering or partial coverage.

Required command cells cover `start/status/results/projects/recheck/stop/mcp`,
`probes plan/compare/methods`, `contracts qualify`, `paths build/plan/check/review`,
`check/check-contracts/check-observe/check-retest/check-review`, and MCP
`initialize/tools.list/get_findings/run_checks/workspace_status`. Required
provider/workflow cross product is **C#, Python, Rust, FluxR** times
`discovery/syntax/clone-spelling/project-context/test-selection/receipt/scalar-replay/reporting`.
Required artifact formats are `results.schema-v2`, `mcp.jsonrpc`,
`checks.report-v1`, `contracts.qualify-v1`, `paths.plan-v1`,
`paths.review-v1`, `trx-v1`, `receipts.v1`, `inventory.v5`,
`migration-state.v1`. Missing cells, an incomplete engine, unsupported
provider, changed format or missing artifacts cannot count as pass. The
baseline's `.cs`-only source inventory and `awaiting_adapter` state must be
reported honestly; a Rust clone-only parity receipt cannot qualify four
provider workflows or the release.

## Process-tree memory and latency gates

The sampler offers `sample <spec.json> <capture.json>`. The
qualifier consumes all evidence as
`qualify <evidence.json> <release-receipt.json> <analysis.json>`.
`benchmark-resident <managed-executable> <native-executable> <new-scratch>`
captures paired cold startup, single edit, 100-file burst, same-commit branch
switch, five-minute active idle and warm reconciliation measurements against
a 100-source Git fixture. Both `benchmark-resident` and
`benchmark-resident-events` also capture a scoped native-to-managed rollback
and a managed-to-managed restart on the same fixture. The scope includes the
old writer, migration peer and replacement writer. These commands retain
process-scope evidence and end-to-end latencies. Checker-process peaks require
captures named `checker` that admit comparable checker workloads in both
engines and include their worker scopes.
`benchmark-checker <legacy> <native> <fixture-dll> <new-scratch>` exercises
the same managed batch adapter through each public foreground `check` route.
Windows samples nested Job membership. Linux places the checker and the
independently supervised adapter service in one measured user slice.
Samples retain the raw scope total and the resident bytes of the separately
pinned measurement controller. The checker peak compares the application
tree with that controller's bytes excluded. Adapter processes and their
descendants remain in scope. Job and cgroup peaks cover the full scope.
Owner harnesses must prepare a stable checkout/rule/policy/toolchain/fixture
closure and run matched cold, warm, **five-minute idle**, single edits,
**100-file burst**, branch switches and native-to-managed rollback, on **both
Windows and Linux**. Put input digests, the two exact `binarySha256` and `legacyBinarySha256`
values, environment,
external execution grants, source revisions and event latencies in the evidence.
The sampler never invents a latency event or declares a missing platform pass.

* **Windows**: sum `PROCESS_MEMORY_COUNTERS_EX2.PrivateWorkingSetSize` as
  resident private memory, `PagefileUsage` as per-process commit; record
  `JOBOBJECT_EXTENDED_LIMIT_INFORMATION.PeakJobMemoryUsed` as a separate Job
  peak. The Windows sampler creates the Job *before* resuming the suspended
  root process, enforces kill-on-job-close and verifies the active count on
  drain. Older Windows without EX2 is unqualified, not estimated from another
  metric. Nested existing jobs, failure to assign, missing children or counter
  reads block that capture.
* **Linux**: sum `/proc/<pid>/smaps_rollup` **Pss** over the tracked scope, and
  separately record `/sys/fs/cgroup/<scope>/memory.peak`. Cgroup membership and
  proven enroll-before-exec are required. The sampler uses a transient user
  service and validates the invocation and kernel cgroup through drainage.
  Measurements involving workers in independent units need a scope that
  includes those worker units.
  Missing delegation, inaccessible PSS, a changed scope or absence of
  `memory.peak` blocks qualification. Do not relabel RSS as PSS.

Each OS is gated separately: native warm idle and checker peak resident memory
**at least 30% lower** than managed, **no more than 10% p95 latency regression**,
**warm reconciliation p95 < 1000 ms**, and **Windows daemon private working set
< 80 MiB**. Keep Windows commit/Job peak and Linux cgroup peak as independent
budgets, not interchangeable resident-memory measurements. Missing samples,
whole-tree scope, latency event counts, OS or exact binary => `unqualified`.
The proof gates for compatibility, containment and recovery must be complete,
OS-matched and identified by nonempty digest lists. An owner should inspect
missing cases and raw evidence rather than promoting based on one percentage.

## Qualification runs

The GitHub Actions `Rust migration qualification` workflow exercises Linux
systemd worker exchanges, coordinator restart recovery, four-language checker
fixtures and the real Rust state peer. Those focused checks provide part of the
release evidence. Run the matched parity and whole-process-tree measurements
for both release platforms before default promotion.

Design references, as inspected at the pinned source on 2026-11-17:
`docs/rust-checker-migration.md`, `docs/robustness.md`, `docs/result-publication.md`,
`src/SlopHawk.Daemon/SourceInventory.cs` schema v5,
`SourceInventory.Scheduling.cs` and `TestRunReceiptDetails.cs` format v1.
Platform definitions:
[Windows process memory EX2](https://learn.microsoft.com/en-us/windows/win32/api/psapi/ns-psapi-process_memory_counters_ex2)
(Windows 10 22H2 September 2023 update or newer),
[Windows Job extended limits](https://learn.microsoft.com/en-us/windows/win32/api/winnt/ns-winnt-jobobject_extended_limit_information),
[Linux procfs](https://docs.kernel.org/filesystems/proc.html), and
[Linux cgroup v2](https://docs.kernel.org/admin-guide/cgroup-v2.html).
