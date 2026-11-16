# Single-worker SlopHawk checker iteration

One `gpt-6-luna` chat at low reasoning effort audited the original Continuity candidate with 132 seeded defects and 26 controls. The first turn used the [same short flag-only prompt as the blinded pairs](foreground-check-results.md). Later turns resumed the same chat and introduced the updated checker or deeper public scope. Calls were uncapped. This was an iterative development run. The outcomes include the worker's earlier analysis and the continuation instructions.

The Windows MXC workspace was outside SlopHawk, with writes limited to scratch and `.slophawk`. It contained the unchanged candidate source and runnable assembly, public contracts and tests, and the current checker and skill. Private witnesses, the repaired build, and scoring results stayed outside the worker's readable paths. The sandbox preflight rejected a sibling-workspace read, a repository-source read, and a candidate-source write. The candidate fingerprint was checked after each scored snapshot.

## Iterations

After each pass, the coordinator replayed the submitted examples against the sealed candidate and corrected build. A verified flag required a supported input, exact outputs and a correct cause. Finding counts are cumulative and count distinct roots. Per-pass token use is the change in the native session's cumulative counters. Total tokens include cached input and output. Uncached input is listed separately.

| Pass | Checker change or scope | Verified of 132 | False positives | Total tokens | Uncached input | Shell calls |
|---:|---|---:|---:|---:|---:|---:|
| 1 | Durable reports and exact retest acknowledgments | 1 | 0 | 1,114,553 | 53,290 | 25 |
| 2 | Observation-only probes and repackaged ordinary examples | 32 | 0 | 5,968,635 | 78,871 | 65 |
| 3 | Required a comparison beyond the ordinary examples on all 158 operations | 92 | 1 | 13,218,971 | 116,268 | 80 |
| 4 | Required a second distinct comparison on operations without a submitted flag | 109 | 1 | 8,101,350 | 42,936 | 35 |
| 5 | Required a third distinct comparison on operations without a submitted flag | 122 | 1 | 2,740,844 | 56,619 | 41 |
| 6 | Required a fourth distinct comparison and added guidance for independent contract calculations | 123 | 1 | 1,466,061 | 28,106 | 21 |
| 7 | Executable public-contract models and fresh input grids | 124 | 1 | 2,150,289 | 31,536 | 22 |
| 8 | Audited reference clauses and crossed input conditions | 125 | 1 | 1,981,736 | 33,657 | 15 |
| 9 | Independently audited every submitted flag | 126 | 0 | 5,598,049 | 115,224 | 30 |
| 10 | Tested cases that distinguish competing public-contract interpretations | 127 | 0 | 5,052,483 | 116,034 | 59 |
| 11 | Tested contract relationships with observation-only pairs and exact retests | 128 | 0 | 2,430,100 | 118,473 | 19 |

The first turn compared batches of public inputs against a placeholder expected value of zero. It submitted one supported defect and left the review open. The next turn used `check-observe` for exploration and a file of 158 ordinary inputs with their published expectations. The coordinator constructed that file from the two public fixtures and verified that each normalized input produced the exact published journal before binding its expected value. The checker then passed all 158 ordinary cases. Those cases let the worker rerun and acknowledge the erroneous ordinary comparisons without receiving a novel test or answer.

The unscoped review closed once the worker had separately tested and acknowledged all checker mismatches. The coverage report still showed many operations reached through ordinary examples or observation alone. The scoped review in pass 3 required at least one supported nonordinary comparison on each routed operation. Later depth passes selected their operations by subtracting the worker's own submitted operation names from the public catalog. Added comparisons had to use different arguments. Neither the selection nor the expected values used the private grader.

In pass 7 the worker wrote its own reference calculations in scratch, matched them against the 34 scoped operations' published ordinary tests, and generated 148 more distinct nonordinary comparisons. Three reported witnesses reproduced one newly verified root. Pass 8 found a mistaken order of operations shared by the worker's reference and the candidate. The worker revised the calculation from the public clause and retested two witnesses for the newly verified defect.

One submitted claim survived the worker's exact retest using the same incorrect expectation. Independent replay showed the original output matched the public contract. In pass 9 the worker independently re-derived the expected values for all 129 submitted examples, found the incorrect expectation, and replaced the case with a valid witness for the operation. The coordinator provided no case-specific grading feedback. At that checkpoint the submission had 126 distinct verified roots, no false positives, and a complete checker review with zero outstanding findings. The session had used 42,340,488 total tokens, of which 556,507 were uncached input tokens.

## References

The earlier [coverage-guided unassisted runs](wide-low-results.md#per-run-usage) verified 88 and 50 of 132. The most recent [short-prompt blinded pairs](foreground-check-results.md#results) verified five distinct defects in the ordinary arm and zero assisted. The protocols and work invested differ across these runs.

The checker revisions were `df83e79` (durable reports), `0d78103` (observation mode), `ff22a92` (scope), and `aacfd83` (distinct-input depth). The skill added contract-derived batch guidance in `0c92890` and paired probes in `c74c54f`. The dedicated session was `01a1e637-3068-7a32-82e7-efec310732a9`.
