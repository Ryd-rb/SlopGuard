# SlopGuard 0.2 design

Version 0.2 takes a saved C# edit from syntax feedback to project-aware diagnostics and relevant unit-test results. It adds a persistent shared index and lets the agent act on findings through the same SlopGuard skill. Keep the 0.1 watcher, CLI and atomic report projection as the starting point.

## Release scope

| Deliverable | First supported behavior |
| --- | --- |
| Semantic analysis | Resolve symbols in configured SDK-style C# projects. Report compiler diagnostics per target. Explain incoming and outgoing calls, implementations and bounded value origins and destinations. |
| Related unit tests | Discover runnable tests and use the evaluated project graph to select related unit-test projects. Run them automatically after a confirmed C# delta under an enabled workspace execution profile. |
| Verification gaps | Explain when an affected behavior has no linked unit-test evidence, when coverage is unknown, and when a matching coverage report recorded no hit. Reuse the semantic and test index. |
| Agent control | Expose job status, checker configuration within policy, and versioned result marks for recheck, further analysis or verified cleanup. Keep one skill and one current report. |

Validate SDK-style .NET 10 first with VSTest. Before self-hosting, exercise conditional multi-target projects and an older installed SDK selected by `global.json`. Treat compiler support and runner support as separate compatibility claims. Detect native Microsoft.Testing.Platform and classic .NET Framework configurations and report unsupported adapters or exact missing toolset prerequisites. Advertise each additional runner and classic configuration only when it passes its own fixture. Keep the [full verification catalog](verification.md) available as a design reference. Version 0.2 evaluates the unit-test and coverage subset. Spelling, clone detection, Sonar, InspectCode and the remaining evidence adapters follow this increment.

## Work from a confirmed delta

1. The watcher withdraws invalid current output and reconciles affected paths. Commit a bounded batch of additions, edits, deletions or moves with old and new source fingerprints and any resulting test intents. An unchanged save creates no delta. A C# path move counts even if the bytes match. Metadata, report output and startup inventory create zero new automatic test intents. [Source reconciliation and automatic test intents](source-reconciliation.md)
2. Publish syntax findings from the reconciled batch. In parallel, put dirty project contexts and affected test projects in a heavy-work queue. Keep undelivered C# changes across superseded jobs so a later metadata event or save cannot erase a pending test obligation.
3. Check the workspace execution profile before evaluation. For each admitted context, evaluate the graph and obtain the compilation inputs. Update the shared index and publish diagnostics or a specific blocker. Resolve related test projects from the old and new graph. Recheck permission and the runner and resource prerequisites before admitting any test attempt.
4. Record test outcomes and verification gaps. If the source, configuration or Git context changes during the run, retain any completed outcome as historical evidence in the bounded cache. Revalidate the current inputs before including it in `.slopguard`.

A branch transition replaces the publication epoch, including a switch between branches at the same commit. A same-commit switch with unchanged C# inputs creates zero new automatic test intents. Revalidate any already committed intents against the new context. A stable reconciliation that confirms a C# path or content delta can trigger related tests. Returning to an earlier branch can reuse matching indexed facts. Jobs from the old epoch lose publication authority.

For example, an edit to `src/SlopGuard.Core/SourceChecker.cs` reaches the daemon and C# provider through project references. The graph reaches `SlopGuard.IntegrationTests` through both. Publish syntax first, then compiler findings for affected contexts. Show the integration suite once in the selection explanation as manual-only. For self-hosting, add an eligible unit-test project around 0.2’s deterministic scheduler and impact rules. An edit to one of those rules then drives a real automatic run in SlopGuard’s own checkout.

## Project and compiler worker

Discover project candidates from solutions and project files inside the checkout. Resolve the SDK or Windows toolset before evaluating. Treat file-name heuristics as provisional. Resolve the SDK from the invocation directory selected for that project or solution entry point and record the applicable `global.json`. An explicit context distinguishes conflicting project and solution entry points when they evaluate differently. Build a context key out of the project path, evaluated target framework, configuration, platform, relevant runtime identifier and toolchain identity. Record actual `Compile` and `ProjectReference` memberships, defines, language options, references and project-system inputs. Evaluate each target independently. Import files, linked sources, analyzer settings, lock and assets inputs, and generated-source provenance are part of the context when applicable. [MSBuild item and property evaluation](https://learn.microsoft.com/en-us/visualstudio/msbuild/evaluate-items-and-properties)

Use an isolated C# worker to load the evaluated Roslyn workspace for the selected toolset. Use evaluation output for graph and membership queries. Use the loaded compilation for binding, compiler diagnostics and operations. Keep Roslyn types in the C# provider. Broker compilation inputs and diagnostics from a build already needed for a test whenever its fingerprint matches. Report missing references, unresolved generated code and unavailable toolset as partial or blocked. A syntax result remains useful while semantic work is pending.

Separate execution policy from checker policy. Project evaluation can invoke MSBuild behavior. Builds, generators, analyzers and tests can execute repository code. Require a user-owned workspace execution profile that grants the specific operations needed, with separate restore and network settings. An MCP checker override cannot grant execution privileges. When restore is disabled, use validated assets and runner-specific flags that prevent an implicit restore. If isolated output paths would invalidate the restored assets, report the prerequisite or use a preconfigured, serialized project output profile. [SDK artifact output layout](https://learn.microsoft.com/en-us/dotnet/core/sdk/artifacts-output)

## Shared index contract

Move the current source index behind a language-neutral read and write interface. Use the same store for the source manifest, evaluated project graph, semantic edges, test identities, findings and job intents. Use provider-specific records and versioned capabilities so another language can add partial symbol support later.

| Partition | Cache identity and use |
| --- | --- |
| Source | Normalized workspace-relative path and source digest, with parse options and checker version for diagnostics. |
| Build context | Evaluated project and target, SDK/compiler identity, configuration and digest of relevant build inputs. Track provisional membership separately. |
| Symbol and flow | Document and build context, declared symbols, typed usages, direct and candidate dispatch, and bounded value-flow summaries. Keep predecessor edges for deleted symbols while a delta is under evaluation. |
| Execution and evidence | Runner, discovered test identities, build and test inputs, execution outcome, optional attributed coverage, and configuration revision. |
| Work and agent marks | Job, scope, publication epoch and observed input revision. Store accepted marks and their scheduled follow-up in one transaction. |

Commit facts and their coverage frontier together. The first semantic worker rebuilds affected compilation partitions and dependent targets conservatively. Narrow document-level reanalysis requires cold and incremental parity for the affected inputs and summary dependencies. A restart validates persisted fingerprints before reuse and resumes committed current job intents. If the cache is corrupt or incompatible, rebuild derived state and report pending coverage.

Version pending work separately from rebuildable facts. Preserve outstanding intents during compatible upgrades. If an upgrade cannot read them, block activation with a recovery reason before establishing a new baseline.

Store SQLite on a local filesystem under per-user state, outside both the checkout and the report directory. Start with the [shared index budgets](semantic-index.md) of 32 MiB for in-process index buffers and 256 MiB for each checkout's database and journal, with a per-user quota across checkouts. Use one writer, short transactions and bounded read snapshots. Defer cold partitions and expire old query cursors under pressure. Keep compilation objects in the worker and give its process tree a separate memory budget. [SQLite write-ahead logging](https://sqlite.org/wal.html)

## Test discovery and selection

Use evaluated test-project properties and runner discovery to confirm runnable test identities. Repository policy classifies unit tests and excludes environment-dependent suites. An ambiguous project appears in status with its detection evidence. Handle VSTest and native Microsoft.Testing.Platform as different adapters, including discovery, arguments and result parsing. The selected SDK, project and runner determine the adapter. Keep user-supplied runner settings typed and pass arguments directly to the executable. [dotnet test runner selection](https://learn.microsoft.com/en-us/dotnet/core/tools/dotnet-test), [Microsoft.Testing.Platform migration](https://learn.microsoft.com/dotnet/core/testing/migrating-vstest-microsoft-testing-platform)

| Change | Selection |
| --- | --- |
| Implementation changed in a known project | Include opted-in unit-test projects that depend on it in the affected target graph. |
| Test source or shared fixture changed | Include its owning test project and known affected consumers. Resolve shared fixtures conservatively. |
| File deleted, moved or removed from compilation | Use membership and reverse dependencies from both sides of the delta. |
| Binding or project impact uncertain | Expand to the configured unit-test group within budget. Explain allowed, deferred and blocked scopes. |

The first automatic mode runs whole affected unit-test projects. Static call paths can explain or prioritize candidates. Narrow test filters require later validation against runner and impact evidence. Reuse a build only when the target, configuration, compiler inputs, relevant outputs and adapter match. Record zero discovered, all skipped, adapter failure, cancelled, timed out and build failure separately from passed and failed tests. Report when build and test results come from a live checkout with pre-run and post-run verification. Such evidence carries less certainty than a verified immutable input snapshot.

Expose typed reasons per selected project, including the triggering path, project dependency chain and fallback. Establishing the startup baseline and changes limited to project, config or coverage inputs create zero new automatic test intents. An explicit MCP request can run the configured scope independently of the watch trigger.

## Verification, reporting and agent marks

For modified symbols, query incoming and outgoing calls, implementations and bounded value flows in the target context. Return static paths with source locations and confidence. Treat unresolved dispatch, reflection and external code as explicit frontiers. Distinguish these outcomes when evaluating unit-test evidence.

- A discovered test linked by static analysis shows a static path with the recorded dispatch confidence.
- A passing run reports test outcome and scope. It supplies execution coverage for a symbol or branch when the corresponding coverage evidence supports it.
- A source-mapped coverage hit proves execution within the recorded run. A measured zero identifies an uncovered line or branch only where the counter maps to that line or branch. Record test assertion and outcome separately.
- A stale report, unavailable collector or incomplete graph leaves coverage unknown. If no linked test is found in a completed search, report the searched projects and any excluded scope.

Start with imported Cobertura line and branch coverage plus a local run receipt that identifies the build, target, source snapshot and collected artifact. Map coverage only where that receipt and the source mapping both agree. Treat aggregate coverage as run-level evidence and require per-test attribution before naming the specific test that hit a branch. Collectors remain optional. [Microsoft .NET code coverage guidance](https://learn.microsoft.com/en-us/dotnet/core/testing/unit-testing-code-coverage)

Publish file and source-digest attribution for findings and commit attribution for project or workspace results in the same bounded `.slopguard/results.json`. Increment the report schema for multiple checkers and per-context coverage. Keep the existing workspace identity, branch, commit and revision fields. Each checker section carries its version, context, coverage state, job or evidence ID, and included and omitted counts. Use a daemon incarnation and publication epoch to guard all writes, including temporary staging and final replacement. Preserve report limits and use MCP for paged detail. Omit previous-epoch findings from the current file while showing the present state for each context.

Keep the 0.1 CLI command meanings and its MCP tools. Add `get_run`, `configure_checker`, `explain_test_selection`, `trace_symbol`, `get_verification_needs`, `mark_result` and the workspace transition calls for the 0.2 scope. The broader MCP contract also describes later tools such as baseline and checkpoint commands. Checkers can be set to enabled, manual-only or disabled within the execution profile. Use a versioned expected configuration for shared changes. For skill-initiated checkout and reset, acknowledge the transition lease once the report is withdrawn and hold publication until reconciliation. Expire an abandoned lease and reconcile before reopening. [MCP tool contracts](mcp.md)

Validate each mark against the report epoch and target version, and apply its idempotency key in the single database writer. Retain marks alongside checker evidence. For recheck or deeper analysis, enqueue a typed job. For cleanup, revalidate ownership and currentness, then remove obsolete exporter-owned results. Scope cleanup to those results. Commit the mark and follow-up intent together before the scheduler runs it. Agents update state through MCP, and the exporter owns the report handle. [Status and file coordination](result-publication.md)

## Work scheduling and upgrade

- Keep watcher and syntax work responsive while a heavy job runs. Start with one heavy worker slot per OS user, backed by per-worktree queues. Acquire an output lease before writing to a configured project output location. Release database transactions before waiting for a worker, lease or reader. The MCP listener accepts job submissions and serves status independently of worker completion.
- Use typed job IDs and report scope. Coalesce jobs with identical inputs and effective policy. Keep queued and running cancellation distinct, supervise time and resource limits, and discard publication rights on an epoch change. Schedule the latest unverified change batch after supersession.
- Include a wire and report schema version in daemon status. An unversioned 0.1 status is recognizable. A 0.2 client encountering 0.1 can report how to stop the older daemon and start the verified build. Keep the old syntax mode available for rollback and keep cache formats versioned outside the report directory.

Use the [delivery gates](delivery.md#version-02-implementation-gates) to stage implementation and collect evidence. Every stage keeps the syntax path usable. Replace the self-hosted daemon after the full 0.2 gates pass.

## Acceptance before self-hosting

| Gate | Evidence required |
| --- | --- |
| Context accuracy | Conditional compile items, linked sources and duplicate names in multiple targets bind correctly. Verify the old SDK and failure cases by exact toolchain. Cold rebuild and warm update agree for the same inputs. |
| Trigger correctness | A related whole unit-test project runs after source add, edit, delete or move. Startup, unchanged saves, metadata-only edits and a same-commit branch switch with unchanged sources produce zero new automatic test intents. Previously committed intents survive replanning and restart. |
| Outcome accuracy | An obsolete result is withdrawn during a checkout or concurrent edit. A blocked toolchain, missing restore, failed test, zero discovery and stale coverage each report their own state. |
| Agent protocol | Concurrent marks on one target conflict cleanly. Identical replays return the original receipt. Marks on disjoint targets can both succeed. A terminated worker or agent does not leave a file lock or publish an old revision's report. |
| Bounded operation | Compare warm edit latency and idle CPU and memory against 0.1 on representative repositories. Log process-tree memory, cache reuse after restart and syntax latency while a heavy job runs. Demonstrate the configured report, index and queue limits under a repeated edit and branch-switch workload. |
| SlopGuard on itself | Keep 0.1 available. With a trusted execution profile, start 0.2 on SlopGuard and observe semantic findings and a relevant automatic unit-test run after a real C# edit. Run the integration suite separately. Verify the report on the final commit, exercise MCP marks, and update the single skill to use the verified build. |
