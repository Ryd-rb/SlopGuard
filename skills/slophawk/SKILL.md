---
name: slophawk
description: Start SlopHawk in the checkout being edited and inspect source reconciliation and available verification evidence. Use when making or reviewing C# changes in a SlopHawk-enabled checkout.
---

# SlopHawk

1. Run `slophawk start --root <checkout>`. The MCP entry point is `slophawk mcp --root <checkout>` and starts the same daemon if needed. If developing SlopHawk itself, use the local build from its README when the command is unavailable. For the verified 0.1.0 syntax release, use its original `slopguard` executable and read findings through that executable.
2. Read the daemon-owned `.slophawk/results.json` through `slophawk results --root <checkout>` or MCP `get_findings`. In 0.2 previews, check the Git context, source count and committed revision. Use `workspace_status` to inspect pending intents and scheduler state. Ready inventory means the source observations reconciled. An `awaiting_adapter` scheduler leaves test intents outstanding.
3. The watcher rechecks edits and Git changes. Use `slophawk recheck --root <checkout>` or MCP `run_checks` for an explicit refresh, then read the new revision. Treat the report as read-only.
4. Before reporting completion, review current evidence and run the repository's build and tests. The installed 0.1 release reports syntax findings. The 0.2 preview delegates compiler and analyzer checks to the build.
