# SG-MIG-13 — receipts and coverage (standalone Rust port)

This is a **source-only** companion to the published `slophawk-verification`
crate. It is **not** on the daemon/CLI/MCP routing path. The integration owner
is responsible for binding the future supervised capture route and tests.

## AdditionalFiles settings witness transport

`additional_file_evidence::audit_capture` consumes the optional
`slophawk.additional-files-evidence.v1` sidecar from the managed capture. It is a
**library-only advisory adapter**. The managed facade can preserve the exact
`BuildIdentityInputs` and settings records that contributed to its existing v1
stamp; enabling `NativeExecutionWork.IncludeAdditionalFileEvidence` requests
that extra data without changing the existing v1 identity or reuse rules. The
flag defaults to false. There is no new public argv, CLI/MCP checker, scheduler
route, execution grant, or cache activation.

The normal managed evaluator still does **not** produce an attributed
AdditionalFiles observation: it requests Compile and ProjectReference items.
Its live per-file validation reads also do not establish whole-capture temporal
coherence. The sidecar therefore always describes its managed observation status
as `evaluated_additional_files_unavailable`. This is a useful settings-witness
foundation, not a completed live AdditionalFiles producer. An independent owner
may supply one separately admitted observation through the existing typed
`input_audit::AdditionalFileObservation` API; merely copying a settings member
into that type cannot establish evaluated membership or relevance.

### Versioned wire and current authority

The sidecar contains exactly these top-level fields:

| Field | Contract |
| --- | --- |
| `schema` | Exactly `slophawk.additional-files-evidence.v1`. |
| `provider` | Exactly `slophawk.managed-build-capture/1`; a comparison label, not authentication. |
| `binding` | `root_identity`, `epoch`, nonnegative `revision`, `operation_id`, `capture_id`, `target_id`, `mapping_identity`, `collection_mode`, `build_sha256`. |
| `identity_inputs` | The existing snake_case `identity::BuildInputs` shape with string enums. |
| `stamp` | The existing `identity::Stamp`, including all eleven sections. |
| `settings_files` | Complete array of `{path, sha256, length}` records, or null/unavailable. Empty is a checked empty list. |
| `observation_status` | Exactly `evaluated_additional_files_unavailable`. |

Unknown fields, duplicate JSON keys, unexpected enum values, null required
objects and malformed JSON are rejected. Optional settings absence remains an
evidence gap. The v1 `BuildInputs` schema is unchanged and still rejects added
identity fields. Logical paths retain their exact canonical spelling; digests
are compared without case sensitivity and the settings aggregate sorts paths
using .NET UTF-16 ordinal order. The adapter neither opens paths nor normalizes
case or Unicode to guess aliases.

The trusted caller supplies four arguments: optional sidecar bytes, a separately
established **current** `BuildInputContext`, an optional independently admitted
observation, and a `PathIdentity` attestation for the **entire settings mapping**.
The sidecar contains no observation, coherence or `PathIdentity` admission flags.
The existing v1 `path_mapping_trusted` field remains identity data and cannot
supply the separate mapping attestation. The caller
must authenticate the actual producer and operation, check the current
root/epoch/revision/target/mapping/build, and ensure unique operation/capture
identity. Never construct the expected context by deserializing this sidecar or
relabel an old observation with its capture ID. Neither an execution grant nor
the text `ProviderVerifiedStable` establishes a coherent live file snapshot.

The adapter validates the exact v1 identity and complete settings aggregate even
when no observation is available. It calls the existing settings-membership
audit only after the external capture-coherence and settings-path attestations
are present. It does not reinterpret the four limitations carried by that audit:
other identity sections remain unchecked, input closure is unproven,
consumption/behavior is unproven, and provider authentication remains external.

| Evidence | Result |
| --- | --- |
| Old producer or omitted sidecar | `Unavailable(EvidenceUnavailable)`. |
| Complete sidecar, no independently admitted observation | `Unavailable(EvaluatedAdditionalFilesUnavailable)`. |
| Null/missing settings list | `Unavailable(SettingsUnavailable)`. |
| Unknown collection mode, incomplete v1 input, unresolved coherent capture, or unverified settings paths | Corresponding `Unavailable` reason. |
| Bad schema/provider/JSON, stale binding, changed closure or stamp, malformed settings, aggregate mismatch, or exceeded cap | Corresponding `Rejected` reason. Rejection is advisory and cannot deny or authorize v1 reuse. |
| Qualified current observation and complete matching settings witness | `Advisory` containing the unchanged v1 audit result: Covered, CandidateGap, Unknown or Unsupported. |

The existing audit still distinguishes absent settings path, changed file digest
and changed length. Those are only membership candidates; they do not prove a
reuse error or actual generator consumption. Link/alias ambiguity,
metadata-sensitive order and dynamic generator reads retain their existing
Unknown/Unsupported dispositions. An unrelated file outside the admitted class
does not become an observation.

Input JSON is capped at **256 KiB before parsing**. The combined identity, stamp,
settings and context data are also checked against the existing 4096-record,
4096-byte-per-field and 1-MiB-total-text bounds before capture, sorting or
cloning into the diagnostic. Work is linear in bounded parsed text plus the
v1 identity and settings sorts, at most `O(N log N)` records. A qualified
observation intentionally invokes the legacy audit after sidecar validation, so
identity/settings validation may be repeated; this does not expand its bounds.

### Regression tests and production prerequisites

`tests/additional_file_evidence.rs` is authored regression source covering the
wire shape, old/missing input, unknown live coherence, independently supplied
path authority, same-name byte changes, stale target/capture, changed closure,
tampered whole settings list, duplicate keys, invalid schema/status and caps.
The UTF-16 settings-order vectors reuse the baseline's independent SG-13 wire
constants. Run the transport and managed capture tests with:

```sh
cargo +1.99.1 test --locked --manifest-path rust/evidence/receipts-coverage/Cargo.toml
dotnet test SlopHawk.slnx --filter "FullyQualifiedName~AdditionalFileEvidenceTests|FullyQualifiedName~BuildOutputIdentityTests"
dotnet test SlopHawk.slnx
```

Public use requires an exact AdditionalFiles evaluation and attribution
producer, coherent current capture and
complete alias-qualified mapping before enabling a public checker. For a later
reuse-error claim, run the owner's single-input cold-versus-reuse perturbation:
change only a selected generator input under the same logical name, compare its
key at the reuse decision, and compare the reused result with an independent,
deterministic cold result. Include an irrelevant-file control, represented
settings input, unresolved generator read, changed target/closure, linked path,
and mutation between evaluation and capture. The present sidecar cannot supply
that missing runtime evidence or authorize those executions.

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
