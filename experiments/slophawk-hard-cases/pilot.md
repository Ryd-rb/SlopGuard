# Norchester Dispatch paired development pilot

The product basis is SlopHawk main `99965e2ff9b524aaa5dfa2e51a1876d94ea23801`. This is one development pair on one dossier. Its results can expose a review ceiling, a scoping problem, or an instrumentation failure. Leave the dossier out of any later confirmatory sample.

## Dossier and access

The packer lays out the Norchester Dispatch release handoff as an ordinary .NET release board, three lane requests, the previously approved request, an evaluated dispatch graph, NuGet restore receipts, the prebuilt quote and smoke programs, and raw supervisor records. The packer and answer key are outside the dossier. Build and run the shipped release board and each of four individually repaired variants in private scratch before dispatch. The shipped board must say `ready` for Delta, each repaired board must say `hold` for Delta, and all five must say `ready` for Ember and pass the ordinary smoke checks. Re-run the four underlying qualification probes during packaging.

Both workers review the same read-only-by-instruction dossier. They may inspect source, execute the package-local programs and run native tool commands with outputs in separate scratch. They may not inspect the author repo or run log, search the internet, inspect sibling workspaces, use other agents, or change the dossier. The coordinator fingerprints every directory and file from two distinct processes before dispatch and again afterward. Use per-entry paths, byte digests and file times. Windows file read-only flags are defense in depth. Both workers share the host principal, so path restrictions are task instructions and can be audited in the run transcript.

Only the assisted prompt includes an additional executable path and commands. The read-only inspector exposes worker-selected `plan PROJECT`, `reuse`, `run ARCHIVE` and `coverage ARCHIVE TARGET SOURCE LINE`. It returns product planner, reuse, archived receipt validation and scoped coverage output. `SameLiveFence` is an equality check on the independent fence export. The tool does not issue a release verdict or discover undeclared assets. Verify both arms can use the native release board, confirm each inspector command behaves as specified and fingerprint the separate inspector payload. Stock SlopHawk has no public entry point for these checks yet. This pilot measures access to a thin bridge over the current verification components.

## Dispatch and measurements

Launch exactly two new `gpt-6-luna` workers at `high` with no forked turns. Randomize which arm starts first using one coin draw before either is dispatched. Each has a three-minute review ceiling and six grouped local tool calls as an instruction-level budget. Keep the review request and allowed native tools identical. The brief asks them to review the change for release, name concrete blockers with evidence and a reproducible check or minimal correction, and identify any material missing evidence. Workers receive no defect names, counts, other arm output or study hypotheses. They report raw citations without tool names so a scorer can compare findings without the arm labels. Record any practical limits to scorer blinding.

Use only the final review delivered inside the time budget. Score verified findings by the four causal decisions in the oracle below and deduplicate by cause. A partly correct or generic concern scores zero. Count distinct unsupported blocking assertions as false positives, explicitly including spurious blockers of Ember, the inactive override, an applicable current receipt, and the compatibility hit. Count caveats about genuinely missing verification separately. No credit for repeating `IncompatibleAssets`, `Reusable`, `Passed` or `Unknown` without tying it to the actual release decision and concrete evidence.

| Finding | Minimum verification |
| --- | --- |
| Selected restore closure | Delta reaches `Route` and then v2 `Tariff`. The receipt at the v2 path identifies v1 in `project.restore.projectPath`. Check declared/evaluated references and the embedded owner or show why traversing and validating the graph blocks Delta. |
| Runtime input left out of reuse | Delta's `Route` resolves through the catalog to `assets/regional.json`, absent from the approved key. Its archived 16 changes to staged 17, so the old approval is invalid. An independently reproduced selected quote or scoped digest comparison suffices. A changed override on its own does not. |
| Superseded qualification | The archived passing receipt agrees with its admitted attempt. Its epoch differs from `Live.Epoch`, which should prevent the board from clearing current qualification even with matching revision and Git. |
| Coverage scope | Delta requests `release-net10`. The consumed hit is for `compat-net10` and proves no execution on the requested target. The target-less join in the gate makes the false clear. A separate explicit-zero run exists. Give credit for distinguishing it from the unrelated positive hit or for correctly leaving the bad join unsupported without it. |

Read the final cumulative usage record for each worker, verify the actual model and effort, and report input, cached and uncached input, output, reasoning included in output, logical total, elapsed time, and tool calls. This is run-local worker usage. Setup and coordinator activity are separate. A snapshot mismatch, incorrect model/effort or unsanctioned access invalidates the matching comparison. Absent usage leaves the token comparison unmeasured. Include any paid failed attempt or timeout in the report. Do not dispatch replacements or scale automatically from this pilot. If either worker reaches a ceiling or the pilot misses its operational bounds, tune a new development dossier before deciding how to scale.

## Local reproduction

```powershell
$scratch = Join-Path ([IO.Path]::GetTempPath()) ('norchester-' + [guid]::NewGuid())
dotnet run --project experiments/slophawk-hard-cases/Qualification.csproj -- pack $scratch
```

The `Norchester` subdirectory is the worker-visible snapshot. The parent holds author-only artifacts and repaired replays. Build `Inspector/Inspector.csproj` into a separate artifact directory and provide its executable only in the assisted briefing. Use `Qualification fingerprint ABSOLUTE_DOSSIER_DIRECTORY` for repeatability and integrity checks. Keep the author replay, fingerprint and worker usage events outside the dossier.
