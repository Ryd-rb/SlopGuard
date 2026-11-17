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
| 1. Pickup rejection changes inventory | Lose a staged decrement after a closed pickup point, clear an existing assignment after fallback is denied, or emit an allocation event on a rejected attempt. Each combines a late rejection with previously staged work. |
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

Before allocation, the author compiles clean and mutated code under the same target and flags. Each variant must pass the ordinary examples, have an admitted setup and at least one private witness, and differ from the reference in a field or outcome covered by its public policy. The reference must meet the same independently stated assertions on ordinary cases, witnesses and neighboring cases. Preserve failed qualifications and replace or remove those variants before freezing. Record the complete inventory and exclusions. Use neither checker’s detection scores to decide which variants qualify.

Freeze the author package, candidate builds, public contracts, executable expectations, fixture adapter, corrected reference, manifests, model, and tool versions before launching workers. Use a recorded seed to select one of the three approved variants per family. Use the same draw in both arms and choose a new seed for each confirmation draw. Repeat selected draws as planned to measure session variability. Keep the seed, defect count, witnesses, and scoring outside the worker workspaces. Save hashes for the exact shared public files.

## Experiment A · Does path exploration add value?

Run a matched pair of fresh Luna 6 Low sessions in separate read-only workspaces with writable scratch. Use one turn, uncapped agent tool calls, and identical public files, fixture adapter, executable intent, ordinary examples, and short prompt. Randomize the order, balancing order across draws. Both receive current SlopHawk. The treatment adds the path explorer and its generic workflow. No precomputed case list or answers appear in either workspace. Apply the normal per-command runtime policy to each arm and record the path search limits. An ordinary-tools arm is optional if the aim also includes measuring the full stack against no SlopHawk.

> Review the solution listed in workspace.json. Flag reproducible behavioral defects without editing the solution. Run any listed checker. Save out/answer.json with the operation, setup and inputs, actual outcome or state, expected outcome or state, and cause for each finding. Tool calls are uncapped.

Give both arms the same state projection, intent evaluator, and batch runner. Use the current checker for exact JSON comparisons and a shared fixture comparator for any additional predicates required by the test. A worker may derive new expectations, inspect the same code and policy, and run the adapter directly. Only the treatment receives graph-directed case generation and its review output. A comparison against the unmodified shipped workflow can also measure the combined improvement from the comparator and path explorer. Report that comparison as a workflow result. Count what each worker submits and retain tool receipts. Score the checker’s own witnesses separately to distinguish discovery from an agent’s decision to report.

Start with one draw and one pair as a pilot. Treat sandbox, adapter, answer format, and scoring failures as protocol issues. A worker that ends early or returns a partial audit is an observed outcome. An infrastructure failure before a usable turn may be retried in a fresh session and must stay in the record. Freeze the final protocol on development fixtures before running new confirmation draws. Retain the pilot separately if its results lead to changes.

For confirmation, plan four new draws with two matched pairs per draw. Report per-arm and paired counts for each draw and family. The repeated sessions on each draw help distinguish difficulty from reviewer variability. Reserve one draw before seeing results for two further repetitions if a per-variant consistency claim is needed. The concrete target is to identify predeclared families that the baseline verifies in at most half of the matched repetitions and the treatment verifies in at least three quarters. Report the denominators. A single miss is evidence from one session.

A proposed product gate is a mean of at least three additional verified primary defects per pair, a positive aggregate gain on at least three of the four confirmation draws, and no increase in verified false positives. Treat broader claims as conditional on results across more domains. Publish raw paired deltas and variability across draws. Several defects found in one session share a context and do not create independent statistical trials.

## Experiment B · Do intent questions help?

Keep the path engine and generated goals fixed. Compare review with the existing written policy alone to review with the proposed intent-request workflow. Both arms see the same prose and candidate. When the checker asks a question, the expectation author receives the policy, fixture setup, and requested observables. Withhold the implementation and actual output from that author where the roles can be separated. Freeze each answer before replay. In a single agent test, record any prior implementation exposure. An inferred answer remains a hypothesis until grounded independently.

Include a small set of prompts where policy is clear, legitimately ambiguous, permits multiple outcomes, or conflicts with a stale example. Judge the answer against an independently authored rubric before comparing detection. Measure the number of grounded rules, correct unresolved decisions, wrong or overconstrained expectations, and confirmed defects. Count authoring tokens, waiting time, reviewer tokens, and replay cost. Treat an effect that required private ground truth to answer as unresolved in the worker session. This experiment isolates the value of asking for intent from the value of reaching difficult inputs.

## Scoring and cost

Independently replay all submitted cases on the frozen candidate and corrected reference. Check the actual setup and input against the public domain. Count at most one verified seeded defect per family on a draw. Require an exact observable discrepancy, even if the worker found a different witness from the author. A false positive asserts a violation on admitted input where the candidate satisfies the independently verified policy. Report incorrect values on real defects, unsupported inputs, setup errors, unresolved expectations, unstable replays, and genuine new defects separately. Require independent confirmation before adding a new defect to the benchmark record.

For the path engine, report supported goals, attributed branch hits, replayed cases, uncovered or unknown goals, and the reasons for stopping. Keep nominal graph edges separate from actual trace coverage. Track total and uncached input tokens, output tokens, tool calls, elapsed time, checker execution and solver work, and any extra intent authoring cost. Cached input can dominate total tokens, so separate it in the report. Judge utility from verified findings and cost as well as the false-positive rate. A clean corrected run and increased branch coverage are complementary measurements.

The pilot should show at least one independently verified return, state, or effect violation reached by a saved path-generated case before scaling. The evidence must still support a no-gain result. Release negative findings and incomplete sessions alongside favorable outcomes.
