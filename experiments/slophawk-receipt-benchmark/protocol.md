# Paired archived verification benchmark

## Hypothesis, surface and budget

Access to the verification components on current SlopHawk main increases the number of unsupported release-evidence claims a fresh Luna 6 High worker correctly challenges and explains. The product basis is main at `3cee615a9173dcfa3ca8ff4c25a98d15b82c269b`, with the verification packet integration in its history.

The stock CLI still exposes inventory, project evaluation and scheduler status. It does not expose these receipt checks. This benchmark adds a separate read-only inspector built against the unmodified internal components. The inspector runs `BuildArtifactCapture`, `BuildOutputIdentity.Compare`, `TestRunReceiptDetails.Validate`, the archived VSTest classifier and `CoverageReceiptCapture`. The inspector reports component results, and the worker must still tie them to a claim and its decisive raw evidence. The benchmark factory separately exercises the current Windows-leased `VstestAttemptRunner` on two actual prebuilt xUnit assemblies to obtain passing and failing TRX evidence. Both arms audit those archived bytes. Worker runs do not launch a new test or use the automatic scheduler. Cobertura specimens and their supervision metadata are controlled synthetic evidence.

There are **six fixed matched dossiers**, nine claims per dossier, and one fresh worker per arm. The maximum is **twelve workers**. Each sees one dossier and no previous turns. Both arms use exactly the same read-only root for a pair. The inspector is supplied only to the assisted worker, and its executable excludes the factory and scoring keys. Both workers receive the same handbook describing the evidence rules and can use local script or file tools. Randomly assign the two dispatch slots with one fair coin per pair. One read-only pair may run concurrently. Budget three minutes and six grouped local tool calls per worker. This is a prompt-level tool-call budget. Runtime counters measure the actual tokens. The coordinator evaluates any claim delivered within the three-minute limit and records late or incomplete runs without replacement.

The fixed sequence is Arc, Basin, Cairn, Dune, Elmwood and Ford. Complete the fixed sample after the offline gate passes. Do not scale it or stop based on favourable scores. Keep the test evidence and audit clocks frozen. Pair labels and the raw verdicts are never shown to another worker.

## Offline dispatch gate

Require all of the following before starting workers.

- Build the factory, inspector and pinned product on the local .NET 10 SDK. Verify that the factory acquires the production Windows lease and the actual runner returns a complete passed artifact and a complete failed artifact. Check the two executed-test counts with the raw XML.
- Qualify all 54 claims against the planted evidence changes and corresponding component outputs. Include same-size/same-time changed bytes, ordered or linked inputs, unknown versus known-empty closure, wrong durable attempt bindings, stale or nonmatching TRX provenance, coverage lease/mapping versus line and branch claims, and a valid near-match for each evidence family in every dossier.
- Confirm the stock CLI's coverage of these operations and preserve the benchmark-only qualification label. Verify the separate inspector emits exactly nine observations per dossier and that the raw dossiers contain no answer key or generated inspector reports.
- Freeze the generator, protocol, six raw dossier fingerprints, inspector identity and randomized assignment before any worker response. Recheck all raw bytes and relevant ownership/time facts after every pair.

## Worker prompt

> Audit every claim in README.md at `<dossier>`. Use local read-only file inspection, PowerShell, or ordinary .NET tools against this dossier. Do not edit or run any specimen assemblies, read other workspaces, or search the internet. Finish within three minutes, using at most six grouped local tool calls. Return one row per claim with ID, SUPPORT or CHALLENGE, and the single decisive reason with raw file/field or measured value. Include tool evidence when it contributed. Report any verification blocker separately.

Control addition

> Use ordinary local tools and the dossier records. Do not use SlopHawk, supplemental analyzers, or other agents.

Assisted addition

> You may additionally run the benchmark SlopHawk receipt inspector in PowerShell using `& '<inspector executable>' '<dossier>'`. It prints one line per claim using the pinned verification components. Use the raw dossier to support your reasoning. Do not use other supplemental analyzers or other agents.

## Fixed scoring and analysis

The private map assigns six challenges and three supported claims to each dossier. The factory sets it from independently controlled input changes before invoking the product. Credit a challenge only when the worker states the exact planted reason with a verifiable comparison. A generic mismatch label, a negative run outcome alone, an absent counter described as zero, or a general warning about aggregate coverage does not suffice. For coverage branch claims, the worker must state that the specific XML counter is aggregate without an identity for the stated arm. Count challenge claims against the three supported controls as false positives. Score supported controls and unsupported/unanswered rows separately, without adding them to the primary challenge endpoint.

The primary metric is the number of credited challenges out of six per matched dossier. Report total detections, pair wins/losses/ties, overlapping and exclusive findings, false positives, and the cases in which the inspector supplied relevant evidence. The one-sided exact paired randomization test enumerates all 64 sign assignments to the six pair differences and counts the sums at least as large as the observed preview-minus-control sum. Compare with alpha 0.05. Six pairs can distinguish a large, consistent effect on this benchmark. The claims share archives and components. Inference uses dossier pairs, with limited external validity.

Read the last cumulative runtime usage event for each exact worker path and parent chat. Report input, cached and uncached input, output and reasoning output, total, and seconds per worker. Compute uncached input as input minus cached input. Compare token totals descriptively. Keep missing runtime measurements missing and exclude setup and coordinator tokens.

## Reproduce

Use Windows with .NET SDK 10.0.201 or a compatible configured .NET 10 SDK. Copy the two files under `Specimen` to an empty project directory and strip the final `.fixture` suffix. Build the same project twice with separate artifact directories, adding `-p:DefineConstants=FAILED_GATE` to the second build. Build `Factory/Factory.csproj` and `Inspector/Inspector.csproj` with separate artifact directories. Their use of the WorkerFixture friend assembly name is confined to these experiment binaries. Invoke the factory with a new absolute root, the installed dotnet host path, SDK version, original passing DLL and failing DLL. It emits native receipts, raw archives, private per-dossier oracles and qualification output. Point the inspector only at an original dossier path, since the build identity intentionally includes its physical mapping. Keep generated run artifacts outside Git.
