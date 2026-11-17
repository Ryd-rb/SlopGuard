# SG-MIG-13 — receipts and coverage (standalone Rust port)

This is a **source-only** companion to the published `slophawk-verification`
crate. It is **not** on the daemon/CLI/MCP routing path. The integration owner
is responsible for binding the future supervised capture route and tests.

## Contracts and trust boundary

- `identity::capture` implements `SlopHawk/SG-03/BuildOutputIdentity/v1`, including
  the eleven section hashes, big-endian 32-bit length-framed UTF-8 fields,
  ordered compile inputs, normalized logical paths, incomplete/mismatch
  reasons and byte-verified output requirements. `compare` refuses incomplete
  or tampered stamps. `closure_checked` additionally checks that observed
  runtime assets are among a separately attested complete required closure.
- `receipt::validate` checks v1 test details against the admitted attempt,
  revision, epoch, fence, root, git fingerprint, lease and scheduler outcome.
  A zero-discovery, skipped or incomplete attempt never becomes a completed
  test execution; a failed, artifact-complete attempt remains failed.
- `capture::capture_coverage` requires independently admitted read-scope facts.
  On **Linux** it pins an O_NOFOLLOW handle chain, checks regular-file and
  single-link identities, reads at most 1 MiB twice, checks metadata, attested
  attempt/lease time bounds (including file mtime) and post-read path identities,
  and returns a private, immutable snapshot. On Windows and other
  platforms this capture **fails closed**; it is not a stub that claims success.
- `cobertura::import_accepted` checks exact run, output, binary, collector,
  source-map, artifact digest and timestamp bindings before XML parsing. XML
  cannot claim aliases. DTD and general entities are rejected, attributes,
  elements, depth, classes, lines, paths and numeric counters are capped.
  The importer records **run-level** class lines and branch aggregates, never
  an exact branch arm or a per-test hit. Incomplete/unmapped scope remains
  explicitly unknown. Failed runs may establish line execution but are not
  reclassified as passed.
- `verification::project` takes only an `AcceptedCoverage` capability. It checks
  target/build/source digests and maps class lines into *line-only*
  `VerificationEvidence` for the published `typed::evaluate_typed` evaluator.
  An exact **document-line** map is **not** an exact **branch-arm** mapping;
  the published evaluator will not mark that counter `Observed`/`Uncovered`
  without a separately verified exact-arm counter from another provider.

`ReadScopeFacts` and `ExpectedRun` are serializable records to cross a trusted
worker boundary, **not bearer grants**: the integrator must construct them from
an admitted supervisor operation, verify workspace epoch/committed revision,
all input/tool/policy digests, required runtime closure, output lease and
containment drainage, and recheck current authority before publication.

## Standalone public argv

`receipts-coverage build-identity INPUT.json`
`receipts-coverage compare-build BEFORE.json AFTER.json`
`receipts-coverage validate-test RECEIPT.json ATTEMPT.json`
`receipts-coverage inspect-cobertura REPORT.xml RECEIPT.json EXPECTED.json`

The last command is an **unattested diagnostic**. Its JSON explicitly says
`accepted_for_verification=false`, even if the counters parse. The plain
public argv is not the new internal worker transport. No credential or
execution authority is inferred from command-line input or source files.
Future private workers must adopt `slophawk.migration-job/1` with bounded
little-endian 32-bit length-prefix JSON, full job/root/epoch/revision/tool/
policy/input/budget/grant binding and a valid terminal before committing output.

## Deferred owner qualification

Commands from the **repository root**, on Rust 1.99.1:

```sh
cd rust/evidence/receipts-coverage
cargo generate-lockfile
cargo test --locked
cargo test --locked --test differential
cargo check --locked --all-targets
```

From repository root with .NET 10 SDK:

```sh
dotnet test SlopHawk.slnx --filter 'FullyQualifiedName~BuildOutputIdentityTests|FullyQualifiedName~CoberturaCoverageImporterTests|FullyQualifiedName~CoverageReceiptCaptureTests'
dotnet test SlopHawk.slnx
```

C# parity reference cases are intentionally stored under the owned directory at
`parity/SGMig13ParityTests.cs`. To compile them, the owner must explicitly import
`parity/LinkParity.targets` *only* into the integration-test project, for example:

```sh
dotnet test tests/SlopHawk.IntegrationTests/SlopHawk.IntegrationTests.csproj \
  -p:CustomAfterMicrosoftCommonTargets="$(pwd)/rust/evidence/receipts-coverage/parity/LinkParity.targets" \
  --filter FullyQualifiedName~SGMig13ParityTests
```

Alternatively link the C# source into that project temporarily and run the same
filter. The matched Rust tests use the same static input vectors; compare section
hashes and states after both are executed by the owner. Test files under `rust/`
remain unreferenced by the existing .NET solution until this explicit import.

The commands above are **deferred to the owner by instruction**. They were
not executed during SG-MIG-13 patch authoring. No `Cargo.lock` is fabricated.
