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

## Detection catalog

Detectors search evaluated project and test metadata, C# symbols and registrations, repository configuration and workflow files, and approved local artifacts or review records. Use typed adapters for known formats and repository-configured mappings. Names and keywords only nominate candidates. Link confirmed evidence to a symbol, feature, service, deployment or user journey and include the detector and source. Report detector availability for every category.

### Pre-production

| Category | Evidence SlopGuard can detect |
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

| Category | Evidence SlopGuard can detect |
| --- | --- |
| Integration tests | Test projects and environment fixtures that exercise an actual service, datastore or adapter boundary. |
| Tap compare | Traffic capture or tap configuration, paired output comparison and divergence reports for old and new behavior. |
| Load tests | Workload definitions with concurrency or rate targets, capacity thresholds and reported results. |
| Shadowing | Mirroring configuration, shadow deployment, side-effect isolation and an optional comparison receipt. |
| Config tests | Config schema validation, deployment preflight checks and environment-specific validation results. |
| Soak tests | Sustained workload plans and long-run resource or failure trends. |

### Release

| Category | Evidence SlopGuard can detect |
| --- | --- |
| Canarying | Staged rollout configuration, success and rollback criteria, and evaluation receipts. |
| Monitoring | Release-specific dashboards, alert rules and gate receipts tied to the rollout. |
| Traffic shaping | Rollout weights and routing policies that match the affected service. |
| Feature flagging | Flag declarations and consumers, defaults, rollout and rollback policy, and flag-path tests. |
| Exception tracking | Exception capture configuration, release identifiers and redacted evidence that the changed path is observable. |

### Post-release

| Category | Evidence SlopGuard can detect |
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
