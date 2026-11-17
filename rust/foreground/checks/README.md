# Foreground checks (SG-MIG-06)

Standalone Rust port of the *published* SlopHawk foreground commands, not a
change to the existing `slophawk` binary or daemon. Rust owns only foreground
check, retest, acknowledgment and review evidence; the C# batch adapter remains
an explicit DLL supplied on the command line. This crate does not grant any
execution authority based on a repository file or previously generated artifact.

## CLI and on-disk contract

Run the native binary using the existing argument sequences:

```text
slophawk-foreground-checks check <assembly.dll> <cases.json> <workspace> [run-verb [prepare-verb]]
slophawk-foreground-checks check-observe <assembly.dll> <requests.json> <workspace> [run-verb [prepare-verb]]
slophawk-foreground-checks check-retest <assembly.dll> <workspace> <run> <decisions.json> <acknowledgments.json>
slophawk-foreground-checks check-review <workspace> [acknowledgments.json [scope.json]] [--run <run>] [--progress]
```

The `check` input is an array of `{"Operation":"...","Args":[1],"Expected":2}`;
`check-observe` uses the same request shape without `Expected`. `Expected`
must be an independently established contract value, never learned from
candidate output. `null` candidate results are unsupported, not findings.
The binary invokes `dotnet exec <assembly.dll> [verb] <requests-or-prepared.json>`
in a per-run directory, with `run-verb` and `prepare-verb` preserved as exact
argument strings. It emits the C# `slophawk-check-v1` receipt and exit codes:
0 for a supported comparison without findings (or successful retest, complete
review, or requested progress), 1 for findings, unsupported or incomplete
review, and an error on stderr with exit 1 on execution/validation failures.

Completed runs are atomically renamed into `.slophawk/checks/<run>/` and contain
`requests.json`, `observed.json`, `report.json` and optional `prepared.json`.
The published C# `CheckRun` fields and raw expectation/result JSON are preserved.
Rust adds `identity.json` (root, operation, exact input/request/prepared/observed
SHA-256, candidate SHA-256, run/prepare verbs) to its own runs; old C# runs
without it remain readable. It is *integrity evidence*, not an authority grant.
Invalid/incomplete attempts remain under `.failed-<run>` and do not appear as
completed runs. A changed retest remains under `.slophawk/retest-attempts/<run>`
for diagnosis and cannot acknowledge its original finding. A successful retest
moves into normal checks under its own run ID before an atomic, exclusively
locked acknowledgment update. A stale or conflicting acknowledgment fails
closed; a stranded `.lock` after a crash requires owner review before removal.

Review selects exactly one `(Candidate, RunVerb, PrepareVerb)` adapter group; a
workspace with multiple groups requires `--run`. Acknowledgments require a
separate same-adapter retest of the original input and exact actual value;
`confirmed` keeps the original reference, while `corrected-expectation`
requires an independently justified replacement matching the observed
candidate. Check-review reports `UnstableInputs`, `Outstanding`,
`UnsupportedComparisons`, `MissingCoverage`, per-operation supported and
observe-only counts, and unique supported outside-ordinary arguments.

Files and individual adapter streams are bounded to 8 MiB and stderr to 64 KiB;
per-adapter execution plus stream drain has a 30-second deadline. Reviews cap
completed runs at 2,048, retained input/report/observation bytes at 64 MiB,
and observations at 65,536. Candidate assembly hashing streams at most 512 MiB.
Adapter output is staged under a bounded worker deadline. The Windows launcher
assigns the process to a Job Object before execution and verifies drainage
before accepting output. The Linux launcher requires a user-systemd cgroup v2
scope. Hosts without authenticated containment refuse adapter execution.

## Deferred integration qualification

The integration owner should regenerate a lockfile for this new crate and
build the real C# CLI plus the C# batch-adapter fixture, then set
`SLOPHAWK_CSHARP_CLI` to the built `slophawk.dll` and
`SLOPHAWK_FOREGROUND_ADAPTER` to the built `Adapter.dll`. The Rust integration
tests exercise C# → Rust and Rust → C# artifact/retest/review interchange,
vanished findings, ordered/short responses, missing coverage, mixed adapters,
unsupported/observe-only inputs, unstable input, identity tampering, stale
acknowledgments and competing acknowledgment writers. Without those explicit
environment variables interop tests deliberately skip external process work;
they must both be set for qualification.

No `cargo`, `dotnet`, fixture, adapter or other runtime command was executed
for the SG-MIG-06 source-only recipient patch. The owner must verify behavior
on supported Windows and Linux targets and separately qualify lifecycle,
process-tree drainage and atomic file replacement on the real filesystems.

## Pinned evidence

Derived from `ForegroundCheck.cs` (`e18eb2b0...`), `CheckRetest.cs`
(`adbfaf1a...`), `CheckReview.cs` (`cc3ed21e...`), the pure
`slophawk-probe-replay` 0.1.0 API (`58a011f3...`) and
`ForegroundEvidenceTests.cs` (`3f5e4679...`) at SlopHawk commit
`a367ef58076f7e29d2ae475a3b593daad04c4998` / tree
`fa4f03394ea061818423a1530831f6868942a94d` (2026-11-17).
Rust language version is 1.99.1 and the independent .NET adapter target is
.NET 10. The `serde` 1.0.229, `serde_json` 1.0.154 and `sha2` 0.10.9
versions follow already published sibling manifests; `uuid` 1.x is a new
dependency and needs an owner-generated lock entry before qualification.
