# SlopHawk benchmark of the 0.2 preview

## Result

The expanded, preregistered benchmark found **no statistically significant improvement in detection** from access to the 0.2 preview. Sixteen paired release candidates had eight independently verified issues each. Across 32 fresh Luna 6 High workers, the assisted arm found **120/128 (93.75%)** and the control found **124/128 (96.88%)**.

The preview arm averaged **−0.25 findings per workspace**, with 2 wins, 5 losses, and 9 ties. The prespecified one-sided exact paired randomization test gives **p = 0.94531** at alpha 0.05 (61,952 of 65,536 sign assignments reached or exceeded the observed sum of −4). This is a test for increased detection on the fixed benchmark.

Both arms found 118 of the same issues. The assisted workers uniquely found two, the controls uniquely found six, and both missed two. The Lumen assisted worker cited the preview graph for Deliver's wrong defaults reference. The Nacre assisted worker found Reject's wrong defaults reference through ordinary MSBuild after reporting that the preview command produced no graph output. Assisted workers explicitly tied 93 of their confirmed findings to preview evidence. These citations show where the tool supplied evidence. The randomized arm comparison measures whether that evidence increased overall detection.

The control arm reported one false positive. Its Elm worker claimed that Label was excluded from CI in one row, and the same review's test log and the independent qualification show that Label executed. There were zero preview false positives, unsupported defect claims, and incomplete runs. The two groups independently confirmed all CI discovery and quantity-limit issues.

| Category | Planted per arm | Control | Preview |
| --- | ---: | ---: | ---: |
| Compiled policy source | 48 | 46 | 44 |
| Defaults project reference | 32 | 30 | 28 |
| CI test discovery | 32 | 32 | 32 |
| Quantity limit | 16 | 16 | 16 |

The control missed just four issues, so there was little room for additional detection on this release-matrix task. The findings describe these generated .NET 10 workspaces and this reviewer workflow. The fixtures share a scaffold and visible acceptance matrix. Production reviews can differ in scope, difficulty and how easily a reviewer can extract effective project metadata.

## Pair scores

`S` denotes a compiled policy-source issue, `R` a defaults reference, `T` CI discovery and `L` an inclusive-limit regression. All 16 projects had the same maximum score of eight.

| Project | Control | Preview | Preview minus control | Missed by control | Missed by preview |
| --- | ---: | ---: | ---: | --- | --- |
| Alder | 8 | 7 | −1 | — | `R-Refund` |
| Beryl | 8 | 7 | −1 | — | `S-Route` |
| Cinder | 8 | 8 | 0 | — | — |
| Delta | 8 | 8 | 0 | — | — |
| Elm | 8 | 8 | 0 | — | — |
| Flint | 8 | 7 | −1 | — | `R-Store` |
| Groveway | 7 | 6 | −1 | `S-Water` | `S-Water`, `R-Seed` |
| Harbour | 8 | 8 | 0 | — | — |
| Iris | 8 | 8 | 0 | — | — |
| Juniper | 8 | 6 | −2 | — | `S-Retire`, `R-Retire` |
| Kestrel | 8 | 8 | 0 | — | — |
| Lumen | 7 | 8 | +1 | `R-Deliver` | — |
| Moraine | 8 | 8 | 0 | — | — |
| Nacre | 7 | 8 | +1 | `R-Reject` | — |
| Opal | 8 | 8 | 0 | — | — |
| Pine | 7 | 7 | 0 | `S-Ship` | `S-Ship` |
| **Total** | **124** | **120** | **−4** | | |

## Runtime token measurements

The preview workers used **1,231,728 fewer total tokens (18.9%)** across all runs, with lower totals in 12 of 16 pairs. They used **122,699 fewer uncached input tokens (18.8%)** and **6,235 more output tokens**. Token use was a descriptive secondary measurement. These runtime counts include repeated context and cache hits. Output includes reasoning output.

| Project | Arm | Run ID | Input | Cached input | Uncached input | Output | Reasoning output | Total | Seconds |
| --- | --- | --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| Alder | preview | `2d203bca7e` | 253,543 | 227,712 | 25,831 | 4,887 | 2,745 | 258,430 | 93.1 |
| Alder | control | `a48f0ba8e7` | 308,651 | 271,104 | 37,547 | 3,807 | 1,386 | 312,458 | 73.4 |
| Beryl | control | `d3e3b8d023` | 308,386 | 266,624 | 41,762 | 4,129 | 1,608 | 312,515 | 85.1 |
| Beryl | preview | `210fdde1e9` | 400,510 | 347,392 | 53,118 | 4,918 | 1,675 | 405,428 | 108.7 |
| Cinder | control | `79e71ba6fe` | 465,122 | 413,312 | 51,810 | 4,540 | 1,781 | 469,662 | 89.2 |
| Cinder | preview | `483ddc6506` | 219,743 | 189,440 | 30,303 | 4,329 | 2,172 | 224,072 | 74.6 |
| Delta | preview | `53ea558db4` | 273,383 | 248,576 | 24,807 | 3,297 | 1,345 | 276,680 | 78.1 |
| Delta | control | `54a0afdbde` | 435,870 | 395,776 | 40,094 | 3,605 | 1,365 | 439,475 | 82.1 |
| Elm | preview | `ba84a56bbb` | 405,537 | 381,184 | 24,353 | 4,579 | 1,898 | 410,116 | 101.8 |
| Elm | control | `05ebbbd4b5` | 265,597 | 244,608 | 20,989 | 4,127 | 2,353 | 269,724 | 89.9 |
| Flint | control | `2584a55f90` | 475,482 | 430,336 | 45,146 | 4,295 | 1,567 | 479,777 | 101.1 |
| Flint | preview | `c67c08094a` | 275,930 | 252,928 | 23,002 | 3,509 | 1,419 | 279,439 | 72.1 |
| Groveway | preview | `a9f88a471e` | 385,997 | 359,680 | 26,317 | 4,657 | 1,836 | 390,654 | 100.6 |
| Groveway | control | `02ee1b574c` | 433,020 | 392,576 | 40,444 | 4,277 | 1,645 | 437,297 | 87.4 |
| Harbour | control | `b70912d519` | 434,595 | 380,160 | 54,435 | 3,846 | 1,271 | 438,441 | 108.3 |
| Harbour | preview | `cfe805bad2` | 225,361 | 190,976 | 34,385 | 3,486 | 1,434 | 228,847 | 86.9 |
| Iris | preview | `9a55c30567` | 419,452 | 372,480 | 46,972 | 4,615 | 2,152 | 424,067 | 98.9 |
| Iris | control | `1e8474b6d0` | 394,131 | 356,608 | 37,523 | 4,112 | 1,469 | 398,243 | 91.9 |
| Juniper | control | `3259d14a6a` | 536,283 | 499,456 | 36,827 | 4,081 | 1,525 | 540,364 | 105.0 |
| Juniper | preview | `c3fe5b9038` | 372,376 | 333,184 | 39,192 | 3,987 | 1,475 | 376,363 | 88.1 |
| Kestrel | control | `47cf562216` | 423,174 | 382,720 | 40,454 | 3,952 | 1,685 | 427,126 | 93.8 |
| Kestrel | preview | `2e8291535a` | 297,864 | 266,880 | 30,984 | 3,845 | 1,652 | 301,709 | 80.8 |
| Lumen | control | `e86279a56a` | 222,107 | 186,112 | 35,995 | 3,866 | 1,612 | 225,973 | 74.3 |
| Lumen | preview | `238405cb3d` | 341,168 | 317,184 | 23,984 | 4,876 | 3,045 | 346,044 | 107.0 |
| Moraine | control | `ea2cbbded1` | 472,535 | 423,808 | 48,727 | 3,661 | 1,453 | 476,196 | 99.4 |
| Moraine | preview | `599c165fe7` | 243,815 | 205,184 | 38,631 | 3,822 | 1,727 | 247,637 | 85.4 |
| Nacre | control | `a885113ae4` | 426,290 | 361,088 | 65,202 | 4,297 | 1,803 | 430,587 | 100.7 |
| Nacre | preview | `510c8bd621` | 416,625 | 386,816 | 29,809 | 5,546 | 2,243 | 422,171 | 144.2 |
| Opal | preview | `7f32e2621f` | 337,659 | 307,072 | 30,587 | 5,045 | 2,358 | 342,704 | 117.6 |
| Opal | control | `8771bf299c` | 439,858 | 413,056 | 26,802 | 4,035 | 1,868 | 443,893 | 113.9 |
| Pine | preview | `eaf651d3d9` | 333,035 | 286,976 | 46,059 | 5,213 | 2,425 | 338,248 | 118.6 |
| Pine | control | `f15f4b5e76` | 398,860 | 371,584 | 27,276 | 3,746 | 1,503 | 402,606 | 96.1 |
| **Total** | **control** | | **6,439,961** | **5,788,928** | **651,033** | **64,376** | **25,894** | **6,504,337** | |
| **Total** | **preview** | | **5,201,998** | **4,673,664** | **528,334** | **70,611** | **31,601** | **5,272,609** | |

## Evidence and limitations

The [protocol and generator](protocol.md) were fixed at `2dd9faa5df2061b069b0795c0e0aa952028b92ab`. The two cloned baselines and uncommitted candidate diffs matched in each pair before and after review. The order above preserves the randomized assignment of the two dispatch slots. Runtime records confirmed `gpt-6-luna` at High effort and all completions within the four-minute limit. The preview was built from the pretrial code at `2dd94b304519f5f0ad17136c821e554c1cb91902` and identified itself as version 0.2.0-preview.1. The SHA-256 of the trial CLI assembly was `19C3AA31FE620BAFA0B8501180FCFA70A28D830F61DC5E29CBA29AA4F492ECA2`.

The independent qualification used .NET SDK 10.0.201 on Windows. All 16 solutions built and their test gate ran the three expected active suites. Executing each module at its documented limit and one above verified effective policy and provider identity and behavior. All 16 assisted daemons recorded one source-edit intent before dispatch and reported `awaiting_adapter`. The ordinary .NET test gate supplied executed-test evidence.

| Project | Control review evidence | Preview review evidence |
| --- | --- | --- |
| Alder | Built the candidate, queried MSBuild, ran the six CLIs and the ordinary CI gate. The completion summary says seven, but explicitly lists eight distinct scored claims with full evidence in the preceding worker message. | Read the complete project graph and reproduced test discovery and Transfer behavior. |
| Beryl | Verified all release routes via build, limit and adjacent CLI runs, solution tests and release profile source. | Complete project graph, build, CI gate and Index smoke confirmed reported findings. Correctly treated Route's verification as manual. |
| Cinder | Matched evaluated MSBuild properties and source predicate to the matrix. Worker build skipped restore and hit NETSDK1004. | Complete graph and source predicate. Worker build skipped restore and hit NETSDK1004. |
| Delta | MSBuild values, restored solution test gate and exact-boundary CLI. | Complete graph, build, CI gate, and boundary commands. Source and reference findings corroborated where probed. |
| Elm | Verified eight issues. Additionally alleged that Label was absent, contradicted by the same report's successful Label suite and the independent oracle. | Complete graph, ordinary build and CI gate, all module boundaries probed. |
| Flint | Evaluated compile items and reference properties, then built, ran the CI gate, and reproduced Store and Allocate from their CLIs. | Used the project graph and solution test results plus the annotated boundary predicate. Pre-existing Git edits were context, not a separate reported defect. |
| Groveway | Explicit seven-item detailed table, MSBuild and CI outputs. The compact summary grouped claims and said six. It treated Water as clean. The additional observation that smoke tests skip the exact boundary is correct, with no separate test requirement in the matrix. | Complete graph, ordinary build and test gate, direct Tray boundary reproduction. |
| Harbour | Profile and project source checks, successful build, executed suite identities, and CLI probes at all boundaries. | Complete graph, build, CI gate and inclusive predicate analysis. |
| Iris | Source and MSBuild values, normal build and CI gate, and direct Return probe. No actual CI workflow files in the fixture, so tested the declared gate. | Complete graph, ordinary build and CI gate, direct 11/12 CLI probes. |
| Juniper | Evaluated items and properties, built, confirmed actual test suites and reproduced the exact boundary. | Complete graph and test gate. Source establishes boundary rejection although direct no-build executable launch failed. |
| Kestrel | MSBuild evaluation and successful restore, build and CI gate. Boundary reproduced. | Complete graph for routing, then ordinary CI gate and the direct boundary predicate. |
| Lumen | MSBuild and source established seven issues. Attempted the CI gate with no restore and hit NETSDK1004. | Complete graph, normal build/test, output from module CLIs. An assisted-only finding was Deliver's defaults reference. |
| Moraine | Release source, module CLI behavior, build and test discovery. | Graph for source/reference evidence. Standard MSBuild property queries and CI test run for discovery. Direct limit predicate. |
| Nacre | Compiled and ran modules, and identified the two CI suite gaps. Missed the Reject defaults reference. | Worker reported that preview projects returned no output. Reached all findings with direct MSBuild queries, build, CI gate and Store boundary probes, including Reject defaults. |
| Opal | Built, ran module CLIs and inspected discovered suites. | Detailed worker message has eight independently checkable rows. Short completion summary says seven. The worker reported one `git status` in the SlopHawk checkout before correcting its root. Subsequent build, test, graph and CLI evidence is candidate-scoped. It reported a pending final status snapshot. |
| Pine | Restored, built, ran CLI probes, and checked ordinary CI discovery. | Graph, build, CI gate and Ship runtime probes. Both workers reported the Ship boundary bug without identifying its separate policy-file mismatch. |

Three worker summaries understated their own finding counts (Alder control, Groveway control and Opal preview). Scoring required the concrete claims in the detailed evidence. The Opal preview worker reported initially reading Git status from the coordinator checkout. The run remained in its assigned arm, and its scored evidence came from the candidate. Its latest status snapshot was pending. The Nacre preview worker used direct MSBuild and .NET after reporting an empty graph response. The two Cinder workers and the Lumen control reported missing build assets after trying to skip restore. Those runs remained in the result and received credit for verifiable source and evaluated findings.

The benchmark generator and the solution at the pretrial revision both built with zero warnings or errors. The solution test suite passed all 247 tests in the isolated worktree.
