# Repository layout

The .NET 10 solution builds four projects. The CLI currently exposes a help command and the class library projects establish ownership boundaries. Each library gains code when its first feature lands.

| Path | Ownership |
| --- | --- |
| `src/SlopGuard.Core` | Language-neutral workspace and revision identity, source spans, provider contracts, evidence, findings, job requests and result schemas. |
| `src/SlopGuard.Daemon` | Watcher and Git integration, indexing, scheduling, resource metering, persistence, and the current-report exporter. |
| `src/SlopGuard.Providers.CSharp` | C# and .NET discovery, Roslyn, MSBuild and test runners, and C# inspection providers. |
| `src/SlopGuard.Cli` | Executable composition root, workspace lifecycle commands and the MCP stdio bridge. |
| `skills/slopguard` | Single agent skill delivered with the CLI. |
| `docs` | Design decisions and implementation acceptance criteria. |

The daemon and C# provider both depend on Core. The CLI will compose the daemon, MCP bridge and built-in providers. Keep the Core project free of compiler, test runner, storage and transport dependencies. Introduce separate persistence, protocol or worker projects when an implemented component needs its own dependencies, process or release boundary.

As runtime features land, put test projects under `tests/`. Name unit-test projects after their assemblies, such as `tests/SlopGuard.Core.Tests`, and put cross-process tests in `tests/SlopGuard.IntegrationTests`. Keep synthetic test workspaces under `tests/fixtures/<provider>/`, with shared Git and filesystem cases in a workspace folder. Keep independently versioned configuration or protocol schemas under `schemas/` when clients begin consuming them.

## Adding more languages

- Register built-in providers under string language and provider IDs. A provider declares how to recognize roots and files, enumerate build contexts, extract index facts, and run or discover checks. A physical file can supply multiple logical documents, such as C# and markup in a Razor view. Record which provider and target evaluated each document.
- Enqueue work from normalized workspace deltas, then ask matching providers for affected contexts. Keep Git validity, queues, index budgets, test execution policy, publication and MCP commands shared. Configure automatic test triggers by language. The first release enables related unit-test runs only for confirmed C# deltas.
- Store common relations as provider-tagged facts and keep provider-specific details versioned. A future provider can expose references, calls or value flow incrementally. Report unavailable capabilities so a checker can reason about partial coverage. Add cross-language links through explicit contracts such as schemas, generated stubs and service routes, with provenance for each inferred link.
- Use the language’s own compatible tooling from its isolated worker. Roslyn symbols and C# project references stay private to the C# adapter. New built-in adapters follow `src/SlopGuard.Providers.<Language>`. Stabilize the contracts with a second language before exposing a public plugin API.
- Keep `.slopguard/results.json`, status marks and the skill uniform across languages. Distinguish findings by provider, language and build context. The agent keeps one set of commands.

## Build the scaffold

```sh
dotnet build SlopGuard.slnx
dotnet run --project src/SlopGuard.Cli -- --help
```

The SDK selected for this repository builds SlopGuard itself. The C# adapter will resolve SDKs and targeting packs separately for each checkout it analyzes. [SDK version selection](https://learn.microsoft.com/en-us/dotnet/core/tools/global-json), [Solution CLI](https://learn.microsoft.com/en-us/dotnet/core/tools/dotnet-sln)
