---
name: slophawk
description: Start SlopHawk in the checkout being edited and inspect source reconciliation and available verification evidence. Use when making or reviewing C# changes in a SlopHawk-enabled checkout.
---

# SlopHawk

For a bounded read-only workspace, use the foreground checker when the application has a batch adapter. Prepare a JSON array with `Operation`, `Args`, and `Expected` from the public contract. Run `slophawk check <assembly.dll> <cases.json> <scratch> [run-verb [prepare-verb]]`. Review its supported observations and mismatches. The application's documentation supplies any adapter verbs. Scratch must already exist and be writable. Only the requests and observations are written there.

For an editable checkout with daemon state available:

1. Run `slophawk start --root <checkout>`. The MCP entry point is `slophawk mcp --root <checkout>` and starts the same daemon if needed. If developing SlopHawk itself, use the local build from its README when the command is unavailable.
2. Read the daemon-owned `.slophawk/results.json` through `slophawk results --root <checkout>` or MCP `get_findings`. In 0.2 previews, check the Git context, source count and committed revision. Use `workspace_status` to inspect pending intents and scheduler state. Ready inventory means the source observations reconciled. An `awaiting_adapter` scheduler leaves test intents outstanding.
3. The watcher rechecks edits and Git changes. Use `slophawk recheck --root <checkout>` or MCP `run_checks` for an explicit refresh, then read the new revision. Treat the report as read-only.
4. Before reporting completion, review current evidence and run the repository's build and tests. SlopHawk delegates compiler and analyzer checks to the build.
