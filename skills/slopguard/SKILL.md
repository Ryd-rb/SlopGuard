---
name: slopguard
description: Start and use the SlopGuard daemon when editing C# in a SlopGuard-enabled checkout. Check findings and test evidence, and request rechecks, deeper analysis or cleanup.
---

# SlopGuard

1. Run `slopguard start --root <checkout>`. If needed, connect using `slopguard mcp --root <checkout>`. Use that connection for all checkers. Read `workspace_status` and capture a baseline for the files or projects in scope.
2. SlopGuard publishes `.slopguard/results.json`. Read it through `slopguard results --root <checkout>` or query `get_findings` over MCP. Use the returned revision and file or commit scope. Missing output may mean reconciliation or checks are pending. Check status before treating it as a clean run.
3. Mark the report, an affected file, or a finding with `mark_result`, using the report token, target version and a short reason. Use `acknowledged` when reviewed, `fix_in_progress` while editing, `recheck_requested` after a fix, `needs_analysis` for deeper inspection, `suspected_false_positive` for review, or `cleanup_requested` for apparently obsolete results. Treat the report file as read-only. On a conflict, reread and reapply the mark only if it still fits.
4. Before changing branches or rewriting the working tree, call `begin_workspace_transition`. Release the lease with `end_workspace_transition` after Git exits, then read the refreshed status.
5. Before reporting completion, run `checkpoint` for the edited scope. Report current findings, test failures and unresolved verification gaps.
