# MCP interface and agent skill

## Skill workflow

The skill should document the installed CLI and MCP entry points, a workspace permission check, and a short edit loop. Package the skill alongside the CLI and keep the skill independent of any particular coding agent.

1. Start from the checkout the agent is about to edit. Resolve the workspace through the CLI and start or reuse the daemon. Connect the MCP stdio bridge and read the effective policy, provider readiness and discovered build contexts.
2. Capture a baseline for the intended scope. If inspection is pending, pin the revision and report the baseline as incomplete until the required checks finish.
3. Make edits normally. Let the watcher drive cheap checks and impacted tests. Query findings since the baseline and use explicit checks for targets or rules that require deeper coverage.
4. Before reporting completion, request a checkpoint with the scope and required checkers. Wait for the returned jobs, then report new findings and failed tests alongside pending, stale or blocked coverage. A newer relevant revision requires revalidation.

The skill treats findings as evidence. It may explain why it believes a finding is a false positive. It may change shared checker policy, suppressions or dictionary entries when authorized. An interrupted skill session closes its bridge and releases temporary session overrides. Other sessions and shared jobs remain active.

## Tool contracts

Use documented JSON schemas with finite budgets and named checker IDs. Scopes accept validated relative paths, project IDs, named workspace scopes and optional target contexts. Job submission returns promptly. Status responses carry workspace revision and configuration revision. Paginate with stable cursors tied to a report revision. Mark expired cursors explicitly.

| Tool | Inputs and result |
| --- | --- |
| `workspace_status` | Returns workspace identity, current revision, watcher and execution status, limits, and pending work. |
| `list_checkers` | Returns checker IDs, versions, triggers, permissions, prerequisites, rule coverage and available target contexts. |
| `get_config` | Returns effective settings and their origin. Redacts sensitive user-owned execution settings. |
| `configure_checker` | Applies schema-validated typed settings or resets an override. Supports `enabled`, `manual_only` and `disabled`. Defaults to a session override. Shared writes require authority and an expected configuration revision. Returns the new effective configuration and invalidated reports. |
| `run_checks` | Accepts checker IDs, scope, targets and a budget. The test checker supports impacted, named and full-project runs. Returns job IDs, requested revision and accepted or blocked contexts. |
| `get_run` | Returns progress and per-checker outcomes for a job group, including cache use, effective scope and coverage. |
| `cancel_run` | Withdraws a session's request. A shared job can continue for other waiting sessions. Reports any work that already completed. |
| `capture_baseline` | Pins report and workspace revisions for a scope. Returns a baseline ID with checker coverage and expiry. |
| `get_findings` | Filters by scope, severity, checker, revision or baseline. Returns findings, related spans, status, provenance and coverage. |
| `checkpoint` | Resolves required checkers and contexts and queues work for stale or missing coverage. Returns job IDs and per-context verification status. |
| `explain_test_selection` | Returns the path-to-project-to-test selection evidence, exclusions, fallbacks and deferred projects for a run or revision. |
| `list_spelling_terms` | Lists scoped, paginated whitelist entries with their origin, spelling, matching mode and reason. |
| `update_spelling_terms` | Adds or removes validated entries atomically with an expected configuration revision. Supports session or authorized repository scope. Returns affected scopes and invalidates only spelling work for those scopes. |

Job submissions accept a session idempotency key. Reusing a key with different parameters yields a conflict. Coalesce equivalent jobs only when effective policy and relevant input hashes match, even across sessions. Session overrides influence that session’s demand and result view. They leave the workspace’s automatic schedule under the shared policy. Disabling a checker invalidates its current coverage for the relevant policy and obsoletes pending work without other demand.

Results should separate inspection findings, test failures and tool failures. Expose coverage with a scope and reason for each partial, deferred, stale or blocked context. Keep responses compact and offer follow-up queries for individual results. Prefer paths, line and column ranges, diagnostic IDs and bounded excerpts over large logs. Tool cancellation, timeouts and precondition failures need stable machine-readable codes. [MCP tools reference](https://modelcontextprotocol.io/specification/2025-11-25/server/tools)

## Configuration and whitelist

Keep versioned team settings in the checkout and per-user preferences outside it. Resolve checker settings in order from defaults to user preferences, repository settings and session overrides, subject to owner-granted capabilities and machine resource limits. Resolve analyzer severities using each provider's own applicable settings. Keep execution permissions, approved tool paths, external read paths, outbound services and resource ceilings in owner-controlled configuration.

Store shared spelling rules in a small, reviewable repository file. An entry has a word or exact identifier, matching mode, optional relative path scope and reason. Allow case-sensitive product terms and case-insensitive ordinary words. Reject empty and malformed entries. Keep run-local exceptions in session state. Use atomic writes and expected revisions so concurrent changes produce a conflict that a client can resolve by rereading. Preserve the last valid configuration during incomplete or invalid external edits, with a visible error and revision.

MCP tools do not accept arbitrary commands, executable paths, raw build arguments or unvalidated globs. Explicit jobs use registered checker adapters and owner-approved execution profiles. Resolve each file scope against the workspace and validate containment before reading, watching or writing. Treat diagnostic text, test stdout and third-party output as untrusted values. Keep the MCP bridge and log transport free of secrets and full file bodies by default.
