# Single-turn search from executable public contracts

A separate translation session received the prose contracts and the published ordinary examples. Its sandbox contained only those inputs and writable output. It produced executable domain and expectation rules for 158 operations. All 158 rules reproduced the published examples before launch. SlopHawk used the rules and ordinary inputs to generate valid candidate requests and calculate expectations. The rules contained formulas and preconditions. The repaired implementation, private witnesses, defect labels, and scores stayed outside both the translation and benchmark sandboxes.

Each benchmark trial used a fresh `gpt-6-luna` session at low reasoning effort, the [unchanged flag-only prompt](foreground-check-results.md), uncapped tool calls, and one turn. The sandbox contained the unchanged runnable candidate, its public project files, the SlopHawk build, and the executable public rules. The measured outcome was the number of distinct seeded defects with independently replayed actual and expected results on a supported input. The worker cause fields identified the applicable rule. The measurements concern behavioral reproduction.

| Trial | Checker commit | Search comparisons | Checker time | Submitted | Exact verified of 132 | Rejected | Worker tokens | Uncached input | Worker time |
|---:|---|---:|---:|---:|---:|---:|---:|---:|---:|
| 1 | `b2c60e1` | 62,097 | 31.6 s | 131 | 130 | 1 | 330,353 | 27,695 | 102.8 s |
| 2 | `6f3974c` | 137,302 | 60.9 s | 131 | 131 | 0 | 609,972 | 44,568 | 133.8 s |
| 3 | `d6a1d42` | 137,313 | 60.5 s | 16 | 15 | 1 | 1,742,530 | 86,512 | 223.8 s |
| 4 | `3222511` | Timed out | 30 s limit | 0 | 0 | 0 | 232,864 | 32,960 | 67.9 s |
| 5 | `97d274d` | 90,613 | 20.9 s | 132 | 132 | 0 | 208,432 | 42,787 | 65.5 s |

Trial 1 completed the scoped checker review and retested all 131 reported comparisons. The rejected input placed the voucher submission before the order. The next trial treated the public rule's submission ordering as a domain condition and requalified all examples. The checker also began probing both sides of numeric boundaries appearing in every contract. Trial 2 ran the search twice and separately retested its 131 submitted findings. The table reports the first search's comparisons and time. The worker time and tokens include the repeat search.

A second author re-derived the public rules for the 27 operations on which the second search reported agreement. The author received the public prose, ordinary examples, and rules for those operations. The author proposed zero corrections. Trial 3 extended the input search using numeric constants from matching candidate methods. The checker still produced 131 exact witnesses. The worker submitted a subset and left 120 checker findings outstanding.

The next revision included constants in compiler-generated local functions. An isolated preflight recovered the remaining disagreement. Trial 4's shell stopped the full search at its selected 30-second timeout, so it submitted only the result of the passing public examples. SlopHawk then increased the number of comparisons per adapter process and added inputs that exercise linked equality conditions together. A 2,048-case per-operation budget produced 90,613 actual comparisons and 132 independently replayable witnesses in 21.7 seconds in a checker-only sandbox preflight. That preflight's reports stayed outside the fresh worker workspace.

Trial 5 used the same 2,048-case budget, one checker search, one batch of exact retests, and eight shell calls in total. The worker submitted all 132 exact independently verified defects with zero rejected claims or false positives. The scoped review recorded 132 acknowledgments, zero missing coverage, and zero outstanding findings. The candidate fingerprints matched before and after. The session was `01a1e6e2-23d1-7470-af1e-49dcf9b7229f`.

The first translation used 822,217 logical tokens and 71,951 uncached input tokens. The independent audit used 350,842 logical tokens and 43,519 uncached input tokens. Those sessions are separate from the worker figures. The rule file hashes were `D9A7CC04C65163C63513F39C9F7174D9718E4EC0750730FAEDA07D79CA4BEA32` and `AFFB5F36BA3AAA993AE5B9C8EC76CA5EDFFFAE8B6769FDD76F0941935329B7DB`.
