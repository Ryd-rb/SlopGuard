# Preview-only paired release-review trial

Product names below use the current branding. The original trial protocol is recorded in commit `2dc5b3bd0f62c1c6daa743b13d291391acb04de0`.

## Hypothesis and protocol

The 0.2.0-preview.1 CLI increases the number of actionable non-syntax findings a clean-context Luna 6 High worker reports during release review. The comparator is the same model and effort using Git, source and ordinary .NET tools. The treatment adds the preview CLI's evaluated project graph, source inventory and scheduler status. The product's real runner is unavailable in this preview. Use the ordinary .NET runner for build and test evidence.

There are three independently paired release candidates in the fixed order Ferry, Tally and Rivet. Use one new worker per arm and per project. Randomize arm order separately for each pair and preserve it before dispatch. Each worker reviews an isolated clone of the same committed baseline with the same uncommitted candidate edits. Source and ordinary test evidence are identical across arms. In the assisted clone start the preview daemon at the clean baseline, wait for a ready inventory and then apply the candidate. The control receives the equivalent Git history and edits. The worker knows its tools. The coordinator has the rubric and scores the final answers.

Send the following base prompt with an absolute workspace path.

> Review the release candidate in `<workspace>` against its README and release verification requirements. It has a committed baseline and working-tree edits. Use the source, Git and normal .NET tooling within this workspace. You may build and test this workspace. Leave source, configuration and Git history intact. Do not inspect parent or sibling workspaces, other tasks, or the internet. Finish within six minutes. For each actionable problem, give the affected file and line, the failing contract or release check, and the commands or source evidence that establish it. Report verification blockers separately. Indicate which evidence led you to each finding.

Append to the control prompt.

> Use standard Git and .NET tools only for this review. Do not invoke supplemental analyzers, SlopHawk, or other agents.

Append to the assisted prompt.

> You may also use SlopHawk 0.2 preview, already watching this workspace. In PowerShell: `& '<preview exe>' projects --root '<workspace>'`, `& '<preview exe>' status --root '<workspace>'`, or `& '<preview exe>' results --root '<workspace>'`. Use normal .NET tools as needed. Do not use other supplemental analyzers or other agents.

## Ground truth and scoring

| ID | Requirement for credit | Preflight oracle |
| --- | --- | --- |
| F1 | Explain the Windows-only calendar regression. The WorkstationDefaults import overrides the release flag, so the Windows Estimates target compiles the legacy Saturday-open calendar. | Both targets build. The Saturday East-route smoke returns Monday on portable and Saturday on Windows. Ordinary MSBuild evaluation shows the Windows fallback and portable linked file. The preview returns the portable items and flags the Windows contexts as unsupported. |
| F2 | Explain the strict greater-than in DeparturePlanner and its effect at exactly 16:00. | The portable Friday 16:00 smoke returns Friday instead of Monday. Friday 15:59 still returns Friday. |
| T1 | Identify the reconciliation test project missing from the prescribed solution test gate and show that it matters for the changed settlement policy. | `dotnet test Tally.slnx` runs two passing import tests. Direct invocation of Tally.Reconcile.Tests fails both midpoint cases. That project reaches the changed policy through Reconcile and Ledger. |
| T2 | Identify banker's rounding in SettlementRounding and describe the monetary error for a midpoint. | `import -10.005 2` returns `posted:-10.00` instead of `posted:-10.01`. |
| T3 | Identify the exclusive lag bound in BankImport and its effect on seven-day rows. | `import 19.42 7` returns `held` instead of `posted:19.42`. A six-day row still posts. |
| R1 | Identify the last-green receipt as stale for CartonPolicy and specify the required packing-suite rerun. A content-hash mismatch or equivalent attributable baseline/candidate comparison earns credit. | The baseline suite passes all three tests and the saved source digest matches its policy. The candidate digest differs. The ready preview reports two pending source intents and an awaiting-adapter scheduler. |
| R2 | Identify the incorrect default capacity in CartonPolicy and explain its effect on default orders. | Candidate `default 12` emits two cartons, 10 and 2. The corresponding candidate test fails. Pilot cartons and labels still pass. |

T1 is a test-gate scope defect and T2 is the production rounding defect. Correcting either leaves the other actionable. R1 is a release-evidence defect and R2 is a product defect. Fixing the product still requires producing current evidence and rerunning tests alone leaves the product defect. A failure symptom earns one credit only when the worker identifies the specific cause or evidence gap required by the rubric. Verify unrelated claims before counting any separately. Count contradicted actionable claims as false positives, unsupported claims as unconfirmed and timeouts as incomplete. Retain assigned runs even when a worker chooses to skip a tool. The intentional negatives include the portable test configuration, legacy calendar for opted-out builds, the manual bank checklist, pilot capacity and the label refactor.

Report qualified findings by project and arm, treatment-only and control-only observations, overlaps, false positives and unconfirmed claims. Attribute a finding to the preview only when the worker's evidence connects an actual preview observation to that finding. A higher score plus a preview-attributable detection exclusive to its arm supports the hypothesis for this sample. If the score ties, record cases where the preview helped provide evidence and whether the same finding was confirmed independently by the control. Report uncertainty from three purposefully selected projects. The project pair is the randomized unit.

## Token measurement

After each worker finishes, match the worker's local session metadata to its exact agent path and parent thread. Read its final cumulative runtime token-use event. Record input including cached input, cached input, output including reasoning, reasoning output and total. Report per-worker counts and arm sums. Report uncached input as input minus cached input. These totals sum across model calls, so repeated prompt context contributes tokens. Cache hits depend on runtime scheduling and can vary within a matched pair. An unavailable runtime measurement stays missing. Exclude the coordinator and fixture qualification from worker totals.

## Reproduction

Build this directory's `Fixtures.csproj` with the .NET 10 SDK. Invoke `dotnet run --project Fixtures.csproj -- base <project> <absolute-new-root>` to materialize the named project in a new location. Rivet requires a real baseline `dotnet test Rivet.slnx`. When its three tests pass, invoke the same generator with `receipt Rivet <root>` to write the baseline receipt. Initialize Git, commit the baseline, and clone it twice with line-ending conversion disabled. An assisted run starts the preview daemon at the untouched baseline. Apply `candidate <project> <root>` to each clone after the assisted daemon is ready. Review matching candidate diffs before starting the workers. Use a separate non-worker checkout for the commands in the rubric. Build the unmodified preview CLI at the Git revision recorded in the results and use only that executable in the assisted arm.
