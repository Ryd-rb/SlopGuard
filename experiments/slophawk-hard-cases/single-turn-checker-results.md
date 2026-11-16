# Fresh Luna 6 Low checker trials

Each trial gives a new `gpt-6-luna` session at low reasoning effort the [same short flag-only prompt](foreground-check-results.md). Its first final response ends the run. Tool calls are uncapped. The target is 132 distinct verified defects on the frozen Continuity candidate.

The Windows MXC profile gives each session its own directory outside SlopHawk, a read-only candidate, and writes to `.slophawk` and `out`. The worker gets the public contracts, source, fixtures, the checker and its skill. The manifest lists all public operations and repackages only the published ordinary expectations. The private input witnesses and corrected implementation remain accessible only to the scoring process. The scorer requires exact actual and expected outputs on supported inputs plus the correct cause.

| Trial | SlopHawk revision | Verified of 132 | Rejected examples | False positives | Uncovered operations | Open checker discrepancies | Total tokens | Uncached input | Wall time |
|---:|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| 1 | `e2bf07f` | 2 | 0 | 0 | 149 | 15 | 1,850,823 | 69,169 | 241.2 s |
| 2 | `4fdab21` | 1 | 2 | 0 | 153 | 6 | 709,122 | 52,815 | 95.3 s |
| 3 | `9bc0023` | 0 (no submission) | — | — | 158 | 0 | 114,064 | 14,493 | 24.7 s |
| 4 | `a380956` | 1 | 4 | 1 | 146 | 3 | 3,145,374 | 70,958 | 336.1 s |

Trial 1 passed the ordinary suite and ran genuine nonordinary comparisons. It saved two reproducible flags and ended with the scoped review open. The session logs showed errors reconstructing original inputs and finding indices for the separate retests.

The next checker revision adds `check-retest`. It replays the exact requests recorded in the original report, uses the same adapter and assembly, and updates the acknowledgment file after a separate observed result. A sandbox preflight confirmed that it can reproduce a discrepancy, retire an incorrect expectation, and reject a different assembly.

Trial 2 ran separate checks by hand and left the batch-retest command unused. Private replay rejected two submitted expected values. One example exposed a genuine defect with a different expected value. The other produced the same value in both builds. Both cause descriptions localized seeded problems elsewhere, so their rejected examples do not count as additional false-positive roots.

Trial 3 added a grouped worklist to the scoped report. Paths were mapped deterministically from the published operation names, contracts, and source filenames for all 158 operations. The worker received a 15-component worklist after the ordinary examples passed. It treated the pending scoped review and read-only candidate as blockers and returned before testing a nonordinary case.

The next revision adds `check-review --progress` for successfully retrieving pending work and keeps the strict completion check for the end of the audit. The workspace ships an empty acknowledgment array for the first progress query. The skill describes how to complete the workflow in the existing writable paths.

Trial 4 generated a correctly scoped progress report, ran additional nonordinary cases, and used the batch-retest command. Independent replay verified one of its five submitted examples. The worker localized three seeded faults but gave incorrect expected values or cases that missed the trigger. It also reported a gap-crossing defect for an input on which both builds correctly stopped before the gap. The worker ended with 146 operations still uncovered and three outstanding checker discrepancies. A repeated check reproduced the values supplied by the worker but could not establish that its expectations followed from the public contract.

The highest single-turn result was 2 of 132. The 132 target remains unmet. These revisions improved execution, retest bookkeeping, and coverage reporting. The checker still needs independently established expectations for the nonordinary cases. The public contracts in this benchmark describe those expectations in prose.

The [earlier 128-defect result](checker-review-iteration-results.md) came from repeated turns in one continuing session. These fresh trials each measure one turn.
