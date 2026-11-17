# Benchmark design for the C# path and intent checker

This is a proposed test protocol for the [checker](../../docs/path-intent-checker.md). The main question is whether adding path exploration to current SlopHawk gives a fresh reviewer more reproducible, contract-grounded defects with the same information. The experiment also measures where extra prompts, confusing reports, or incorrect rules introduce false positives or extra cost.

## Proving the checker works

Test the mechanics through the public CLI and the declared adapter before running any agent pair. These small cases are development tests. Use different source and defect variants for the blinded holdout.

| Test | Required evidence |
|---|---|
| Branch polarity and short circuit | Match the Roslyn edges to execution for nested Boolean conditions and a short-circuited divide. Record a trace only where the adapter proves the edge. |
| Reachability on small finite methods | Exhaustively execute a small input and fixture domain in C#. Compare that ground truth with supported solver predictions and every infeasibility claim. Include overlapping guards and a contradictory path. |
| Numeric semantics | Exercise signedness, integer promotions, negative remainders, narrowing conversions, and overflow in checked and unchecked contexts. Compare to the compiled program, including exception type. |
| Public setup | Verify a reachable pre-state via the public fixture. A requested state the setup cannot establish is a setup failure. |
| Returns, state and effects | Use a correct return value with an incorrect state, a correct state with an extra effect, and an intentional exception with the expected preserved state. Test missing fields separately from explicit nulls. |
| Allowed outcomes | Verify two allowed outcomes each pass. A hybrid combining the return value from one outcome with the state from the other fails. |
| State isolation and sequence | Fresh equivalent fixtures reproduce. Two calls sharing state run in order. A poisoned fixture or incorrect reset yields an observable setup error or a changed replay. |
| Response attribution | Swap two same-length batch results, reuse a case ID for a different recipe, and fail during the first reviewed operation. Preserve the correct case, setup phase and invocation in each verdict. |
| Expectation provenance | Replay a wrong rule that passed its examples. Preserve the discrepancy and its source. A reference or authoritative policy must be able to disprove it. Exercise conflicting and unanswered intent. |
| Unknowns and budgets | Exercise an opaque call, a loop beyond the search limit, an unsupported value type, solver unknown, and adapter timeout. Each records an unresolved frontier and yields no definitive finding for that frontier. |
| Evidence identity | Change the build, fixture, target, or intent after planning. The previous plan cannot be represented as evidence for the changed inputs. Compare repeated normalized plans and observations on the same fixture. |
| Review integration | Keep changed observations out of the reproduced findings. Verify the review cannot combine evidence across build or adapter identities. Compare small persisted receipts through real review entry points. |

Use exhaustive and differential checks during development where the domain is small enough. Test implementations with different branch spelling and method names to catch dependence on surface syntax. Exercise structural analysis without an execution adapter and runtime comparison without a trace. The output should distinguish those capabilities.

### Concrete development defect

Use a pickup coordinator with a fast path that stages a last-copy allocation for reserved pickup. Both the normal path and the fast path recognize a closed pickup branch. When policy denies fallback, the fast path in the mutant returns the correct `BranchClosed` value without rolling back the staged decrement.

| Input or control | Expected observation |
|---|---|
| One free copy, reserved pickup, closed branch, fallback denied | Return `BranchClosed`, retain the free copy and pending hold, emit no allocation. |
| Two free copies, same request and branch policy | Return `BranchClosed`, retain both free copies and the pending hold, emit no allocation. |
| One free copy, reserved pickup, branch open, fallback denied | Return an assignment, consume one copy, mark the hold assigned, emit one allocation. |
| Same first case against the corrected version | Match the expected return, state, and effects. |

The mutant should exhibit a difference only in available stock on the first input. Require a saved replayable setup, exact state observation, and a sourced expectation before blaming the rollback. Use the branch identities to test that conjunctive search reaches the fast-path rejection. Keep this example and its near neighbors out of scored recognition tests. An independently authored holdout can use a different late rejection and state projection.

## Fresh holdout

Use a C# library consortium circulation system. It has several branches, pickup holds, patron eligibility, copy assignment, renewals, a charge ledger, and recorded notification events. It exposes normal methods that return decisions and update an in-memory fixture. All time and dependency outcomes come from declared fixture inputs. The public adapter can set up a reachable state, invoke the same production code in every arm, and return a structured observation. Every worker has the same access to observations and the same public policy. The author keeps a separately implemented reference and private witnesses.

Author three genuinely different variants for each of the 16 families below, for 48 qualified variants. Draw one per family and include eight correct controls. The table gives proposed mechanisms. The author chooses the exact guards, legal states, method names, and witness values privately after the checker team freezes its design assumptions. Families 1–12 are the primary comparison. The last four probe the limits of the initial model and are reported separately. These cases are hypotheses about reviewer difficulty.

| Family | Proposed three variations |
|---|---|
| 1. Late transit receipt reopens a retired hold | Reopen a withdrawn hold, extend an expired hold’s pickup deadline, or emit pickup-ready for a completed hold when a late transit receipt is accepted. Keep receipt logging correct while checking the retired hold and effects. |
| 2. Repeated scan repeats a side effect | A second scan with the same receipt creates another loan row, adds another fee, or emits another event while returning the cached success decision. |
| 3. Equal timestamps reorder holds | Two holds share the same readiness time. Break the tie by the wrong original receipt, by the normalized branch alias, or before filtering an ineligible hold. Inspect the stored queue. |
| 4. Valid renewal updates the wrong field | Return the new due date while changing a sibling loan, retaining the outdated stored due date, or leaving a conflicting renewal event. Use a valid case with both a current and a historical loan record. |
| 5. Waiver applies to the wrong revision | Use stale, future or adjacent-version authorizations on renewal, under a path combining current eligibility, renewal count, and grandfathered status. A near neighbor is eligible under the same policy. |
| 6. Optional pickup defaults fail | On different routes, a missing pickup preference, unavailable preferred slot, or absent override record should use a documented fallback. Seed an exception or incorrect default on only one admitted late branch per variant. |
| 7. Returning a copy loses a hold transition | Return to this branch while a transferable hold exists. Seed a missed transfer status, a stale reservation owner, or a wrong effect payload on the equal-priority or last-copy branch. |
| 8. Replacement quote mutates the ledger | A quote with a partial credit can incorrectly post a debit, reserve the credit, or advance the ledger version even though the result amount is right. |
| 9. Returning a different copy closes the wrong loan | Combine an item identifier collision, a copy-level match, and distinct patron or branch records. Variants select by catalog ID, stale copy alias, or the first matching owner. |
| 10. Notification allowed only after eligibility | Mutate the event when opt-out and a deadline coincide, dispatch to the old patron after an allowed transfer, or issue a second notification after a suppressed retry. Judge the recorded effects and metadata. |
| 11. Inventory transfer double-applies a boundary delta | On the last free copy, decrement the sending count twice, increment the destination on a rejected alternate route, or let a zero-item transfer consume the pending quota. Include valid adjacent cases. |
| 12. Staged adjustment uses the wrong snapshot | Make two allowed calls. In different variants use the pre-return count, the newly staged count before commit, or the canceled snapshot during final availability calculation. Score the resulting count and assignment. |
| 13. Exception cleanup drops state | Seed a failure in a declared dependency scenario and exercise catch, finally, or deferred cleanup variants. Record the rollback or allowed partial outcome without assuming complete static exception modeling. |
| 14. Bounded queue loop misses a candidate | Vary where a skip, stop, or duplicate row causes the first or second iteration to miss a later eligible hold. Record when the loop exceeds the supported bound. |
| 15. Distinct objects share a mutable record | Use reachable aliasing through a stored reference, a shallow copy, or shared collection membership. Observe the unexpected change in the untouched view. Score the behavior separately from static alias reasoning. |
| 16. External condition changes the valid state transition | Through a declared test double, vary a failed lookup, stale dependency revision, or deferred response. Each changes the permitted result. No private dependency outcomes are supplied as path witnesses to the worker. |

Correct controls cover a deliberate exception with preserved state, an allowed alternative result, a correct zero-delta transition, a correct rejected-call rollback, a legitimate repeated-call replay, a guard with a contradictory path, an opaque helper, and an explicit effect whose ordering is outside the contract. Publish these operations alongside the defect families without control labels. Add admitted near neighbors for each private witness so the scorer can distinguish a real violation from an input that merely entered a rare branch.

Preclassify the variants by mechanism before seeing detection results. Include rare guard conjunctions, branch-neutral differences within the same path, and cases mainly limited by state or fixture construction. A correct receipt can leave the wrong sibling loan updated on a path both modes reach. An oracle must distinguish the sibling states and the search must choose unequal values for that case to expose the error. Expose the same neutral legal setup recipes to both modes.

Before allocation, the author compiles clean and mutated code under the same target and flags. Each variant must pass the ordinary examples, have an admitted setup and at least one private witness, and differ from the reference in a field or outcome covered by its public policy. The reference must meet the same independently stated assertions on ordinary cases, witnesses and neighboring cases. Preserve failed qualifications and replace or remove those variants before freezing. Record the complete inventory and exclusions. Use neither checker’s detection scores to decide which variants qualify.

Freeze the author package, candidate builds, public contracts, executable expectations, fixture adapter, corrected reference, manifests, model, and tool versions before launching workers. Use a recorded seed to select one approved variant per family. Use the same draw within a pair. Repeat chosen draws as planned. Give workers a single candidate snapshot with neutral fixture names and no clean-to-mutant diff or change-priority hint. Keep the generator, seed, defect count, witnesses, reference, and scoring outside the worker workspaces. Preflight private and cross-arm access and hash the shared files.

## Experiment A · Does path exploration add value?

First compare deterministic graph-off and graph-on search. Hold the method targets, legal setup recipes, intent, observation comparator, summary format, and budget of distinct candidate executions fixed. The graph-off mode includes ordinary cases and generic argument and fixture-state or sequence probes. Record the earliest admitted witness, search work, and overlap for each mode. A shared witness tagged with a graph goal is still shared. A verified graph-on-only witness measures added discovery within the fixed budget.

Then run a matched pair of fresh Luna 6 Low sessions in separate read-only workspaces with writable scratch. Use one turn, uncapped agent tool calls, and identical public files, adapter, executable intent, examples, and short prompt. Randomize order and balance it across draws. Both receive current SlopHawk. The treatment adds the path explorer and its generic workflow. This pair measures the effect of the workflow on the reviewer. Publish no precomputed distinguishing inputs. Choose a common end-to-end deadline and record per-command and analyzer limits before launch. Record deadline truncation as an outcome. An ordinary-tools arm can separately measure the full workflow against no SlopHawk.

> Review the solution listed in workspace.json. Flag reproducible behavioral defects without editing the solution. Run any listed checker. Save out/answer.json with the operation, setup and inputs, actual outcome or state, expected outcome or state, the public rule supporting the expectation, and cause for each finding. Tool calls are uncapped.

Give both arms the same state projection, intent evaluator, and batch runner. Use the current checker for exact JSON comparisons and a shared fixture comparator for any additional predicates required by the test. A worker may derive new expectations, inspect the same code and policy, and run the adapter directly. Only the treatment receives graph-directed case generation and its review output. A comparison against the unmodified shipped workflow can also measure the combined improvement from the comparator and path explorer. Report that comparison as a workflow result. Count what each worker submits and retain tool receipts. Score the checker’s own witnesses separately to distinguish discovery from an agent’s decision to report.

Start with one draw, the deterministic comparison, and at most one reviewer pair. Estimate and reserve the total run budget before launching. A worker that ends early or returns a partial audit is an observed outcome. An infrastructure failure before a usable turn may be retried on the same draw with a fresh session and stays in the record. Freeze any revised protocol before new confirmation draws, retain the pilot, and distinguish workflow gain, no gain, and an inconclusive protocol failure.

For confirmation, plan four new draws with two matched pairs per draw. Report per-arm counts and paired gains and losses, grouped by draw and by predeclared family. Use draw-level results as the unit for claims across draws. Repeated sessions on the same draw measure reviewer variability. Select a draw for any additional per-variant repetitions before seeing outcomes. Use the same exact variant for a per-variant consistency claim. Family rates can aggregate the planned draws, with their denominators and variant composition. A descriptive target is baseline verification in at most half the matched repetitions and treatment verification in at least three quarters.

A proposed descriptive product gate is a mean of at least three additional verified primary defects per pair, a positive aggregate gain on at least three of four confirmation draws, and no increase in false positives. Four draws are a small sample, and several defects found in one session share context. Publish the raw pairs, draw-level variability, and submission error rates. Broader claims need more domains and an inference plan set before collecting those data.

## Experiment B · Do intent questions help?

Keep the path engine and generated goals fixed. Give both arms the same neutral setup card naming the admissible inputs and observable fields. This holds attention on the same behavior. Add structured intent solicitation in one arm. The expectation author receives the policy and card in a workspace without the candidate, observed output, or private reference. Freeze sourced answers before candidate replay. Treat a single agent’s self-review as a separate workflow measurement if the agent already saw the implementation.

Start with six to eight prewritten cards where policy is clear, legitimately ambiguous, permits multiple outcomes, or conflicts with a stale example. Grade against an independently authored rubric before paying for additional reviewer pairs. Measure grounded rules, correct unresolved decisions, wrong or overconstrained expectations, and verified defects when paired with replay. Count authoring tokens, waiting time, reviewer tokens, and replay cost. An effect that requires private ground truth to answer stays unresolved in the worker session.

## Scoring and cost

Remove arm and run hints from submissions before adjudication. Independently replay cases on the frozen candidate and corrected reference. Validate the realized setup, admitted input, cited rule and exact observation. Where the policy permits multiple whole outcomes, compare against that policy so two valid implementations can differ. Resolve reference and policy disagreements before crediting a finding. Count at most one verified seeded defect per family and draw, even if a worker finds multiple witnesses. Score demonstrated behavior separately from causal or branch attribution. A false positive asserts a violation where the candidate satisfies the independently grounded policy. Report incorrect values on real defects, unsupported inputs, setup errors, unresolved expectations, unstable replays, and genuine new defects with total submissions as a denominator. Require independent confirmation before adding a new defect to the record.

For the path engine, report supported goals, attributed branch hits, replayed cases, uncovered or unknown goals, and the reasons for stopping. Keep nominal graph edges separate from actual trace coverage. Track total and uncached input tokens, output tokens, tool calls, elapsed time, checker execution and solver work, and any extra intent authoring cost. Cached input can dominate total tokens, so separate it in the report. Judge utility from verified findings and cost as well as the false-positive rate. A clean corrected run and increased branch coverage are complementary measurements.

The pilot should show an independently verified return, state, or effect violation reached by a saved graph-goal case that the matched graph-off mode missed within budget before scaling. A failure to meet that gate remains a no-gain result when the protocol worked. Release negative findings and incomplete sessions alongside favorable outcomes.
