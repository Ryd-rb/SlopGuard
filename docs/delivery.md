# Delivery and acceptance

The [roadmap](roadmap.md) sequences the coming year. The [0.2 release design](version-0.2.md) defines the next release. Keep host, protocol, workspace graph and built-in providers separately testable. Introduce a third-party adapter contract once multiple adapters demonstrate a stable common surface.

## Version 0.2 implementation gates

| Stage | Implementation boundary | Acceptance criteria |
| --- | --- | --- |
| Durable source inventory | Implemented in the development build. Route the existing watcher through the reconciliation coordinator and SQLite store. Expose committed revision and pending-intent scope. | Check invalidation, transaction failure, restart, quota fallback and incomplete source observations. |
| Durable scheduling | Implemented with an in-process fake adapter. Persist attempts, retry deadlines and per-target outcomes. Retire the standalone syntax pass. | Recover interrupted and exhausted attempts across restart. Preserve pending targets through supersession and outcome-write failure. Keep source reconciliation independent of compiler and analyzer execution. |
| Evaluated C# membership | Implemented for plain .NET and .NET Standard targets evaluated by SDK 10. Persist project target graphs and select eligible test targets with the fake runner. | Verify conditional and linked compile items, nearest and explicit target mappings, metadata replanning, predecessor recovery and partial scope. |
| Supervised execution | Implemented profile-bound worker admission and output leases. Windows Job Objects supervise process-backed work. The runner remains a fixture. | Verify grants and revocation, conflicting worktrees, cancellation of descendants and restart recovery with the old process scope still active. |
| Real related tests | Add the VSTest adapter, one eligible synthetic project and a SlopGuard unit project. Admit tests only after the execution profile and process supervision are active. | Observe a real automatic run on a confirmed edit and reuse a matching test receipt without rerunning it. Verify zero tests, all skipped, failing tests, prerequisite build failure, interrupted runs and blocked permissions independently. |
| Coverage and agent control | Import bound coverage, publish the revised report, expose marks and transition leases, and update the skill. | Verify aggregate versus per-test scope, stale or mismatched receipts, idempotent marks and concurrent disjoint targets. Exercise raw file sharing errors with validated CLI and MCP readers. Exercise self-hosting and rollback before release. |

Summarize fixture results with the commit, environment, checker and runner versions, input revision and expected and observed coverage in the change discussion. Keep generated receipts out of Git. Record whether each compatibility row passed, was blocked or is still untested. The runtime support matrix follows those results. Run the integration suite separately from automatic unit-test runs. Measure idle resource use and time from edit to committed intent with and without an active heavy worker.

Upgrade by stopping 0.1, starting the verified 0.2 build and observing source reconciliation and pending intents before enabling repository execution. Exercise the tool and report version handshake with each unsupported client/daemon pairing. For rollback, stop 0.2 and start 0.1 once the worker process scopes and output leases are clear. Keep 0.2 state outside the checkout so rollback preserves outstanding verification. The next 0.2 startup revalidates its state and recovers pending work.

## Next implementation stages

1. **Run related VSTest projects.** Register the first real runner with the supervised worker. Declare its required operations and output and intermediate roots for the selected target and its referenced projects. Reuse matching build outputs or receipts. Run the minimum prerequisite build when required by an authorized test job. Record the runner, exact target and test inputs and verify scoped fingerprints again before committing current evidence. Add an eligible self-hosting unit-test project and keep integration suites manual. [First runner adapter](checkers.md#first-runner-adapter)
2. **Explain impact and coverage.** Add bounded Roslyn symbol queries for change explanations and import matching coverage. Expose target selection reasons, receipt history and coverage gaps through MCP. Use synthetic fixtures to distinguish static reachability from observed coverage and to verify stale and blocked outcomes. [Coverage import](verification.md#coverage-import-for-02)

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
| Incremental source reconciliation | p95 below 1 second from the final save to the committed revision and intent status on a warm 100,000-line checkout, including the debounce window. |
| Multi-file agent patch | Keep event and job queue memory bounded. Republish a reconciled batch after a 100-file edit, and show any delayed deep work in status. |
| Duplicate index | Process the same fixture incrementally after a one-file edit and deletion. Report cold and warm time, peak memory and candidate recall against labeled fixtures. |
| Shared index and report limits | Repeated edits, branch changes and long queries respect cache, memory and publication limits. Verify only one current report file is retained, that truncation is reported per checker, and that evicted index partitions can be rebuilt. |
| Heavy jobs | One heavy worker slot per user by default. Report complete process-tree peak memory and deadlines. Leave the watcher and MCP responsive during a long build or inspection. |

Profile idle and load on Windows and Linux. Measure startup, save bursts, branch switches, watcher overflow, and parallel coding sessions. Compare daemon private memory, total process-tree memory, CPU time and feedback latency against a baseline before tightening defaults. Record results for large solutions and any limits needed to keep them responsive.
