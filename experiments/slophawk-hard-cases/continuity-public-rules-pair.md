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
