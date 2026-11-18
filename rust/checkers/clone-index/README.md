# SG-RUST-1 — whole-block token-clone pilot

This standalone library ports the pretokenized `TokenCloneIndex.Upsert`, `Delete`, and
`Query` behavioral surface at pinned SlopHawk commit
`978a6464a6e5013fb1ad3a56c4e2877e62c18ac4`. The current native checker calls the
v1 index; the historical pilot description is not its current wiring status.
The separate `query_near_misses` API below is opt-in and has no native report or
scheduler wiring. This crate does not parse code, persist indexes or launch
providers. The
reference is the [C# implementation and harness at commit `7196845`](https://github.com/Ryd-rb/SlopHawk/tree/71968455a60a716bd4cc46ecf4e210b0410f9860).

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

## Opt-in bounded near misses

`TokenCloneIndex::query_near_misses(NearMissOptions, NearMissLimits)` returns a
`NearMissQueryResult`; the new public types live in `near_miss`. It borrows the
existing index and rebuilds temporary candidates for each call. The existing
`query`, match classes, IDs, fingerprints, defaults, budgets, ordering, parity
adapter and replacement/deletion logic are unchanged. A near query consumes
only its own limits and never updates the index.

This is a whole-block syntactic predicate over the **existing v1 normalized
typed tokens**. Let `m,n` be the block lengths and `L` their longest common
subsequence length. Insertion/deletion distance is `D = m+n-2L`; insertion and
deletion cost one, replacement costs two, and transposition is not a primitive.
Similarity is `2L/(m+n)`, matching the aligned-token score in `docs/checkers.md`.
Acceptance uses integer arithmetic:

```text
0 < D <= min(maximum_edits, floor((100 - minimum_similarity_percent) * (m+n) / 100))
```

The default near profile is 80 percent and at most 64 edits. These are tuning
choices, not measured accuracy or latency guarantees. Distance-zero exact or
renamed pairs stay in v1's domain, even when a v1 posting cap hid them. Changing
a bound identifier, literal, operator or token kind is never normalized away;
it can still be an accepted nonzero edit. A near match does not prove equivalent
behavior, safe refactoring, a shared defect or actionable duplication.

Only locals normalize to first-occurrence ordinals. Inserting an earlier new
local can shift other ordinals and cause a miss. The result explicitly lists
unsupported semantic equivalence, provider classification validation, renaming
invariance across edits, sub-block source locations and language/context
partitioning. The caller must partition inputs and preserve provider frontiers.

### Candidates and ordered confirmation

Admit metadata-bounded blocks in .NET UTF-16 ordinal `(document_id, block_id)`
order. Token and token-byte budgets stop at the first block that will not fit;
they do not partially index that block. Intern typed atoms across this admitted
snapshot, preserving exact text and local ordinals as separate values. Rank
normalized atoms by increasing admitted-block document frequency, then by their
deterministic atom ID. Rebuild frequencies and rank on each query.

For integer threshold `theta`, nonempty block length `m` and edit bound `E`, use
`r = max(m.saturating_sub(E), ceil(theta*m/(200-theta)))`. Sort token occurrences
by the shared rank and index their first `m-r+1` occurrences. Preserve
multiplicity until after prefix selection, then deduplicate each block's posting.
Any accepted pair has multiset overlap at least its LCS, and its LCS is at least
each block's `r`; disjoint prefixes would contradict that bound. This is a
necessary candidate condition before caps, not a score or final match decision.

Suppress an entire prefix posting when it exceeds its cap; other shared prefix
atoms may still retrieve the pair. Enumerate postings by `(frequency, atom ID)`,
bound all posting-pair visits including duplicates, and retain bounded distinct
candidate key pairs. Verification visits those pairs in source-key order.
Length difference is a necessary distance filter. Compact two-row diagonal-band
DP then computes the exact minimum distance within the cutoff, maximizing exact
token matches among equally short normalized alignments. Boundary/initialized
cells consume the cell budget; exhaustion is distinct from proven rejection.

Each accepted pair reports both original whole-block locations, versions and
lengths, distance, normalized aligned count, maximum exact aligned count and
score. No token-to-source edit mapping is inferred. The pair namespace is
`sg10.token-near-misses.v1`; UTF-16 length framing and endpoint ordering match
v1's conventions. The result separately names the unchanged v1 normalization.
Same-document overlapping half-open spans are excluded and counted; disjoint
same-document blocks are allowed. Other nested duplicates are not collapsed.

### Bounds and incomplete results

| Limit | Default | What is bounded |
| --- | ---: | --- |
| `maximum_blocks` | 4096 | Retained/admitted eligible block references |
| `maximum_tokens` | 262144 | Copied token occurrences across admitted blocks |
| `maximum_token_bytes` | 16777216 | Exact UTF-8 token bytes, counting repeated occurrences |
| `maximum_metadata_bytes_per_block` | 16384 | Document ID, version and block ID bytes combined |
| `maximum_postings_per_token` | 64 | Prefix posting membership before whole-bucket suppression |
| `maximum_posting_visits` | 100000 | Posting pair visits, including duplicates |
| `maximum_candidate_pairs` | 20000 | Stored distinct nominated pairs, before other exclusions |
| `maximum_alignment_cells` | 2000000 | Total active DP cells, including initialization |
| `maximum_output_pairs` | 1000 | Returned pairs |

Every limit must be positive; zero returns `InvalidLimits`. Percentages outside
1..=100 or zero maximum edits return `InvalidSettings`. A 100-percent query is
valid and returns no positive-distance matches. Whole-block oversized metadata
and index-oversized blocks are summarized separately. Generated, excluded and
below-minimum inputs are policy exclusions with visible counts.

`is_truncated()` means a recorded scope/budget gap exists. Gap counts for
oversized blocks, metadata exclusions, unselected blocks and suppressed buckets
are exact counts of those objects. Other counts denote an encountered frontier,
not an estimate of all missed pairs. Suppression can be conservative: another
seed may recover a pair, or unvisited work may only duplicate known candidates.
An empty truncated result is not evidence that no qualifying pair exists.

Selection still scans all stored document/block metadata. `maximum_blocks`
bounds retained references, not catalog-scan time or the pre-existing index.
After admission, scratch memory is linear in the admitted tokens/blocks plus
capped candidates, output and a band of at most `2*cutoff+1` cells per row.
Token bytes bound string comparisons during interning; metadata bytes bound
key comparisons and owned result strings. DP uses integer atom IDs. Initialization
clears only the active band. These are finite input/operation bounds, not a
wall-clock deadline, allocation-success promise or resident-memory measurement.

Returned pairs sort by decreasing exact rational similarity, then decreasing
combined token length, then UTF-16 pair ID. The output cap applies during source
pair visitation, before this final sort: results are **not a global top-K**.
Caps can change which pairs are admitted; increasing one cap does not guarantee
a result superset under other fixed caps. Repeated queries, input permutations
and cold reconstruction with identical contents and limits are deterministic.

Provider/report/scheduler opt-in wiring, literal bucketing, sub-block refinement,
maximal-span collapse, cancellation and durable posting maintenance are separate
integration work. The native checker currently still reports near support as
unsupported. Draft regressions are in `tests/near_miss.rs`; semantic validation
and performance measurements are owner-side work.

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

The Rust `clone-parity` binary consumes the UTF-8 tab-delimited `fixtures/parity.tsv`
and is tested against the captured C# output. To regenerate the reference output,
set `reference` to a checkout of `71968455a60a716bd4cc46ecf4e210b0410f9860` and run
the historical C# harness with .NET 10:

```sh
dotnet run -c Release --project "$reference/tools/SlopHawk.RustCloneParity/SlopHawk.RustCloneParity.csproj" \
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
dotnet test "$reference/SlopHawk.slnx" --filter FullyQualifiedName~TokenCloneIndexTests
```

## Matched synthetic **engine-only** measurements

Run both release-mode binaries on the same Linux host and record stdout outside
this repository. Use the same four positional parameters and input generator:

```sh
dotnet run -c Release --no-build --project "$reference/tools/SlopHawk.RustCloneParity" \
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
counts; the API validates all values that have C# equivalents. This crate has no
persistence, IPC or providers. The opt-in near API operates on whole supplied
blocks; it does not implement sub-block search or add native reporting.
Whole-block parity and native measurements remain owner validation gates.

### Golden reference trace

`fixtures/parity.expected.tsv` contains the actual stdout captured on Linux
from the compiled pinned C# `TokenCloneIndex.cs`, via this packet's C# harness.
The Rust integration test `tests/parity_trace.rs` runs the Rust binary against
`parity.tsv` and checks **every output byte**, including pair ordering, stats,
versions, offsets, caps, gaps, and post-update/cold-rebuild states. This makes
an offline Rust-only parity regression possible after the initial reference
capture.
