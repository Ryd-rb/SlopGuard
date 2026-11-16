# Verification needs and coverage

The verification checker asks what changed, which behaviors it can affect, and what evidence exists for those behaviors. It covers the categories in Cindy Sridharan’s testing diagram, including checks before deployment and practices used during deployment, release and production operation. A repository can declare additional categories or map its own artifacts to the built-in categories.

## From change to warning

Apply versioned rules to the [shared semantic index](semantic-index.md). Each rule maps concrete change signals to verification needs and defines the acceptable evidence and scope. Use repository policy to mark checks required or advisory and to declare exceptions. Treat change signals and check evidence independently.

| Changed behavior | Example verification needs |
| --- | --- |
| Branches, validation, error paths or parsing | Unit, regression and coverage checks for edited branches. Consider property-based and fuzz tests for known invariants and untrusted inputs. |
| Public API, serialized value or input/output contract | Contract, component, integration and compatibility checks for affected callers and implementations. Consider shadowing or a comparison check for a behavior-preserving migration. |
| Dependency wiring, configuration, deployment startup or flag | Config, integration and smoke tests. Consider canarying, monitoring and rollback-related flag checks when the affected path is released gradually. |
| Concurrency, queue limits, retries or hot-path work | Performance, benchmark, load and soak checks. Consider tracing and profiling to see where time or capacity is spent. Consider chaos testing evidence when repository policy identifies a resilience objective. |
| Authentication, authorization or sensitive value flow | Unit and regression checks, static analysis, threat modeling, audit evidence and a record of any required authorized security review. |
| User flow or rendered interaction | Acceptance and functional checks, semantic UI/UX assertions, and manual usability or visual review where applicable. |

A warning includes the changed range and symbol, candidate caller or value-flow chain, rule and why it matched, test or operational category, search scope, evidence found, and a concrete missing or stale condition. If a changed deserializer reads an additional field from an HTTP endpoint and passes it to persistence, the checker can point to the candidate endpoint-to-write chain and request evidence for the new field’s serialization contract and invalid inputs. Existing unit tests that reach the parser count as static reachability until an applicable runner result or branch-coverage artifact is available.

For each needed category, report the strongest applicable evidence and any gaps.

| State | Meaning |
| --- | --- |
| `observed` | Matching execution or review evidence applies to the required behavior and target. An instrumented branch hit establishes execution coverage for that branch. Assertion quality and successful outcomes are reported independently. |
| `uncovered` | A matching report instrumented the required changed behavior and recorded no hit within the measured scope. |
| `configured` | A recognized test or operational check is declared and linked to the scope. Current execution or review evidence is unavailable. |
| `stale` | Prior evidence refers to different inputs, targets, deployments, tool versions or an expired review window. |
| `missing` | The enabled detectors completed the applicable search scope and found no matching evidence. The report identifies the search boundaries. |
| `unknown` | Indexing is incomplete, a dynamic boundary blocks tracing, an artifact is unavailable, or the detector cannot establish the scope. Show the blocker and any partial evidence. |

Use exact build and source mappings when attributing coverage to changed lines or branches. Record collector, binary or build identity, target, source mapping and execution outcome. Coverage aggregated across tests can show that code ran. Linking a specific test to a hit requires per-test data. Preserve distinctions among an unexecuted changed branch, code executing without a relevant assertion, a linked test that has never run, and a test that failed. When evidence is heuristic, label it and avoid definitive claims about coverage. [Microsoft .NET code coverage guidance](https://learn.microsoft.com/en-us/dotnet/core/testing/unit-testing-code-coverage)

## Checks for coding agents

These proposed checks look for specific ways a successful build or test run can leave the edited behavior unchecked. Run them on a reconciled change and surface the smallest next action an agent can take. Each result carries the rule version, the current revision and concrete target, the comparison revision or fixture when applicable, a source location or typed record, and a short reason. Compare one bounded scope at a time and coalesce duplicates caused by the same changed edge.

Use the existing verification states for evidence availability. Carry the check's result independently, with a concrete counterexample, a candidate that needs confirmation, or a completed comparison with no counterexample in scope. Incomplete observations remain `unknown` with their reason. Reconcile both the source and required input identities before publishing a result as current. Preserve historical results as historical. Start in advisory mode. Read-only checks consume available observations. A replay uses existing worker admission and an isolated, disposable fixture or snapshot when requested through an authorized scope.

### The edit lost a test route

**Trigger.** A surviving C# file changes, or a project or import change could affect the membership of surviving source. Compare two complete evaluated graphs for the affected contexts. Match concrete targets by canonical project, framework, configuration, platform and RID, retaining SDK and evaluation identities with the observations. Match logical compile items within each target, with explicit rename mappings when available. Traverse the prior and current reverse reference paths to the configured test group for each context.

**Finding.** A live source item disappeared from an explicitly supported context, or the last configured test route for its affected context disappeared. Report the old compile item or reference chain, the first missing edge and its defining file when the evaluator can attribute it. A target replacement needs an explicit mapping, plus current membership and a valid successor route. Attribute declared target retirements and scope changes to their configuration. Keep budget-deferred, unapproved and unevaluated targets distinct from a lost route. Evaluation-only membership omits items created by targets at execution time, so keep those contexts unknown until an applicable build observation is available. [MSBuild evaluation and target execution](https://learn.microsoft.com/en-us/visualstudio/msbuild/evaluate-items-and-properties?view=vs-2022)

**Agent action.** Inspect the named condition or project reference. Restore the route, verify the declared replacement, or ask for a decision on the target's supported status. The finding might show that `Parser.cs` still compiles for the portable target and that the changed `Compile Remove` condition excludes it from the supported Windows target.

**Acceptance.** Detect a conditional removal in one context even when the other context passes. Include a lost transitive test route, an unchanged alternate route, a linked file with distinct logical items, a declared retirement, and a partial graph. The same snapshots produce identical warm and cold findings. Use the evaluated target graph and [selection explanations](checkers.md#related-test-discovery-and-execution) for attribution. A capped explanation alone cannot establish absence.

### The regression test accepts the old behavior

**Trigger.** An agent requests evidence for a named bug fix and supplies one stable unit-test case and its intended observable assertion. Run the same test source, assertion, fixture inputs and adapter-scoped case in two separately built, isolated snapshots. Match SDK, target, runner, dependency inputs and relevant environment. Preserve the candidate test and fixture in both arms. Substitute only the bounded production change under review. Record the exact substituted paths and digests. Use a complete product baseline only when it keeps the rest of the comparison compatible.

**Finding.** A pass on both arms means this case accepts the old behavior. A pass on the candidate and the intended assertion failure on the old arm establishes that the test distinguishes the two builds for that input. Compare an attributable assertion location or typed oracle, including the expected observation. A generic failed run can be reported as such. A compile error, missing test case, setup failure, ambiguous data row, mismatched fixture, or inconsistent replay leaves the behavior comparison unknown. [SWT-Bench uses tests to reproduce and filter fixes](https://arxiv.org/html/2406.12952v3), [Probe to Generate uses program variants to expose weak tests](https://arxiv.org/abs/2604.01518v2)

**Agent action.** Add an input or assertion that distinguishes the intended behavior, then repeat the paired check. For a parser fix, assert the specific rejection reason and preserved state, and keep an adjacent valid case as a control.

**Acceptance.** A fixture must distinguish an intended old-arm assertion failure from an unrelated exception. Include a no-op candidate, a neighboring valid case, a baseline that cannot compile with the new test, and a case that loses stable row identity. Keep the initial scope to one test identity and two arms with existing execution time limits. Preserve both receipts even if an arm fails early. Report inconsistent reruns when repeated under the existing execution policy.

### A deferred effect outlives its freshness check

**Trigger.** An edit touches a declared effect, the validation it depends on, or the control flow between them. Start with one SlopHawk contract for publishing a report tied to a source epoch. Bind the production methods by symbol. Track validation of the same epoch and root, relevant invalidation boundaries such as suspension or queued execution, and protection across the final effect. Recognize only declared helper contracts, including a commit wrapper that validates and commits under the same synchronization or transaction. Track the underlying state identity through supported copies. A passing check on one branch says nothing about other paths to the effect. [Roslyn control-flow graph](https://github.com/dotnet/roslyn/blob/main/src/Compilers/Core/Portable/Operations/ControlFlowGraph.cs), [SlopHawk publication contract](result-publication.md)

**Finding.** Show the changed reachable path from the last relevant validation through the boundary to the effect. Call this a candidate until a controlled fixture demonstrates the outcome. With a deterministic gate in owned fixture code, pause after validation, invalidate the epoch, resume and record whether a stale report was committed. An unresolved delegate, alias or helper exposes an unknown frontier. A wait by itself is insufficient evidence.

**Agent action.** Use the appropriate commit-time validation wrapper or put validation and the effect under the contract's shared synchronization. Add a deterministic invalidation fixture at that boundary.

**Acceptance.** Pause the same fixture at the same point for an old and a corrected implementation. The old variant attempts or publishes a stale report as defined by the fixture oracle, and the corrected variant rejects it and still publishes when current. Include safe work after an await with an owned immutable input and a wrapper with an unresolved dispatch. Begin with intraprocedural analysis and explicitly modeled helpers. Use controlled gates in place of elapsed-time sleeps.

### An observed input is absent from the reuse identity

**Trigger.** An edit affects an input consumed by an admitted build, test or evidence collector, or changes the code that collects reuse inputs. Start with a provider for exact inputs identified by evaluated items or typed, attributed build observations. Compare its input identity and digest with the inputs represented by the relevant stamp, including any validated aggregate that covers the input. Associate the observation with the exact operation, target and receipt class. For instance, an `AdditionalFiles` input can affect generated code even when no C# source changed. [Bazel action inputs and remote caching](https://bazel.build/remote/caching)

**Finding.** Show the observed input change and the stamp section that should cover it. Confirm a reuse error in a fixture by changing only that input and observing both an unchanged reuse key at the decision point and a changed, contract-relevant result on an independent cold run. Compare the input key before reuse, even if a full cold build would change the final output digest. A dynamic read outside the provider's scope, an unresolved aggregate digest, or a nondeterministic cold result remains unknown. Output equality alone cannot establish complete input coverage.

**Agent action.** Bind the missing input to the appropriate input identity or make the affected operation ineligible for reuse until the dependency can be represented. Rerun the one-input fixture and verify that the previous receipt is no longer applicable.

**Acceptance.** Cover an omitted generator input, the same input included through a proven aggregate, a changed irrelevant file, and an unresolved dynamic input. Warm and cold paths must agree on the corrected result. Use [build and test receipt provenance](robustness.md) and the existing `BuildOutputIdentity` sections where applicable. A hash confirms the bytes supplied to it. It cannot establish that the collector saw every dependency.

### Equivalent entry points disagree

**Trigger.** A change reaches a declared parity group. Start with one read-only operation that presents current test selection or verification evidence through both CLI and MCP. Give each entry point the same logical request, admitted scope, capability set, and pinned daemon state. Map the responses to a small typed projection of agreed fields, such as revision, disposition, selected target IDs and reason codes. Normalize timestamps, request IDs or ordering only where the contract explicitly permits it.

**Finding.** Show the first differing contract field and the fixture state that produced it. A stale report called current by one surface is a useful counterexample even if both interfaces returned successfully. Capability differences and state changes between calls make the comparison unknown when they prevent a like-for-like replay. Agreement establishes parity for the fields compared. A fixture with an independent expected result handles a shared wrong answer. [McKeeman, Differential Testing for Software](https://www.cs.tufts.edu/comp/150FP/archive/bill-mckeeman/DifferentailTesting.pdf)

**Agent action.** Inspect the adapter projection for the differing field and reproduce the paired request after a change. For example, verify both surfaces tell the agent a target was deferred for budget rather than implying it ran.

**Acceptance.** Include matching states with intentionally different formatting, stale versus current results, budget-deferred targets, a capability mismatch, and a shared wrong result caught by the fixture oracle. The first implementation reads from one fixed fixture revision. Run paired replays on a changed operation or explicit request.

### Implementation order and evaluation

1. Add lost-route comparison over captured evaluated graphs after the current selection projection is available. It has the clearest initial evidence contract and can be exercised without a runner. Start with disappearance from an unchanged supported target, then add successor mapping.
2. Add read-only parity fixtures with the self-hosting surfaces. Add paired regression tests and reuse-input perturbation once the admitted runner can reliably identify the exact case, build and receipt. Land the fixture-only checks before adding an agent-facing command.
3. Add the deferred-effect candidate rule after the June value-flow increment. Qualification starts with report publication and a deterministic state-transition fixture. Extend to another declared effect only after verifying the state model and false positives for the first.

For each check, freeze a small set of ordinary edits and plausible regressions whose usual builds and tests pass, with independently authored expected findings and at least one held-out fixture family. Compare against existing selection and verification output. Count extra actionable findings, false positives, missed regressions, unknowns, and the added warm/cold time and replay count. Require all mandatory fixtures to cover their stated behavior, no definitive claims on incomplete evidence, and reproducible evidence for every reported counterexample. Before enabling an advisory by default, report precision on held-out edits and compare the added cost with the [delivery budgets](delivery.md#initial-performance-targets). A paired agent trial can then measure how often a finding leads to the correct next verification step, retaining timeouts and unknowns in the denominator. Fixture success qualifies the check on measured cases. Agent benefit remains an empirical question.

## Coverage import for 0.2

Import a bounded local Cobertura artifact and a receipt written by the run supervisor. The receipt binds the artifact digest to the run, collector version, build and binary identities, target, source digests and path mappings. Hash and parse the same artifact bytes under an owned read root. A standalone report without a matching receipt can be listed as unverified evidence. An artifact from another run or build remains historical. Aggregate receipts may cover several targets only if their measurements remain attributable by target.

| Evidence available | Claim |
| --- | --- |
| Discovered test with a static path | Configured reachability, with exact or possible binding and the traversal frontier. |
| Completed test result for the target | Test outcome and measured test scope. Symbol execution stays unknown until linked by runtime evidence. |
| Matching line counter | Observed or uncovered line within the instrumented scope. |
| Matching branch counter with stable source mapping | Observed or uncovered branch within the instrumented scope. A line-level condition percentage identifies aggregate branch coverage. An individual arm stays unknown without a mapping to that arm. |
| Mapping ambiguity, truncated artifact or missing instrumentation | Unknown for the affected scope. A measured zero requires a counter for the exact mapped behavior. |

Use target-specific source context for linked files and restrict path aliases to a receipt's validated mapping. A historical line query identifies what was measured at that target, path and line. To ask about the staged source, supply its verified SHA-256 as well. A historical hit with a different source digest leaves current execution unknown. Keep runtime execution, linked assertion evidence and run outcome independent. A failed run can still show valid hits if the collector and receipt establish what was measured. Static links and coverage silence cannot establish that an assertion checked the relevant value. A completed search with no linked tests identifies its searched and excluded targets. An incomplete search reports the unknown frontier.

## Detection catalog

Detectors search evaluated project and test metadata, C# symbols and registrations, repository configuration and workflow files, and approved local artifacts or review records. Use typed adapters for known formats and repository-configured mappings. Names and keywords only nominate candidates. Link confirmed evidence to a symbol, feature, service, deployment or user journey and include the detector and source. Report detector availability for every category.

### Pre-production

| Category | Evidence SlopHawk can detect |
| --- | --- |
| Unit tests | Test adapter IDs, test entry points and assertions targeting a changed unit, plus matching run records. |
| Functional tests | Scenarios and assertions exercising observable feature behavior. |
| Component tests | Test fixtures running a component with controlled dependency boundaries. |
| Fuzz tests | Fuzz targets, corpora and run summaries mapped to parsers or input boundaries. |
| Property-based tests | Generator, property and invariant registrations tied to the tested symbol or type. |
| Formal verification | Specifications, proof tasks and verifier results linked to a modeled behavior. |
| Static analysis | Analyzer configuration, enabled rule IDs and matching diagnostics or analysis receipts. |
| Mutation tests | Mutation targets and reports of killed, survived and untested mutants. |
| Coverage tests | Collector settings and source-mapped line or branch reports for affected code. |
| Benchmark tests | Benchmark entry points, parameter sets, baselines and measured deltas tied to hot paths. |
| Contract tests | Provider and consumer test suites, schema compatibility checks and contract results. |
| Lint tests | Lint and formatting configuration and reports for affected sources. |
| Acceptance tests | Business scenarios, criteria and linked test runs or review records. |
| Regression tests | Tests or suites linked to a known defect, prior failure or invariant. |
| Smoke tests | Short startup or critical-path probes and their execution evidence. |
| Usability tests | Scoped manual sessions, protocols and review records tied to a changed user journey. |
| UI/UX tests | Interaction, layout, navigation and accessibility checks that use semantic assertions, plus manual review records. |
| Penetration tests | Authorized scope, review records and imported findings linked to an affected boundary. |
| Threat modeling | Threat model changes, trust-boundary mappings, mitigations and review records. |
| Performance tests | Workloads, thresholds and latency, throughput, CPU or memory results tied to a changed behavior. |

### Deploy

| Category | Evidence SlopHawk can detect |
| --- | --- |
| Integration tests | Test projects and environment fixtures that exercise an actual service, datastore or adapter boundary. |
| Tap compare | Traffic capture or tap configuration, paired output comparison and divergence reports for old and new behavior. |
| Load tests | Workload definitions with concurrency or rate targets, capacity thresholds and reported results. |
| Shadowing | Mirroring configuration, shadow deployment, side-effect isolation and an optional comparison receipt. |
| Config tests | Config schema validation, deployment preflight checks and environment-specific validation results. |
| Soak tests | Sustained workload plans and long-run resource or failure trends. |

### Release

| Category | Evidence SlopHawk can detect |
| --- | --- |
| Canarying | Staged rollout configuration, success and rollback criteria, and evaluation receipts. |
| Monitoring | Release-specific dashboards, alert rules and gate receipts tied to the rollout. |
| Traffic shaping | Rollout weights and routing policies that match the affected service. |
| Feature flagging | Flag declarations and consumers, defaults, rollout and rollback policy, and flag-path tests. |
| Exception tracking | Exception capture configuration, release identifiers and redacted evidence that the changed path is observable. |

### Post-release

| Category | Evidence SlopHawk can detect |
| --- | --- |
| Teeing | Traffic or event fan-out, destination mappings, isolation constraints and routed-volume receipts. |
| Profiling | Profiler configuration or imported summaries mapped to binaries and changed symbols. |
| Logs/events | Structured logging and event declarations, schema and sink configuration, and approved receipt metadata. |
| Chaos testing | Existing resilience experiment specifications, abort criteria and result records. |
| Monitoring | Ongoing service indicators, alert rules, dashboards and alert-validation receipts. |
| A/B tests | Experiment flags, cohorts, assignment and metric definitions, and evaluation records. |
| Tracing | Span instrumentation, context propagation and approved trace summaries for the affected path. |
| Dynamic exploration | Exploratory test charters, manually executed session records and findings. |
| Real user monitoring | Client instrumentation, consent-aware configuration and approved aggregate experience metrics. |
| Auditing | Audit events and retention or integrity policy, plus audit review evidence for affected operations. |
| On-call experience | Service ownership, escalation mappings, alerts, runbooks and drill or incident-review records. |

One artifact may support several categories. Keep the reason and scope for each mapping. Production checkers inspect local declarations or explicitly imported evidence. Deployment, traffic manipulation, experiments and operational interventions remain external activities. Automatic test execution continues to be limited to related unit tests after a C# delta. Heavy or manual verification categories appear as evidence requests. Use aggregate or redacted operational evidence and retain only the metadata needed to tie it to a changed behavior.
