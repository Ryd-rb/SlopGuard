# Production-inspired Luna Low results

The three fresh Luna 6 Low pairs reviewed local adaptations of two production incidents and one public benchmark case. The unassisted workers verified 3/3 blockers. The SlopHawk-assisted workers verified 2/3. The public profiles and real smoke receipts passed in all three projects. Each author-only trigger reproduced a distinct behavioral failure, and an isolated correction changed that outcome while preserving both public profiles and smoke. See the [frozen protocol and source mapping](incident-low.md).

| Project | Access | Verified | False positives | Input | Cached | Uncached | Output | Reasoning within output | Total | Local calls | Seconds |
| --- | --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| Trestle | Assisted | 1/1 | 1 | 210,797 | 192,256 | 18,541 | 2,364 | 265 | 213,161 | 7 | 54.9 |
| Trestle | Unassisted | 1/1 | 1 | 208,215 | 194,432 | 13,783 | 2,041 | 0 | 210,256 | 8 | 45.6 |
| Fallow | Unassisted | 1/1 | 0 | 153,610 | 145,792 | 7,818 | 1,393 | 0 | 155,003 | 6 | 37.2 |
| Fallow | Assisted | 1/1 | 0 | 152,817 | 139,904 | 12,913 | 1,332 | 0 | 154,149 | 6 | 32.7 |
| Marlin | Assisted | 0/1 | 1 | 200,903 | 166,400 | 34,503 | 1,490 | 0 | 202,393 | 8 | 37.2 |
| Marlin | Unassisted | 1/1 | 0 | 254,956 | 238,208 | 16,748 | 2,608 | 0 | 257,564 | 9 | 62.2 |

The assisted arm used 569,703 total tokens versus 622,823 unassisted, a reduction of 53,120 (8.5%). It used 65,957 uncached input tokens versus 38,349 unassisted, an increase of 27,608 (72.0%). The differences are descriptive. The only large total-token reduction was on Marlin, where the assisted worker missed the seeded issue. Tool-call counts were uncapped. The Trestle assisted worker and Marlin unassisted worker each sent one unrequested status message to the coordinator, contrary to their prompts. Neither received evidence or guidance in reply. Retain both runs, include those sends in runtime tokens, and count only local calls in the table. All workers finished within the four-minute instruction.

## Verified behavior and scoring

| Project | Observed findings |
| --- | --- |
| Trestle | Both found that after the first compare-exchange fails, the retry uses a new revision without rechecking generation. The unassisted worker reproduced the author trigger, `plan=24;routes=`, after the newer install and compaction. The assisted worker changed the order and resurrected plan 24 after compaction, `plan=24;routes=B1`. That also violates the generation invariant, and an independent replay matched. Both called the current profile blocked even though its supplied ordering installs plan 25 with all routes. Count one standing-profile false positive per arm when scoring the supplied profile. If their statements are read as warnings about a different legal future schedule, the Trestle count is zero for both arms. |
| Fallow | Both traced why the first lookup selects positions in the active catalog while the second lookup uses the stream and ordinal alone. Both gave a valid one-extra-column repro. It returned `unavailable;columns=3` for width two. The author trigger gave four columns. Warnings about a future overlap were conditional and the supplied current profile remained valid. |
| Marlin | The unassisted worker demonstrated that moving a preview field changes the shared sort index in the parent, returning `preview=A,B;delivery=B,C,A`. On the exact same input, the isolated fix returned `preview=A,B;delivery=A,C,B`. The assisted worker missed that defect and claimed job selection used a fixed index. Its proposed input returned `preview=A,B;delivery=A,C,B` on the original build. `Array.IndexOf(columns, "job")` resolves the actual job column. That disproved its claimed blocker. |

With the supplied-profile scoring, false positives totaled two assisted and one unassisted. With the conditional reading of Trestle, the counts are one and zero. Honest questions about test scope and binary provenance were excluded from false positives. The native receipt contains checked counters, observed timestamps and TRX identity. Its optional build and runner digests are absent, and the original TRX names the author build path. It establishes a passing smoke run within the archive's recorded scope. All three assisted workers invoked the inspector, which correctly reported a valid passing smoke receipt each time. It did not exercise the later behavioral input combinations.

## Provenance and integrity

The design adapts the stale publish race described by AWS, the expanded catalog visibility described by Cloudflare, and the shared ordered-query state from SWE-bench's Django instance. These are reduced local workloads with their own deterministic tests. [AWS event summary](https://aws.amazon.com/message/101925/) · [Cloudflare postmortem](https://blog.cloudflare.com/18-november-2025-outage/) · [Django ticket](https://code.djangoproject.com/ticket/29834) · [SWE-prime case index](https://huggingface.co/datasets/SWE-bench/SWE-prime)

The online search also found Microsoft’s C# SWE-Sharp-Bench and two concrete Polly cases. One loses a configured duration callback between construction and the controller. Another overflows exponential delay after a long retry sequence. These upstream cases are separate from the measured local projects. [SWE-Sharp-Bench dataset](https://huggingface.co/datasets/microsoft/SWE-Sharp-Bench) · [Polly #1852](https://github.com/App-vNext/Polly/pull/1852) · [Polly #2164](https://github.com/App-vNext/Polly/pull/2164)

Base SlopHawk was `0547270`. The protocol and package sources were frozen at `a313675` before starting Luna. Balanced assignment draw 5 gave assisted-first on Trestle and Marlin and unassisted-first on Fallow. The assignment froze at `2026-11-16T12:46:18.697Z`. Each run had one fresh `gpt-6-luna` turn at `low` effort. The worker IDs in table order were `01a1e465-3e2a-7ca2-91d8-773b9e91a6c9`, `01a1e466-3e64-75b2-b1b4-af4ab9c93732`, `01a1e467-25c9-7a22-9ef1-81b755561d56`, `01a1e467-d912-76f0-abbe-0d0d36baf183`, `01a1e468-8c28-7eb1-9c99-5e5f128e3252`, and `01a1e469-510b-7773-9592-14cbf8241996`. The coordinator scored with access to assignments. Runtime totals include all worker activity and exclude research, authoring and coordinator activity.

| Snapshot | SHA-256 of per-entry manifest |
| --- | --- |
| Trestle, 20 entries | `213AA1175874EA5F5F78D3B7F96B9D828D3670294D7B566957D18E3DDF1AF745` |
| Fallow, 20 entries | `B191C3D48A479BDC9253C6A519E105BE078018A62C7250EFCAD73DB745C89F73` |
| Marlin, 20 entries | `5910642C147F65819D0F35E95F3099F4DB7F33F0DC8E5F1E050DD813E45E7CD5` |
| Inspector | `FA3419625B1B1D329CC9F29C785F0ED77F9CC1FA0253076006B3C9FFF2503396` |

All manifests matched across processes before dispatch and again after the trials. Worker command traces showed no reads from the author-only workspace or peer sessions. The solution built with zero warnings and errors, and 377 tests passed with two environment-dependent skips.
