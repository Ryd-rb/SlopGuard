# ForgeLedger holdout

ForgeLedger models offline fabrication planning. Its generator has 20 defect families with three variants each, plus six correct controls. Each draw selects one implementation for every defect family using a recorded seed. Use the same draw for both arms of a pair. Draw a new seed for the next pair.

## Generate a draw

Run from the repository root with the .NET 10 SDK and an empty external destination. Omit the seed to draw a new one. The generator prints the seed and records the selections under `Private`.

```powershell
$fixture = 'C:/benchmarks/forge-ledger/draw-1'
dotnet run --project experiments/slophawk-forge-ledger/HoldoutGenerator.csproj -- $fixture
```

Pass the reported seed as the second argument to reproduce a draw. The tested draw used `865379010`.

```powershell
Push-Location $fixture
dotnet build Public/src/ForgeLedger.csproj -c Release --artifacts-path Private/CandidateBuild -o Public/bin
dotnet build Private/ReferenceSrc/ForgeLedger.Reference.csproj -c Release -o Private/ReferenceBin
dotnet run --project Private/Qualification/ForgeLedger.Qualification.csproj -c Release -- Private/qualification-results.json
dotnet Private/ReferenceBin/ForgeLedger.Reference.dll batch Public/examples.json > Public/examples-expected.json
Pop-Location
```

Give workers only `Public`. Keep the generator, qualifier, reference, witnesses, and selection receipt outside their sandbox. When using `check-contracts`, pair the published requests in `examples.json` with the corresponding reference results by index, producing rows with `Operation`, `Args`, and `Expected`. Provide the same inputs and expectations to both arms.

The qualifier checks a separate witness for each of the 60 variants, confirms all variants agree on their ordinary examples, and compares the clean implementation and reference with the public rule on 1,000 admitted inputs per operation. This draw passed 26,000 such comparisons, and both compiled assemblies agreed on all 59 published examples. Their outputs differed as expected on each of the 20 private selected witnesses.

## Blinded Luna 6 Low pair

The generator author qualified the fixture independently of the checker. The checker was frozen at `8141258` before running the holdout. The public operation IDs differed from the internal methods for all 26 operations. The public rules contained no input hints. Both fresh workers received byte-identical public files and the same short flag-only prompt. Each worker had one turn and uncapped tool calls. The prompt withheld the defect count and draw. The assisted workspace included the frozen checker, its skill, and the manifest entry with its commands.

Both Windows sandboxes passed the same Node and ordinary-example preflight. They blocked reads of the independent reference, witnesses, draw receipt, generator, and sibling workspace, as well as writes to the candidate source. Randomization put the unassisted session first. Post-run fingerprints matched both frozen copies.

A verified bug required a distinct seeded operation, an admitted submitted input, and exact results from both the candidate and independent reference. A false positive was a reported discrepancy on an admitted input where those builds produced the same result. An incorrect expected value on a real discrepancy was scored separately. Total tokens include cached and uncached input plus output. Uncached input is shown separately.

| Arm | Submitted | Verified of 20 | False positives | Wrong-value witnesses | Total tokens | Uncached input | Shell calls | Wall time |
|---|---:|---:|---:|---:|---:|---:|---:|---:|
| With SlopHawk | 20 | 20 | 0 | 0 | 328,097 | 77,645 | 8 | 98.7 s |
| Without SlopHawk | 4 | 2 | 1 | 1 | 568,777 | 41,573 | 17 | 158.3 s |

The assisted session ran `check-contracts` for 23,479 supported comparisons and retested all 20 flags. Scoped review recorded zero missing operations and zero outstanding findings. Independent replay verified all 20 submitted witnesses. The assisted run used 240,680 fewer total tokens and 36,072 more uncached input tokens.

After both workers finished, the previous checker build at `157f108` independently verified 18 of the same 20 defects at the same 2,048-case budget per operation. Running the new checker on the corrected build produced zero flags in 52,478 comparisons. The updated build also reproduced all 132 existing Continuity defects with input hints removed in a separate regression. The new search projects conjoined comparisons from the public rule and derives the expected result from the unmodified public formula. Numeric literals from methods with compatible integral signatures supply probe values when the operation name yields no useful literals.

The checked-in generator reproduced the exact benchmark draw from seed `865379010`. A second random draw preserved all 20 families and picked a different variant for nine of them. The second draw was used to verify the generator.

| Arm | Session |
|---|---|
| With SlopHawk | `01a1e878-5637-7bb0-b011-aa8cda4d5d17` |
| Without SlopHawk | `01a1e875-cbed-7ad1-a567-4b7bfe3e1c66` |

The candidate assembly SHA-256 was `AF01C74F259008981F0F95324EFF234DA72C83C80064259C94FBEB2C7175A136`. The shared executable rules SHA-256 was `1DB5B7943F2290CC800166B7C9D07133E7466D6AEFFF5DD9B9E41FC6E2A15CA4`. The checker CLI assembly SHA-256 was `7686BB7721D5106BC3B753B0709214221D678E7B0E2834B0F7B38ECA335AB612`. The local coordination bundle `slophawk-combined-c4994e8c00` retains native session records, frozen hashes, and scoring. The author bundle `ForgeLedger-Holdout-20261117` retains the draw receipt and qualification.

## Evidence handling audit

The reporting update preserved all 20 independently verified discrepancies. Two runs on the frozen draw produced identical inputs and observations across all 17 search batches, identical findings, and identical per-operation coverage. Each run made 23,479 comparisons. The corrected build produced zero flags in 52,478 comparisons. An isolated Windows run also retested all 20 discrepancies and completed the scoped review. These were direct CLI runs.

A deliberately altered quote rule added one cent for a one-minute job. It still reproduced both published examples and produced a reproducible false positive on the corrected build. The search receipt identifies the supplied rules as the source of expectations and the published examples as the basis for qualification. Independent replay against the correct reference remains the basis for the benchmark's verified-defect count.

Regression tests exercise changing observations, review coverage from different adapters, inconsistent saved reports, and a failed retest. The updated reporting also preserved all 132 independently replayed Continuity discrepancies with optional input hints removed.
