# Scalar path pilot result

On November 17, 2026, the bounded path checker found six of six independently qualified return-value defects in a synthetic terminal-policy workload. Graph-off found five. All three clean controls remained clean in each arm.

| Measurement | Graph off | Graph on |
|---|---:|---:|
| Seeded defects verified | 5 / 6 | 6 / 6 |
| Reproduced witnesses accepted | 10 | 29 |
| False positives | 0 | 0 |
| Distinct cases compared | 2,016 | 2,016 |
| Confirmation executions | 10 | 29 |
| Scalar model evaluations | 0 | 969 |
| Total measured planning and check wall time | 14.542 s | 16.861 s |

The extra defect was a thermal-release boundary error under an authorized override. Two graph-generated inputs exposed it, and neither input appeared in the matched graph-off stream. Both reproduced on the original candidate. The independent evaluator verified each reported input, actual and expected return against the corrected build and a separately implemented public-policy oracle. It also replayed all 4,032 initial observations. The graph associations are model predictions.

## Fixed comparison

The product implementation was frozen at `ceb0dd844eb3c702db5269c11fdbed68e921fb82` before opening the separately authored workload. Both workload patches passed indexed replay on their assigned base, `acf3874fa4d12b3671e2b2a0fa9cb8ea9ba29cc7`. The candidate and public files were copied into a neutral workspace. The private evaluator and its authored inputs stayed in separate scratch. Both modes used the same candidate DLL, rule and example hashes, method targets, seed, and hashed baseline probe streams, with independent run directories.

The cap was 240 distinct search cases plus 16 confirmation slots per operation, 128 model evaluations per graph-enabled operation, and a 60-second planning and execution budget per operation. Eight methods used the full search allocation. The ninth had 96 admitted cases and both arms exhausted that domain. Every run completed under four seconds. The wall time includes command startup, planning, and separate same-build retests. Private scoring occurred afterward and outside the measured search budget.

The workload qualified 45 ordinary examples, 11 private witnesses across six seeded families, 26 admitted neighbors, and 112 clean comparisons. Each family had one fixed candidate variant. The private draw selected a qualification witness per family. The checker received the candidate, public rules and ordinary examples. A separate smoke run of the shipped `check-contracts` command found the same five seeded operations as graph-off.

This single-workload result meets the scalar gate. It measures controlled .NET 10 return comparisons. Stateful checks, intent solicitation, and reviewer sessions have yet to be measured. This run used no model tokens.
