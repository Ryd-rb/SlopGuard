# SlopGuard

SlopGuard is a local C# verification daemon for coding agents. An agent skill starts it in the checkout the agent is editing. The daemon watches that checkout and serves verification results and controls over the Model Context Protocol (MCP). MSBuild owns compiler and build-time analyzer checks. SlopGuard 0.2 focuses on test impact, coverage gaps and checks beyond the configured build.

After an edit, the agent can ask these questions.

1. What changed, and which projects and behaviors could it affect?
2. Does the change duplicate existing code or violate a repository rule?
3. Which related tests ran, and which affected targets still need verification?

| Area | Planned behavior |
| --- | --- |
| On-save verification | Coalesce saves and confirm source deltas. Use them to select affected test projects and refresh verification evidence. |
| Supplemental inspection | Apply checks beyond the configured build. Consider optional Sonar and InspectCode rules where they provide additional coverage. |
| Duplication | Find exact and near copies across a checkout, with both source ranges, matched token counts and a reproducible similarity score. |
| Automated tests | Discover tests in the checkout. Run impacted tests automatically after a C# source change, including additions and deletions. Allow explicit runs through MCP. |
| Verification gaps | Trace modified symbols, callers, implementations and value flows to recommend relevant tests and operational checks. Distinguish static reachability from observed coverage. |
| Shared index | Reuse semantic and test evidence across checkers, with a bounded memory budget and an incremental disk cache. |
| Current reports | Publish current findings from all checkers in `.slopguard/results.json` with file and commit attribution. Withdraw reports when branch or input context changes. |
| Agent controls | Request or cancel checks, inspect results and coverage, change checker settings, and maintain a scoped spelling whitelist. |
| Compatibility | Run the daemon on .NET 10. Validate project-aware analysis first on SDK-style projects. Add older SDKs and .NET Framework configurations through verified toolchain fixtures. |

## Design

- [Daemon architecture](docs/architecture.md) covers watching, change tracking, scheduling, compatibility, trust and resource use.
- [Checker and test design](docs/checkers.md) covers the rule catalog, similarity scoring, and test discovery and selection.
- [Shared semantic index](docs/semantic-index.md) covers incremental symbol, call and value-flow tracking.
- [Verification needs](docs/verification.md) covers test gaps and detection of pre-production, deploy, release and post-release checks.
- [Current report publication](docs/result-publication.md) covers attribution, stale output invalidation and disk limits.
- [MCP and agent skill](docs/mcp.md) defines the agent workflow, tool contracts and policy boundaries.
- [SlopGuard skill](skills/slopguard/SKILL.md) is the single agent entry point for the current CLI and MCP tools.
- [Repository layout](docs/repository-layout.md) covers the buildable .NET solution and the boundaries for adding languages.
- [Delivery and acceptance](docs/delivery.md) defines the implementation order, fixtures and measurable acceptance criteria.
- [Version 0.2 design](docs/version-0.2.md) specifies project-aware analysis, related tests and agent result control for the next release.
- [Source reconciliation](docs/source-reconciliation.md) defines how a verified C# edit creates durable test work across saves, branch transitions and restart.

## Use the 0.2 development build

Build with the .NET 10 SDK. Git workspaces require Git on `PATH`. Stop any 0.1 daemon for the checkout before starting the development build.

```sh
dotnet build SlopGuard.slnx
dotnet run --no-build --project src/SlopGuard.Cli -- start
dotnet run --no-build --project src/SlopGuard.Cli -- status
dotnet run --no-build --project src/SlopGuard.Cli -- results
dotnet run --no-build --project src/SlopGuard.Cli -- stop
```

The preview publishes a source inventory report with Git attribution. It fingerprints C# source and stores pending test intents in SQLite. The CLI and MCP status expose `committedRevision`, `pendingTestIntents`, `recordingError` and `testScheduler`. The separate 0.1 release supplies the historical syntax-check workflow. Use the project's configured build for compiler and analyzer diagnostics.

The durable scheduler is exercised through the daemon with an in-process fake adapter in integration tests. The preview reports `awaiting_adapter` while real project evaluation and test execution are under development. [Current implementation](docs/source-reconciliation.md#current-implementation), [Next implementation stages](docs/delivery.md#next-implementation-stages)

Released under the [MIT License](LICENSE).
