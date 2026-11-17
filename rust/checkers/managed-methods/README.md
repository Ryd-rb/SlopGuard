# Passive managed-method checker (SG-MIG-04)

Standalone Rust 1.99.1, edition 2024, standard-library-only source port of the
pinned C# `ManagedBodyComparison` at SlopHawk commit
`a367ef58076f7e29d2ae475a3b593daad04c4998`. The existing `probes methods`
route remains in C#; this crate is **not wired into the daemon, private IPC,
MCP, or production CLI**. Neither the library nor the standalone CLI executes,
reflection-loads, or JITs input assemblies.

## Public argv and library API

```
managed-methods probes methods <candidate.dll> <reference.dll>
managed-methods probes literals-names <assembly.dll> <name>...
managed-methods probes literals-operations <assembly.dll> <operation:arity>...
```

The first command returns the pinned C# record-array JSON shape with `Method`,
`Name`, `Change`, `CommonPrefixCount`, ordered `Candidate`/`Reference`
instruction excerpts and sorted distinct `IntegerLiterals`. The literal
commands return an object mapping operation names to lists of integer values.
The name form includes compiler-generated `<Method>g__` and `<Method>b__` methods.
The operation/arity form prefers direct useful values, then applies C#'s
arity-filtered histogram fallback (up to 16 candidates, frequency ascending,
value descending). For names with a colon use the final colon as the arity
separator. Values of `long.MinValue` are excluded from the useful/absolute
histogram, but preserved by direct literal extraction and comparison.

Library exports:

- `compare(&[u8], &[u8]) -> Result<Vec<ManagedBodyDifference>>`
- `integer_literals_for_names(&[u8], &[String]) -> Result<Vec<LiteralEntry>>`
- `integer_literals_for_operations(&[u8], &[(String, usize)]) -> Result<Vec<LiteralEntry>>`
- `differences_json` and `literals_json` for the C#-compatible JSON shapes

## Defensive allocation and validation

Input file reads cap at **64 MiB each**, metadata directories at **16 MiB**,
method IL at **1 MiB**, managed methods at **100,000**, and method/signature
recursion at **64**. A monotonic **32 MiB structural work budget per comparison
(or literal request)** charges non-input allocations before growing them,
including metadata name-cache copies, TypeSpec and generic expansions, decoded
UTF-16 user strings, escaped IL operands, instructions, branch maps, table
owners, locals, normalized IL records, diagnostic excerpts and result sets.
Individual formatted strings are capped at 2 MiB; final JSON at 8 MiB,
including escaping. Malformed PE/table/section ranges, signatures, compressed
integers, IL operands/targets and EH boundaries return typed errors. Unsupported
unoptimized pointer tables return `unsupported` instead of being guessed.
Limits are structural, not measured RSS: caller-owned assembly byte slices,
allocator overhead and toolchain memory are outside the counted budget.

Repeated `ldstr` references deliberately re-charge the large `#US` value and
its JSON-normalized form **before decoding/allocating** each occurrence. Complex
acyclic metadata graphs that expand signatures exponentially also hit the
formatting counter before any multi-megabyte concatenation. No untrusted input
can increase these caps. All test programs and fixture assemblies are owner-run
only; see `fixtures/README.md`.

This source is submitted without `Cargo.lock`. The owner must generate a genuine
lock with Rust 1.99.1 (there are no crate dependencies) before using `--locked`
or integrating the crate into an application or workspace.
