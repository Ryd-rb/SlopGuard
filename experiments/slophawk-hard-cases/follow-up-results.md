# Verification follow-up results

The repaired inspector reached 4/4 on the unchanged Norchester evidence, matching the original control. On the new Ravensbourne handoff, the assisted worker found 2/2 and the fresh control found 1/2. The assisted-only finding was a stale transitive restore with matching ownership and framework. Both found a valid historical coverage hit that could not be attributed to the staged source bytes.

| Dossier and access | Verified blockers | False-positive assertions | Input | Cached input | Uncached input | Output | Reasoning within output | Total tokens | Local calls | Seconds |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| Norchester original control | 4/4 | 2 | 204,703 | 171,648 | 33,055 | 3,455 | 1,274 | 208,158 | 6 | 56.8 |
| Norchester original inspector | 1/4 | 0 | 239,985 | 209,536 | 30,449 | 3,412 | 1,503 | 243,397 | 7 | 80.3 |
| Norchester repaired inspector | 4/4 | 1 | 192,534 | 174,592 | 17,942 | 3,406 | 1,660 | 195,940 | 6 | 64.2 |
| Ravensbourne ordinary tools | 1/2 | 0 | 222,349 | 193,024 | 29,325 | 3,825 | 2,084 | 226,174 | 6 | 63.5 |
| Ravensbourne repaired inspector | 2/2 | 0 | 281,336 | 256,512 | 24,824 | 4,828 | 2,928 | 286,164 | 8 | 81.4 |

Usage comes from the final cumulative runtime event for each worker. Total is input plus output and includes repeated cached context. Reasoning is included in output. The repaired Norchester worker used 5.9% fewer total and 45.7% fewer uncached input tokens than the original control. On the harder handoff, assistance used 59,990 more total tokens (+26.5%) and 4,501 fewer uncached input tokens (−15.3%). The assisted worker made eight local calls on Ravensbourne, exceeding the six-call instruction. Its first inspector call was the fourth local call and returned both discrepancies. The final score uses the full in-time response and the reported cost includes all calls. These are descriptive development runs.

## What changed

- The new inspector `review` command uses one dossier-relative release request to select the project, consumer route, run archive, coverage target and current source. It also accepts dossier-relative paths on the individual commands. A bad project selector returns the available evaluated paths.
- The SlopHawk capture boundary can validate independently observed runtime inputs against the declaration. An omission invalidates reuse while leaving file-read authorization with the original scope. The fixture adapter reads the same catalog entry as the executable. Its report shows independently compared approved and current runtime input digests.
- SlopHawk can query a historical coverage counter by target, file and line or query current applicability with the verified source digest. The new inspector reports the archive and current result separately.

## Scoring evidence

The new Norchester worker reproduced the green board, named the v1 restore owner embedded under the v2 project, cited the selected `assets/regional.json` change from 16 to 17, compared the admitted and live epochs, and identified the compatibility hit as outside the requested shipping target. It also claimed the unit suite should have the same target name as the coverage request. The frozen rubric counts that as one unsupported blocking assertion, with the same scope ambiguity described in the [original pilot](pilot-results.md).

| Ravensbourne oracle | Ordinary tools | Assisted |
| --- | --- | --- |
| Restore represents an earlier reference graph | Missed. The worker opened several restore records but reported no stale reference edge. | Verified. The selected `Release.Check` graph goes through Route and v2 Tariff. The evaluated v2 project references Calendar, while the v2 receipt's `project.restore.frameworks.net10.0.projectReferences` is empty. Both receipts name the same v2 project. The planner reports `IncompatibleAssets` for its references. The worker checked the three project files and the receipt. |
| Line hit describes different source bytes | Verified the selected release archive, current hash, seven archived hits, and missing source digest check at the gate. | Verified the same values and the divergence between historical `Observed` and current `Unknown`. |

The ordinary build and smoke checks passed. The shipped board cleared Delta. Replaying either isolated correction held Delta while preserving Ember and the smoke checks. The current unit receipt and approved rate inputs remained valid. The compatibility receipt for the current source remained attributable even with a failed outcome.

## Trial and integrity

The inspector used SlopHawk main plus the runtime changes at `9ea13e5`. The Ravensbourne packer and protocol were frozen at `93ec8be`. The admitted tool tree digest was `1E884736E279FE575D12559D493055C01C29CCA39826299C4C5AA038F1CB0B8F`. It was identical before and after the runs. The final inspector source follows the specimen's single catalog lookup. Four replays across the two dossiers showed identical lane outputs after removing an unused profile convention.

The original Norchester snapshot matched its prior manifest at `9ED9FEFEEA4F099EE69E6F2ED0F98144C29BAE727ADC9C8D63E6AED71A22C9A9`. Ravensbourne's preflight and post-run manifests matched at `0A5ED6DA1482AD909F2E2121ACC82938C85AEA3F781143EA8107B2F3A85A1B0C` over 243 entries. Ravensbourne's assignment was randomized and frozen at `2026-11-16T12:09:59.846Z`. The assisted reviewer ran first. The three new session IDs in dispatch order were `01a1e442-68d4-7e30-a25e-34742b74fd86`, `01a1e444-0a7d-7311-9cba-685f231adc40`, and `01a1e445-6492-78a3-875b-058ff3eb4e54`. Each session used one fresh turn with `gpt-6-luna` at `high` effort. Reviewers saw no answer key or peer output. Their tools were visible to them, and the coordinator knew the arms while scoring.

The solution tests passed 377 checks with two environment-dependent skips. Both the packer and inspector built, and the solution built with zero warnings and errors. To regenerate the harder fixture, build the qualification project and run `Qualification pack-hard NEW_ABSOLUTE_DIRECTORY`. The resulting `Ravensbourne` directory contains the worker handoff. The parent contains the supervised author replays. The stock SlopHawk CLI continues to expose its existing inventory and status commands. The study used the benchmark inspector over the current internal verification components.
