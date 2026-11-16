# Isolated Luna 6 Low issue audit

This audit reused the 132 seeded operation implementations and 26 controls from Continuity. It supplied the candidate source, a runnable candidate binary, public contracts, routing documentation, and ordinary checks. The compiled repaired release, repaired source, private input witnesses, prior results, and grading ledgers stayed outside the workers' permitted directories. The coordinator independently confirmed that the 132 private inputs still distinguished the exact public candidate and private reference. The staged candidate passed all 316 public checks.

Four independent Codex CLI chat sessions started at `gpt-6-luna` with low reasoning effort. They ran in four separate directories outside SlopHawk. The randomized order was SlopHawk access first in pair 1 and ordinary access first in pair 2. Each audit used the exact same prompt.

> Review the solution listed in workspace.json. Accurately flag its behavioral issues without editing it. Run any listed checker and use its results. Save out/answer.json with a Findings array containing Operation, Args, Actual, Expected, and Cause for each reproducible issue. Tool calls are uncapped.

The only treatment difference was the presence of SlopHawk at main `cc63041918bfa52958071cfe06c6a89c9f478d05` and its skill in the assigned tool manifest. The workers received no suggested probe count, search procedure, seed count, peer findings or benchmark answer. The brief required flagging and left the source read-only. Earlier repair attempts were interrupted before the flagging protocol and are excluded from this experiment.

## Access boundary

Each fresh session used a Windows MXC permission profile that allowed reads within its assigned directory, writes in its own scratch, and access to platform runtime files. Shell networking was disabled. The CLI launched with user configuration, apps, plugins, web search, memories, and delegation disabled. A separate Codex canary could read its assigned file and received a denial on a sibling file. A shell preflight under the same profile denied reads to four actual private artifacts, denied writing the public source, and allowed a scratch write and the public smoke replay. The staged solutions matched in file contents before dispatch.

The saved turn contexts confirmed the assigned working directory, model, low effort, approval policy `never`, and exact prompt text for every session. All four solution fingerprints matched their own frozen values afterward. The full tool outputs and scratch evidence remain outside the repository in the benchmark directory. [Codex filesystem and network permission settings](https://learn.chatgpt.com/docs/config-file/config-reference)

## Results

A verified flag requires a correct root cause and a supported input with independently replayed actual and expected values. A localized cause names a real fault even when its example misses the trigger or predicts the wrong value. A false positive is an asserted additional defect disproved by replay. The table also counts rejected reproductions of genuine defects so the false-positive column does not hide bad examples.

| Pair | Access | Verified flags | Correctly localized causes | Rejected reproductions | False positives | Total tokens | Uncached input | Commands |
|---:|---|---:|---:|---:|---:|---:|---:|---:|
| 1 | SlopHawk | 2 / 132 | 3 | 1 | 0 | 416,814 | 36,373 | 12 |
| 1 | Ordinary | 0 / 132 | 1 | 1 | 0 | 342,360 | 62,773 | 13 |
| 2 | Ordinary | 5 / 132 | 9 | 4 | 0 | 544,394 | 67,973 | 14 |
| 2 | SlopHawk | 6 / 132 | 8 | 2 | 0 | 6,642,065 | 111,092 | 96 |

The SlopHawk-access runs verified eight distinct defects using 7,058,879 total tokens, including 147,465 uncached input tokens. Ordinary runs verified five distinct defects using 886,754 total tokens, including 130,746 uncached input tokens. Both arms found `ConsolidatedEnd`. The four sessions together verified 12 distinct defects. The paired differences were +2 and +1 verified flags for access, with one assisted run accounting for most of the extra work.

Both assisted workers invoked the probe planner with a limit of one input per operation. It returned the ordinary examples. The sandbox blocked daemon startup and inventory reads because the daemon tried to access the Windows drive root. One worker also invoked `probes compare` with binary paths in place of result files, and the command failed. Neither assisted worker completed a SlopHawk behavioral comparison. The verified flags came from reviewing the public contracts and implementation and independently replaying candidate inputs. This experiment compares access under the verified isolation boundary. The paired differences do not establish that the checker found the extra bugs.

The original program was preserved. This was a flagging audit, and its isolation excludes the predecessor and the supplied search procedure from the earlier 132-of-132 replay demonstration.

| Run | Codex session |
|---:|---|
| 621 | `01a1e4e1-96a4-7c83-a7da-72cc05d4ba9b` |
| 622 | `01a1e4e1-96a4-7c03-9fa8-2ec317c3c04f` |
| 623 | `01a1e4e1-96a9-7373-86d6-1822b179a6b6` |
| 624 | `01a1e4e1-96a3-7f71-951b-e9ddc04d9e52` |
