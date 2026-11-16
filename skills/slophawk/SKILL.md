---
name: slophawk
description: Run SlopHawk, test checker findings, and review verification evidence in an editable or bounded read-only workspace.
---

# SlopHawk

For a bounded read-only workspace with a batch adapter, follow this loop.

1. Inventory the components and operations from the public contracts and routing documentation. Keep a checklist in writable scratch. Work through every component in batches and save results after each batch. Ordinary examples are a guard against bad expectations. Exercise the in-contract conditions the examples leave untested, including boundaries and interactions between arguments. A smoke pass alone leaves coverage open.
2. Write cases with `Operation`, `Args`, and `Expected` derived from the public contract. Run `slophawk check <assembly.dll> <cases.json> <workspace> [run-verb [prepare-verb]]` with the documented adapter verbs. For requests without an independently established expected outcome, use `slophawk check-observe <assembly.dll> <requests.json> <workspace> [run-verb [prepare-verb]]` and omit `Expected`. Never enter a placeholder or use the candidate output as the source of an expectation. The workspace must permit writes to `.slophawk`. Each run saves `report.json`, `requests.json`, and `observed.json` under `.slophawk/checks/<run>/`. Read the files even when check exits 1. Review the coverage and outstanding findings with `slophawk check-review <workspace>`.
3. Acknowledge and test every outstanding finding. Trace the input through the public contract and candidate implementation. Run the exact input again in a separate check after independently deriving its expected outcome. Use a second boundary or neighboring case when it helps distinguish the cause. If the original expectation was wrong, retest with the corrected value. Unsupported or invalid cases leave coverage open. Replace them with supported, in-contract tests.
4. Save the acknowledgment array in writable scratch. Each entry has `Run` and zero-based `Index` for the original finding, `RetestRun` and `RetestIndex` for its separate check, `Disposition` set to `confirmed` or `corrected-expectation`, and `Reason` explaining the contract and cause or why the first expectation was wrong. Run `slophawk check-review <workspace> <acknowledgments.json>`. Resolve everything in `.slophawk/checks/review.json` under `Outstanding`, retest as needed, and rerun review.
5. Continue until every component in the checklist has been reviewed and every checker finding is acknowledged and tested, with `Complete: true` in the final review file. A clean review with untested operations leaves the audit unfinished. For flagging requests, leave the solution unchanged and submit only reproducible supported defects with actual, expected, and cause. If a concrete blocker prevents completion, record the remaining work and the blocker.

For an editable checkout with daemon state available:

1. Run `slophawk start --root <checkout>`. The MCP entry point is `slophawk mcp --root <checkout>` and starts the same daemon if needed. If developing SlopHawk itself, use the local build from its README when the command is unavailable.
2. Read the daemon-owned `.slophawk/results.json` through `slophawk results --root <checkout>` or MCP `get_findings`. In 0.2 previews, check the Git context, source count and committed revision. Use `workspace_status` to inspect pending intents and scheduler state. Ready inventory means the source observations reconciled. An `awaiting_adapter` scheduler leaves test intents outstanding.
3. The watcher rechecks edits and Git changes. Use `slophawk recheck --root <checkout>` or MCP `run_checks` for an explicit refresh, then read the new revision. Treat the report as read-only. Acknowledge each reported problem and exercise an appropriate reproducing or regression test before closing it. Recheck after changes and account for new findings the same way.
4. Before reporting completion, review current evidence and run the repository's build and tests. SlopHawk delegates compiler and analyzer checks to the build.
