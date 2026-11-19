# Checkers and test selection

## Inspection providers

Each adapter declares required inputs, supported target contexts, execution permissions, cache granularity, and cost. MSBuild and its configured compiler, analyzers and targets own syntax, binding errors, configured code style and project validation. Select supplemental rules by comparing the provider's coverage with the actual build configuration for each target. Publish enabled and evaluated rule IDs when the provider exposes them. Distinguish unsupported rules, suppressed rules, partial analysis and missing providers.

| Checker | Findings and initial design |
| --- | --- |
| Sonar C# rules | Consider an optional pinned `SonarAnalyzer.CSharp` profile for rules with additional coverage. Run those rules with the target settings and report the active rule set. Server profiles and quality gates depend on the configured scan. |
| JetBrains inspections | Consider optional InspectCode rules with additional coverage. Use solution settings and machine-readable output. Cache by solution context and schedule as heavyweight work. |
| Reliability and performance | Use declared hot paths, repository context or runtime evidence to check behavior beyond the configured analyzers. |
| Defensive coding | Detect hardcoded secrets without returning the values, unsafe query construction, missing validation at declared boundaries, permissive certificate validation, and logging of configured sensitive fields. Keep signature-based and semantic results labeled with their evidence. |
| Dependency policy | Surface unapproved dependency changes or advisory evidence beyond the configured build. Use evaluated references and restore inputs for context. |
| Architectural boundaries | Check declared forbidden references, project cycles, layering constraints and duplicate public contracts. Compare public API against a baseline and report changes with project and target context. |
| Agent edit integrity | Detect newly disabled or skipped tests, removed assertions, newly added broad warning suppressions, newly public members and copied placeholder bodies. Report structural evidence and confidence. Use diff context to avoid resurfacing unrelated debt. |
| Repository invariants | Provide declarative rules for forbidden APIs, required cancellation forwarding, expected DI registrations or required integration seams. Use typed symbol signatures for semantic rules and typed config for exceptions. |
| Spelling | Split PascalCase, camelCase, acronyms and snake_case in identifiers. Check comments and docs under their own policy. Exclude URLs, hashes, generated code and string literals by default. Report the unknown token and bounded suggestions from a versioned local dictionary, with user and repository whitelists. |
| Test outcomes | Report passed, failed, skipped, zero discovered, build failure, adapter failure and unavailable coverage by runtime and target. Attach the failing test identity and a bounded diagnostic excerpt. Preserve first failures across explicitly requested reruns. |
| Verification needs | Use incoming and outgoing calls, implementations and value flow around modified symbols to identify which test or operational checks might be needed. Compare those needs with current, scoped evidence and explain each gap. |

For each adapter, define the smallest trustworthy evaluation scope, the inputs that invalidate it and a complete-result fixture with both a positive case and a near match. When two providers report the same span, group the display only where identities and rules can be attributed. Keep both original records and rule outcomes. Apply baselines for equivalent coverage only. Suppressions use a versioned rule ID, bounded scope, reason and optional expiry. An expired or unresolved suppression is visible in policy status.

Provider contracts are based on their own documentation. [Sonar C# analyzer package](https://packages.nuget.org/packages/SonarAnalyzer.CSharp), [SonarScanner workflow](https://docs.sonarsource.com/sonarqube-cloud/analyzing-source-code/scanners/sonarscanner-for-dotnet/using), [JetBrains InspectCode](https://www.jetbrains.com/help/resharper/InspectCode.html), [Microsoft package reporting](https://learn.microsoft.com/en-us/dotnet/core/tools/dotnet-package-list)

## Duplication detection

Index Roslyn tokens in methods, constructors, accessors, local functions, lambdas and sufficiently large statement runs. Persist fingerprints in the [shared codebase index](semantic-index.md). Start with a minimum of 40 substantive tokens. Ignore comments and trivia, exclude generated files by default, and index two representations.

- **Exact token match** preserves identifiers and literal values. Report 1.0 only after comparing the token spans.
- **Renamed or edited match** normalizes local identifiers consistently within a fragment and buckets configurable literal types. Preserve operators, control flow and resolved member identities when available. Label the normalization and show whether symbol resolution was available.

Hash sliding token shingles and select sparse fingerprints via winnowing. Look up matching postings to nominate likely peers, and bound postings for common boilerplate. Refine candidate spans with bounded token alignment. Compute similarity as `2 × aligned matching tokens / (left span tokens + right span tokens)` over the selected representation. Report the score, normalized and exact matched token counts, both spans, tuning profile and whether candidate limits were reached. Start with a near-match threshold of 0.80. Calibrate using real fixtures containing overloads, generated data, repeated guard clauses, renamed copies and edited clones. Treat normalization and threshold changes as index version changes.

Update postings for changed files and remove deleted postings and affected result pairs transactionally. Recompute peers of removed or changed pairs so stale matches disappear. Prefer maximal spans and collapse nested overlapping results. Order ties by score, span size and path. Save only compact fingerprints, offsets and lookup metadata on disk. Heavy solution scans can expand limits under explicit request. Keep this checker independent of JetBrains tooling. [JetBrains dupFinder retirement](https://blog.jetbrains.com/dotnet/2021/08/12/sunsetting-dupfinder-command-line-tool/), [Winnowing paper](https://theory.stanford.edu/~aiken/publications/papers/sigmod03.pdf)

## Opt-in analysis and planning

SlopHawk exposes these optional checks through the installed CLI and MCP. Each uses the same existing input admission and evidence scope as its underlying producer.

| Check | CLI | MCP | Evidence |
| --- | --- | --- | --- |
| Near-miss token clones | Set `nearMissClones` to `true` in an explicit `shadow --request` request or in the owner’s [resident source-check profile](rust-migration-qualification.md#scheduled-source-checks). Read resident results with `results`. | Read the current `checkerReport.nearMissClones` via `get_findings` after the owner enables it. | Same-language admitted blocks with SHA-256-bound source coordinates. Python also requires compatible suite structure and logical lines. See [Python clone layout](python-clone-layout.md) for the supported edits. |
| Correlated scalar planning | Use `paths plan <spec> <scratch> <build-manifest> graph-correlated-v1`, then use the normal `paths check` and `paths review` flow. | Call `plan_checker` with `checker: "scalar_correlated"` and the scalar wire text in `facts`. | The CLI binds the mode and the Roslyn projections to the sealed plan. Its `candidates.json` includes `ScalarFacts` keyed by operation for the opt-in mode. Interactive results are predictions over the supplied facts. |
| Diverse contract cases | Use `check-contracts --diverse <assembly.dll> <rules.json> <published.json> <workspace> [per-operation [run-verb [prepare-verb]]]`. The summary records `CaseSelection: "diverse-v1"`. | Call `plan_checker` with `checker: "contract_diversity"`, arrays `rules`, `published`, and `pool`, and a `perOperation` budget. | Both paths qualify rules against published examples and select from an ordered input pool. The CLI generates a pool of up to four times each existing pass budget, capped at 8192 per operation, and executes only cases admitted by the original per-operation budget. MCP returns selected inputs and rule-derived expectations. |

The CLI `checker-plan --request <request.json>` accepts the same JSON as the MCP tool arguments. These two interactive entry points read only the supplied evidence and report it as `caller_supplied`. Planning runs in memory over inline evidence. Adapter execution and workspace publication use the separate owner-granted CLI or resident workflows. Both bound JSON input and output to 256 KiB. Interactive scalar requests allow at most 256 work units, 128 cases and 32 parameters, with a separate expanded expression budget. Interactive contract requests allow 16 rules, 256 published examples, 1024 pool rows and 128 cases per operation.

Near-miss matches live in a separate report field. Each result includes the two block spans, edit distance and similarity. A replacement counts as two edits. The query uses an 80 percent similarity floor and at most 64 edits, with other work limits enforced by the index and at most 32 returned pairs per language. A report byte limit can trim near-miss pairs and leaves a visible gap. Findings describe token alignment, with provider coverage and unknown frontiers recorded separately.

The remaining TAVO checks require producers for additional evidence.

| Check | Required evidence outside the live producer |
| --- | --- |
| Lost test routes | Two revision-bound, evaluation-sufficient project graphs with complete configured test groups and surviving-source observations. |
| Deferred publication freshness | Typed validation, invalidation and effect facts for the scheduled Rust code. |
| Paired regression evidence | Matched test case and assertion observations across two qualified builds. |
| AdditionalFiles identity audit | An attributed file observation and a verified settings witness for the same build. |
| C# resource lifetime regressions | Matched analyzed methods and use sites across two certified compiler snapshots. |
| Python import-layer regressions | Two admitted resolved import graphs with complete roots and edges for the configured policy. |
| Rust cancellation advisories | Resolved macro, callee and version evidence for specific select arms. |


## Related test discovery and execution

Discover candidate test projects under the workspace from solution membership, evaluated project properties, project references and installed adapters. Support projects outside solutions. Group test IDs by target framework and adapter. Treat naming conventions and framework attributes as discovery hints. Obtain runnable test identities from the runner for the corresponding build, and store them in the shared index against test assembly and configuration inputs. Honor repository settings and distinguish unit projects from integration and end-to-end projects through project policy and test metadata. Surface unclassified or intentionally excluded coverage.

For each batch containing confirmed C# additions, edits, deletions or renames, select tests in this order.

1. Start from both old and new project membership and test ownership for each path. Reverse-walk project dependencies separately for each affected build context. A changed test may affect its containing class, shared fixture or entire project.
2. Run the entire affected unit-test project for each selected target in 0.2. Use evaluated reference edges between concrete targets. Deduplicate by project, target and runner while retaining every trigger and at least one dependency path per cause within the explanation budget.
3. If project impact is uncertain, expand to the configured unit-test group within the automatic budget. Report the unknown graph frontier, that fallback and every deferred, excluded or manual-only scope. An authoritative empty selection requires complete membership and dependency coverage of the relevant configured group. Later versions can use static links or past coverage to propose narrower filters after validating filter completeness against the runner. Automatic admission stays within explicitly eligible unit-test scopes.

Use VSTest for compatible test projects in SDK-style and classic Windows workspaces. Use Microsoft.Testing.Platform when selected by compatible projects and SDK settings. Resolve runner-specific filters and output formats through adapters for xUnit, NUnit and MSTest. Add explicit adapters for legacy runners when fixtures demonstrate the need. Reuse a build when the relevant input and output graph matches, including generated sources and adapter binaries. Reuse test receipts that meet the same evidence requirements as a SlopHawk run, including receipts from tests already run by the build. Build only when an admitted verification job needs missing outputs. Surface build failures as verification blockers with a reference to the original output. [dotnet test and runner selection](https://learn.microsoft.com/en-us/dotnet/core/tools/dotnet-test), [Microsoft.Testing.Platform migration](https://learn.microsoft.com/dotnet/core/testing/migrating-vstest-microsoft-testing-platform)

Startup inventory, metadata-only events and tool outputs never trigger automatic tests by themselves. Metadata changes can invalidate discovery, and a later C# batch uses refreshed discovery. MCP can request explicit tests against the current workspace even when no C# file changed. Reports identify triggering paths, selected and omitted test projects, any filters, reason for selection, toolchain, elapsed time and the source revision the results actually covered.

### First runner adapter

Start with an SDK-style .NET 10 VSTest project. Choose the adapter from the actual selected SDK, test mode and evaluated project, including relevant directory configuration. Route native Microsoft.Testing.Platform and classic projects through declared compatibility entries when fixtures establish their behavior. Show an identified but unsupported runner as blocked.

- Evaluate the inputs and use a build matching the target and output path. If build is allowed and restore is disabled, require compatible assets in the configured intermediate location and use `--no-restore`. When invoking VSTest on the verified build through `dotnet test`, use `--no-build` and the same project, target, configuration and output settings. Tests can load repository code even during discovery. [VSTest CLI options](https://learn.microsoft.com/en-us/dotnet/core/tools/dotnet-test-vstest), [MTP CLI options](https://learn.microsoft.com/en-us/dotnet/core/tools/dotnet-test-mtp)
- Allocate an empty daemon-owned results directory per attempt. Request machine-readable output, starting with TRX for VSTest. Bound artifact size, record the artifact digest and validate the run summary and completeness alongside the adapter exit status. Ignore results from earlier runs. A missing or corrupt artifact reports an adapter failure even when the process exits successfully.
- Keep test outcome, executed and skipped counts, and adapter health independent. A recognized failing test with a complete runner receipt is a failed test run. Zero discovered, all skipped, build failure, adapter failure, timeout and cancellation get distinct dispositions. Resolve an automatic intent on a current passed or failed test run. Keep all-skipped scope visible as incomplete.
- Retain adapter-scoped identities and bounded failure metadata. Data rows or repeated display names require stable adapter identifiers before attaching per-test evidence. When the adapter only supports a project-level identity, report project-level evidence.

The [verification checker](verification.md) uses the same symbol and test edges to identify missing or stale evidence. Its catalog covers unit tests through post-release checks. A recommendation does not schedule a test outside the automatic unit-test policy.
