# SlopGuard

SlopGuard is a design for a local C# quality daemon for coding agents. An agent skill starts it in the checkout the agent is editing. The daemon watches that checkout, runs deterministic checks, and serves findings and controls over the Model Context Protocol (MCP).

After an edit, the agent can ask these questions.

1. What changed, and what diagnostics did the change introduce?
2. Does the change duplicate existing code or violate a repository rule?
3. Which related tests ran, and which affected targets still need verification?

| Area | Planned behavior |
| --- | --- |
| On-save feedback | Watch file and directory creation, modification, deletion and rename. Coalesce save bursts and update findings for affected files and projects. |
| C# inspection | Use the repository’s compiler and analyzers, local Sonar C# rules, and optional JetBrains InspectCode. Make tool and rule coverage visible. |
| Duplication | Find exact and near copies across a checkout, with both source ranges, matched token counts and a reproducible similarity score. |
| Automated tests | Discover tests in the checkout. Run impacted tests automatically after a C# source change, including additions and deletions. Allow explicit runs through MCP. |
| Agent controls | Request or cancel checks, inspect results and coverage, change checker settings, and maintain a scoped spelling whitelist. |
| Compatibility | Run the daemon on .NET 10. Analyze C# projects targeting .NET Framework 4.6 through 4.8.1, .NET Core, .NET 5–10, and .NET Standard using the applicable toolchain. |

## Design

- [Daemon architecture](docs/architecture.md) covers watching, change tracking, scheduling, compatibility, trust and resource use.
- [Checker and test design](docs/checkers.md) covers the rule catalog, similarity scoring, and test discovery and selection.
- [MCP and agent skill](docs/mcp.md) defines the agent workflow, tool contracts and policy boundaries.
- [Delivery and acceptance](docs/delivery.md) defines the implementation order, fixtures and measurable acceptance criteria.

This initial repository contains the specification. The daemon, checkers and agent skill are planned deliverables.
