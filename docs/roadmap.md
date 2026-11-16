# SlopGuard roadmap

**Planning window** December 2026 through November 2027. Dates are targets. Each release waits for its acceptance gate. The [development build](source-reconciliation.md#current-implementation) has durable source inventory, evaluated C# membership, test intents, worker admission and Windows process supervision. Production still needs a test runner. The 0.1 binary remains the syntax-only fallback.

## Twelve monthly increments

| Month | Deliverable and dependencies | Exit gate |
| --- | --- | --- |
| Dec 2026 | **Recovery and attribution.** Exercise v1–v4 state migration, transaction failures and quota fallback. Harden publication across Git transitions and root replacement. Start local IPC resource limits and version negotiation. | Restart preserves pending causes and interrupted attempts. Incompatible or unreadable state produces a blocker. Validated readers reject old epochs, including branch changes at the same commit. |
| Jan 2027 | **First real test adapter.** Define build identity and artifact provenance. Parse bounded TRX. Run one approved SDK-style .NET 10 VSTest target with Windows supervision. Reuse matching outputs and distinguish prerequisite failures. | A fixture edit runs a related test project. Passed, failed, zero discovery, all skipped, malformed output, denied operations and interruption each produce the proper disposition. Identical validated inputs reuse the receipt. |
| Feb 2027 | **Linux parity.** Add supervised Linux admission after proving a durable scope and atomic enrollment. Complete target-specific selection explanations and manual MCP invocation. | On supported Linux, cancellation and daemon replacement drain descendant processes before output leases are reused. Unsupported VM configurations report a blocker. Windows and Linux agree on target selection for shared fixtures. |
| Mar 2027 | **0.2 self-hosting candidate.** Bind Cobertura coverage to runner and build receipts. Expose scheduler, report and coverage states through CLI/MCP. Add report marks, idempotency and transition leases. | A C# edit in SlopGuard runs an eligible SlopGuard unit suite and explains its scope. The manual integration suite remains separate. Exercise rollback to 0.1 with no outstanding worker leases. |
| Apr 2027 | **Semantic context.** Extract Roslyn declarations and direct calls for evaluated targets. Persist versioned per-context partitions, physical/logical document identities and bounded predecessor facts. | Warm and cold facts agree on conditional targets, linked and deleted files, partial declarations and overloads. Missing generator or reference inputs surface an explicit frontier. |
| May 2027 | **0.3 explainable verification.** Add reference and implementation queries, bounded call traversal, conservative dispatch and test reachability. Evaluate the first verification rules for edited branches, public contracts and serialization. | Old and new caller chains explain removed symbols. Exact, possible, unresolved and truncated edges remain distinct. Static paths, observed hits and test outcomes remain independent. |
| Jun 2027 | **Value flow and evidence.** Add intra-procedural summaries and bounded propagation through supported calls. Detect pre-production evidence using typed adapters for test, analysis, benchmark and contract artifacts. | A changed value can be traced across a fixture's parser, validation and write boundary. Catalog fixtures cover matches, misleading names and missing evidence. Unknown edges stay visible. |
| Jul 2027 | **0.4 supplemental checks.** Index exact and renamed clones, then calibrate near-clone candidates on labeled fixtures. Add scoped spelling and baseline comparison. | Changes invalidate both sides of stale pairs. Reports reproduce token ranges and scores. Measure recall, false positives and memory on a labeled fixture before enabling heuristic findings by default. |
| Aug 2027 | **Repository policy.** Introduce typed checker modes, versioned settings, scoped suppressions and repository rules for public API and dependency boundaries. Add optional inspection adapters where fixture comparisons show additional coverage. | Policy changes invalidate affected results. Rules preserve provenance and target context. Unavailable licenses, tools or unapproved operations produce explicit blockers. |
| Sep 2027 | **0.5 compatibility and onboarding.** Add native Microsoft.Testing.Platform behind a separate adapter. Exercise older SDKs, .NET Standard and a small explicit classic .NET Framework matrix on Windows. Package the CLI and skill. | Every claimed SDK, TFM, OS and runner row passes its own fixture. Unsupported combinations expose the missing prerequisite with a path to recovery. A clean checkout can install and start the supported package. |
| Oct 2027 | **Release and operational evidence.** Complete typed detectors for deployment, release and post-release evidence. Add agent checkpoints and configurable release gates. Test concurrent agents and backpressure under realistic edit bursts. | Checkpoints carry current scope, assumptions and blockers. Operational signals are imported or referenced through approved read paths. A missing production artifact yields unknown or missing according to detector completeness. |
| Nov 2027 | **1.0 release candidate.** Stabilize public schemas, packaging and support matrix. Run platform, stress, fault-injection, upgrade and rollback gates. Review default settings and documentation against shipped behavior. | Two consecutive clean candidate validations pass with no unresolved correctness or data-loss blocker. Compatibility, limits and rollback procedures are reproducible from the tag. |

If Linux supervision, VSTest, or freshness slips, preserve their acceptance criteria and move the self-hosting release. Semantic extraction, parsers and catalog rules can progress in isolation. Additional languages and a third-party plugin API require a separate release decision after the C# contracts stabilize.

The [coding-agent checks](verification.md#checks-for-coding-agents) extend the verification increments. Start with lost test routes after selection explanations, then add parity and paired evidence checks as the reader and runner contracts stabilize. Pilot the report-publication check after value flow. Their fixture and cost gates determine when they appear by default.

## Initial implementation slices

These slices are intended for independent patches against a shared base. A producer can land before its consumer; production stays on the existing path until both are reviewed. Add focused tests in named, separate files to minimize parallel edits.

| Slice | First independently testable increment | Follow-on |
| --- | --- | --- |
| SG-01 | Inventory migration and write-failure recovery fixtures | Broader quota and crash fault injection |
| SG-02 | Bounded VSTest TRX and exit-status classifier | Integrate with the runner and durable receipts |
| SG-03 | Build and output identity for reuse decisions | Capture identities through the real supervised build |
| SG-04 | Deterministic evaluated test-selection explanations | Surface them through an MCP tool and report |
| SG-05 | Linux supervised process scope prototype | Wire admission, recovery and permissions after real-platform proof |
| SG-06 | Current report fencing across Git and watcher invalidation | Add explicit agent transition leases |
| SG-07 | Bounded, versioned local daemon IPC | Negotiate optional MCP features from daemon capabilities |
| SG-08 | Roslyn document facts for an explicit compilation | Add persistent partitions and graph traversal |
| SG-09 | Coverage import and attribution for local Cobertura | Connect coverage intake to supervised run receipts |
| SG-10 | Deterministic incremental token-clone matching | Add scalable persistence and measured near-clone recall |
| SG-11 | Verification rule evaluation for typed evidence | Extend the catalog and add checkpoint queries |

## Release policy

- Treat Windows and Linux x64 SDK-style .NET 10 as the initial supported build matrix. List OS-specific execution prerequisites separately. Extend advertised support only after the corresponding fixtures pass.
- Preserve read-only operation when an optional SDK, adapter, analyzer, collector or provider is unavailable. Make blocked scope visible in CLI and MCP. Deploy, release and production checks consume approved evidence without initiating operational changes.
- Keep repository assets to implementation, source fixtures, build configuration, current design and roadmap. Report test and review results alongside the change. Runtime receipts and transient tool output stay in owned, bounded state or temporary storage.
- Measure the [delivery budgets](delivery.md#initial-performance-targets) on a fixed cold/warm fixture, a representative large repository and a concurrent edit/build workload. Record the environment, command, base and measured outputs with the validation result. Treat a changed environment or unknown input as a new comparison.
- Keep compatibility and rollback tests for each released on-disk and wire schema. Before an incompatible change, document the reader range, migration and rollback path. Advertise feature support from the daemon handshake.

See the [robustness contracts](robustness.md) and [delivery fixtures](delivery.md#correctness-fixtures) for the release acceptance criteria.
