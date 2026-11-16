# Blinded Luna effort comparison results

All six fresh Luna 6 workers verified their project's seeded blocker. The ordinary-tools arm found 3/3 with zero false positives. The SlopHawk-assisted arm found 3/3 and made one false-positive assertion about a valid neighboring handoff. Tool calls were uncapped. The assisted arm used 115,610 more total tokens (+24.4%) and 10,979 fewer uncached input tokens (−21.0%) across the three pairs.

| Project and effort | Access | Verified | False positives | Input | Cached | Uncached | Output | Reasoning within output | Total | Calls | Seconds |
| --- | --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| Kestrel, Low | Ordinary | 1/1 | 0 | 147,924 | 135,936 | 11,988 | 2,166 | 926 | 150,090 | 6 | 40.5 |
| Kestrel, Low | Assisted | 1/1 | 1 | 150,905 | 143,872 | 7,033 | 1,248 | 0 | 152,153 | 6 | 29.4 |
| Wren, Medium | Ordinary | 1/1 | 0 | 172,382 | 148,352 | 24,030 | 2,028 | 408 | 174,410 | 7 | 40.9 |
| Wren, Medium | Assisted | 1/1 | 0 | 195,209 | 180,352 | 14,857 | 1,731 | 447 | 196,940 | 7 | 36.7 |
| Oriole, High | Ordinary | 1/1 | 0 | 145,560 | 129,408 | 16,152 | 3,276 | 1,696 | 148,836 | 5 | 52.2 |
| Oriole, High | Assisted | 1/1 | 0 | 237,541 | 218,240 | 19,301 | 2,312 | 642 | 239,853 | 9 | 53.8 |

Total logical tokens were 473,336 ordinary and 588,946 assisted. Uncached input was 52,170 ordinary and 41,191 assisted. The paired total-token increases were 2,063 at Low, 22,530 at Medium and 91,017 at High. The High assisted worker chose nine calls while the control chose five. Final cumulative session counters include repeated input context and failed calls. Reasoning output is included once, in output. Authoring, preflight, Sol design, and coordination usage are outside these worker numbers.

The randomized launch order put the ordinary-tools worker first in all three pairs. Cache warming could contribute to the observed uncached-input difference. Each effort was assigned a different project, so the token and detection results support comparisons within a pair. All pairs hit the detection ceiling and constitute three descriptive observations.

## Findings

| Project | Minimum verified finding | Evidence from the pair |
| --- | --- | --- |
| Kestrel | Two collector paths both measure `src/ParcelRule.cs:14` in the proposed report, despite a green `2/2` aggregate. | Both workers independently identified the two aliases and repeated line. The assisted worker used the importer on the candidate only. It extended the malformed verdict to the standing report, whose second alias reports line 15 and whose scoped import returns `Imported` and `Observed`. That extension is the one false positive. |
| Wren | A passing retry includes `Escaped` and `Split` instead of `Split` and `Boundary`. | Both workers named attempt 2548 and its wrong case set. The ordinary arm confirmed it in the TRX. The assisted arm used both inspector results and identified the mismatch from the archived dispatch and required matrix. The inspector validated both receipts, which were genuine passing runs. |
| Oriole | The front-desk receipt is inapplicable to the accessibility profile despite sharing the same DLL, target and passing status. | The ordinary arm matched the selected route, stamp, settings and TRX case. The assisted arm confirmed a settings-only mismatch with the inspector and checked that the standing receipt matched the effective profile. The ordinary arm flagged the external `TestMethod codeBase` path as a verification limitation, without blocking the standing handoff on it. |

## Integrity and reproduction

Two fresh `gpt-6.1-sol` design contexts independently contributed cases. The Medium designer proposed Kestrel and Wren. The High designer proposed Oriole. Each ordinary gate cleared the proposed and standing handoff and passed smoke. In three private replays, the isolated corrections held the proposed handoff and preserved smoke and the standing handoff. Native VSTest produced five passing TRXs. The author confirmed the selected case identities in the raw files. Kestrel uses controlled Cobertura and Wren's prior timeout is a controlled archive record.

The base SlopHawk main was `db122b2`. The benchmark wrapper and [protocol](effort-matrix.md) were committed at `b92872b` before any of the six reviews. The recorded assignment freeze time was `2026-11-16T12:28:17.581Z`. These are per-entry snapshot digests over paths, file bytes and timestamps. All matched across processes before dispatch and after the runs.

| Snapshot | Entries | SHA-256 |
| --- | ---: | --- |
| Kestrel | 17 | `CE1DE2033215CD1860E981456D9CC9D9744CDDF2A02FB7D1CF67A8BEE3C71B55` |
| Wren | 27 | `E90EF5AF75463EAA3F280BFD5D964DCABA1D8DC43D05998E6B85D53A15B31EB8` |
| Oriole | 30 | `02F470EA9C1C9B8E3F4EFA19309FBBB101054265899618E8FDF5514F448D5D86` |
| Inspector | | `8179480FCE41F161E577DE9D594AF766753D2129CCD44ADC7561E6573878E783` |

The run IDs in table order were `01a1e454-aedc-7f62-9bfd-ed76c8f3df9f`, `01a1e455-836a-7ba1-9165-5995aa8b731b`, `01a1e456-1573-7362-9da8-a599bcb158ec`, `01a1e456-d605-7963-8389-6408b2579009`, `01a1e457-7ffb-7af0-bf24-5480549c1466`, and `01a1e458-6e22-71f3-b31d-2ab2d07432dc`. Runtime logs confirmed one fresh turn per separate worker, assigned model and effort, and completion inside four minutes. The snapshots and command traces showed reads within the projects and approved output locations. The workers were blind to the seeds and peer reviews. The coordinator knew the assignment while scoring the frozen oracle.

The solution built with zero warnings and errors. The suite passed 377 tests with two environment-dependent skips. `Qualification matrix NEW_ABSOLUTE_DIRECTORY` recreates the projects and private replays. The treatment uses the benchmark inspector over current SlopHawk verification components.
