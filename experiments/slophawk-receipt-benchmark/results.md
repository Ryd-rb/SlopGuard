# SlopHawk verification evidence benchmark results

## Outcome

In the four fingerprint-verified pairs, both arms correctly challenged **24 of 24 unsupported claims**. The control accepted 11 of 12 valid claims. The assisted arm accepted all 12. Both arms found every build identity, durable run receipt and coverage defect in these four dossiers.

The inspector helped the Elmwood worker retain a valid line hit from a completed failed run. The control worker used a coverage path without its `run/` segment, could not complete the independent check, and challenged the valid claim. That counted as one control false positive. There were zero assisted false positives.

The paired detection differences were all zero. The descriptive exact one-sided randomization result is **p = 1.0**. Four verified pairs have a minimum one-sided p-value of 0.0625, and both arms reached the detection ceiling on these dossiers.

| Dossier | Phase | Control challenges found | Assisted challenges found | Control valid claims supported | Assisted valid claims supported |
| --- | --- | ---: | ---: | ---: | ---: |
| Arc | pilot | 5/6 | 6/6 | 3/3 | 3/3 |
| Basin | pilot | 6/6 | 6/6 | 3/3 | 3/3 |
| Cairn | verified | 6/6 | 6/6 | 3/3 | 3/3 |
| Dune | verified | 6/6 | 6/6 | 3/3 | 3/3 |
| Elmwood | verified | 6/6 | 6/6 | 2/3 | 3/3 |
| Ford | verified | 6/6 | 6/6 | 3/3 | 3/3 |

The first two pairs are retained as pilot observations. In Arc the assisted worker used the freshness check to identify an old coverage artifact that the control accepted. Basin tied. The [original protocol](protocol.md) and [prospective fingerprint correction](fingerprint-correction.md) specify why these two pairs are outside the verified comparison.

## Worker token use

Across the four verified pairs, the inspector arm used **104,747 fewer total tokens (11.0%)** and **20,666 fewer uncached input tokens (15.7%)**. Its total was lower in 3 of the four pairs. The pilot totals were 487,767 control and 360,654 assisted. These are descriptive measurements from the final cumulative runtime usage events. Input includes cached input and repeated context. Output includes reasoning output.

| Dossier | Phase | Arm | Run ID | Input | Cached input | Uncached input | Output | Reasoning output | Total | Local tool calls | Seconds |
| --- | --- | --- | --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| Arc | pilot | control | `487051fb1d` | 227,532 | 197,504 | 30,028 | 4,219 | 1,690 | 231,751 | 6 | 65.3 |
| Arc | pilot | preview | `6df5ec4f22` | 214,927 | 173,696 | 41,231 | 2,998 | 1,097 | 217,925 | 6 | 50.8 |
| Basin | pilot | control | `4cdb1e2414` | 251,333 | 222,592 | 28,741 | 4,683 | 2,367 | 256,016 | 7 | 73.5 |
| Basin | pilot | preview | `30f7168326` | 140,464 | 116,992 | 23,472 | 2,265 | 805 | 142,729 | 4 | 36.7 |
| Cairn | verified | control | `7c05fcc5e5` | 185,455 | 153,472 | 31,983 | 3,255 | 900 | 188,710 | 5 | 49.1 |
| Cairn | verified | preview | `d187adf940` | 188,613 | 163,072 | 25,541 | 2,345 | 754 | 190,958 | 6 | 39.2 |
| Dune | verified | preview | `24f1d8d75f` | 229,147 | 201,088 | 28,059 | 2,971 | 903 | 232,118 | 5 | 46.6 |
| Dune | verified | control | `2200a9df79` | 239,806 | 199,936 | 39,870 | 3,442 | 1,205 | 243,248 | 6 | 54.0 |
| Elmwood | verified | preview | `3ab631993e` | 208,762 | 179,328 | 29,434 | 3,115 | 1,124 | 211,877 | 6 | 48.2 |
| Elmwood | verified | control | `539929fe3e` | 281,765 | 250,240 | 31,525 | 5,091 | 2,497 | 286,856 | 7 | 74.3 |
| Ford | verified | preview | `c3f7b0b57f` | 211,118 | 182,784 | 28,334 | 2,776 | 923 | 213,894 | 6 | 45.1 |
| Ford | verified | control | `7d8c5659c1` | 231,536 | 202,880 | 28,656 | 3,244 | 1,346 | 234,780 | 6 | 49.7 |

| Phase | Arm | Input | Cached input | Uncached input | Output | Reasoning output | Total |
| --- | --- | ---: | ---: | ---: | ---: | ---: | ---: |
| pilot | control | 478,865 | 420,096 | 58,769 | 8,902 | 4,057 | 487,767 |
| pilot | preview | 355,391 | 290,688 | 64,703 | 5,263 | 1,902 | 360,654 |
| verified | control | 938,562 | 806,528 | 132,034 | 15,032 | 5,948 | 953,594 |
| verified | preview | 837,640 | 726,272 | 111,368 | 11,207 | 3,704 | 848,847 |

The runtime identified every worker as `gpt-6-luna` at High effort. All twelve finished within three minutes. Two control workers recorded seven grouped local calls. Basin was a pilot and Elmwood was a verified run. The instruction-level call budget was six. Both assigned runs remain in their respective results.

## Tool and evidence qualification

The shipped code basis was SlopHawk main at `3cee615a9173dcfa3ca8ff4c25a98d15b82c269b`. The daemon assembly SHA-256 was `F8B72A4FC681A411B15BF34BAB98FA99E429AF932336689952CD1B5D730475A9`. The separate read-only inspector assembly SHA-256 was `CA5EDF0895981A99D84600379FF5599D9C680800717EE537BFD5B7C78D1B2F90`. The original freeze was `69c07e98039ca48f94284a1df9794d956684edf6`. The fingerprint correction was committed at `0deab4f78e11728429a50108d91cd1627cbef5a7` before dispatching the remaining eight workers.

The factory ran two actual prebuilt test assemblies through the Windows worker lease and VSTest runner. Independent XML counts confirmed that attempt 411 passed both executed tests, and attempt 438 failed one of two. The 54 raw claims were all qualified against the planted changes and the production component observations. The benchmark uses measured build bytes, archived test artifacts and synthesized Cobertura with independently controlled attempt and lease metadata. The stock preview CLI reported `awaiting_adapter`. Worker evidence came through the benchmark inspector and raw files. The workers did not exercise automatic scheduling or launch the specimen assemblies during their reviews.

The first fingerprint implementation could not reproduce the frozen digest for opened or unopened dossiers. The correction retained a per-entry manifest that matched in separate processes before the verified dispatch and after each of the four verified pairs. The trial remains limited to structured release handoffs with explicit evidence rules. It does not estimate a production-wide review effect.

| Dossier | Control evidence | Assisted evidence |
| --- | --- | --- |
| Arc | Supported the stale coverage artifact after checking alias and lease identities, without checking its modification time. | Verified settings bytes, Compile order, the all-F digest against replay, coverage file time and the fence sequence. |
| Basin | Found all six unsupported claims and accepted the controls. Independently computed the runtime asset hash. Reported that the valid failure receipt's Git fingerprint had not been recomputed. | The inspector and raw evidence supported all six challenges. The stated runtime asset byte count was inaccurate. The identified byte-level runtime-assets mismatch was independently verified. |
| Cairn | Checked exact coverage alias and branch scope, link identities, measured hashes, recomputed the admitted Git fingerprint, and compared TRX run IDs. | Named the relevant raw scope or binding for each challenged claim and cited the matching inspector observation. |
| Dune | Used exact hashes, ordered compile sequence, signed attempt fields, branch counter and the undrained lease flag. | Grouped the inspector and raw measurements. Correctly accepted a completed failure and the mapped zero counter. |
| Elmwood | Found all six unsupported claims. A coverage-path lookup missed the required run subdirectory, so the worker challenged a valid failed-run line hit that it could not independently verify. | Inspector and raw files distinguished a missing exact line counter, branch aggregate, stale TRX, epoch and incomplete closure. Kept valid failed-run coverage. |
| Ford | Identified alternate link, run identifier and lease, aggregate branch claim, byte change and unavailable collector. Independently hashed the recorded run and source. | Named all six raw differences and clearly separated verified zero coverage and a valid failure receipt. |

The solution and benchmark tools built with zero warnings and errors. `dotnet test SlopHawk.slnx` passed 376 tests and skipped two environment-dependent integration tests. The factory separately exercised the native VSTest path.
