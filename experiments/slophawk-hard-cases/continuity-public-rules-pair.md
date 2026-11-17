# Continuity pair with executable public rules

This pair uses the original Continuity operations project, which has 132 seeded behavioral defects and 26 correct operations. It follows the [earlier assisted trials](public-contract-search-results.md) and the [separate Blue Harbor holdout](blue-harbor-holdout-results.md). The checker had already been developed and evaluated on Continuity, so this is a matched rerun on the development corpus.

## Protocol

Two fresh `gpt-6-luna` sessions at low reasoning effort received the [same short flag-only prompt](foreground-check-results.md) and byte-identical copies of the candidate source and assembly, public prose and executable rules, scope, and published examples. All 158 executable rules qualified against the ordinary examples. Each session had one turn, without follow-ups or a tool-call cap. Randomization put the unassisted session first. The assisted workspace added the SlopHawk checker, its skill, and a manifest entry listing its commands. Both workers could run the candidate's capture, batch, and smoke commands.

Each session ran in a separate read-only Windows MXC workspace with writable scratch and checker output. Preflight denied reading the private oracle, corrected assembly, other worker's manifest, and development repository instructions. It also denied opening the candidate source for writing. Both workspaces passed the 316-case smoke suite. The host confirmed identical path, type, and hash for all 210 public entries across workspaces and verified both fingerprints after completion. Saved turn metadata confirmed the exact prompt, model, effort, and working directory for each session.

## Results

A verified defect required a supported input, exact reported outputs on replay against the distributed and corrected assemblies, and agreement between the public rule and corrected output. A false positive was a reported discrepancy on a valid input where the assemblies agreed. Total tokens are runtime-reported input plus output, including cached input. Uncached input excludes output.

| Arm | Submitted | Verified of 132 | False positives | Total tokens | Uncached input | Shell calls | Wall time |
|---|---:|---:|---:|---:|---:|---:|---:|
| With SlopHawk | 132 | 132 | 0 | 678,588 | 75,060 | 15 | 93.0 s |
| Without SlopHawk | 0 | 0 | 0 | 369,447 | 41,112 | 11 | 77.1 s |

The assisted worker ran `check-contracts` once. It produced 132 discrepancies in 90,613 supported comparisons over 21.3 seconds. A separate batch retest and scoped review recorded 132 acknowledgments, zero missing coverage, and zero outstanding findings. The submitted explanations identified the violated public rules. Scoring verified the behavioral discrepancies, including the applicability of the public rules to each input.

The unassisted worker confirmed the 316-case smoke suite and ran 158 published inputs through the candidate's capture and batch commands. All passed. It submitted an empty findings array after running the published inputs. This run measures what the worker found in one session under the shared prompt.

The assisted session used 309,141 more total tokens and 33,948 more uncached input tokens in this pair. The unassisted worker used 326,016 cached input and 2,319 output tokens. The assisted worker used 601,472 cached input and 2,056 output tokens.

| Arm | Session |
|---|---|
| With SlopHawk | `01a1e84b-a230-78a1-a532-30aa2c9d83a6` |
| Without SlopHawk | `01a1e849-fc05-7360-92f6-9f0e86121638` |

The benchmark pinned main at `7fcac90545c5ce9c5809ea6b482514fcc8f3958f` and used the same runtime code as `157f108`. The candidate assembly SHA-256 was `6094EEB443F1CD4E98FBD3287E6BB6DB1F5887CB8995DEFCF31B43B5DC508FE3`. The shared executable rules SHA-256 was `AFFB5F36BA3AAA993AE5B9C8EC76CA5EDFFFAE8B6769FDD76F0941935329B7DB`. The local coordination bundle `continuity-pair-0a21371ad` retains the frozen payload, session transcripts, isolation receipts, and adjudication.

## Rerun with a target of 132

Two new Luna 6 Low sessions used the same candidate, executable public rules, permissions, and scoring. The prompt gave both workers the defect count and asked them to continue within the single turn until all 132 were reproduced. Calls were uncapped. The randomized order put the assisted worker first. The sandbox preflights again passed the 316-case smoke suite and denied access to the oracle, corrected assembly, other worker, and both prior answer files. The candidate fingerprints matched before and after both sessions. Main was pinned at `68289fe` with the same checker runtime as the first pair.

> Review the solution listed in `workspace.json`. It contains 132 distinct behavioral defects. Continue in this single turn until you have found and reproduced all 132. Tool calls are uncapped. Leave the solution unchanged.
>
> Use the public rules to derive expected results and generate new inputs beyond the published examples. Run any checker listed in `workspace.json` and retest its findings. For each distinct defect, execute a valid input that demonstrates the discrepancy and verify the expected result against the public rules. Count each defect once.
>
> Save `out/answer.json` with a `Findings` array containing `Operation`, `Args`, `Actual`, `Expected`, and `Cause` for each verified defect.

| Arm | Submitted findings | Verified of 132 | Submitted false positives | Total tokens | Uncached input | Shell calls | Wall time |
|---|---:|---:|---:|---:|---:|---:|---:|
| With SlopHawk | 132 | 132 | 0 | 338,389 | 82,770 | 11 | 81.5 s |
| Without SlopHawk | No submission | 0 | 0 | 549,748 | 39,992 | 16 | 98.1 s |

The assisted worker used SlopHawk to run 90,613 supported comparisons in 20.8 seconds. It retested all 132 findings in a separate batch and recorded zero outstanding findings and zero missing operations. Host replay confirmed all 132 results against the distributed candidate, the corrected assembly, and the public rules.

The unassisted worker replayed all 158 published cases successfully. Its attempt to run a new harness with Node failed when Node tried to resolve the drive root under the sandbox. It ended the turn without additional verified inputs or an answer file. In its final message it also claimed the documented smoke command failed. Its command supplied the normalized `public-examples.json` file. The documented smoke command supplies `standard.json` and passed in both preflights. The prose claim is recorded separately from submitted behavioral findings.

The assisted session used 211,359 fewer total tokens and 42,778 more uncached input tokens. Its input included 253,824 cached tokens and it generated 1,795 output tokens. The unassisted session used 506,624 cached input and 3,132 output tokens. This measures the two workflows as exercised in the bounded Windows environment.

| Arm | Session |
|---|---|
| With SlopHawk | `01a1e854-4ac3-7613-bc90-4d0621fda91a` |
| Without SlopHawk | `01a1e855-bdd9-7853-9a0b-3cfdc1ea2c21` |

The local coordination bundle `continuity-pair-target-0a2596d011` retains the frozen plan, native session and timing records, preflight receipts, and adjudication.

## Rerun after fixing Node script loading

The previous unassisted worker's Node process failed when resolving the drive root. With default Node options, preflight reproduced that failure in both fresh sandboxes. Both worker launches set `NODE_OPTIONS="--preserve-symlinks-main --preserve-symlinks"` for the launcher and through `shell_environment_policy.set.NODE_OPTIONS`. Preflight then loaded a script and a sibling module and read all 158 public rules. Node and PowerShell still denied protected reads and opening candidate source for writing. Node still could not read drive-root metadata. The preflights passed the 316-case smoke suite.

Two fresh Luna 6 Low sessions received the exact prompt above, the same public rules and candidate, and one turn with uncapped tool calls. The oracle, corrected build, and earlier answers stayed outside the workers' sandboxes. Candidate fingerprints matched before and after each run. Randomization put the unassisted worker first. This pair pinned main at `97d5672` with the same checker runtime as the earlier pairs.

| Arm | Submitted | Verified of 132 | False positives | Wrong expected values | Total tokens | Uncached input | Shell calls | Wall time |
|---|---:|---:|---:|---:|---:|---:|---:|---:|
| With SlopHawk | 132 | 132 | 0 | 0 | 403,470 | 37,756 | 13 | 102.7 s |
| Without SlopHawk | 9 | 5 | 0 | 4 | 2,341,358 | 82,273 | 43 | 308.3 s |

The assisted worker ran 90,613 supported comparisons in 21.0 seconds. It separately retested all 132 findings, and the scoped review recorded zero missing operations and zero outstanding findings. Independent replay confirmed every submission.

The unassisted worker used PowerShell and the candidate's batch runner. All nine reports targeted real discrepancies. Five gave exact actual and expected values. Four gave the correct observed result and the wrong expected result, so they failed the exact-reproduction criterion. No worker command invoked Node in the unassisted session. This pair shows how the workflows performed with Node available in both sandboxes.

The assisted session used 1,937,888 fewer total tokens and 44,517 fewer uncached input tokens. The runtime recorded 363,008 cached input and 2,706 output tokens with SlopHawk, and 2,247,936 cached input and 11,149 output tokens without it.

| Arm | Session |
|---|---|
| With SlopHawk | `01a1e85f-07c1-71c0-ab00-afa4a6a86069` |
| Without SlopHawk | `01a1e85a-46ce-7741-b166-b68e19ca78bb` |

The local coordination bundle `continuity-pair-node-cff269b458` retains the frozen plan, Node failure and success receipts, native session records, and adjudication.

## Generality audit

Continuity was used to develop the search heuristics. Its 132 defects span several kinds of numeric business behavior, and all operations use the same managed integer batch interface. It remains a development corpus. The [Blue Harbor paired holdout](blue-harbor-holdout-results.md) provides separate worker evidence on 18 fresh defects.

The implementation uses public preconditions, expected-value formulas, and passing examples to generate and evaluate inputs. It probes nearby numeric boundaries, equalities, and tuples that can collide under arithmetic encodings. It also extracts integer literals from matching managed methods to guide input selection. Method names and literals affect the search. Each finding's expected value comes from the public rule. The [generator](../../src/SlopHawk.Core/ContractCases.cs), [checker](../../src/SlopHawk.Cli/ContractCheck.cs), and [literal reader](../../src/SlopHawk.Core/ManagedBodyComparison.cs) use the supplied operation names. None references an operation from either benchmark, a defect count, or a private witness.

Thirty-two of Continuity's 158 rules had optional `Basis` entries containing 142 hint values. Removing every entry while preserving the formulas, legal domains, published examples, and candidate yielded 132 independently replayed defects in 90,262 comparisons. That run used the original 2,048-case budget per operation.

A new [water-utility fixture](Generality/Fixture.cs.fixture) exercised rounding, signed modulo, bit masks, overflow, ties, interval boundaries, tuple collisions, and faults triggered by several conditions together. Its 22 public rules included zero `Basis` entries. The candidate, the method-renamed variant, and the corrected reference agreed on all 22 ordinary examples. The 16 private witnesses were qualified against the public rules and reproduced a defect in both candidate variants before the checker ran. Six operations were correct. The method-renamed variant changed internal method identifiers while preserving the public operation IDs and the implementations.

| Frozen checker run | Budget per operation | Verified defects | False positives | Comparisons |
|---|---:|---:|---:|---:|
| Continuity without input hints | 2,048 | 132/132 | 0 | 90,262 |
| Fresh fixture | 512 | 12/16 | 0 | 6,467 |
| Fresh fixture, renamed methods | 512 | 11/16 | 0 | 6,466 |
| Fresh fixture | 2,048 | 13/16 | 0 | 21,173 |
| Fresh fixture, renamed methods | 2,048 | 12/16 | 0 | 22,964 |
| Corrected fresh fixture | 2,048 | 0/0 | 0 | 44,370 |

Independent replay checked the actual and expected values of every flag. The 2,048-case search missed `LedgerCarry`, `SluiceCap`, and `IsolationSequence`. These require combinations of derived arithmetic conditions or three simultaneous equalities. Renaming methods also hid the `IndependentTickets` tuple collision from the search. The engine uses portable heuristics within its integer contract model. Coverage on fresh code depends on the search budget and the conditions needed to activate each defect.

The audit pinned main at `46a219c` and ran the checker build used in the prior benchmark. Its CLI assembly SHA-256 is `93C91AA75860A4888DCF62DF44CF02B8A5C73151808FA863F505335DF4AD19C6`. The fresh rules SHA-256 is `8B5BEFBF4694EC7A6D9454A5BE8A332D55521C138A8E9DD6C534E96BC896735D`. The checker received only the relevant assembly, public rules, ordinary examples, and an output directory. The author kept the reference and private witnesses for scoring. Neither the runtime nor the skill changed.

To regenerate the fixture, create an external directory with `author`, `public`, and `private` subdirectories. Create a .NET 10 console project named `Fixture` in `author` and replace `Program.cs` with the saved fixture. Run it with the external root as its only argument and build the three generated projects under `private`. Run the reference's `batch` command on `public/requests.json` to obtain the published expected values in request order. Use the request and expected values as `published.json` and run `check-contracts` with the desired budget and the `batch` adapter. Replay flags and private witnesses against both builds. The receipts and adjudication are retained locally in `slophawk-generality-124fbefc89`.
