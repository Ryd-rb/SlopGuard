# Blinded Luna 6 Low audit with the foreground checker

Two pairs of fresh Codex CLI chats audited the same Continuity candidate with 132 seeded defects and 26 controls. Every chat used `gpt-6-luna` at low reasoning effort and received the same short prompt. Call counts were uncapped. The order draw put the assisted chat first in pair 1 and second in pair 2.

> Review the solution listed in workspace.json. Accurately flag its behavioral issues without editing it. Run any listed checker and use its results. Save out/answer.json with a Findings array containing Operation, Args, Actual, Expected, and Cause for each reproducible issue. Tool calls are uncapped.

Each chat received the candidate source and runnable assembly, the public contracts, and the ordinary examples. The assisted chats also received the SlopHawk executable from `a4ee76b81bd8beff8dddc2d166a79b03bce496fc`, its skill, and the command syntax. The tool manifest required an actual comparison on a worker-authored case outside the ordinary examples. It supplied no search recipe or expected outcomes.

Each chat had its own Windows MXC sandbox outside the SlopHawk repository. The profile allowed reads in the assigned directory and platform runtime paths, writes in its own scratch directory, and disabled shell networking. The sandbox preflight blocked reads of the private oracle, repaired assembly, and both SlopHawk checkouts. It also blocked a candidate-source write and allowed the ordinary smoke replay, which passed all 316 cases. The foreground checker ran both application stages and returned the expected discrepancy on a separate preflight case. Chat metadata confirmed the exact prompt, model, effort, and assigned directory. All four candidate fingerprints remained unchanged. Private replay began after the chats finished.

## Results

Each submitted flag was replayed against the candidate and the private corrected implementation. A verified flag required supported inputs, exact actual and expected outputs, and the correct cause. Correctly localized causes include verified flags and genuine defects reported with a bad example. Rejected examples include false positives. A rejected example counted as a false positive when its claimed defect was unsupported by both the replay and the stated cause.

| Pair | Access | Submitted | Verified | Correctly localized | Rejected examples | False positives | Total tokens | Uncached input tokens | Shell calls |
|---:|---|---:|---:|---:|---:|---:|---:|---:|---:|
| 1 | SlopHawk | 2 | 0 | 1 | 2 | 1 | 268,455 | 61,577 | 10 |
| 1 | Ordinary | 6 | 5 | 6 | 1 | 0 | 620,126 | 52,988 | 16 |
| 2 | Ordinary | 3 | 0 | 1 | 3 | 2 | 466,743 | 65,547 | 15 |
| 2 | SlopHawk | 0 | 0 | 0 | 0 | 0 | 183,847 | 26,479 | 8 |

Ordinary runs verified five distinct defects out of 132. Assisted runs verified zero. The paired differences were −5 and 0 verified flags for SlopHawk. Assisted runs used 452,302 total tokens and ordinary runs used 1,086,869, a 58.4% reduction in this sample. Total tokens sum input and output across model requests, including cached input. Uncached input was 88,056 tokens assisted and 118,535 ordinary. Both assisted runs ended after fewer shell calls. The two pairs establish the observed difference on this candidate. A broader claim about detection or cost would require more independent pairs and projects.

The pair 1 control verified `ChooseSettlementState`, `ReplayableHistoryStart`, `RecoverTransferReserve`, `RecoveredWindowNet`, and `RestoredExcerptBytes`. It localized `QuietOverlap` with an incorrect overlap count. The pair 2 control localized `ShiftEnd` but reported the wrong expected output. Its other two flags claimed failures on cases where the results were correct.

## Checker execution

| Pair | Completed comparisons | Worker-authored cases outside ordinary examples | Reported discrepancies |
|---:|---:|---:|---:|
| 1 | 13 | 2 | 2 |
| 2 | 1 | 1 | 0 |

Both assisted runs produced native command receipts and preserved the checker’s request and observed-result artifacts. The pair 1 worker's attempt to copy the report to its requested filename failed. The session log captured the original checker JSON. Its two mismatches came from the worker entering incorrect expectations for `LeaseRenewalDelay` and `EscalationLevel`. The worker identified the retry-wrap cause in `LeaseRenewalDelay` but submitted an example that produced the correct answer. Its final reply acknowledged that both submitted values conflicted with the public contract. The pair 2 worker tried `RefundCap` outside the ordinary examples and got the expected value. The checker establishes what the candidate returned for a given input. The worker must still derive the expected answer from the contract.

| Pair | Access | Codex session |
|---:|---|---|
| 1 | SlopHawk | `01a1e502-6568-7bc1-80ee-cc69d1d1e769` |
| 1 | Ordinary | `01a1e502-655e-7d01-a9d2-9f6ac15f0c7e` |
| 2 | Ordinary | `01a1e502-6568-7242-9fda-359fe86a76f0` |
| 2 | SlopHawk | `01a1e502-6567-7b83-93cb-5f3b6a9de428` |
