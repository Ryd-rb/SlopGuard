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
slophawk-contracts-probes check-contracts candidate.dll rules.json published.json workspace [per-operation [run-verb [prepare-verb]]]
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
