# Delivery and acceptance

The [0.2 release design](version-0.2.md) specifies the next implementation increment and the evidence required before switching SlopGuard itself to 0.2. The broader feature sequence stays below.

## Implementation order

| Increment | Deliverable | Acceptance evidence |
| --- | --- | --- |
| 1. Local feedback | .NET 10 CLI, per-worktree daemon, native watcher and reconciliation, MCP bridge, shared index storage, current report exporter, Roslyn syntax checker, spelling and whitelist. | Two concurrent sessions share one daemon. File and directory saves, moves and deletes converge to the inventory. Report withdrawal and restart recovery work across Git transitions and concurrent saves. MCP handles configuration conflicts. |
| 2. Build and test coverage | Project and target graph, SDK resolver, build diagnostics, VSTest and Microsoft.Testing.Platform adapters, indexed symbol edges, test impact and CLI status. | SDK-style and classic Windows fixtures produce per-target evidence. C# changes trigger relevant tests, with a recorded selection reason. Unavailable toolsets yield explicit partial coverage. |
| 3. Duplication and deep inspection | Incremental clone index, Sonar C# profile, InspectCode integration, bounded value-flow queries, verification-needs rules and baseline comparison. | Edited and renamed clones report reproducible scores. Deleting either side clears the pair. A changed branch or value flow produces explainable coverage needs. Deep runs use bounded worker slots and keep quick checks responsive. |
| 4. Agent integration | Packaged skill and tool, local checkpoint commands, complete evidence catalog, public-API and repository rule checks, resource profiling. | The skill can start and use a daemon in a clean checkout. A checkpoint reports the verified revision and all required coverage, including blockers. All catalog entries have detection fixtures. Windows and Linux pass the applicable test suites. |

Keep the host, protocol contracts, workspace graph and worker providers separately testable. Start with builtin adapters. Add a third-party adapter contract once the compiler, test and inspection providers demonstrate a stable common surface.

## Correctness fixtures

| Area | Cases to cover |
| --- | --- |
| Watcher | Duplicate notifications, editor replace-by-rename, multiple saves in one batch, create/delete in a batch, case-only rename on Windows, populated directory move, generated-output storm, simulated overflow, locked file, root disappearance and cache recovery. |
| Compatibility | SDK-style .NET 10, at least one older SDK selected by `global.json`, multi-target conditional builds, .NET Standard library, classic Framework 4.6 and 4.8, and a missing legacy toolset. Expand the fixture matrix before claiming support for every target. |
| Scope | Multiple independent solutions, projects outside a solution, linked files, conditional project references, shared project imports, and an external linked file that requires explicit read access. |
| Test impact | Direct and transitive dependencies, deleted test, renamed fixture, changed shared fixture, misleading test name, no tests discovered, unsupported granularity, adapter crash and a project deferred by budget. Verify zero automatic invocations at startup and after metadata-only changes. |
| Symbol index | Direct and virtual calls, interface implementations, ambiguous reflection, recursive calls, overload changes, removed symbols, linked or generated sources, `ref` and `out` propagation, exceptions, and value origins or destinations across method boundaries. Compare warm update results with a full rebuild for the same context. |
| Verification needs | A positive detection fixture and misleading near-match per catalog entry, static reachability with missing branch evidence, failed test with valid coverage, outdated receipts, and a change that crosses a serialization or deployment boundary. Verify that manual and production checks stay evidence-only. |
| Results | Concurrent edit during build, obsolete result, zero findings, partial provider output, tool timeout, cancellation with shared demand, worker crash and daemon restart with running jobs. |
| Report publication | Checkout between branches at the same commit, detached HEAD, commit amend, reset, stash or restore, linked worktree, watcher overflow, rapid A to B to A changes, interrupted checkout and agent lease expiry. Verify withdrawn results stay unavailable and late workers cannot restore an old epoch. Exercise concurrent readers, atomic replacement failure, restart from leftover output, disk limits and unknown paths. |
| Agent marks | Concurrent agents mark the same and different targets, exercise every status, replay a request, and submit across a checkout. Verify stale and conflicting marks fail promptly. Simulate a reader holding the file, confirm MCP remains responsive and verify restart recovers a committed follow-up intent. |
| Duplication and spelling | Exact and renamed copies, partial and edited matches, repeated boilerplate, genuine algorithm differences, generated files, whitelist case and scope, and stale findings after deletion. |
| Permissions | Untrusted or unapproved execution, no implicit restore, escape through links or traversal, MCP override conflict, provider output with forged instructions, and concurrent jobs competing for an output directory. |

Use synthetic repositories and checked-in source fixtures. Unit-test event coalescing, impact selection and stale-result rules with a deterministic clock and input journal. Exercise real watchers and process launch through integration tests. Record toolchain versions and document which legacy combinations were exercised.

## Initial performance targets

These are engineering targets to validate, with hardware, OS and checkout profiles reported alongside measurements.

| Scenario | Proposed target |
| --- | --- |
| Idle after index warmup | Average below 0.5% of one logical core over five idle minutes and below 80 MiB private memory for one daemon. Report any polling fallback separately. |
| Incremental source feedback | p95 below 1 second from the final save to published local findings on a warm 100,000-line checkout, including the debounce window. |
| Multi-file agent patch | Keep event and job queue memory bounded. Republish a reconciled batch after a 100-file edit, and show any delayed deep work in status. |
| Duplicate index | Process the same fixture incrementally after a one-file edit and deletion. Report cold and warm time, peak memory and candidate recall against labeled fixtures. |
| Shared index and report limits | Repeated edits, branch changes and long queries respect cache, memory and publication limits. Verify only one current report file is retained, that truncation is reported per checker, and that evicted index partitions can be rebuilt. |
| Heavy jobs | One heavy worker slot per user by default. Report complete process-tree peak memory and deadlines. Leave the watcher and MCP responsive during a long build or inspection. |

Profile idle and load on Windows and Linux. Measure startup, save bursts, branch switches, watcher overflow, and parallel coding sessions. Compare daemon private memory, total process-tree memory, CPU time and feedback latency against a baseline before tightening defaults. Record results for large solutions and any limits needed to keep them responsive.
