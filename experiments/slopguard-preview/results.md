# SlopGuard 0.2 preview-only trial

## Findings

**Both arms found all seven prespecified issues.** The current preview added verification context about project membership and outstanding test intents. All seven findings were independently established by the control workers. There were zero false positives, unconfirmed defect claims or incomplete runs.

| Project | Standard tooling | With 0.2 preview | Difference |
| --- | ---: | ---: | ---: |
| Ferry | 2/2 | 2/2 | 0 |
| Tally | 3/3 | 3/3 | 0 |
| Rivet | 2/2 | 2/2 | 0 |
| **Total** | **7/7** | **7/7** | **0** |

- **Ferry.** Both workers traced the Windows calendar fallback to `build/WorkstationDefaults.props:3` and reproduced the target-specific Saturday behavior. Both found the inclusive 16:00 boundary in `DeparturePlanner.cs:11`. The assisted worker also observed that the preview returned a partial project graph and flagged the Windows CLI and Estimates contexts as `unsupported_project_context`. The preview showed the portable linked calendar. Neither Windows context appeared in its results, although both built and ran under normal .NET commands.
- **Tally.** Both workers found the solution's omitted reconciliation suite, the midpoint rounding regression and the exclusive settlement lag bound. The assisted worker queried the preview project graph, which linked Reconcile.Tests → Reconcile → Ledger → Policy. Its final evidence for the CI gap was the solution membership and the difference between the solution and direct test runs. The control established the same gap directly. The control separately noted the outstanding manual sandbox check, which was outside the offline test gate.
- **Rivet.** Both workers compared the receipt and current policy digests, reran the packing suite and identified the 10-unit default instead of 16. The assisted worker also observed a ready inventory, two outstanding source intents and an `awaiting_adapter` scheduler. The preview status supplied a separate signal that work remained pending. The receipt comparison established which evidence was stale. Both reviewers verified the behavior with the ordinary test runner.

This pilot reached the scoring ceiling for both arms. It shows the current preview's evidence and coverage limits on three deliberately selected release candidates. It gives no estimate of detection lift on a representative workload.

## Runtime tokens

The worker session's final cumulative token event supplies each row. The runtime recorded `gpt-6-luna` and High effort for all six runs. Input includes cached input. Output includes reasoning output. Total is input plus output. These counts include repeated context across calls and exclude the coordinator, fixture setup and preflight.

| Project | Arm | Run | Input | Cached input | Uncached input | Output | Reasoning output | Total |
| --- | --- | --- | ---: | ---: | ---: | ---: | ---: | ---: |
| Ferry | preview | `2eb7f6dc39` | 168,396 | 145,920 | 22,476 | 3,816 | 1,164 | 172,212 |
| Ferry | control | `d2b4af9350` | 219,979 | 194,304 | 25,675 | 2,650 | 1,120 | 222,629 |
| Tally | preview | `10b6ba108f` | 146,249 | 113,280 | 32,969 | 2,959 | 1,308 | 149,208 |
| Tally | control | `4b16d888ba` | 184,566 | 168,192 | 16,374 | 2,879 | 813 | 187,445 |
| Rivet | control | `b5249ec1dc` | 147,708 | 140,672 | 7,036 | 1,681 | 528 | 149,389 |
| Rivet | preview | `e5deaac98f` | 160,703 | 139,904 | 20,799 | 2,611 | 1,023 | 163,314 |
| **Arm total** | **control** | | **552,253** | **503,168** | **49,085** | **7,210** | **2,461** | **559,463** |
| **Arm total** | **preview** | | **475,348** | **399,104** | **76,244** | **9,386** | **3,495** | **484,734** |

The preview arm used 74,729 fewer total tokens (13.4%) and 27,159 more uncached input tokens (55.3%). It used 2,176 more output tokens. Ferry and Tally had lower totals in the assisted runs. Rivet had a higher assisted total. Cache behavior and the number and size of model calls vary across runs.

## Qualification and input identity

The [protocol and fixtures](protocol.md) were fixed at `2dc5b3bd0f62c1c6daa743b13d291391acb04de0` before dispatch. Each arm received a fresh worker session and an isolated clone of the same Git baseline with byte-identical candidate files. Git state and every tracked input were checked before and after the reviews. Run IDs above are in dispatch order. Worker setup used the preview only. The coordinator's installed syntax release checked the SlopGuard repository as its development skill requires.

All candidate solutions built with zero warnings and errors using .NET SDK 10.0.201 on Windows. The actual product build was 0.2.0-preview.1 from `2679ca5068adc8b17b136c52aee05993b099fa06`. Its `slopguard.dll` SHA-256 was `CA0AB3D2E0A5FD120AE9CF511E4CCF566D661308AB5EDC92C91717ABF4900208`.

| Project | Baseline tree | Candidate stable patch ID | Independent oracle |
| --- | --- | --- | --- |
| Ferry | `aaef452ceaca82ea5d652e2fb41a456b8d879888` | `859acccdaa03937aad82ec839298d28a815fa940` | Two tests passed. Saturday smoke gave Nov 23 on portable and Nov 21 on Windows. Friday at 16:00 still gave Nov 20 on portable. MSBuild evaluated the linked regional calendar for portable and the legacy calendar for Windows. |
| Tally | `70d981558f029ed58e66e3e280ea6f1188f090f4` | `a61cfa04fd3dd016fd2c2c0a03dffc89dff5f390` | Two solution tests passed. Both directly run reconciliation midpoint tests failed. The CLI printed `posted:-10.00` for `-10.005` and `held` at seven days. |
| Rivet | `20ddb5b7fb13a773ec407de034ee3e54f21af604` | `81905f1341e114c237ff4668e0febcbe4b55d54c` | Three baseline tests passed before writing the receipt. The candidate digest changed from `B9FE232F…` to `36FA8B85…`. One candidate packing test failed and two passed. CLI output was `RV-DEFAULT 10+2` and `RV-PILOT 9+9+2`. |

The preview reported complete explicit project graphs for Tally and Rivet. Ferry's graph had four supported targets and two unsupported Windows contexts. Before worker dispatch, ready inventory recorded one pending C# edit for Ferry and two each for Tally and Rivet. All three schedulers reported `awaiting_adapter`. No assisted workspace executed tests through SlopGuard.

`dotnet test SlopGuard.slnx` passed 247 tests. The full solution build and the fixture generator build succeeded with zero warnings and errors.
