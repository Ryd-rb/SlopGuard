# Delivery and acceptance

## Implementation order

| Increment | Deliverable | Acceptance evidence |
| --- | --- | --- |
| 1. Local feedback | .NET 10 CLI, per-worktree daemon, native watcher and reconciliation, MCP bridge, result store, Roslyn syntax checker, spelling and whitelist. | Two concurrent sessions share one daemon. File and directory saves, moves and deletes converge to the inventory. Findings clear on deletion. MCP controls handle restart and configuration conflicts. |
| 2. Build and test coverage | Project and target graph, SDK resolver, build diagnostics, VSTest and Microsoft.Testing.Platform adapters, test impact and CLI status. | SDK-style and classic Windows fixtures produce per-target evidence. C# changes trigger relevant tests, with a recorded selection reason. Unavailable toolsets yield explicit partial coverage. |
| 3. Duplication and deep inspection | Incremental clone index, Sonar C# profile, InspectCode integration, rule coverage reporting and baseline comparison. | Edited and renamed clones report reproducible scores. Deleting either side clears the pair. Deep runs use bounded worker slots and keep quick checks responsive. |
| 4. Agent integration | Packaged skill and tool, local checkpoint commands, public-API and repository rule checks, resource profiling. | The skill can start and use a daemon in a clean checkout. A checkpoint reports the verified revision and all required coverage, including blockers. Windows and Linux pass the applicable test suites. |

Keep the host, protocol contracts, workspace graph and worker providers separately testable. Start with builtin adapters. Add a third-party adapter contract once the compiler, test and inspection providers demonstrate a stable common surface.

## Correctness fixtures

| Area | Cases to cover |
| --- | --- |
| Watcher | Duplicate notifications, editor replace-by-rename, multiple saves in one batch, create/delete in a batch, case-only rename on Windows, populated directory move, generated-output storm, simulated overflow, locked file, root disappearance and cache recovery. |
| Compatibility | SDK-style .NET 10, at least one older SDK selected by `global.json`, multi-target conditional builds, .NET Standard library, classic Framework 4.6 and 4.8, and a missing legacy toolset. Expand the fixture matrix before claiming support for every target. |
| Scope | Multiple independent solutions, projects outside a solution, linked files, conditional project references, shared project imports, and an external linked file that requires explicit read access. |
| Test impact | Direct and transitive dependencies, deleted test, renamed fixture, changed shared fixture, misleading test name, no tests discovered, unsupported granularity, adapter crash and a project deferred by budget. Verify zero automatic invocations at startup and after metadata-only changes. |
| Results | Concurrent edit during build, obsolete result, zero findings, partial provider output, tool timeout, cancellation with shared demand, worker crash and daemon restart with running jobs. |
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
| Heavy jobs | One heavy worker slot per user by default. Report complete process-tree peak memory and deadlines. Leave the watcher and MCP responsive during a long build or inspection. |

Profile idle and load on Windows and Linux. Measure startup, save bursts, branch switches, watcher overflow, and parallel coding sessions. Compare daemon private memory, total process-tree memory, CPU time and feedback latency against a baseline before tightening defaults. Record results for large solutions and any limits needed to keep them responsive.
