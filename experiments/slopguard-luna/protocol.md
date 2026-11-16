# Paired Luna review pilot

## Hypothesis and design

SlopGuard increases the number of confirmed defects surfaced by a clean-context Luna 6 High review. The null prediction is zero increase. The experimental unit is a project pair, with a fresh worker for each arm of each of the three pairs. One arm receives ordinary .NET tools and the other also receives the syntax release and project-evaluation preview. The workers receive one input project each, with identical starting files within each pair and separately materialized directories. The assignment of tool access to the first or second run is randomized independently for each pair. Run the pairs in Beacon, Grove, Parcel order.

Each worker receives the following base prompt. Replace <workspace> with its assigned root.

> Perform a read-only release review of the C# project at <workspace>. Identify concrete defects against the project's README and source. Review only this workspace and use normal local source and .NET inspection tools as needed. You may build and run the project without editing source. Do not search parent or sibling directories, other tasks, or the internet. Finish within six minutes. Return each confirmed finding with the file and line, the defect and its observed or predicted effect, and how you verified it. List key commands and any verification blockers. Report only issues you would ask the maintainer to fix.

The control arm receives this final paragraph.

> Use the workspace's source and standard .NET tooling. Do not invoke SlopGuard, an equivalent supplemental scanner, or other agents.

The SlopGuard arm receives this final paragraph.

> You may additionally use these SlopGuard commands in PowerShell, replacing <workspace> with the supplied path. Use standard .NET tools as needed to verify your findings. State which evidence led you to each finding.
>
> & '<release exe>' start --root '<workspace>'
> & '<release exe>' results --root '<workspace>'
> & '<preview exe>' projects --root '<workspace>'

The supplemental tools are SlopGuard 0.1.0 for syntax and 0.2.0-preview.1 from the pilot's parent code revision for the evaluated project graph. The graph is factual input. The worker must identify the defect from the graph and the spec. Treatment workers may choose how to use their tools. Only the coordinator sees this protocol or other workers' outputs. This is a worker-isolated, open-label trial with scoring by the coordinator.

## Ground truth and scoring

| ID | Defect required for credit | Observable check |
| --- | --- | --- |
| B1 | Missing semicolon in MonthlySchedule.NextExport | 0.1.0 reports CS1002 in Beacon/Plans/MonthlySchedule.cs |
| B2 | EnableExportJson mismatches EnableJsonExport and leaves Json.cs out of compilation | formats prints only csv |
| B3 | ExportName swaps month and day | name 2026-03-08 prints beacon-2026-08-03 |
| G1 | Missing closing parenthesis in WateringSchedule.StartFor | 0.1.0 reports CS1026 in Grove.App/Reference/WateringSchedule.cs |
| G2 | The net9.0 predicate prevents the Organic.cs link on net10.0 | policies prints only standard |
| G3 | Capacity excludes the allowed boundary of 120 | seeds 12 10 prints over:120 |
| P1 | Missing switch-expression closing brace in LegacyWeight.AddPackaging | 0.1.0 reports CS1513 in Parcel.App/Maintenance/LegacyWeight.cs |
| P2 | The nonrecursive include leaves Regional/RemoteZone.cs out of compilation | zones prints only local |
| P3 | The bulk discount halves the priority surcharge | quote 20 priority prints 16 |

The negative cases are Text, Archive and TrainingZone exclusion, the intentionally off-build directories, and the adjacent valid C# in each. Normal builds should succeed. The positive cases include behaviors outside SlopGuard's measured capabilities. Before dispatch, check the builds, expected outputs, release syntax diagnostics, and preview membership with a third copy of each project. Freeze the input trees, assignment and rubric before collecting answers.

Credit a finding if it identifies the specific location and effect in the table. Count each defect once. Correlated symptoms alone do not earn separate credit. Verify any additional independent claim against the source and public contract and report it separately. Score contradicted defect claims as false positives, and leave unsupported claims unconfirmed. Record timeouts or unavailable evidence on the assigned run and do not replace an unfavourable result.

The primary observation is the paired count difference for each project and its sum across the nine opportunities per arm. Report the three differences, treatment-only and control-only defects, overlaps, and false positives. Record which new findings were traceable to SlopGuard output. A higher count with a traceable treatment-only detection answers the question for this pilot. Equal scores do not establish equivalence. Three purposively selected projects cannot establish typical production lift. Do not treat the nine defects as independent randomized trials.

## Reproduction

Materialize each project in a fresh directory outside this repository. Copy the contents of its folder under fixtures and strip the final .fixture extension from source and project files. The README in each project gives its shipping contract. Restore/build the named app with the .NET 10 SDK. Run the checks in the table and use the CLI commands shown above for supplemental verification. The .fixture suffix keeps specimen code and deliberate syntax errors out of the SlopGuard solution and its live source inventory.
