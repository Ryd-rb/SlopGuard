# SlopHawk

SlopHawk is a local C# verification daemon for coding agents. An agent skill starts it in the checkout the agent is editing. The daemon watches that checkout and serves verification results and controls over the Model Context Protocol (MCP). MSBuild owns compiler and build-time analyzer checks. SlopHawk 0.2 focuses on test impact, coverage gaps and checks beyond the configured build.

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
| Current reports | Publish current findings from all checkers in `.slophawk/results.json` with file and commit attribution. Withdraw reports when branch or input context changes. |
| Agent controls | Request or cancel checks, inspect results and coverage, change checker settings, and maintain a scoped spelling whitelist. |
| Compatibility | Run the daemon on .NET 10. Validate project-aware analysis first on SDK-style projects. Add older SDKs and .NET Framework configurations through verified toolchain fixtures. |

## Design

- [Daemon architecture](docs/architecture.md) covers watching, change tracking, scheduling, compatibility, trust and resource use.
- [Checker and test design](docs/checkers.md) covers the rule catalog, similarity scoring, and test discovery and selection.
- [Rust checker migration](docs/rust-checker-migration.md) covers the four language providers, compatibility and RAM acceptance.
- [Shared semantic index](docs/semantic-index.md) covers incremental symbol, call and value-flow tracking.
- [Verification needs](docs/verification.md) covers test gaps and detection of pre-production, deploy, release and post-release checks.
- [Current report publication](docs/result-publication.md) covers attribution, stale output invalidation and disk limits.
- [MCP and agent skill](docs/mcp.md) defines the agent workflow, tool contracts and policy boundaries.
- [SlopHawk skill](skills/slophawk/SKILL.md) is the single agent entry point for the current CLI and MCP tools.
- [Repository layout](docs/repository-layout.md) covers the buildable .NET solution and the boundaries for adding languages.
- [One-year roadmap](docs/roadmap.md) defines the December 2026 to November 2027 milestones and initial implementation slices.
- [Robustness contracts](docs/robustness.md) defines failure, recovery, execution and evidence boundaries.
- [Delivery and acceptance](docs/delivery.md) defines release gates, fixtures and measurable acceptance criteria.
- [Version 0.2 design](docs/version-0.2.md) specifies project-aware analysis, related tests and agent result control for the next release.
- [Source reconciliation](docs/source-reconciliation.md) defines how a verified C# edit creates durable test work across saves, branch transitions and restart.

## Use the 0.2 development build

Build with the .NET 10 SDK. The [portable Linux toolchain](docs/portable-linux.md) provides the SDK and locked NuGet dependencies for restricted VMs. Git workspaces require Git on `PATH`. Stop earlier daemons for this checkout with their matching CLI before starting this build. This build initializes inventory in the SlopHawk state directory.

```sh
dotnet build SlopHawk.slnx
dotnet run --no-build --project src/SlopHawk.Cli -- start
dotnet run --no-build --project src/SlopHawk.Cli -- status
dotnet run --no-build --project src/SlopHawk.Cli -- results
dotnet run --no-build --project src/SlopHawk.Cli -- projects
dotnet run --no-build --project src/SlopHawk.Cli -- stop
```

The preview publishes a source inventory report with Git attribution. It fingerprints C# source and stores pending test intents in SQLite. The CLI and MCP status expose `committedRevision`, `pendingTestIntents`, `recordingError` and `testScheduler`. Use the project's configured build for compiler and analyzer diagnostics.

Use `slophawk projects --root <checkout>` to explicitly evaluate a checkout and print per-target C# membership, linked items, reference mappings and any unresolved scope. This command uses the installed .NET 10 SDK for evaluation and requests no build targets. On Windows it uses the shared worker slot and a Job Object. The daemon can schedule C#, Python, Cargo and FluxR tests with the owner’s private executable-test registration. Without a registration it reports `awaiting_adapter`. [Executable-test configuration](docs/rust-migration-qualification.md#scheduled-executable-tests)

### Workspace execution profiles

The daemon reports the user-owned profile path and current root identity under `executionProfile` in `status`. Use that identity when creating the profile. The following example grants project evaluation and tests and reserves one approved output subtree.

```json
{
  "schemaVersion": 1,
  "rootIdentity": "<executionProfile.rootIdentity from status>",
  "operations": ["evaluate", "test"],
  "outputRoots": ["C:/path/to/approved-output"]
}
```

Use absolute physical directory paths for outputs. Each grant is explicit. The other operations are `build`, `restore`, `network` and `compilerExtensions`. Replace the profile file atomically to change grants while the daemon is running. A missing, invalid or revoked profile blocks scheduled execution and leaves source intents pending. The explicit `projects` invocation authorizes a single evaluation. [Execution profile and worker behavior](docs/architecture.md#repository-execution)

### Differential probes

`slophawk check-contracts <assembly.dll> <rules.json> <published.json> <workspace> [per-operation [run-verb [prepare-verb]]]` derives inputs and expected results from the supplied rules. It tests the rules against published `Operation`, `Args`, and `Expected` examples before searching for new inputs. The receipt records the two input file hashes and the basis for qualification and expectations. Qualification establishes agreement on the supplied examples. Check a reported expectation against the intended behavior before treating the discrepancy as a code defect.

The search saves qualification, explored inputs, and observations under `.slophawk/contract-searches/`. The ordinary check report contains representative comparisons whose actual values repeated exactly. `ReplayChanges` in the search summary records any changed actual values, including discrepancies that disappeared, new discrepancies, and results that became unsupported. The original search batch and raw replay remain available for examination. Use `slophawk contracts qualify <rules.json> <published.json>` to check agreement with the published examples without running the application.

Rules describe signed integer operations. Each object names an `Operation`, ordered `Parameters`, `Domain`, `Expected`, and public `Source`. An optional ordered `Let` object defines local expressions. Optional `Basis` lists documented scalar constants by parameter, such as enum members or boundaries. The engine mutates public examples, probes both sides of numeric comparisons, and exercises linked equality conditions together. Numeric literals from candidate methods with matching operation names and their compiler-generated local functions guide additional inputs, including compensating changes across fields. The engine validates each input with `Domain` and computes its expectation with `Expected`. A prose contract needs a corresponding executable rule. For example:

```json
[
  {
    "Operation": "Remaining",
    "Parameters": ["capacity", "used"],
    "Domain": "capacity >= 0 && used >= 0",
    "Expected": "max(0, capacity - used)",
    "Source": "allocation.md"
  }
]
```

Expressions support integer arithmetic, comparisons, bitwise operators, lazy boolean operators and conditionals, and `min`, `max`, `abs`, `clamp`, `mod`, and `popcount`. Division truncates toward zero, `mod` returns nonnegative residues for positive divisors, and expected results must fit signed 64-bit integers.

`slophawk check <assembly.dll> <cases.json> <workspace> [run-verb [prepare-verb]]` runs a foreground behavioral check, including in a restricted workspace. It reads the supplied case file and candidate assembly, runs the application's own batch endpoint, and compares its results to the expectations. It inherits the caller's sandbox and writes the report, requests, and observations under `<workspace>/.slophawk/checks/<run>/`. That directory needs write access. The source and daemon state can stay read-only.

The case file is a JSON array whose entries contain `Operation`, signed-integer `Args`, and a non-null `Expected` JSON value. The checker passes only `Operation` and `Args` to the adapter. It runs `dotnet exec <assembly.dll> [run-verb] <input>`, and expects one ordered JSON result per input. An optional prepare verb transforms the ordinary request file into the batch endpoint's input format. Supply an assembly you trust to run. A completed check emits a `slophawk-check-v1` JSON report. Exit zero means every case matched. Mismatches, unsupported results, and execution errors give a nonzero exit.

`slophawk check-observe <assembly.dll> <requests.json> <workspace> [run-verb [prepare-verb]]` runs the same adapter using `Operation` and `Args` only. It saves observations for exploratory runs. Those runs produce zero comparisons and do not satisfy the retest requirement.

`slophawk check-retest <assembly.dll> <workspace> <run> <decisions.json> <acknowledgments.json>` replays selected findings on the original candidate and adapter. Supply a JSON array with the original zero-based `Index` and a contract-based `Reason` for each finding. Omit `Expected` to retest the original discrepancy, or supply the corrected expectation when independent review establishes that the original observation was right. The command adds a check report and updates the acknowledgments after the observations reproduce. Changed observations stay in `.slophawk/retest-attempts/`.

`slophawk check-review <workspace> [acknowledgments.json [scope.json]] [--run <run>]` writes per-operation coverage and outstanding findings in `.slophawk/checks/review.json`. An acknowledgment links the original finding and a separate comparison of the same candidate, adapter, and input. It records whether the discrepancy reproduced or a corrected expectation matched, along with the reviewer's reason. If the workspace has evidence for multiple candidates or adapters, pass `--run` with a receipt ID to validate the comparisons in its evidence set. Unsupported comparisons and conflicting observations for an identical request leave the review incomplete. The report counts the unsupported comparisons and lists conflicting observations under `UnstableInputs`.

An optional scope file lists operation names in `Operations` and public `Operation` and `Args` examples in `Ordinary`. It requires a supported comparison beyond those examples for each listed operation. Set `MinimumOutsideOrdinary` to require more distinct inputs per operation. To include a grouped `Worklist` for missing operations, supply `Sources` entries with `Operation`, `Component`, `Contract`, and `Implementation`. `Complete` describes the recorded comparisons, coverage threshold, and handling of their findings. See the [SlopHawk skill](skills/slophawk/SKILL.md) for the audit loop.

Append `--progress` when retrieving the worklist during an audit, including before the first check when a scope is supplied. A report published successfully exits zero in this mode. The JSON still records whether the review is complete. The default exit status expresses completion.

`slophawk probes plan` accepts a JSON array of known examples such as `[{"Operation":"quote","Args":[5,7]}]`. It generates bounded signed-integer probes around those examples, including neighboring values, ties and correlated changes. The count is per operation. An optional random seed varies sampling. An optional JSON array of positive scales greater than one adds compensating changes across pairs of fields, which can expose behavior around scaled quantities and composite keys.

```sh
slophawk probes plan seeds.json 512 > probes.json
slophawk probes plan seeds.json 512 0 scales.json > scaled-probes.json
slophawk probes methods candidate.dll reference.dll > methods.json
slophawk probes compare probes.json candidate-results.json reference-results.json > comparison.json
```

Use the project's own runner or adapter to replay the **same ordered probes** against both versions. Each result file must be a JSON array with one entry per input in that order. Use `null` for an unsupported input. The comparison reports counts, support mismatches, and the first differing supported result per operation with both observed values and its input. A bounded search can miss differences. Replay evidence applies to the supplied inputs and reference behavior.

### Scalar C# path pilot

Use `paths` for a controlled .NET 10 executable with the foreground batch protocol. Place the spec next to the project and use a separate existing writable scratch directory. The controlled builder compiles the declared sources directly with SDK Roslyn and the .NET 10 reference pack. List the exact source closure and any generated files explicitly. It uses C# 14, nullable enabled, and the specified defines, optimization and overflow options. MSBuild evaluation and generator execution are outside this route. The resulting assembly is the candidate used for the graph and replay.

```json
{
  "build": {
    "project": "Rates.csproj",
    "sources": ["Program.cs", "Rates.cs"],
    "referencePack": "/path/to/dotnet/packs/Microsoft.NETCore.App.Ref/10.0.x/ref/net10.0",
    "defines": ["NET10_0"]
  },
  "targets": [
    { "operation": "rate", "document": "Rates.cs", "signature": "Example.Rates.Quote(int, int)" }
  ],
  "rules": "rules.json",
  "published": "published.json",
  "runVerb": "probe-batch",
  "perOperation": 256,
  "workLimit": 128,
  "retestLimit": 16,
  "seed": 1
}
```

The signature is Roslyn's qualified C# method display with parameter types. Use a declared, trusted adapter that dispatches the operation to that body and preserves request order. Encode Boolean inputs and results as integer zero or one. Named compiler options and target scope are in the build receipt. Supply `prepareVerb` if the adapter also has a prepare entry point. The builder produces a standalone managed app for the .NET 10 runtime running SlopHawk.

```text
slophawk paths build <spec.json> <scratch>
slophawk paths plan <spec.json> <scratch> <build-manifest.json> graph-on
slophawk paths check <plan-manifest.json>
slophawk paths review <review-manifest.json>
```

Use the `Build`, `Plan` and `Review` paths printed by the preceding commands. Plan re-emits and verifies the candidate's build hash. Pass `-` as the build input for a structural graph. Run a second plan in `graph-off` mode with the same settings for an ablation. Its candidate stream includes ordinary cases and current generic, literal, boundary and combined-condition probes. The plan records shared candidates and graph-only proposals separately.

Review the sealed run under `<scratch>/.slophawk/path-checks/`. Treat graph edges as structural and input-to-edge claims as predictions. The review reports which exact comparisons reproduced in a single-case replay on the original build, which remain unconfirmed, and which changed on replay. It also reports search gaps and invalidates currentness if inputs changed. Confirm the cited public rule before flagging a defect. An exit of 1 can accompany a sealed discrepancy or incomplete outcome. [Checker design](docs/path-intent-checker.md) · [Scalar gate](experiments/slophawk-path-intent/benchmark-design.md#scalar-gate)

The optional method comparison reads managed metadata and shows changed bodies, instruction excerpts and integer literals without loading either assembly for execution. Use the method results to prioritize investigation or derive scales. Method changes can include semantic equivalents and compiler-generated code. Confirm observable differences with replay before treating a method change as a regression.

Released under the [MIT License](LICENSE).
