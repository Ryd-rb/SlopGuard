# C# path exploration with explicit intent

**Proposed design.** The [holdout and acceptance plan](../experiments/slophawk-path-intent/benchmark-design.md) defines the first experiment and the evidence required to expand it.

## Scope and first decision

SlopHawk already searches inputs against supplied integer contracts, records observations, and verifies that representative discrepancies reproduce. A path checker should add inputs aimed at code branches and state transitions that ordinary probes rarely reach. Its primary output is a short review file with concrete witnesses and explicit gaps. A full control-flow graph is useful as supporting evidence. [Contract search and evidence workflow](../README.md)

A review result must answer three questions independently.

| Question | Evidence |
|---|---|
| What control flow does Roslyn represent? | A graph tied to its declared analysis context, with build attribution when compiler inputs are verified. |
| Which behavior happened? | An admitted fixture and input, the observed exit and state, and any branch trace the adapter actually recorded. |
| Why is it a defect? | A reproducible difference from a grounded expectation, with the requirement and the exact property that failed. |

The first implementation is an advisory, on-demand graph-guided input planner for synchronous, acyclic C# methods in one controlled .NET 10 project. Cover primitive inputs expressible by the existing integer request format and non-null return comparisons against independently sourced rules. Use one declared mapping from public operation to bound method and existing numeric adapter. The stateful workflow below is a proposed extension, gated by a measured gain over current probes and its own adapter and observation checks. Its first scope uses finite public recipes and can replay two calls while modeling only one.

## User and agent workflow

1. **Choose the behavior to review.** Within the selected compilation, resolve exactly one bound method body and retain its symbol identity and declaration and body spans. An ID or location is a selection hint. Identify a local function separately from its enclosing body. Project evaluation supplies candidate membership and reference mappings. A build-tied graph requires a verified compiler-input manifest for the selected SDK, target and configuration. Include effective parse and compilation options, ordered source and generated trees, reference assembly content and aliases, and relevant generator inputs. Retain the manifest origin and fingerprint separately from project evaluation and the candidate assembly hash. Missing inputs, ambiguous overloads or a pasted fragment yield an explicitly scoped structural view or a source skeleton. [MSBuild Csc inputs](https://learn.microsoft.com/en-us/visualstudio/msbuild/csc-task?view=visualstudio)
2. **Capture intent.** Read approved requirements, public contracts, existing acceptance assertions, and documented caller assumptions. For new code, capture the intent during the task before observing the implementation. For existing code, record the source behind every rule. Keep agent-inferred expectations available as hypotheses.
3. **Plan the search.** Export the graph and choose goals near changes, uncovered branches, writes to specified state, exception sites, and outcomes that overlap a stated rule. For a goal with unclear intent, publish a question the agent can answer without seeing the candidate observation.
4. **Freeze each answer before executing its cases.** The answer may supply a reusable rule, specify a single expected case, identify an approved rule, or leave policy open. Store the question, source snapshot, applicability and approved expectation under a stable revision. Record what the author had seen before freezing it. A later correction creates another revision and retains its exposure history.
5. **Execute and replay.** Run the candidate with a fresh fixture per case. Record the return value or exception, projected state, effects, and available trace. Repeat reported discrepancies on the same build, fixture and adapter. Route ordinary exact comparisons through existing SlopHawk evidence where their semantics match.
6. **Review.** Surface reproduced discrepancies first, with the input, setup, failing property, source location, and replay link. Include meaningful coverage gaps and unresolved intent. A bounded search may end with results and open items.

A filesystem exchange keeps the workflow usable from a CLI, an agent skill, or a future MCP tool. A possible CLI shape is `paths plan` to write the graph and questions and `paths check` to consume answers and execute the plan. They communicate through the same versioned artifact format. An agent can respond between tool calls within one turn.

## Intent requests

Ask about an outcome the requirement leaves open. A useful question names the public operation, admitted setup, proposed input, and state or effect to inspect. Include approved requirement references when available. Keep candidate outputs, branch spans, suggested outcomes inferred from code, and private witnesses out of the author-facing request. Record whether public policy alone selected the question or the candidate influenced case selection. Even a neutral card can direct attention to a candidate-selected condition. Record exposure to source, prior observations of the operation, and any unknown exposure separately from the time the answer was frozen. The reviewing agent may use the graph. A reviewer answering their own questions after seeing code contributes a separate workflow result.

> A patron requests pickup of the last available copy. The pickup branch is closed and policy denies fallback. What should the call return? Which inventory, reservation, and event fields may change? Cite the policy that determines the answer, or mark the case unresolved.

Prefer a reusable answer grounded in conditions available before calling the method. For example, an eligible pickup request at a closed branch with fallback denied must return BranchClosed, retain the original inventory and reservation, and emit no allocation event. Record any policy priority, such as patron ineligibility taking precedence over branch closure. Keep three decisions separate. Public admissibility determines whether the case is permitted. The fixture determines whether the requested initial state can be constructed. The rule antecedent determines which requirements apply. A condition on the candidate’s return can express an invariant about that return. A required return has a pre-state and input antecedent so a wrong return cannot make the rule vacuous.

A proposal for the first intent format follows. Names, types, projections, and effect kinds come from the adapter’s declared schema.

```json
{
  "ruleId": "pickup.branch-closed",
  "source": {
    "kind": "requirement",
    "path": "contracts/pickup.md",
    "anchor": "declined-pickup"
  },
  "admitted": "args.requested > 0",
    "when": "before.patronEligible && before.pickupClosed && !before.fallbackAllowed",
  "expect": {
    "outcome": { "kind": "return", "value": "BranchClosed" },
    "state": {
      "available": { "equals": "before.available" },
      "reservationStatus": { "equals": "before.reservationStatus" }
    },
    "effects": { "allocationCreated": { "count": 0 } }
  }
}
```

Start with a small typed language for input and pre-state predicates, exact scalars, enum sets, equality to pre-state, bounded deltas, and counts of named effects. Evaluate each permitted alternative as a nonempty *whole* outcome on the same receipt. An alternative passes when all its assertions pass, fails when a required assertion is contradicted, and stays unknown when an unevaluated assertion could change the answer. The rule passes if an alternative passes, fails if every alternative fails, and stays unknown otherwise. Define ordered effects and unordered sets separately. Tag a valid null inside a typed observation. An absent field means missing evidence. Observed event counts are lower bounds until the recorder confirms complete capture for the relevant invocation and effect kind. An application exception can be an expected outcome with separate state and effect assertions.

Intent precedence comes from the project’s own authority. For each assertion, record the policy revision, exact clause and authority decision. Verify that the clause supports the return, state or effect property actually asserted and that the translation preserves its meaning and priority. A rule-level citation can point to a common source when it authorizes every assertion. Example qualification establishes agreement on those samples. An inference or conflict stays unresolved pending an authorized decision. Record inadmissible input, setup failure, a false antecedent, unknown applicability, unresolved authority, and an incomplete observation as distinct outcomes. A post-observation revision can support triage and retains its exposure classification when used to interpret old observations.

### Example of a state-only defect

Suppose pickup first reserves a copy, then checks the selected pickup point. A late rejection occurs only when the point is closed, fallback is denied, and this request reaches the last-copy staging branch. The graph identifies the conjunction. The adapter creates the state through normal fixture setup and verifies there is one free copy before calling the method. The expectation above predicts `BranchClosed`, one free copy, the original reservation status, and no allocation event.

A candidate that returns `BranchClosed` with zero free copies has violated the inventory assertion. A repeat from a fresh equivalent fixture can establish the discrepancy. A nearby case with fallback allowed can help localize the guard if its expected outcome is also documented. Seeing the correct return value or hitting the rejection branch alone would leave the inventory violation undetected.

## Graph and search

Build the graph from Roslyn’s bound method-body control flow. Export block and edge IDs, conditional and fall-through successor roles, branch condition kind and semantics, and entering, leaving and finally regions. Preserve edges with no destination. Associate spans with the operations that supplied them and distinguish exact, enclosing, synthesized and unmapped locations. A written condition can yield several graph edges. A return edge through a finally block can lead to a thrown exception. Exceptional control and cleanup remain structural or unknown until their transitions are modeled and checked against C# execution. [Roslyn control-flow branches](https://learn.microsoft.com/en-us/dotnet/api/microsoft.codeanalysis.flowanalysis.controlflowbranch?view=roslyn-dotnet-5.3.0) · [C# semantic provider](../src/SlopHawk.Providers.CSharp/RoslynDocumentFacts.cs)

Use goal-directed bounded exploration.

1. Start from ordinary cases and the current probes. Prioritize changed branch edges and writes to contract fields. Reserve part of the search budget for unexplored goals outside the highest-ranked group.
2. For a goal, walk supported predecessor paths using resolved Roslyn operations. Start with built-in `int` and `bool` arguments, locals, assignments, comparisons, short-circuit Boolean operators, and unchecked `int` addition and subtraction with 32-bit wrapping. Bind guard-relevant pre-state to actual recipes when the stateful adapter is available. Use fixed-width arithmetic, preserve evaluation order, and record operand types, conversions and checked context. Defer proof for additional operators or conversions until compiled differential tests qualify them. For example, exact modeling of division must account for exceptional exits and short-circuiting, beyond the solver’s quotient. Unsupported or unbound transitions leave the affected path unknown. [C# expressions and numeric promotions](https://learn.microsoft.com/en-us/dotnet/csharp/language-reference/language-specification/expressions#1247-numeric-promotions) · [SMT-LIB fixed-size bit vectors](https://smt-lib.org/theories-FixedSizeBitVectors.shtml)
3. Seek distinct outcomes within a reached path. Try a violated postcondition only where the necessary transition and expected behavior are both modeled. Otherwise use legal input and recipe changes and compare the resulting observations. Spend some of the budget on guard-preserving neighbors that vary rule-relevant values, such as two distinct record IDs where the first model used equal IDs. Record correlated changes. Deduplicate executions and retain every origin, parent case, goal, recipe, and proposal order, including shared probes that the graph merely prioritizes.
4. Bound path states, solver work, and executions. Proposed pilot starting limits are 128 solver queries and 256 candidate executions per method, including a confirmation reserve, with an independent wall-time guard. Start with acyclic paths and log unmodeled loop frontiers. Choose targets and inputs in a stable order under a recorded model and seed. Report incomplete search explicitly.
5. Treat externally supplied state, dynamic dispatch, opaque helpers, asynchronous continuations, and unsupported arithmetic as explicit unknowns unless a supported model applies. Preserve already supported parts of the method. Start intraprocedurally. Consider adaptive trace-guided exploration after measuring the added value of static goals and replay. Pex is precedent for using execution traces and solved constraints to generate new inputs under a path bound. [Pex research](https://www.microsoft.com/en-us/research/publication/pex-white-box-test-generation-for-net/)

Keep these claims distinct in the graph and summary.

| Status | Meaning |
|---|---|
| Structural | Roslyn represented this edge in the declared analysis context. A build-tied claim requires the verified compiler inputs above. |
| Candidate input | The typed model predicts a route through the edge under recorded assumptions. |
| Observed | An attributed trace hit the edge for a saved fixture and input. |
| Infeasible within model | A named path is unsatisfiable within its recorded input domain and supported transition model. The pilot makes no edge-level or program-wide unreachability claim. |
| Unknown | A missing model, incomplete compilation, unsearched path, budget limit, or missing trace prevents the stronger claim. |

A successful replay can establish a behavioral discrepancy without a branch trace. Attribute an observed edge only with invocation-scoped trace evidence and a verified map from its counters or probe IDs to the specific graph and build. Aggregate coverage can describe the covered run at its own granularity. An instrumented build has a separate identity and mapping. Use the original candidate for the authoritative behavioral replay.

## Execution and evidence

Use a project-declared adapter. For scalar cases run the declared numeric adapter with the current exact comparator and separate retest. Verify the adapter's operation-to-method mapping and request ordering against the selected build. For stateful cases use a finite catalog of recipes. Each recipe identifies a public setup history with declared dependency inputs and verifiable guard-relevant values. Check the realized pre-state and admission before invoking the normal entry point. For a two-call case the admitted first operation must establish the second call’s actual pre-state. An inability to materialize it remains a setup result.

In the typed adapter, each receipt echoes a unique request ID and a digest computed from the bound request actually handled, plus phase, invocation index and entry point. Duplicate, missing, or mismatched replies leave the batch incomplete. Snapshot detached typed pre-state and after-state and record outcome, effects and observation completeness for each invocation. An expected application exception matches a throw attributed to the method call. Setup, snapshot, transport, timeout and cancellation failures have separate records. Verify reset of the declared state and effect domain, including fixture dependencies outside its objects, and replay a complete two-call sequence as a unit. Foreground adapters inherit the caller's execution boundary.

The legacy review groups by assembly and adapter verbs and identifies inputs by operation and integer arguments. Its exact comparator is available for compatible complete non-null results. The typed workflow needs its own verifier and identity. Bind the case key to the compiled context, candidate and adapter, declared dependency closure and reset contract, recipe version and parameters, ordered typed calls, target invocation, and observation schema. Use a canonical, versioned encoding that preserves type, ordered lists and field presence. Keep the case key, observed-facts digest, receipt occurrence, and assertion evaluation under a frozen intent revision distinct. Retain both observations when equivalent replays disagree. Incomplete scope limits the claims that can be reproduced. [Current comparison](../src/SlopHawk.Core/ProbeReplay.cs) · [Review identity](../rust/foreground/checks/src/review.rs)

Use `.slophawk/path-checks/<run>/` under authorized writable scratch, with any legacy checker receipts in that scratch workspace too. The daemon exporter owns `.slophawk` under the source checkout and rejects entries besides its report. Keep scratch outside that export directory and make the run location available through the foreground CLI. [Daemon report ownership](result-publication.md)

| Artifact | Contents |
|---|---|
| `graph.json` | Versioned graph, types, source spans, and compilation fingerprint. |
| `intent-requests.json` and `intent.json` | Unanswered questions, sourced rules, authority, and frozen revisions. |
| `plan.json` | Selected goals, fixture recipes, proposed cases, model assumptions, limits, and fingerprints. |
| `observations.jsonl` | Case IDs, requests, typed outputs, trace provenance, and replay receipts. |
| `review.json` | Bounded summary of exact failing assertions and replay IDs, search scope and stop reason, trace status, unresolved or unstable cases, and omitted counts with detail references. |
| `manifest.json` | Supervisor-issued seal binding run state and context, referenced file paths, byte lengths, digests, and expected and observed record counts. |

Stage artifacts in a unique run directory. Check cross-file references and planned versus received counts, write the seal last, and publish the sealed snapshot on the same filesystem. The seal declares any missing scope. A validated reader checks it against the artifacts and distinguishes the current context from historical runs. Record aborted attempts separately. Report publication, completed comparisons, and observed coverage have independent statuses. Versioned canonical ordering and saved observations make runs comparable. A new intent revision may evaluate an eligible observation under a new evaluation ID. Changed compiler, adapter or fixture identity requires fresh execution.

## Implementation order

1. Verify compiler context and bound body selection. Export the graph and exercise one real CLI and skill workflow through a declared numeric adapter, the existing exact comparator, a separate retest, and a scoped review artifact. A structural graph is useful for diagnosis if execution is blocked.
2. Add bounded scalar search and cross-check the model against compiled execution on small finite domains. Freeze a scalar ablation against current probes before expanding. Require a saved, independently verified incremental witness, with cost and false positives reported within predeclared limits.
3. If the scalar gate passes, qualify the typed receipt, fixture reset, and per-assertion intent evaluator through the same foreground workflow. Then run the stateful holdout. Test structured intent questions separately once the cases and observation path work.

## Correlated scalar receipt binding

The native `graph-correlated-v1` route binds the inputs used by its planner to
the inspections used by `paths check` and `paths review`. This is content
attribution for a bounded prediction. It does not establish an observed branch
or prove the scalar model complete. The original candidate, independent public
expectations, forward/reverse replay and separate discrepancy retests retain
their existing roles.

Runnable correlated plans and their reviews use the authenticated seal format
`slophawk-scalar-run-v2`. Build receipts, legacy `graph-on`/`graph-off` plans and
reviews, and blocked structural plans retain `slophawk-scalar-run-v1`. A v1
reader rejects v2 instead of silently ignoring the additional validation.
There is no change to the worker protocol, scalar fact wire schema 1, search
policy, default mode, target language, CLI arguments or execution grants.

The new `plan.json` field `PlannerEvidence` has the following contract.

| Field | Bound content |
| --- | --- |
| `Format` | `slophawk-scalar-planner-evidence-v1`, identifying the interpretation below. |
| `Mode` | Exactly `graph-correlated-v1`, also equal to the plan mode. |
| `PlannerRevision` | `correlated-equalities-v1/replay-v1`, identifying the policy and receipt validation contract. |
| `PlannerSha256` | Uppercase SHA-256 of the complete running native coordinator executable, including its linked planner. The pathname is not part of the identity. |
| `InspectionSha256` | Canonical digest of the complete typed `InspectResult`: compiler/input closure, original candidate digest, worker runtime version and the ordered target array. Each target retains operation/signature, binding/eligibility/reason, rule source, parameter types, exact fact string, graph, original expectations and ordinary keys. The sealed spec additionally binds each target's document. |
| `ScalarFactsSha256` | Canonical digest of the existing `candidates.json` `ScalarFacts` object, keyed by operation. The fact strings are not duplicated in the new record. |
| `GraphsSha256` | Canonical digest of the complete existing `graph.json`, including `BuildAttested`. |

Canonicalization recursively sorts JSON object keys by Rust string order and
uses compact `serde_json` scalar encoding. It preserves array order, integer
values and exact string contents. It is a local versioned encoding, **not JCS**.
The input is the typed worker reply after deserialization: unknown worker fields
are outside this schema, and absent optional fields become their typed `None`
values, serialized as null. Adding a consumed field or changing this encoding
requires a new evidence revision. Facts remain exact UTF-8 string content,
including whitespace, diagnostics and CRLF/LF differences; they are not parsed
and reserialized for hashing. Target order is part of the binding. A JSON map's
property ordering is not. No graph operation, block, child, parameter, seed or
expectation array is sorted.

The reader verifies the existing file closure and HMAC first, then requires the
embedded plan identity to equal its seal identity. Correlated artifacts require
one parseable fact stream and graph entry for every unique planned operation.
Their content hashes must match the binding. Before the first replay, the
coordinator requires the same planner executable and compares a fresh full
inspection with the saved binding. It compares the final inspection again
before publishing a review, and revalidates the original plan seal and digest.
Review currentness uses the same planner and inspection checks. Neither path
regenerates the search to reinterpret old predictions under today's planner.

| Input or drift | Disposition |
| --- | --- |
| Same authority, build, planner and ordered inspection | Normal original-build checking; a valid retained review can be current. |
| Changed facts, graph, target metadata or worker runtime | Before replay: error and no candidate execution. Detected after replay: error and no published review. On a later review: `Current: false`, a reason, and the retained `Result`. |
| Changed native executable or unsupported evidence revision | New checking is unavailable; replan. An otherwise valid retained review remains readable with `Current: false`. |
| Old v1 correlated plan without `PlannerEvidence` | New checking is unavailable. An intact v1 review remains readable as historical with its original `Result`; no binding is invented. |
| New v2 receipt missing its binding; malformed hash, inconsistent mode/identity, missing or malformed saved facts, or mismatched cross-file content | Invalid receipt: hard error, never a current or silently downgraded result. |
| Changed worker DLL, compiler bytes, source/reference closure, or missing/reordered live targets | Existing checks still reject these. Compiler version labels alone have never substituted for the recorded Roslyn binary hashes. |
| Legacy graph-on/off receipt | Existing search and compatibility behavior; no new correlated evidence is claimed. |
| Structural-only plan, including a requested correlated mode | Still blocked with zero runnable cases; no original-build or planner-evidence claim. |

The digest is deliberately conservative: rebuilding an unrelated part of the
native executable, using the standalone foreground executable instead of the
public CLI, or changing platform invalidates correlated currentness. Replan
with the chosen executable; do not copy a new digest into an old plan. A future
more portable planner identity needs its own qualified compatibility contract.

The planner executable is read in 64 KiB chunks, must be an ordinary nonempty
file of at most 256 MiB, and must retain its recorded length/modification time
during hashing. It must be discoverable through `current_exe`; otherwise the
correlated checker is unavailable. The trusted installation must remain
immutable throughout each operation. This is an artifact hash, not attestation
of loaded memory or a concurrent executable replacement defense. Existing
16 MiB worker/artifact limits still apply, including to canonical evidence.
The fixed-size binding can cause a plan near the existing limit to be rejected;
it does not raise the cap or publish a partial seal. Hashing reuses the existing
inspections and adds no worker execution, search pass or adapter invocation.

The existing worker DLL digest, Roslyn byte/input closure and host authority
remain mandatory. Inspection content binding catches a separately deployed
provider changing its facts while those identities remain the same. It accepts
an equivalent-output provider dependency as content-equivalent: it does not
identify every loaded provider/Core/runtime dependency, independently validate
the public-rule evaluator, or authenticate an owner who controls the seal key.
Full managed deployment provenance would require an attested loaded dependency
closure from the worker; this receipt makes no such claim. The owner must keep
that installation trusted. CLI/MCP `checker-plan`/`plan_checker` previews remain
unsealed `caller_supplied` predictions with their existing limits. Neither they
nor the new digests produce invocation-scoped measured branch coverage.
