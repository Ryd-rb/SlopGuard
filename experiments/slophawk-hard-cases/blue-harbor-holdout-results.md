# Blue Harbor holdout pair

Blue Harbor is a new cold-chain freight application with 24 operations across dispatch, equipment condition, inventory, and settlement. It contains 18 seeded behavioral defects and six correct operations. Both builds passed all 24 ordinary examples. Each private witness was in the public domain and distinguished the candidate from the corrected reference. An additional 11,259 generated comparisons agreed between the executable public rules and the corrected reference.

## Protocol

Two fresh `gpt-6-luna` sessions at low reasoning effort received the [same short flag-only prompt](foreground-check-results.md) and byte-identical solution packages, including the candidate source and assembly, public prose and executable rules, and ordinary examples. Each session ended after its first turn and had uncapped tool calls. The randomized order put the SlopHawk session first. SlopHawk at `157f108`, its skill, and its manifest entry were the treatment. Both workers had access to the candidate's ordinary smoke and batch commands.

Each worker ran in its own external Windows MXC sandbox. Preflight in both workspaces denied reads of the private witnesses, corrected assembly, sibling workspace manifest, and repository instructions. It also denied opening the candidate source for writing and passed the ordinary smoke command. The host checked both candidate fingerprints after completion. Saved turn metadata confirmed the two distinct sessions, model, effort, working directories, exact prompt, and approval policy.

## Results

A verified defect required a valid submitted input, distinct candidate and reference outputs, and exact reported values. The two false-positive submissions asserted discrepancies on valid inputs where replay produced identical values. The wrong-number column counts a real discrepancy submitted with an incorrect expected result. Total tokens are cached plus uncached input and output as reported by the runtime.

| Arm | Submitted | Verified of 18 | False positives | Wrong-number examples | Total tokens | Uncached input | Shell calls | Wall time |
|---|---:|---:|---:|---:|---:|---:|---:|---:|
| With SlopHawk | 18 | 18 | 0 | 0 | 362,518 | 48,270 | 10 | 95.0 s |
| Without SlopHawk | 12 | 9 | 2 | 1 | 170,192 | 68,572 | 7 | 68.2 s |

The assisted worker ran 13,236 comparisons in 1.35 seconds through `check-contracts`. The separate batch retest and scoped review recorded 18 acknowledgments, zero missing operations, and zero outstanding findings. The ordinary worker used targeted native batch replays. Its `SensorConsensus` witness confirmed the tie defect, while its explanation overstated when sensor C replaces the current selection. Its two false-positive examples targeted `DockBooking` and `InventoryReceiptWindow`. Its `FuelSurcharge` example replayed the fault but claimed an expected value of 22 where the contract gives 18.

In this pair SlopHawk access yielded nine more verified defects, 192,326 more total tokens, and 20,302 fewer uncached input tokens. The observed differences are from one paired holdout.

| Arm | Session |
|---|---|
| With SlopHawk | `01a1e842-b60a-71f2-869c-ac32d9369469` |
| Without SlopHawk | `01a1e844-4f8c-78c1-a82e-5046f69249b4` |

The candidate assembly SHA-256 was `5C6B198BB9E0A91DD501D2FA318FF278109F705DE51882F6906371AC424B7686`. The shared executable rules SHA-256 was `3BC92A6CF56A5E20FDBA485AD26AE8D32698CE9A211974E5CFB6E06CB36033C2`. The local run bundle under `holdout-blueharbor-4a50433f4b` retains the frozen payload, native session records, and private replay evidence.
