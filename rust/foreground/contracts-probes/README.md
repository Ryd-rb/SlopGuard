# Foreground contracts and probes (Rust, opt-in pilot)

This crate provides a separate `slophawk-contracts-probes` binary. It does **not** replace
`slophawk`, the .NET CLI, daemon, MCP or private IPC. Its pure decisions use the exact
published `rust/checkers/contracts` and `rust/checkers/probe-replay` public APIs at
SlopHawk commit `a367ef58076f7e29d2ae475a3b593daad04c4998`.

## CLI shape

```text
slophawk-contracts-probes contracts qualify rules.json published.json
slophawk-contracts-probes probes plan seeds.json count [seed [scales.json]]
slophawk-contracts-probes probes compare inputs.json candidate.json reference.json
slophawk-contracts-probes probes methods candidate.dll reference.dll
slophawk-contracts-probes check-contracts [--diverse] candidate.dll rules.json published.json workspace [per-operation [run-verb [prepare-verb]]]
```

Exit 0 is qualified/clean, exit 1 is qualification/search findings, replay changes or
unsupported input, and exit 2 is invalid usage, missing/bad peer evidence or other
infrastructure failure. All command payloads use the existing C# PascalCase JSON
field names, including `slophawk-contract-search-v1` qualification and summary,
`slophawk-check-v1` replay reports and exact SHA-256 of source bytes (uppercase).
`probes methods` reads assemblies with the [Rust managed-method checker](../../checkers/managed-methods/README.md).
The native public shell shares the contract qualification and passive probe commands.

## Scoped adapter execution and evidence

`check-contracts` uses the shared Rust foreground checker for ordered batches.
The checker executes the supplied .NET adapter and optional preparation verb
within the OS worker scope. It validates the candidate SHA-256 and ordered
requests and captures bounded observations before publishing evidence.
If the peer fails or evidence disagrees, the attempt staging tree is discarded.
Across batches the candidate must remain byte-identical. Successful initial
search receipts stay under contract-search evidence; a separate successful replay
is promoted to `.slophawk/checks/<run>` or a stable subset report is published
there. A `ReplayChanges` element retains `Before` and `After` as unnormalized JSON,
including `null`, newly appeared and vanished mismatches. Missing or unstable
replay never authorizes publication of that witness.

`check-contracts` uses the explicit assembly supplied by the operator as the
adapter. Contract qualification and passive probes read their inputs without
starting a worker.

## IL-literal hints (documented *data* boundary)

C#'s `ManagedBodyComparison.IntegerLiterals` is **internal**, and `probes methods`
outputs comparisons, not the per-operation literal dictionary. To avoid claiming
this internal method is a public command, the Rust binary accepts optional data
from a separately admitted C# IL-literal provider:

```text
SLOPHAWK_IL_HINTS_JSON=/trusted-scratch/literals.json
SLOPHAWK_IL_HINTS_SHA256=<uppercase SHA-256 of exact literals.json bytes>
```

The UTF-8 JSON must be a versioned envelope bound to the exact candidate
assembly hash and each qualified operation's arity. For example:

```json
{"Format":"slophawk-il-literals-v1",
 "AssemblySha256":"<UPPERCASE ASSEMBLY SHA-256>",
 "Operations":[{"Operation":"Route","Arity":4,"Literals":[31,67]}]}
```

The peer/provider must determine the **same assembly hash and operation arities**
before emitting the file, publish it in a trusted scratch path, and supply the
exact file SHA-256 out of band. The Rust consumer bounds file size, count and
types and rejects mismatched assembly hashes, unqualified operation/arity pairs,
and duplicate entries. It honors the exact file digest. It does not use these values as rules, expected outcomes
or execution grants. With no hint file, the second pass still performs rule-based
guided boundaries, equalities and combined-condition search, but lacks
implementation-literal collision guidance. Providing this provider is an owner
integration action because the pinned C# command does not export these hints.

## Qualification and search decision provenance

Qualification is against **published** `ProbeExpectation` examples; the domain,
ordered `Let` locals, `Basis`, expression compiler and signed arbitrary-precision
arithmetic are the pinned Rust `ContractCases` implementation. Generation uses
`.NET 10` seeded `System.Random(int)` order, one RNG per operation, mixed arities,
per-operation budgets and dedup; an initial (up to 64) seed-0 pass precedes a
seed-1 guided pass if budget/remaining operations permit it. Batches hold no
more than 2048 cases. The first mismatch per operation is selected, first match
is retained as a control witness, and all witnesses are checked in a **new**
peer execution. Candidate outputs never create expected values.

The Rust planner has explicit scope ceilings (64 contracts, 8192 cases per
operation, 65536 per generated call). The orchestrator requests one operation
at a time to avoid the single-call ceiling while preserving per-operation RNG
state and concatenated ordered batches. Inputs beyond those ceilings fail
closed with an explicit error; none are silently sampled. The Rust foreground
engine remains opt-in until the owner executes differential qualifications.

## Diverse live allocation audit

With `check-contracts --diverse`, Summary and Review include the same optional
`SelectionAudit` object, with `Format: "slophawk-contract-selection-audit-v1"`.
`CaseSelection: "diverse-v1"`, the outer document formats, default budget 512,
the 1..=8192 budget range, seeds, candidate order, and execution authorization
are unchanged. Legacy searches omit the new object. An older diverse receipt
without it has **unavailable accounting**, not zero work. The small stdout
receipt continues to link to Summary and Review rather than copying the audit.

`GeneratorBasis: "emitted-legal-nonpublished-cases"` counts only cases returned
by `ContractCases::generate`. That producer already removes published inputs,
duplicate proposals, invalid domains and unavailable expectations. Its internal
attempt/rejection counts are not exposed. A completed empty generator is not
proof that the domain has no other legal inputs. Published anchors can influence
selection features but never count as new executions or observed coverage.

`ExecutionBasis: "validated-observations"` means the peer returned an observation
whose ordered inputs, counts, findings and candidate identity passed validation.
It does not prove a particular target body or branch ran, and it excludes failed
or incomplete adapter attempts. No input-partition or program coverage metric
is produced. Missing or invalid observation evidence remains an error.

`Passes` is ordered by pass, then qualified-operation order. Pass 0 reserves
`min(perOperation, 64)` calls; pass 1, when configured, reserves the remaining
`perOperation - 64`. Each generated pool has cap `min(4 * Budget, 8192)`.
Only qualified operations receive audit rows; qualification errors remain in
the existing `Operations`/qualification records.

| Field | Unit and meaning |
| --- | --- |
| `Operation`, `Pass`, `Budget`, `PoolLimit` | Operation, seed/pass 0 or 1, reserved search calls, and emitted pool cap. |
| `Generation` | `completed` means bounded generation returned, even when empty; `skipped-after-finding` means no pool was requested for a stopped operation. |
| `Nominated` | Legal, distinct, nonpublished rows emitted in this pass, before excluding prior executions. The same input can be nominated again in another pass. |
| `PreviouslyExecuted`, `Eligible` | Emitted rows already seen in a validated initial batch, including unsupported rows; the remaining pool supplied to the selector. |
| `Selected`, `NotSelected` | Shortlisted rows and eligible rows not shortlisted within this pass's quota. Selection alone is not an observation. |
| `SkippedAfterFinding`, `SkippedDuplicate` | Selected rows dropped at flush because an earlier batch found that operation, or because the exact request was already admitted. The finding reason takes precedence. |
| `Executed`, `Compared`, `Unsupported` | Validated initial observations; non-null supported comparisons; null unsupported observations. All returned rows in a batch count, including rows after its first mismatch. |
| `UnusedBudget` | Reserved calls minus validated initial observations. Unused slots are not transferred to another operation or pass. |
| `NominatedInputsSha256`, `SelectedInputsSha256` | Ordered input identities. Completed empty vectors hash `[]`; skipped generation has null digests. |

Every completed pass record satisfies these conservation rules:

```text
Nominated = PreviouslyExecuted + Eligible
Eligible = Selected + NotSelected
Selected = Executed + SkippedAfterFinding + SkippedDuplicate
Executed = Compared + Unsupported
Budget = Executed + UnusedBudget
```

`Batches` holds one record per validated initial peer call, with `Pass`, `Run`,
`Report`, `InputsSha256`, `Executed`, `Compared` and `Unsupported`. Read the
report's sibling `requests.json`, `observed.json` and `identity.json` to audit
the ordered actual cases and independent expectations in `report.json`.
`Replay` is null when no supported witness exists; otherwise it records the
separate witness call using the same observation fields. It is not charged to
the search quota or included again in initial `Compared`. Promotion relocates
its `Report` while preserving `Run`. For a stable subset, it still identifies
the full original replay, not the subset's new run. Existing `ReplayChanges`,
`ReplayUnsupported`, published findings and retest indices remain authoritative.

Input digests are uppercase SHA-256 of UTF-8 compact JSON from `inputs_json`:
ordered objects with `Operation` followed by `Args`, without expectations.
They identify stages, not recoverable input lists or tamper-proof seals.
Reconstructing an omitted nomination/shortlist requires the exact generator,
rules, examples and hints; a digest alone cannot supply those inputs. The
existing rule/published hashes bind the loaded snapshot, not a later file edit.
The audit does not authorize new execution or extend `check-review` coverage.

The live limits imply at most 128 pass records, 256 initial batch records and
one replay record, independent of how many nominations are discarded. At the
maximum budget, at most 540672 rows are emitted across both passes and 524288
initial requests can receive observations; replay adds at most 64 witnesses.
Generator attempts, guided-source preparation and oracle work have separate
existing caps. Hashing adds work proportional to serialized input bytes and
temporarily retains one generated pool while selecting. Names and paths still
contribute variable bytes; no new hard Summary/Review byte limit is introduced.

`diversify_cases` and CLI/MCP `contract_diversity` planning remain read-only
over caller-supplied evidence, with their existing caps and result schema.
They do not expose this live audit or claim adapter execution. The selector's
same-pool prefix property remains limited: at budget two, a diversity choice
can omit the only failing second pool entry in favor of a harmless boundary.
The regression fixture in the contracts crate records this fixed-budget limit.

## Integration prerequisites and static provenance

Target toolchains: Rust 1.99.1, .NET 10. `serde=1.0.229`,
`serde_json=1.0.154`, and `sha2=0.10.9` are versions appearing in pinned
Rust-project lockfiles; `serde_json`'s `preserve_order` and `raw_value` features
are required. No `Cargo.lock` is shipped from this source-only change. The
owner must regenerate and commit the lockfile in an authorized build context.

Design: `src/SlopHawk.Cli/{ProbeCommands,ContractCheck,ForegroundCheck,CheckReview}.cs`,
`src/SlopHawk.Core/{ContractCases,ContractConditionCases,ProbeReplay}.cs`,
`docs/rust-checker-migration.md`, `docs/robustness.md`, and the pinned published
Rust checker crates. Execute source and differential tests only at integration;
this recipient pass intentionally neither builds nor runs fixtures.
