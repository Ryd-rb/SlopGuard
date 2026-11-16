# SlopHawk paired replay benchmark

We reused the exact [132-defect journal release](wide-low-results.md#final-round) with 26 unchanged operations and its packaged predecessor. The main build was `7f72c4aa787f82b3fb42b5b786083967eaa74ac3`. Every worker started a fresh `gpt-6-luna` turn at `low` effort. Both roles had ordinary PowerShell, .NET, identical dossier contents and uncapped tool calls. Only the main-access role received the pinned CLI. Workers did not receive seed counts, the private key, peer results or an assignment label. The initial balanced draw placed the ordinary assignment first in pair 1 and main access first in pair 2. The dossier fingerprint was `96FF213C9D01A7B75D9F382F8D8702D932AF09297FE20076309727D9F026145D` across 204 entries.

## Current main

An exact confirmed finding requires an input with distinct supported numeric results from both executables, the correct actual and expected labels, and a correct cause. Numeric replay also counts reproducible differences in raw worker probe logs. Those values show compatible-batch evidence separately from the worker's source diagnosis. A false positive is an additional claimed defect contradicted by replay. Missing output, inadequate source coverage, and non-use of the offered tool are recorded alongside each run. Token totals include cached input and output. Uncached input is provided separately.

| Pair | Run | Assignment | Main used | Bodies visible | Verified causes | Raw differences by operation | False positives | Total tokens | Uncached input | Local calls | Seconds |
|---:|---:|---|---|---:|---:|---:|---:|---:|---:|---:|---:|
| 1 | 271 | Ordinary | No | 158 | 0 | 46 | 0 | 773,265 | 63,562 | 13 | 86.048 |
| 1 | 272 | Main access | No | 158 | 0 | 84 | 0 | 681,038 | 57,035 | 12 | 70.040 |
| 2 | 273 | Main access | No | 158 | 2 | 19 | 0 | 934,963 | 56,767 | 21 | 107.894 |
| 2 | 274 | Ordinary | No | 93 | 0 | 26 | 0 | 403,346 | 37,735 | 10 | 49.352 |
| 1 | 275 | Main access, repeat | Yes | 8 | 1 | 32 | 0 | 490,821 | 29,605 | 13 | 94.254 |
| 2 | 276 | Ordinary, repeat | No | 19 | 0 | 0 | 0 | 907,919 | 67,445 | 21 | 97.128 |
| 2 | 277 | Main access, repeat | Yes | 158 | 0 | 91 | 0 | 1,255,835 | 55,102 | 26 | 157.569 |
| 1 | 278 | Main access, repeat | Yes | 89 | 0 | 0 | 0 | 521,929 | 49,970 | 14 | 67.320 |

In the two initial assignments, main access changed verified diagnoses by 0 and +2. Neither worker who received access used the CLI, so these differences cannot be attributed to it. Actual-use attempts yielded one, zero and zero verified diagnoses. The only actual-use worker with full source coverage reproduced many numeric differences but submitted no diagnosis. The behavior results are descriptive because neither randomized pair completed the requested review with both source coverage and actual assigned tool use. Repeats used the same frozen code. Their briefs made CLI use explicit in the assisted role after both initial assisted workers skipped it. Repeats were triggered by coverage or exposure, not the findings score.

Run 271 submitted unexplained differences and sent an unsolicited status message. It received no reply. Its separate journal log replayed 206 numeric differences across 46 distinct operations. Run 272 loaded the source bodies into context but explicitly marked its own review unfinished. Run 273 reproduced `ShiftEnd` and `ReportingPeriodIndex` without calling SlopHawk. Run 274 found a genuine `SharedSuccess` difference but reversed the candidate and predecessor output labels, so it received no exact credit. Run 276 claimed full coverage, while only 19 bodies appeared in its tool returns. The actual-use runs created a new `.slophawk` directory. The candidate and predecessor bytes matched the frozen dossier in all runs.

Main's evidence for this dossier consisted of inventory and project evaluation. It did not run either product version against behavioral inputs. The replay feature added afterward accepts ordinary input and result arrays. The IL comparison reads the assemblies to prioritize method changes and derives useful integer literals. Neither check contains corpus names, source paths, private witnesses, special rules for particular operations, or an inferred definition of which inputs a product supports.

## Enhanced SlopHawk

A fresh Luna 6 Low worker used the enhanced CLI at `1634f3ef7543bac675062f9a723cb95128e788a4` on another byte-identical copy. Its scratch had a normalized copy of the 158 admitted public examples, verified against the standard profile on both release binaries. This run had a fixed replay recipe based on the earlier development exercises and had no access to the answer key. It is a detection demonstration with a different workflow from the original paired audit.

| Stage | Recipe | Supported probes compared | Unsupported probes | Numeric differences | New defects detected |
|---|---|---:|---:|---:|---:|
| 1 | Up to 512 probes per public operation | 79,132 | 1,243 | 8,840 | 125 |
| 2 | Up to 8,192 new probes per unresolved operation with changed method evidence | 55,512 | 1,832 | 78 | 6 |
| 3 | Up to 1,024 probes with scales from the remaining method and its generated helper | 1,024 | 0 | 4 | 1 |

Independent replay confirmed the worker's **132 of 132** distinct numeric counterexamples, with **zero false positives** across the 26 unchanged operations. Each confirmed operation had a matching changed method in the IL report. The reviewer did not attach method references or submit individual causal explanations. The result establishes reproducible behavioral detection for every seed on the original dossier under the published compatibility contract. It does not convert the outcome into 132 causal diagnoses. The worker's unresolved list still named the last case left after stage 2. Its final replay records include that operation and its confirmed stage 3 witness.

This worker used **345,885 total tokens**, including 318,720 cached input, 22,236 uncached input, and 4,929 output. It made 12 local calls and finished in 121.091 seconds. The dossier fingerprint matched the original after completion. The worker ID was `01a1e4ce-c1bf-76e0-9278-f1149df1a31f`.

## Independent holdout

A separate Sol author froze Harborline, a release with 15 operations, 32 passing ordinary examples, 11 seeded faults, and four unchanged controls. Its public evidence manifest had SHA-256 `089B594E83E5D200AF6BFFF79FB6E9B57E0D0235EE33EEC7845D89D1ADC6DC98`. The author kept the witness vectors and labels private while the detector was developed.

The first attempt on Harborline revealed a generic input-shape error. The planner required one argument count per operation. The correction shares the operation budget across the argument shapes already observed in its public examples. With the final CLI frozen, we ran the same search budgets and opened the holdout labels only after saving the observed results. Replay detected **9 of 11** defects and produced **zero false positives** on the four controls. `renewal_charge` and `coverage_end` were missed. No rule was added to match either case after unblinding.

The first development run also exposed a PowerShell extraction error that substituted zeros for public values. The corrected record uses normalized vectors whose 158 values replayed against the corresponding public expectations in both builds. Those vectors supplied the enhanced worker's public seeds. Program-specific capture and batch execution stay with the project's ordinary runner.

The full solution build passed with zero warnings or errors. Tests passed 383 cases with two environment-dependent skips. Raw probe files, frozen assignment and artifact hashes, scorer outputs, and holdout evidence are retained in the local benchmark scratch.
