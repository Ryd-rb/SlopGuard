# Unassisted Luna 6 Low bug discovery

Each round used fresh `gpt-6-luna` workers at `low` effort, with ordinary PowerShell and .NET tools and uncapped tool calls. The workers received a release review brief and a single frozen dossier. They had no SlopHawk output, seed counts, private witnesses, patches or other reviews. The public checks passed in both the shipping and repaired builds. Every seed had a private valid input that distinguished those builds. A verified finding additionally required the worker to describe the defect and report a valid input with the correct shipping and expected outputs. Localization records correct diagnoses for any seeds that the worker failed to verify.

| Round | Dossiers | Unique seeds in round | Verified | Missed | Localized | False positives | Logical tokens | Uncached input |
|---|---:|---:|---:|---:|---:|---:|---:|---:|
| Standalone services | 10 | 100 | 63 | 37 | 68 | 1 | 1,640,845 | 154,600 |
| Routed services | 3 | 106 | 88 | 18 | 90 | 0 | 1,299,136 | 117,661 |

## Final round

Both source-complete journal reviews missed more than half of the 132 unique seeds. All 316 public checks passed in both builds. The author replayed all 132 private counterexamples through the journal and through the same producer command the workers used. The tested workers used no SlopHawk tools or reports.

| Eligible run | Verified causes | Missed causes | Distinct numeric differences in probes | False positives | Logical tokens | Uncached input |
|---|---:|---:|---:|---:|---:|---:|
| Journal 1 | 2 / 132 | 130 | 61 | 0 | 775,628 | 51,618 |
| Journal 2 | 16 / 132 | 116 | 21 | 0 | 1,095,686 | 80,575 |

Replay confirmed 298 numeric differences across the first worker's 1,896 captured requests and 21 differences across the second worker's 158 captured requests. Both observed numeric differences for 18 of the same seeds. Taken together, their probes exposed 64 of 132 roots, leaving 68 (51.5%) without a replayed difference. They correctly diagnosed 17 distinct compatibility defects between them. Under the original operation contracts' narrower input rules, the second worker verified 12 and the first still verified two.

The [standalone round](wide-low.md) used 120 passing ordinary checks and a deadline of eight minutes per dossier. The [routed round](wide-integrated-low.md) retained all 100 roots and added six faults in producer handoffs. Its 252 public checks covered accepted and rejected routes. The deadlines were 24 minutes for FieldService and DataPlatform and 32 minutes for ReportingHub. Every full review finished before its deadline. All source fingerprints matched after the reviews.

The [continuity release](wide-continuity-low.md) preserves the 106 earlier roots and adds 26 distinct reconciliation faults. Both builds passed 316 public checks and each of the 132 private witnesses distinguished the two builds. New cases include a return priced using the report date instead of the refund date, legitimate but obsolete sequence numbers in inactive ring slots, and a two-leg reservation transfer whose ledgers persist across restart independently.

## Per-run usage

Logical tokens include cached input, uncached input, and output. Reasoning tokens are part of output. Calls count local tool invocations. Authoring, qualification, and adjudication are outside these worker totals.

| Round | Dossier | Verified / seeds | Localized | False positives | Logical tokens | Uncached input | Calls | Elapsed |
|---|---|---:|---:|---:|---:|---:|---:|---:|
| Standalone | Juniper | 7 / 10 | 9 | 0 | 141,144 | 16,360 | 5 | 48.603 s |
| Standalone | Lattice | 1 / 10 | 1 | 0 | 161,927 | 14,938 | 6 | 34.245 s |
| Standalone | Turnstile | 7 / 10 | 8 | 0 | 142,961 | 17,399 | 5 | 41.985 s |
| Standalone | Quarry | 9 / 10 | 9 | 0 | 169,648 | 16,523 | 6 | 53.914 s |
| Standalone | Rook | 5 / 10 | 5 | 0 | 138,237 | 8,264 | 5 | 40.343 s |
| Standalone | Folio | 7 / 10 | 7 | 1 | 141,599 | 16,161 | 5 | 39.319 s |
| Standalone | Switchyard | 1 / 10 | 2 | 0 | 218,205 | 17,737 | 7 | 45.493 s |
| Standalone | Quorum | 9 / 10 | 9 | 0 | 166,539 | 10,813 | 6 | 48.374 s |
| Standalone | Quill | 8 / 10 | 9 | 0 | 220,086 | 21,622 | 8 | 60.548 s |
| Standalone | Council | 9 / 10 | 9 | 0 | 140,499 | 14,783 | 5 | 42.771 s |
| Routed | FieldService | 28 / 32 | 28 | 0 | 386,644 | 40,799 | 10 | 125.858 s |
| Routed | DataPlatform | 28 / 32 | 30 | 0 | 397,515 | 37,038 | 10 | 163.476 s |
| Routed | ReportingHub | 32 / 42 | 32 | 0 | 514,977 | 39,824 | 12 | 139.372 s |
| Continuity release | Review 1 | 43 / 132 | 44 | 0 | 1,268,903 | 71,693 | 21 | 224.961 s |
| Continuity release | Review 2 | 7 / 132 | 8 | 0 | 362,216 | 36,304 | 9 | 66.271 s |
| Continuity with coverage | Review 1 | 88 / 132 | 94 | 0 | 2,274,766 | 106,520 | 31 | 684.371 s |
| Continuity with coverage | Review 2 | 50 / 132 | 54 | 0 | 1,653,521 | 136,925 | 28 | 260.434 s |
| Journal, initial | Incomplete review 1 | 0 submitted | 0 | 0 | 389,399 | 42,855 | 10 | 50.490 s |
| Journal, initial | Incomplete review 2 | No valid answer | — | — | 1,052,583 | 49,702 | 21 | 119.331 s |
| Journal, revised | Journal 1 | 2 / 132 | 2 | 0 | 775,628 | 51,618 | 17 | 93.749 s |
| Journal, revised | Incomplete source coverage | 7 numeric replays | — | — | 520,532 | 27,657 | 14 | 89.111 s |
| Journal, revised | Journal 2 | 16 / 132 | 16 | 0 | 1,095,686 | 80,575 | 18 | 231.605 s |

## Adjudication notes

- The Folio review asserted an additional terminal-boundary crash in `WeightedPercentile`. A positive total and a valid percentile below 100 put the target strictly below the final cumulative boundary. That claim is a false positive. Its separate valid zero-weight witness received credit.
- The Rook worker reported its five findings in its response but left no answer file. Its saved probes supplied the five argument vectors and actual results. The stated contracts and diagnoses supplied the two missing numeric expectations. All five were independently replayed. If the recovery were omitted the standalone total would be 58 verified.
- The Switchyard review sent one unsolicited progress message. It remained unanswered. This message is outside its seven local calls and included in the runtime token totals.
- The FieldService reviewer submitted a top-level JSON array. The reports retain the original bytes. The adjudicator accepts either the array or the findings object without changing any assertion. The DataPlatform reviewer correctly diagnosed `ReplicaWriteDomains` and `ReadLagState` but gave a miscounted expected result and an inapplicable current-term example, respectively. Both count as localization only.
- The two natural continuity reviews finished early and left components unaudited. Review 1 read implementation source in 12 of 15 components. Among the 102 seeds in those components, the replay matched 44 outputs. One witness used negative document coordinates outside the contract. It verified 43 and missed 59 (57.8%) within the read source, and verified 13 of the 26 new roots. Review 2 read implementation source in two components, which contained 14 seeds, and verified seven. In `DispatchFrame`, its equal base offsets caused both builds to produce the same result. It described the missing independent right-frame adjustment correctly, so that claim counts as localization. Both natural runs are retained. Their full-release miss counts reflect incomplete source coverage.
- The two coverage follow-ups read all 15 components. Numeric replay matched 91 and 51 assertions. Review 1 used negative document coordinates, simultaneous preparation and receipt, and an undocumented tail record kind in three witnesses. Review 2 specified distinct start and end symbols for a single run. These witnesses count as localization only, along with three nonmatching output claims in each review. The verified miss rates were 44/132 (33.3%) and 82/132 (62.1%). Both verified 44 of the same causes, either verified 94, and neither verified 38. Only the second complete audit missed half.
- The two initial journal reviews were incomplete. The first submitted no findings and marked all components unread. The second established that the prior release passed its public profile, reported `null` on manually built probes, and returned no parseable answer. The first two runs supply no full-scope miss rate. Their dossier's root directory timestamp changed, while every file and other entry retained its recorded hash and metadata. The revised package adds a validated producer command for capturing journals from ordinary business fields.
- The first eligible journal review submitted two exact diagnoses and generated a separate broader probe log. One command addressed a nonexistent `operations` directory one level above the dossier. It returned an error and no evidence. The other initial reviewer of the revised journal marked every component reviewed but sent file contents to `Out-Null`. Only seven implementations reached its context. Keep its seven numeric replays and tokens as an incomplete review. A fresh replacement used the same pinned dossier, with explicit instructions that source bodies must reach its context. Both eligible reviewers received all 158 operation implementations in the recorded tool outputs.
- The second eligible journal review submitted 19 cases with exact numeric replay and two unexplained differences. Three of the 19 gave the wrong cause. `RefundSlice` subtracts two cumulative truncated totals, `DailyAccrual` keeps the original period anchor, and `RecoverTransferReserve` checks release and successor persistence independently. Four more cases identified the correct compatibility cause but used requests outside earlier per-operation assumptions about reset readings, generation ordering, alias capacity or attempt state. The revised compatibility contract counts admitted numeric behavior, so those four are valid under that contract. Neither eligible review asserted an additional defect disproved by replay.

## Corpus pins

The standalone services were frozen at `d35b2e87f431e1acdb5a32198b0958255c20b2b5` on 2026-11-16 at 13:15:42 UTC. The routed services were frozen at `67f0ea73b52ddac5a16118ab10694f7ac0ce9a5e` at 13:25:25 UTC. These are descriptive rounds on a deliberately constructed corpus. Reused roots and clustering by dossier preclude treating the rounds or individual cases as independent production observations.

The continuity release was frozen at `69b6b6e71ace2122701dc64da694f28743f8fe65` at 13:37:13 UTC. The 199-entry fingerprint is `896B55F16A434D79D4C8F98818BE509DD1EE39011881903A26B1F16F9B8F709A`. Both natural reviews preserved that fingerprint. The same dossier and the coverage brief were pinned before the follow-up at `f39b8028cf11a1e9f418ce12b4784c8d1a9f7f7c` at 13:43:35 UTC.

The [journal protocol](wide-deployment-low.md) was first frozen at `aeed913479d9264a913ca3bb1110f3d360802339` at 14:00:20 UTC. Its 204-entry fingerprint is `D56B3840C4DC337CF892B2088742485059743995EDC335C89DB2F71552AE1E95`. The revised copy with the producer command was frozen at `f12303809d78e237eda7591ef80f8cfc0404a519` at 14:04:39 UTC with fingerprint `96FF213C9D01A7B75D9F382F8D8702D932AF09297FE20076309727D9F026145D`.

The two eligible journal worker IDs were `01a1e4ac-ec86-7473-9808-e2befa62efb7` and `01a1e4af-c7a6-79e2-b879-28babc93160f`. Their post-review fingerprints matched the frozen journal exactly. The workloads combine synthetic business policies with production-inspired replay and persistence paths. The changes in review scope, contract presentation and journal workflow do not isolate which factor reduced detection. The solution build had zero warnings and errors, and 377 tests passed with two environment-dependent skips. The final qualification harness was built and run separately.
