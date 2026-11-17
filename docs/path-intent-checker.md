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

The first implementation targets synchronous C# methods whose input and relevant state can be represented with bounded integers, Booleans, enums, nullable values, and simple record fields. It supports a single call and a two-call scenario where the project supplies the setup and observation adapter. A graph can describe larger methods. The search marks operations it cannot model and exposes the reason. The first release is advisory and runs on demand for a selected method or scenario.

## User and agent workflow

1. **Choose the behavior to review.** Resolve the method from the evaluated project and a declaration location or documentation ID. Record the exact target framework, configuration, symbol signature, and source and build identities. A pasted method supports a structural view when it can be bound in an explicit context. Ambiguous overloads and missing compilation context remain visible.
2. **Capture intent.** Read approved requirements, public contracts, existing acceptance assertions, and documented caller assumptions. For new code, capture the intent during the task before observing the implementation. For existing code, record the source behind every rule. Keep agent-inferred expectations available as hypotheses.
3. **Plan the search.** Export the graph and choose goals near changes, uncovered branches, writes to specified state, exception sites, and outcomes that overlap a stated rule. For a goal with unclear intent, publish a question the agent can answer without seeing the candidate observation.
4. **Freeze each answer before executing its cases.** The answer may supply a rule covering many inputs, specify a single expected case, point to an existing rule, or mark a policy question open. Use stable IDs and hashes. An answer revised after observing the candidate is a new intent revision with provenance.
5. **Execute and replay.** Run the candidate with a fresh fixture per case. Record the return value or exception, projected state, effects, and available trace. Repeat reported discrepancies on the same build, fixture and adapter. Route ordinary exact comparisons through existing SlopHawk evidence where their semantics match.
6. **Review.** Surface reproduced discrepancies first, with the input, setup, failing property, source location, and replay link. Include meaningful coverage gaps and unresolved intent. A bounded search may end with results and open items.

A filesystem exchange keeps the workflow usable from a CLI, an agent skill, or a future MCP tool. A possible CLI shape is `paths plan` to write the graph and questions and `paths check` to consume answers and execute the plan. They communicate through the same versioned artifact format. An agent can respond between tool calls within one turn.

## Intent requests

Ask about an outcome the requirement leaves open. A useful question names the public operation, admitted setup, proposed input, and state or effect to inspect. Include requirement references and alternate outcomes only when they help the agent identify the decision. Withhold the candidate’s output, suspected fault, and private benchmark witness from the question. Keep the graph available to the reviewer and record when the expectation author has already seen the implementation.

> A patron requests pickup of the last available copy. The pickup branch is closed and policy denies fallback. What should the call return? Which inventory, reservation, and event fields may change? Cite the policy that determines the answer, or mark the case unresolved.

Prefer a reusable answer such as “For an admitted pickup request that returns BranchClosed, retain the original inventory and reservation and emit no allocation event.” A typed rule can apply to dozens of generated inputs. Avoid one prompt per path. Ask about the smallest missing contract. If a decision needs project-owner intent, preserve the ambiguity for the agent to raise when it matters.

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
  "when": "before.pickupClosed && !before.fallbackAllowed",
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

Start with a small typed language for input and pre-state predicates, exact scalars, enum sets, equality to pre-state, bounded deltas, and counts of named effects. Support explicit alternative *whole* outcomes when the policy permits them, so the checker avoids combining the result from one valid alternative with the state from another. Define ordered effects and unordered sets separately. An absent snapshot field means missing observation. A JSON null can be a valid value. State that the contract leaves unconstrained stays outside comparison. A documented exception can be an expected outcome, with independent state and effect assertions.

Intent precedence comes from the project’s own authority. Source-linked requirements and acceptance rules can ground findings. A proposed inference or contradictory source stays unresolved pending review. Freeze the applicable domain and expectation before the first candidate execution that is used as evidence. Contradictory rules for overlapping cases, unresolved types, and unobserved fields surface as diagnostics. Passing example qualification tells the reviewer which samples agreed. It cannot validate all intent.

### Example of a state-only defect

Suppose pickup first reserves a copy, then checks the selected pickup point. A late rejection occurs only when the point is closed, fallback is denied, and this request reaches the last-copy staging branch. The graph identifies the conjunction. The adapter creates the state through normal fixture setup and verifies there is one free copy before calling the method. The expectation above predicts `BranchClosed`, one free copy, the original reservation status, and no allocation event.

A candidate that returns `BranchClosed` with zero free copies has violated the inventory assertion. A repeat from a fresh equivalent fixture can establish the discrepancy. A nearby case with fallback allowed can help localize the guard if its expected outcome is also documented. Seeing the correct return value or hitting the rejection branch alone would leave the inventory violation undetected.

## Graph and search

Use the [C# semantic provider](../src/SlopHawk.Providers.CSharp/RoslynDocumentFacts.cs) and evaluated build context where available. Add a graph provider around Roslyn’s bound method-body control flow. Export block and edge IDs, source spans, edge polarity, return and explicit throw sites, regions, and calls tied to resolved symbols. Preserve loop back edges and exception regions. Model implicit runtime failures such as checked overflow or division by zero separately at supported operation sites. A structural edge and a modeled exceptional transition have different provenance. [Roslyn ControlFlowGraph](https://learn.microsoft.com/en-us/dotnet/api/microsoft.codeanalysis.flowanalysis.controlflowgraph?view=roslyn-dotnet-5.3.0)

Use goal-directed bounded exploration.

1. Start from ordinary cases and the current probes. Prioritize changed branch edges and writes to contract fields. Reserve part of the search budget for unexplored goals outside the highest-ranked group.
2. For a goal, walk bounded predecessor paths through the graph and preserve branch polarity, short-circuit evaluation, and local assignments. Convert a supported path into predicates over admitted inputs and observable pre-state. Project the existing probes onto simple conditions when possible. Use a fixed-width constraint solver for supported combinations that need exact solving. The existing mathematical-integer contract evaluator remains an expectation evaluator. C# execution needs the actual operand types, signedness, checked context, and conversion semantics. [C# overflow context](https://learn.microsoft.com/en-us/dotnet/csharp/language-reference/statements/checked-and-unchecked) · [SMT-LIB fixed-size bit vectors](https://smt-lib.org/theories-FixedSizeBitVectors.shtml)
3. Ask for one concrete model and prioritize nearby control cases. For a conjunction, try a one-condition neighbor when it is admitted. This can help the agent identify which guard or interaction explains the failure. An input proposed by a solver is a candidate for replay.
4. Bound loop unrolling, path states, solver work, and executions. Initial pilot defaults are two loop back edges per path, 128 solver queries and 256 candidate executions per method, with an independent wall-time guard. Reserve part of the execution budget for confirmation and report any remaining unconfirmed observations. Choose targets and inputs in a stable order under a recorded model and seed. Wall-time exhaustion is a reported frontier.
5. Treat externally supplied state, dynamic dispatch, opaque helpers, asynchronous continuations, and unsupported arithmetic as explicit unknowns unless a supported model applies. Preserve already supported parts of the method. Start intraprocedurally. Consider adaptive trace-guided exploration after measuring the added value of static goals and replay. Pex is precedent for using execution traces and solved constraints to generate new inputs under a path bound. [Pex research](https://www.microsoft.com/en-us/research/publication/pex-white-box-test-generation-for-net/)

Keep these claims distinct in the graph and summary.

| Status | Meaning |
|---|---|
| Structural | Roslyn represented this edge in the selected compilation. |
| Candidate input | The typed model predicts a route through the edge under recorded assumptions. |
| Observed | An attributed trace hit the edge for a saved fixture and input. |
| Infeasible within model | A named path is unsatisfiable under the stated model. An edge gets this label only after all predecessor paths inside the declared bound have been modeled and proved unsatisfiable. |
| Unknown | A missing model, incomplete compilation, unsearched path, budget limit, or missing trace prevents the stronger claim. |

A successful replay of a return or state difference can establish a behavioral discrepancy without a branch trace. It leaves the precise branch attribution predicted. An optional trace adapter may use a verified test coverage observation or an instrumented build with a separate identity. When tracing uses an instrumented build, use the original candidate for the authoritative replay. Record which graph spans map to each observed edge.

## Execution and evidence

Use a project-declared test or batch adapter to construct reachable initial state, invoke the normal entry point, and observe a small state projection after the call. A setup recipe through a documented constructor, import, or prior operation can establish pre-state. Check the actual pre-state against the requested case. A hypothetical receiver state that the public entry point cannot reach establishes no counterexample for that entry point.

Each case has a setup, input or two-call sequence, build identity, and expectation revision. The adapter returns a tagged outcome, before and after state, named recorded effects, trace if available, and its own status. Keep expected application exceptions distinct from failed fixture setup, process failures, timeout, and cancellation. Execute each independent case with a reset fixture. Replay an identical sequence as a unit. The harness exposes the same state and effects to the assisted and comparison workers. A recorded test double represents a defined dependency scenario, and its behavior is part of the receipt.

Reuse the existing [comparison](../src/SlopHawk.Core/ProbeReplay.cs), [retest](../src/SlopHawk.Cli/CheckRetest.cs), and [review](../src/SlopHawk.Cli/CheckReview.cs) mechanics for exact projected values. For predicate, allowed-set, and effect comparisons, record explicit per-assertion results against the observed fields. Keep missing observations and unstable replay outside reproduced findings. Preserve both replay values when they differ. Attach source line, input, pre-state, expected predicate, actual value, and the smallest demonstrated difference to each reported finding.

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

1. Export a bound graph and scoped review artifact for one method. Add structured intent input and one stateful adapter fixture. Demonstrate return, state, and effect comparison through the normal entry point.
2. Add bounded integer and Boolean path goals and replay. Cross-check the model against exhaustive execution on small finite methods. Demonstrate target and compiled runtime semantics agree before adding more C# operations.
3. Add a two-call fixture scenario, exception observations, and explicit unknown frontiers. Add the skill workflow that asks for the missing rule and exposes the exact source and evidence needed for review.
4. Run the holdout pilot at a frozen implementation. Expand graph modeling or execution support only where the pilot and product acceptance tests justify it.
