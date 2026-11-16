# Luna 6 High paired review results

## Outcome

SlopHawk 0.1.0 surfaced two invalid C# snippets that the control workers missed. Both snippets were outside their project's compile set. The third off-build syntax error was independently found by both workers. The prespecified score was **8 of 9 with SlopHawk and 7 of 9 without it**.

| Project | Without SlopHawk | With SlopHawk | Difference |
| --- | --- | --- | --- |
| Beacon | 2/3 | 3/3 | +1 |
| Grove | 3/3 | 2/3 | −1 |
| Parcel | 2/3 | 3/3 | +1 |
| Total | 7/9 | 8/9 | +1 |

The assisted Grove worker used the preview project graph to identify that Organic was missing from the build and reproduced the symptom. The [frozen rubric](protocol.md) required identifying the conditional include that caused the omission. The control worker located it. Counting confirmed symptoms regardless of root cause gives 9 of 9 with SlopHawk and 7 of 9 without it. Across the two arms, six defects earned credit in both, two earned credit only with SlopHawk (B1 and P1), and one earned credit only in the control (G2). Both arms found all three application logic bugs. There were zero false-positive defect claims, zero unsupported defect claims and zero incomplete reviews.

The preview graph supplied useful membership evidence for Grove. Every membership symptom was independently reproduced in the control arm. The extra detections were attributed to the syntax release. The projects were small and deliberately included off-build syntax errors, so this pilot establishes that SlopHawk can surface misses in this setting. The sample is too small and targeted to estimate production review lift, and the workers and scorer knew the tool assignment.

## Worker evidence

Runs are listed in dispatch order. Tool attribution comes from the workers' recorded evidence.

| Run | Arm | Credited | Evidence from the final review |
| --- | --- | --- | --- |
| Beacon `edc936ad19` | SlopHawk | B1, B2, B3 | Cited SlopHawk CS1002 at `MonthlySchedule.cs:7:60`. Traced the property-name mismatch through `Build/Exports.props:4` and `Directory.Build.targets:5`. Ran both documented commands and identified the date pattern in `ExportName.cs:8`. |
| Beacon `6f5566eb82` | Control | B2, B3 | Tested both commands in Debug and Release and inspected evaluated MSBuild inputs. Setting `EnableExportJson=true` restored JSON. Located the date pattern in `ExportName.cs:8`. |
| Grove `788b3f140a` | Control | G1, G2, G3 | Visually found the unbalanced expression in `WateringSchedule.cs:5`. Located the stale target predicate in `Build/Compatibility.props:2`. Ran both documented commands and traced the boundary error to `SeedCapacity.cs:10`. |
| Grove `ebb2885374` | SlopHawk | G1, G3 | Cited SlopHawk CS1026 at `WateringSchedule.cs:5`. Ran boundary and below-capacity samples. Used the preview graph and `policies` output to verify the missing Organic policy. Cited the existing Standard link and left the stale predicate unidentified, so G2 received no credit. |
| Parcel `035b402ab4` | SlopHawk | P1, P2, P3 | Cited SlopHawk syntax diagnostics on lines 8 and 10 and located the missing switch brace at `LegacyWeight.cs:9`. Traced the absent Remote zone to the glob at `Build/Zones.props:3`. Ran the zone and quote examples and located the fee ordering at `Quote.cs:7–8`. |
| Parcel `953f7875f5` | Control | P2, P3 | Ran the zone and quote examples, including the one-unit quote. Located the nonrecursive glob at `Build/Zones.props:3` and the fee ordering at `Quote.cs:7–8`. |

The [protocol, scoring rubric, and fixtures](protocol.md) were frozen at commit `89ddf0baf460c171b5013b6fa72ab5f937a90c10` before any worker was dispatched. Six separate `gpt-6-luna` workers ran at High effort without inheriting the coordinator’s chat history. Each worker saw one copy of one project and the prompt for its arm. The coordinator checked the final answers against the rubric. The workers knew which tools they could use.

## Inputs and preflight

The reference copy of each project built with .NET SDK 10.0.201 on Windows and reported zero warnings and errors. The two review copies matched the reference source bytes before dispatch. The lists below show the initial ground truth from the coordinator's separate validation copies.

| Project | 0.1.0 syntax result | 0.2 preview build membership | Actual CLI output |
| --- | --- | --- | --- |
| Beacon | `Beacon/Plans/MonthlySchedule.cs:7:60`, CS1002 | CSV compiled, JSON absent | `formats` → `csv`. `name 2026-03-08` → `beacon-2026-08-03`. |
| Grove | `Grove.App/Reference/WateringSchedule.cs:5:89`, CS1026 | Standard linked, Organic absent | `policies` → `standard`. `seeds 12 10` → `over:120`. |
| Parcel | `Parcel.App/Maintenance/LegacyWeight.cs`, CS1003, CS1002 and CS1513 | Local compiled, Remote absent | `zones` → `local`. `quote 20 priority` → `16`. |

The release scanned eight C# files per project, with complete results. The preview evaluated one complete .NET 10 Debug target per project, with four compile items per target. The off-build reference snippets and the intentionally unshipped Text, Archive and TrainingZone types were absent from their respective build inputs as designed. The release daemon was primed in each assisted workspace before its worker started.

The shipped 0.1.0 CLI assembly hashed to `BEBB07C110BB4CE0761C79D6FC363CC966F4D5B215A790E8818135CCB3CCEDF9`. The 0.2.0-preview.1 build used the code at `99c440c0438a9b70b50afd39bde83bfcb0e7548b` and its CLI assembly hashed to `4B92BA9FFB8D2154CA9D8A7C888A76352AA65E44B3FDAAF7FF84CD9185A706D3`.

Piping a cold 0.1.0 `start` directly into PowerShell JSON conversion stalled during coordinator preflight. Standalone `start` and a separate `results` command worked. The assisted workers had ready daemons for all three runs. Input hashes were checked again after the reviews. All six copies still matched their templates, with only standard build outputs and the assisted syntax reports added.

The solution built with zero warnings and errors, and all 247 integration tests passed.
