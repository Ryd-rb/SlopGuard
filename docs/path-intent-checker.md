# C# path exploration with explicit intent

**Proposed design.** The [holdout and acceptance plan](../experiments/slophawk-path-intent/benchmark-design.md) defines how to decide whether to build beyond the pilot.

## Scope and first decision

SlopHawk already searches inputs against supplied integer contracts, records observations, and verifies that representative discrepancies reproduce. A path checker should add inputs aimed at code branches and state transitions that ordinary probes rarely reach. Its primary output is a short review file with concrete witnesses and explicit gaps. A full control-flow graph is useful as supporting evidence. [Contract search and evidence workflow](../README.md)

A review result must answer three questions independently.

| Question | Evidence |
|---|---|
| What can the method syntactically do? | A Roslyn graph tied to the evaluated project, target and source revision. |
| Which behavior happened? | An admitted fixture and input, the observed exit and state, and any branch trace the adapter actually recorded. |
| Why is it a defect? | A reproducible difference from a grounded expectation, with the requirement and the exact property that failed. |

The first implementation targets synchronous C# methods in one controlled .NET 10 project and adapter. Use primitive arguments and a finite catalog of fixture recipes. Model acyclic paths with supported Boolean and integral operations first. A two-call scenario can be replayed through the adapter while only one invocation is initially modeled. The graph can describe larger methods. The search marks operations it cannot model and exposes the reason. The first release is advisory and runs on demand.

## User and agent workflow

1. **Choose the behavior to review.** Resolve the method from the project and a declaration location or documentation ID. For a graph tied to a real build, use the actual compiler context, including target, compilation and parse options, references, and generated sources. The existing evaluator supplies project membership and references. The semantic provider accepts a compilation that a caller supplies. Start with a controlled project or an independently verified compiler manifest to bridge that gap. A pasted method can produce a structural view in an explicit context. Ambiguous overloads or missing compiler inputs leave scope and attribution open.
2. **Capture intent.** Read approved requirements, public contracts, existing acceptance assertions, and documented caller assumptions. For new code, capture the intent during the task before observing the implementation. For existing code, record the source behind every rule. Keep agent-inferred expectations available as hypotheses.
3. **Plan the search.** Export the graph and choose goals near changes, uncovered branches, writes to specified state, exception sites, and outcomes that overlap a stated rule. For a goal with unclear intent, publish a question the agent can answer without seeing the candidate observation.
4. **Freeze each answer before executing its cases.** The answer may supply a rule covering many inputs, specify a single expected case, point to an existing rule, or mark a policy question open. Use stable IDs and hashes. An answer revised after observing the candidate is a new intent revision with provenance.
5. **Execute and replay.** Run the candidate with a fresh fixture per case. Record the return value or exception, projected state, effects, and available trace. Repeat reported discrepancies on the same build, fixture and adapter. Route ordinary exact comparisons through existing SlopHawk evidence where their semantics match.
6. **Review.** Surface reproduced discrepancies first, with the input, setup, failing property, source location, and replay link. Include meaningful coverage gaps and unresolved intent. A bounded search may end with results and open items.

A filesystem exchange keeps the workflow usable from a CLI, an agent skill, or a future MCP tool. A possible CLI shape is `paths plan` to write the graph and questions and `paths check` to consume answers and execute the plan. They communicate through the same versioned artifact format. An agent can respond between tool calls within one turn.

## Intent requests

Ask about an outcome the requirement leaves open. A useful question names the public operation, admitted setup, proposed input, and state or effect to inspect. Include requirement references and alternate outcomes only when they help the agent identify the decision. Withhold the candidate’s output, suspected fault, and private benchmark witness from the question. Keep the graph available to the reviewer and record when the expectation author has already seen the implementation.

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

Start with a small typed language for input and pre-state predicates, exact scalars, enum sets, equality to pre-state, bounded deltas, and counts of named effects. Support alternative *whole* outcomes when policy permits them. An alternative passes when all its assertions pass, fails when a required assertion is contradicted, and stays unknown when an unevaluated assertion could change the answer. The whole rule passes if an alternative passes, fails if every alternative fails, and stays unknown otherwise. Define ordered effects and unordered sets separately. Tag a valid null inside a typed observation. An absent field means missing evidence. An empty recorded event list proves zero effects only for a complete recorder at the declared boundary. A documented exception can be an expected outcome with separate state and effect assertions.

Intent precedence comes from the project’s own authority. Keep each assertion linked to its source and verify that its translation preserves the source’s meaning and priority before giving it authority. A proposed inference or contradictory source stays unresolved pending review. Freeze the applicable domain and expectation before the first candidate execution used as evidence. Contradictory rules for overlapping cases, unresolved types, and unobserved fields surface as diagnostics. Passing example qualification establishes agreement on those samples.

### Example of a state-only defect

Suppose pickup first reserves a copy, then checks the selected pickup point. A late rejection occurs only when the point is closed, fallback is denied, and this request reaches the last-copy staging branch. The graph identifies the conjunction. The adapter creates the state through normal fixture setup and verifies there is one free copy before calling the method. The expectation above predicts `BranchClosed`, one free copy, the original reservation status, and no allocation event.

A candidate that returns `BranchClosed` with zero free copies has violated the inventory assertion. A repeat from a fresh equivalent fixture can establish the discrepancy. A nearby case with fallback allowed can help localize the guard if its expected outcome is also documented. Seeing the correct return value or hitting the rejection branch alone would leave the inventory violation undetected.

## Graph and search

Build the graph from Roslyn’s bound method-body control flow. Export block and edge IDs, source spans, edge polarity, return and throw sites, calls tied to symbols, and loop or exception regions. A return edge through a finally block can lead to a thrown exception. Exceptional control and cleanup remain structural or unknown until their transitions are modeled and checked against C# execution. Record actual adapter exceptions separately. [Roslyn ControlFlowGraph](https://learn.microsoft.com/en-us/dotnet/api/microsoft.codeanalysis.flowanalysis.controlflowgraph?view=roslyn-dotnet-5.3.0) · [C# semantic provider](../src/SlopHawk.Providers.CSharp/RoslynDocumentFacts.cs)

Use goal-directed bounded exploration.

1. Start from ordinary cases and the current probes. Prioritize changed branch edges and writes to contract fields. Reserve part of the search budget for unexplored goals outside the highest-ranked group.
2. For a goal, walk supported predecessor paths and preserve short-circuit evaluation, polarity, and local assignments. Bind guard-relevant pre-state to actual fixture recipes and solve over admissible arguments. Use a fixed-width solver for supported combinations that need exact solving. Log the modeled types, signedness, conversions, and checked context. Encode exceptional conditions explicitly when modeling such an operation. A mathematical or bit-vector quotient alone misses C# division failures such as the signed minimum divided by minus one. Unsupported transitions and unmodeled reads invalidate a path proof. [C# numeric semantics](https://learn.microsoft.com/en-us/dotnet/csharp/language-reference/statements/checked-and-unchecked) · [C# division](https://learn.microsoft.com/en-us/dotnet/csharp/language-reference/language-specification/expressions#12133-division-operator) · [SMT-LIB fixed-size bit vectors](https://smt-lib.org/theories-FixedSizeBitVectors.shtml)
3. Seek distinct outcomes within a reached path. Where both code and intended behavior are modeled, solve the path together with a violated postcondition as a candidate test. Else try boundary and alternate models, and admitted near neighbors. Some neighboring input controls change several correlated conditions. Record that dependency. Link each proposed case to its goal, recipe and generation method, including cases reused from the shared probe pool.
4. Bound path states, solver work, and executions. Proposed pilot starting limits are 128 solver queries and 256 candidate executions per method, including a confirmation reserve, with an independent wall-time guard. Start with acyclic paths and log unmodeled loop frontiers. Choose targets and inputs in a stable order under a recorded model and seed. Report incomplete search explicitly.
5. Treat externally supplied state, dynamic dispatch, opaque helpers, asynchronous continuations, and unsupported arithmetic as explicit unknowns unless a supported model applies. Preserve already supported parts of the method. Start intraprocedurally. Consider adaptive trace-guided exploration after measuring the added value of static goals and replay. Pex is precedent for using execution traces and solved constraints to generate new inputs under a path bound. [Pex research](https://www.microsoft.com/en-us/research/publication/pex-white-box-test-generation-for-net/)

Keep these claims distinct in the graph and summary.

| Status | Meaning |
|---|---|
| Structural | Roslyn represented this edge in the selected compilation. |
| Candidate input | The typed model predicts a route through the edge under recorded assumptions. |
| Observed | An attributed trace hit the edge for a saved fixture and input. |
| Infeasible within model | A named path is unsatisfiable under the stated model. An edge can receive this label when the transition model conservatively covers all paths inside the stated bound and all were proved unsatisfiable. Incomplete predecessors leave the edge unknown. Defer edge-level proof in the pilot. |
| Unknown | A missing model, incomplete compilation, unsearched path, budget limit, or missing trace prevents the stronger claim. |

A successful replay of a return or state difference can establish a behavioral discrepancy without a branch trace. It leaves the precise branch attribution predicted. An optional trace adapter may use a verified test coverage observation or an instrumented build with a separate identity. When tracing uses an instrumented build, use the original candidate for the authoritative replay. Record which graph spans map to each observed edge.

## Execution and evidence

Use a project-declared test or batch adapter and a finite catalog of recipe IDs. Each recipe uses a public setup route with declared dependency inputs and verifiable guard-relevant values. Solving freely over projected fields can yield impossible combinations. Bind only the supported input arguments and recipe choices in the pilot. Check the realized pre-state and admission before invoking the normal entry point. An inability to materialize the case remains a setup result.

Each receipt echoes the case ID, phase, invocation index, and bound entry point. Match each response to its request explicitly, including in batches. Separate scaffolding failure from failure in the first operation under test. Snapshot a detached pre-state before invocation, and tag outcome, after-state, effects and observation completeness for each call. Keep application exceptions distinct from adapter setup, snapshot, transport, timeout and cancellation failures. Reset the fixture for each independent case and replay a complete two-call sequence as a unit. Keep test doubles and side-effect boundaries explicit.

Reuse narrow primitives from the existing comparison, publication, and review workflow. Use typed path receipts for scenario identity. The legacy review groups by assembly and adapter verbs and identifies an input by operation and integer arguments. A path receipt also needs the compiled context and dependency identities, recipe and fixture version, ordered calls, target invocation, and observation schema. Keep the observation key separate from the intent revision used to evaluate it. Record per-assertion results. Preserve both values on unstable replay and keep incomplete observations outside the corresponding reproduced claim. [Current comparison](../src/SlopHawk.Core/ProbeReplay.cs) · [Current retest and review](../src/SlopHawk.Cli/CheckRetest.cs) · [Review identity](../src/SlopHawk.Cli/CheckReview.cs)

Use `.slophawk/path-checks/<run>/` in the authorized writable workspace. Keep the graph and raw receipts separate from the short agent summary.

| Artifact | Contents |
|---|---|
| `graph.json` | Versioned graph, types, source spans, and compilation fingerprint. |
| `intent-requests.json` and `intent.json` | Unanswered questions, sourced rules, authority, and frozen revisions. |
| `plan.json` | Selected goals, fixture recipes, proposed cases, model assumptions, limits, and fingerprints. |
| `observations.jsonl` | Case IDs, requests, typed outputs, trace provenance, and replay receipts. |
| `review.json` | Reproduced differences, unresolved intent, unsupported and unstable cases, actual coverage, and remaining goals. |

Publish completed artifacts atomically. Record the evaluated project, source and assembly hashes, relevant dependency or fixture identities, checker and solver versions, selected target, input and intent hashes, and search limits. Run IDs and timing are operational metadata. Canonical case ordering and saved observations make two equivalent runs comparable. A report can be resumed after additional intent answers if its graph and inputs still match. A new build or expectation revision gets a distinct evidence record.

## Implementation order

1. Export a bound graph and scoped review artifact for one controlled project. Add structured intent, a finite recipe catalog, and typed receipts for return, state, and effect comparison through one normal entry point.
2. Add bounded integer and Boolean path goals and replay. Cross-check the model against exhaustive execution on small finite methods. Demonstrate target and compiled runtime semantics agree before adding more C# operations.
3. Add a two-call fixture scenario, exception observations, and explicit unknown frontiers. Add the skill workflow that asks for the missing rule and exposes the exact source and evidence needed for review.
4. Run the holdout pilot at a frozen implementation. Expand graph modeling or execution support only where the pilot and product acceptance tests justify it.
