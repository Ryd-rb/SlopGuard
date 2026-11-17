# SG-RUST-1 — whole-block token-clone pilot

This standalone library ports the pretokenized `TokenCloneIndex.Upsert`, `Delete`, and
`Query` behavioral surface at pinned SlopHawk commit
`978a6464a6e5013fb1ad3a56c4e2877e62c18ac4`. It does **not** parse code, search
near clones, persist indexes, launch providers, or integrate with the daemon. The
reference is `src/SlopHawk.Core/TokenCloneIndex.cs` and its focused tests.

## Surface and identity

- `TokenCloneIndex::new(CloneIndexSettings) -> Result<…>` validates positive settings.
  `upsert(CloneTokenDocument)` validates the complete snapshot before removing prior
  postings, even when `version` repeats. `delete(&str)` removes both endpoints;
  `query(CloneQueryLimits)` reports matches, metadata, caps, statistics, and scope gaps.
- `CloneTokenKind` is a typed discriminator: `Other=0`, `LocalIdentifier=1`,
  `BoundIdentifier=2`, `Literal=3`. Only local names are replaced by their
  first-appearance ordinal, separately for each block; repetition pattern matters.
  Every candidate **rechecks** the original normalized and exact token sequences.
- Fingerprints are SHA-256 of, for every token, a **little-endian 32-bit kind**,
  **little-endian 32-bit UTF-8 byte length**, and the UTF-8 token text. The hash is
  indexed as 32 raw bytes and rendered uppercase hex in scope gaps. Pair IDs use
  the rule prefix `sg10.token-clones.v1` and **UTF-16 code-unit** string lengths.
  Visitation and result ordering follow .NET `StringComparer.Ordinal` on UTF-16,
  **not** Rust UTF-8 string order. UTF-16 offsets are caller-supplied and copied.
- Index tokens are two compact fields (`CloneTokenKind` and `u32`), with shared
  per-document interned text and 32-byte fingerprint keys. Data structures and
  query scratch are bounded by supplied document count, per-block token maximum,
  bucket caps, candidate/output limits, and the compared-token budget. A wholly
  in-memory map remains part of the pilot; no resident-memory guarantee is implied.

## Run with Rust 1.99.1

From the repository root, using a pinned Rust 1.99.1 toolchain and a native linker:

```sh
manifest=rust/checkers/clone-index/Cargo.toml
cargo +1.99.1 fmt --manifest-path "$manifest" -- --check
cargo +1.99.1 build --manifest-path "$manifest" --locked --offline
cargo +1.99.1 test --manifest-path "$manifest" --locked --offline
cargo +1.99.1 clippy --manifest-path "$manifest" --locked --offline --all-targets -- -D warnings
```

The crate has **no third-party dependencies**. `Cargo.lock` is local to the crate.

## Diffable C#/Rust fixture

The identical UTF-8 tab-delimited `fixtures/parity.tsv` file is consumed by
`clone-parity` and `tools/SlopHawk.RustCloneParity`. Run the C# harness after
restoring/building the solution with .NET 10 and run the Rust fixture in release:

```sh
dotnet run -c Release --project tools/SlopHawk.RustCloneParity/SlopHawk.RustCloneParity.csproj \
  -- rust/checkers/clone-index/fixtures/parity.tsv > /tmp/clone-csharp.txt
cargo +1.99.1 run --manifest-path rust/checkers/clone-index/Cargo.toml \
  --locked --offline --release --bin clone-parity \
  -- rust/checkers/clone-index/fixtures/parity.tsv > /tmp/clone-rust.txt
diff -u /tmp/clone-csharp.txt /tmp/clone-rust.txt
```

Trace record `Q` reports settings/limits/statistics, `P` contains ordered pair IDs,
classification, versions, half-open offsets and counts, and `G` contains ordered
scope gaps. The fixture covers Unicode UTF-16 ordering (`𐐀` versus ``), token
framing with Unicode (`é`, `🌙`), exact + consistently renamed matches, a
one-token near miss, generated/excluded and oversized blocks, overfull postings,
three separate query budgets, repeated version, deletion and cold rebuild. The
near miss **must not be reported as a match** by this pilot.

The runner is deliberately a minimal file adapter around the existing C# class;
no source parsing is involved. The C# focused test command at the original
checkout remains:

```sh
dotnet test SlopHawk.slnx --filter FullyQualifiedName~TokenCloneIndexTests
```

## Matched synthetic **engine-only** measurements

Run both release-mode binaries on the same Linux host and record stdout outside
this repository. Use the same four positional parameters and input generator:

```sh
dotnet run -c Release --no-build --project tools/SlopHawk.RustCloneParity \
  -- --bench 30 64 5 10000
cargo +1.99.1 run --manifest-path rust/checkers/clone-index/Cargo.toml \
  --locked --offline --release --bin clone-bench -- 30 64 5 10000
# Repeat each with 600 256 5 10000
```

Each `M` row has columns `engine, docs, tokens, repetitions, candidate cap,
sample, phase, elapsed µs, PSS before KiB, PSS after KiB`. All rows use identical
prebuilt input documents, one block per document, a group of three documents
sharing a token sequence, `MinimumTokens=4`, maximum 4096, maximum postings 64,
comparison budget 2,000,000, and the same replacement and deletion. Five
**actual** samples of each phase in each case are required. Timings exclude
process startup, fixture precomputation, PSS sampling, provider parsing, daemon
work, and serialization; PSS samples are live-process snapshots and **not
peak memory**. The reported `cold_query` is the first query after upsert on a
new index, while `warm_query` immediately repeats it. This is *not* an
end-to-end product benchmark. Report operating system and runtime versions,
median and p95, phase and PSS, and separate toolchain/measurement gaps.

## Differences, unsupported and follow-on

The Rust type system excludes C# `null`, undefined `CloneTokenKind` enum integers
and ill-formed UTF-16 strings; malformed UTF-16 `.NET string` serialization is
not represented by a Rust `String`. The integer settings accept Rust `usize`
counts; the API validates all values that have C# equivalents. This pilot
intentionally has no persistence, IPC, providers, overlap/near-match search,
or daemon integration. Whole-block parity and native measurements must be
verified before promoting it beyond this proof of concept.

### Golden reference trace

`fixtures/parity.expected.tsv` contains the actual stdout captured on Linux
from the compiled pinned C# `TokenCloneIndex.cs`, via this packet's C# harness.
The Rust integration test `tests/parity_trace.rs` runs the Rust binary against
`parity.tsv` and checks **every output byte**, including pair ordering, stats,
versions, offsets, caps, gaps, and post-update/cold-rebuild states. This makes
an offline Rust-only parity regression possible after the initial reference
capture. Refresh the golden only after validating a deliberate C# rule change.
