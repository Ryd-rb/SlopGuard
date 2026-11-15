# MCP interface and agent skill

## Skill workflow

This workflow describes the full agent integration. Version 0.1 and incremental 0.2 builds expose the subset advertised by their daemon. The skill uses status and the available recheck when baseline or checkpoint tools are unavailable, then reports the coverage it could verify.

Package the [SlopGuard skill](../skills/slopguard/SKILL.md) alongside the CLI. It is the sole skill an agent needs to start the daemon, use all configured checkers, read reports and submit status changes. Keep provider routing and scheduling in the daemon.

1. Start from the checkout the agent is about to edit. Resolve the workspace through the CLI and start or reuse the daemon. Connect the MCP stdio bridge and read the effective policy, provider readiness and discovered build contexts.
2. Capture a baseline for the intended scope. If inspection is pending, pin the revision and report the baseline as incomplete until the required checks finish.
3. Make edits normally. Let the watcher drive cheap checks and impacted tests. Query findings since the baseline and inspect verification needs when edited symbols affect callers, implementations or observable values. Use explicit checks for targets or rules that require deeper coverage.
4. Before reporting completion, request a checkpoint with the scope and required checkers. Wait for the returned jobs, then report new findings, failed tests and relevant verification gaps alongside pending, stale or blocked coverage. A newer relevant revision requires revalidation. Use the [validated results reader](result-publication.md) when consuming the on-disk projection.

The skill treats findings as evidence. It may explain why it believes a finding is a false positive. It may change shared checker policy, suppressions or dictionary entries when authorized. An interrupted skill session closes its bridge and releases temporary session overrides. Other sessions and shared jobs remain active.

Before a working-tree transition initiated by the agent, request withdrawal of the current report. Complete the transition lease after the Git command returns and wait for reconciliation before using findings from the new checkout state.

## Protocol availability

Version 0.1 exposes `workspace_status`, `list_checkers`, `run_checks` and `get_findings` for syntax. Preserve those entry points and advertise supported tool and checker capabilities from the connected daemon. Expose wire, report and capability versions during the startup handshake. A client seeing an incompatible daemon reports its version and how to switch builds. Leave that daemon running until the user explicitly stops it.

## Tool contracts

Use documented JSON schemas with finite budgets and named checker IDs. Scopes accept validated relative paths, project IDs, named workspace scopes and optional target contexts. Job submission returns promptly. Status responses carry workspace revision and configuration revision. Paginate with opaque cursors bound to the workspace, daemon incarnation, query parameters and projection revision. A cursor for a current query becomes stale when that scope is invalidated. A historical cursor can continue within its retention limit and effective policy. Return an explicit expiry when its snapshot has been reclaimed. Distinguish opted-in history from the default current view in every result.

| Tool | Inputs and result |
| --- | --- |
| `workspace_status` | Returns workspace identity, Git context, current revision, publication epoch and status, watcher and execution status, limits, and pending work. |
| `list_checkers` | Returns checker IDs, versions, triggers, permissions, prerequisites, rule coverage and available target contexts. |
| `get_config` | Returns effective settings and their origin. Redacts sensitive user-owned execution settings. |
| `configure_checker` | Applies schema-validated typed settings or resets an override. Supports `enabled`, `manual_only` and `disabled`. Defaults to a session override. Shared writes require authority and an expected configuration revision. Returns the new effective configuration and invalidated reports. |
| `run_checks` | Accepts checker IDs, scope, targets and a budget. The 0.2 test checker accepts impacted or named project and target scopes and runs whole projects. Finer filters require a runner capability. Returns job IDs, requested revision and accepted or blocked contexts. |
| `get_run` | Returns progress and per-checker outcomes for a job group, including cache use, effective scope and coverage. |
| `cancel_run` | Withdraws a session's request. A shared job can continue for other waiting sessions. Reports any work that already completed. |
| `capture_baseline` | Pins report and workspace revisions for a scope. Returns a baseline ID with checker coverage and expiry. |
| `get_findings` | Filters by scope, severity, checker, revision or baseline. Returns findings, related spans, file or commit scope, provenance and coverage. Current queries validate the publication context. Historical queries identify the requested revision. |
| `mark_result` | Takes a report token, target version, request ID, status and short reason. Targets the report, an affected path in that report or a finding ID. Returns the accepted mark version and any queued follow-up job, or a conflict if the target has changed. |
| `checkpoint` | Resolves required checkers and contexts and queues work for stale or missing coverage. Returns job IDs and per-context verification status. |
| `explain_test_selection` | Returns the path-to-project-to-test selection evidence, exclusions, fallbacks and deferred projects for a run or revision. |
| `trace_symbol` | Takes a symbol ID or file position, target context, direction and budget. Returns incoming and outgoing call chains, typed usages, implementations, value origins or destinations, and any unresolved traversal frontier. |
| `get_verification_needs` | Returns applicable check categories for a change set, their source or artifact links, evidence state, and any missing or uncertain coverage. Accepts an optional baseline, target filter and evidence category filter. |
| `begin_workspace_transition` | Withdraws the current report and returns an expiring lease after publication is paused. Reports an explicit failure if the report could not be withdrawn. |
| `end_workspace_transition` | Releases the caller's lease and requests Git and input reconciliation. Returns reconciliation status and the new publication epoch. |
| `list_spelling_terms` | Lists scoped, paginated whitelist entries with their origin, spelling, matching mode and reason. |
| `update_spelling_terms` | Adds or removes validated entries atomically with an expected configuration revision. Supports session or authorized repository scope. Returns affected scopes and invalidates only spelling work for those scopes. |

Job submissions accept a session idempotency key. Reusing a key with different parameters yields a conflict. Coalesce equivalent jobs only when effective policy and relevant input hashes match, even across sessions. Session overrides influence that session’s demand and result view. They leave the workspace’s automatic schedule under the shared policy. Disabling a checker invalidates its current coverage for the relevant policy and obsoletes pending work without other demand.

An optional checkpoint pins the required checkers, effective policy and target matrix at the reconciled revision. Return `pending` with job IDs while coverage is being gathered, `blocked` with unmet prerequisites, or `complete` with independent pass or failure outcomes. Report `superseded` when relevant inputs, authority or epoch change while the checkpoint is open. The client can request a new checkpoint. A historical checkpoint remains attributable to its original revision.

Return structured domain failures with stable codes such as `stale_revision`, `conflict`, `cursor_expired`, `blocked_policy`, `unsupported_context` or `budget_exceeded`. Include the affected scope and a bounded reason. Return the relevant current token on a conflict when it is safe to use. Use protocol errors for malformed MCP requests and unknown methods.

Results should separate inspection findings, test failures and tool failures. Expose coverage with a scope and reason for each partial, deferred or blocked context. Attribute stale evidence to its historical revision. Keep responses compact and offer follow-up queries for individual results. Prefer paths, line and column ranges, diagnostic IDs and bounded excerpts over large logs. Tool cancellation, timeouts and precondition failures need stable machine-readable codes. [MCP tools reference](https://modelcontextprotocol.io/specification/2025-11-25/server/tools)

## Configuration and whitelist

Keep versioned team settings in the checkout and per-user preferences outside it. Resolve checker settings in order from defaults to user preferences, repository settings and session overrides, subject to owner-granted capabilities and machine resource limits. Resolve analyzer severities using each provider's own applicable settings. Keep execution permissions, approved tool paths, external read paths, outbound services and resource ceilings in owner-controlled configuration.

Store shared spelling rules in a small, reviewable repository file. An entry has a word or exact identifier, matching mode, optional relative path scope and reason. Allow case-sensitive product terms and case-insensitive ordinary words. Reject empty and malformed entries. Keep run-local exceptions in session state. Use atomic writes and expected revisions so concurrent changes produce a conflict that a client can resolve by rereading. Preserve the last valid checker settings during incomplete or invalid repository edits, with a visible error and revision. An unreadable, invalid or removed user execution profile closes admission for previously granted operations until it can be validated. Changes to an execution grant are independently versioned and invalidate affected scoped results.

MCP tools do not accept arbitrary commands, executable paths, raw build arguments or unvalidated globs. Explicit jobs use registered checker adapters and owner-approved execution profiles. Resolve each file scope against the workspace and validate containment before reading, watching or writing. Treat diagnostic text, test stdout and third-party output as untrusted values. Keep the MCP bridge and log transport free of secrets and full file bodies by default.
