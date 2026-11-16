# Norchester Dispatch pilot results

One fresh Luna 6 High worker with ordinary tools verified **4 of 4** seeded release blockers. The worker with the SlopHawk inspector verified **1 of 4**. Both correctly recommended holding Delta. The assisted worker spent **35,239 more logical tokens (+16.9%)** and **2,606 fewer uncached input tokens (−7.9%)**. The count includes unsuccessful inspector calls. One dossier and an overrun of the instruction-level tool budget leave this as a development result.

| Access | Verified blockers | False-positive blocking assertions | Input | Cached input | Uncached input | Output | Reasoning within output | Total | Grouped local calls | Seconds |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| Ordinary .NET and PowerShell | 4/4 | 2 | 204,703 | 171,648 | 33,055 | 3,455 | 1,274 | 208,158 | 6 | 56.8 |
| Ordinary tools plus SlopHawk inspector | 1/4 | 0 | 239,985 | 209,536 | 30,449 | 3,412 | 1,503 | 243,397 | 7 | 80.3 |

The assisted worker also sent the coordinator a status message and received no review assistance in return. That call is included in worker usage and excluded from the grouped local tool count. Both finished inside three minutes. The six-call ceiling was a prompt instruction and the assisted worker exceeded it by one. Retain both assigned runs. The costs are final cumulative runtime counters, with repeated context included in input. Reasoning output is a subset of output and is already included in the total. Together the workers used 451,555 logical tokens. The table excludes packer, preflight and coordinator usage.

## Verified findings and false positives

| Oracle | Control | Assisted |
| --- | --- | --- |
| Transitive restore belongs to the wrong project | Traced the board's one-project restore check and the selected graph to v2 `Tariff`. Measured `project.restore.projectPath` naming v1 and observed `D17` from the saved executable. | No finding. Neither planner call reached the requested project. |
| Selected runtime input missing from the reuse key | Joined `Route=branch` to `assets/regional.json`, checked staged 17 versus approved 16, and cited the manifest omission. | No finding. Cited `Reusable` and `Complete` without checking the selected file against the manifest. |
| Superseded qualification clears the gate | Compared admitted and live epochs despite matching revision and Git, cited the board's missing epoch check. | Same finding with both exact epochs. Independently reproduced the board clearing Delta and identified the missing check in `Cleared`. |
| Coverage hit belongs to a different target | Compared Delta's `release-net10` request with the `compat-net10` archive it actually consumes and checked the shipping XML's explicit zero. | No finding. Cited the separate shipping archive's target without checking which archive Delta consumes or its line result. |

Under the declared rubric the control made two additional unsupported blocking assertions. It required the qualification suite's ID to equal the requested coverage target. The brief uses the unit qualification to verify the workspace incarnation and the separate coverage target to attribute a line hit. The current Ember receipt also uses `unit-net10` while its line target is `compat-net10`. The control also required a passed coverage run, which would reject Ember’s valid attributed hit from a completed failed run. Counting the target-name observation as a scope ambiguity gives one definite control false positive. The assisted worker made no unsupported blocking assertions. Its missed or falsely cleared cases are counted in detections.

## Tool use and interpretation

- In qualification, the pinned inspector returned `IncompatibleAssets` for Delta’s selected graph, reported a valid Delta receipt bound to a different live fence, and reported `Unknown` for the compatibility hit queried against the shipping target. The valid compatibility hit was `Observed` and the explicit shipping zero was `Uncovered`.
- The assisted worker's first `plan` call used a nonexistent solution path. The second prepended `dispatch/` to the project path when the inspector expected a path relative to that directory. The run and coverage calls supplied paths where the inspector expected record IDs. The inspector threw generic exceptions for these calls. The receipt finding came from the raw JSON. The successful `reuse` command compared the declared scope and returned a green result with `catalog/index.json` as its only runtime asset. The missing file is outside that comparison's evidence.
- This pair tested the provided interaction as assigned, including input conventions and how the reviewer interpreted successful output. The stock CLI does not expose these checks. The separate inspector calls current components, with archived test and coverage records and a live-fence comparison in the wrapper. No automatic scheduler or new native VSTest attempt ran in the pair.
- Do not scale this setup. The control reached the seeded detection ceiling, the assisted run exceeded the call budget, and broadening the sample would replicate a poorly discoverable command interface. A new development dossier should first verify that a worker can choose a target, submit an ordinary dossier-relative path, and distinguish a green scoped comparison from a complete release decision. Keep the shipping contract explicit about separately named qualification and coverage scopes. Retire this dossier from any later confirmatory sample.

## Integrity

The product was current main at `99965e2ff9b524aaa5dfa2e51a1876d94ea23801`. The frozen author code and scoring protocol were committed at `e24fcb4` before dispatch. The exact Windows .NET SDK was 10.0.201. The offline replay showed the shipped board clearing Delta. Each of four independently repaired boards held Delta while all held the paused lane, accepted Ember and passed the smoke checks. Product component probes also passed. That establishes the four seeded decisions independently of the reviews.

| Frozen item | SHA-256 |
| --- | --- |
| Norchester snapshot, 197 entries including 137 files | `9ED9FEFEEA4F099EE69E6F2ED0F98144C29BAE727ADC9C8D63E6AED71A22C9A9` |
| Inspector payload tree | `D040ADEE0964EE66DBA8F5721731A6407EA7C325272165474830EE895D0E37BA` |
| Inspector assembly | `82A08C0D3E98A899C4896C04AF6DD9762F3006F2F9BD6AFD46AC85989BDF902B` |
| SlopHawk daemon assembly used by the inspector | `C0DF101C5AF1FC5565029DAE9871909D6805F11A21A755D490C76E340B271145` |

The pre/post per-entry manifests matched exactly for both the dossier and the inspector tree. Both command traces stayed within the dossier, permitted scratch and the assigned inspector. The worker contexts had no prior turns and received neither the answer key nor each other's review. The subjects could tell which tools they had. The coordinator scored the final answers against the frozen rubric with access to the assignment and audit trail.

The assignment was frozen at `2026-11-16T11:50:46.706Z`. Control started first, then assisted 9.315 seconds later. The worker sessions were `01a1e432-5a49-79c0-a43d-68a586c8c62e` and `01a1e432-7f0c-7431-9aa3-9edf2d78c056`, respectively. Each runtime recorded a single turn at `gpt-6-luna` with `high` effort. The results describe this paired dossier only.

Both benchmark tools and the SlopHawk solution built with zero warnings and errors. The solution test suite passed 376 tests and skipped two environment-dependent tests.
