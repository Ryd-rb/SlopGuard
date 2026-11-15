---
name: slopguard
description: Start SlopGuard in the checkout being edited and inspect current C# syntax findings. Use when making or reviewing C# changes in a SlopGuard-enabled checkout.
---

# SlopGuard

1. Run `slopguard start --root <checkout>`. The MCP entry point is `slopguard mcp --root <checkout>` and starts the same daemon if needed. If developing SlopGuard itself, use the local build from its README when the command is unavailable.
2. Read the daemon-owned `.slopguard/results.json` through `slopguard results --root <checkout>` or MCP `get_findings`. Check the file paths, Git context and revision. A pending, degraded or partial run is incomplete. Use `workspace_status` to see why.
3. The watcher rechecks edits and Git changes. Use `slopguard recheck --root <checkout>` or MCP `run_checks` for an explicit refresh, then read the new revision. Treat the report as read-only.
4. Before reporting completion, review current findings and run the repository's build and tests. Version 0.1 checks syntax. Use MCP `list_checkers` to confirm available coverage.
