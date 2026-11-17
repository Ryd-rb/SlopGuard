# SG-MIG-09: bounded persisted facts and SG-10 clone postings

This is an **independent, opt-in Rust library and foreground executable**, not a daemon replacement, parser, inventory authority, or execution grant. It is deliberately confined to `rust/storage/fact-index/`. It consumes preclassified `Document` snapshots and captured `RoslynDocumentFacts` schema-v1 JSON without a dependency on sibling migration tasks. Current SlopHawk routing remains unchanged; an integration owner must explicitly supply a trusted producer, lease, checkpoint policy, and revision fences before enabling it.

## Pinned sources and dependency policy

The intended base is SlopHawk commit `a367ef58076f7e29d2ae475a3b593daad04c4998` (tree `fa4f03394ea061818423a1530831f6868942a94d`). The existing Rust SG-10 clone pilot, `rust/checkers/clone-index/src/lib.rs`, defines `sg10.token-clones.v1`. The C# `TokenCloneIndex` and parity fixture specify pair IDs, UTF-16 ordinal ordering, two-pass matching, statistics and explicit omissions. `RoslynDocumentFacts` supplies provider-qualified evidence, not a clone tokenizer. `SourceInventory` remains the authority for confirmed source revisions, not this cache.

- Rust `serde = 1.0.229`, `serde_json = 1.0.154` and `sha2 = 0.10.9` are versions present in pinned Rust lockfiles. They are **not** a generated lock for this new crate.
- `rusqlite = 0.37.0` with feature `bundled` uses `libsqlite3-sys = 0.35.0`, whose bundle targets SQLite **3.50.2**. SQLite is public domain; `rusqlite` is MIT. The proposed target platforms are Windows x64 and Linux x64 with the native C build prerequisites audited during owner integration. These crate versions and bundled native bytes **still require owner lock generation, review and digest verification**; this source-only delivery did not resolve, download or build them.
- The daemon's separate NuGet lock pins `SQLitePCLRaw.lib.e_sqlite3 = 2.1.13` (plus `Microsoft.Data.Sqlite = 10.0.14`). That is an **owner-side C ABI native asset alternative**, not an ABI-equivalent package, a binary redistributable, or a dependency of this crate. No native artifact is included here.

Relevant primary SQLite behavior is documented in [C API](https://www.sqlite.org/c3ref/intro.html), [atomic commit](https://www.sqlite.org/atomiccommit.html), [limits](https://www.sqlite.org/limits.html), [WAL](https://www.sqlite.org/wal.html), and [public-domain license](https://www.sqlite.org/copyright.html). The bounded resource proposal comes from `docs/semantic-index.md`; the migration requirements and failure obligations come from `docs/rust-checker-migration.md` and `docs/robustness.md` at the pinned tree.

## Durable contract

`FactIndex::open(db_path, root_id, Settings)` initializes a derived SQLite partition when missing, validates `application_id="SFI9"`, `user_version=1`, object inventory, foreign keys, SQLite `quick_check`, persisted settings and physical checkout root on reopen, and refuses unknown or corrupt formats instead of destroying their contents. A schema/version/setting change requires an explicit owner-selected rebuild of this **derived** partition. The current writer must be serialized externally; readers obtain short revision-consistent SQLite transactions. WAL is local-only and not a distributed lease. Do not put the database on an unsupported network filesystem.

`FactIndex::apply(Batch)` requires the exact previous committed revision and a `Fence` containing root ID, epoch, **next** committed revision and exact input/tool/policy SHA-256 strings. A batch of up to 128 operations is one SQLite `IMMEDIATE` transaction. `replace` atomically withdraws old blocks, facts and both postings, then inserts the new snapshot. `delete` removes document and posts. `rename` updates document identity, optional source ID, paths, context and version, including posting sort keys and referring fact/block IDs under cascaded foreign keys. `invalidate` withdraws derived evidence and postings while retaining a `valid=0` source marker. Unknown IDs, duplicate block IDs and bad spans reject the transaction. Capture of a published Roslyn JSON frame retains its context, logical and physical paths, schema/tool versions, declaration/reference/call/frontier payloads and source spans, but does not infer clone tokens. A repeated version does not imply a no-op.

`query(&Fence, QueryLimits)` only sees the exact recorded revision and verified current block sequences. It uses SQLite-backed **64-entry keyset pages**, independent exact and normalized SHA-256 postings, 64-fanout upper bound, and token-by-token collision verification in the same snapshot. Only provider-proven `LocalIdentifier` tokens are renamed, by first occurrence. Kind, bound name, punctuation and literal remain exact. The digest is the existing SG-10 SHA-256 stream over little-endian `i32` kind, little-endian `i32` UTF-8 text length and text bytes. Pair IDs use lengths counted as UTF-16 code units, and all sorting uses .NET ordinal UTF-16 comparison, including supplementary-plane characters. Exact pass always precedes normalized pass, output is ordered by class, descending token count then ordinal pair ID; `seen` prevents double reporting. Full-block scores are 1.0. No near clones, spelling, or new inspections are introduced.

`facts_page(&Fence, document_id, from_ordinal, max_records)` reads at most 128 provider records and 1 MiB decoded JSON per call. Its cursor is an ordinal scoped to the exact fence; a changed revision returns `Stale`, not a mixed result. An invalidation leaves no queryable facts. All public query results include the fence and the provider classification context.

## Limits, frontiers and recovery

| Resident allocation / resource | Proposed bound / behavior |
| --- | --- |
| SQLite page cache | `cache_size=-8192` (8 MiB; no memory mapping); no read-connection pool is allocated |
| Decoded tokens and strings | 8 MiB planning allowance; a document has at most 4096 blocks and 20,000 facts, each token at most 256 bytes by default; one candidate's token bodies at a time |
| Candidate pages | 1 MiB planning allowance; keyset pages of 64 block headers, fingerprint buckets bounded to 64 entries |
| Prepared SQL/SQLite scratch | 1 MiB planning allowance; short scoped statements, single connection |
| Input queue | 4 MiB (one framed request, checked **before** allocation); library enforces a corresponding aggregate batch allowance |
| Output queue | 4 MiB (one framed terminal, checked before emission); query additionally budgets 2 MiB estimated pair output |
| Miscellaneous reserve | 6 MiB for allocator capacity, metadata and temporary strings; owner must profile actual resident and peak consumption |
| Disk and journal | 256 MiB proposed per-checkout total DB + `-wal` + `-shm`; SQLite `max_page_count`, 8 MiB WAL journal limit, immediate checkpoint attempt after commit, and a minimum 8 MiB headroom before optional writes; deletion/invalidation may progress under pressure |
| Query work | 20,000 candidate pairs, 2 million compared tokens, 10,000 output pairs, 200,000 scanned headers, 512 gaps, 2 MiB estimated pair output by default; all limits validated against finite ceilings |

`gaps` explicitly identify `OversizedBlock`, `BoilerplatePostingLimit`, `CandidatePairLimit`, `ComparedTokenLimit`, `OutputPairLimit`, or additional `ScannedBlockLimit`, `GapLimit`, `OutputBytesLimit`. These are incomplete scope evidence, never absence proofs. A busy post-commit checkpoint or disk quota excess is reported in `CommitReceipt.retention_frontier`: the **transaction is already committed** and the integrator must not replay it as uncommitted. Revalidate physical root, source inputs and policy before any retry. Long readers and WAL growth need owner-managed revocation and checkpoint schedules; no automatic destructor removes an unknown database. The disk quota is an admission/checkpoint policy, **not a formal filesystem hard limit** on WAL growth; do not claim it was measured or enforced end-to-end before qualification.

## Foreground executable and transport

The executable `fact-index <sqlite-db-path> <physical-root-id>` reads a **single** little-endian 32-bit length-prefixed UTF-8 JSON request from standard input and writes a **single** same-format JSON terminal to standard output. Request and terminal limits are 4 MiB. Invalid/missing framing has no trusted identity, causes a bounded diagnostic on stderr and **does not emit a terminal**; an orchestrator must discard staged output. The transport version is `slophawk.migration-job/1` and uses strict tagged JSON actions: `ingest` (a `Batch`), `query` (a `Fence` and optional limits), `facts_page` (fence/document/ordinal/page size), and `state`. The envelope is:

```json
{
  "protocol": "slophawk.migration-job/1",
  "identity": {
    "operation": "state", "job_id": "local-read-1", "root_id": "checkout-identity",
    "context_key": "*", "epoch": 0, "committed_revision": 0,
    "input_digest": "0000000000000000000000000000000000000000000000000000000000000000",
    "tool_digest": "0000000000000000000000000000000000000000000000000000000000000000",
    "policy_digest": "0000000000000000000000000000000000000000000000000000000000000000",
    "budget_ref": "caller-budget-identifier", "grant_ref": "caller-grant-identifier"
  },
  "request": { "action": "state" }
}
```

The initial `state` fence is zeroed. All other actions echo a fully identical `identity` including operation/job/root/context/epoch/revision/input/tool/policy and budget/grant references. The terminal has `protocol`, `identity`, `status` (`complete`, `partial`, `unsupported`, `deferred`, `cancelled`, `stale`, `failed`), bounded `reason`, bounded `omissions` and either a bounded `result` or `null`. A publication consumer must verify identity and terminal status **and** child exit/containment before accepting staged results. Repository-controlled budget/grant reference strings are inert: they **do not authorize execution or publication**. Only a caller's external policy can do so. This executable launches no tool, parser, worker, or executable sourced from a repository. The current v1 supports `context_key="*"` as a workspace-wide scope only; finer scopes need a separately versioned contract.

## Authored regression source (not executed here)

`tests/lifecycle.rs` covers atomic replace/rename/invalidate/delete, facts and UTF-16 spans after reopen, stale fences, input failure rollback, conflicting changes, interrupted SQLite transactions, unknown schema/root and corrupt postings, fanout and limits, generated and excluded documents, Roslyn frame-v1 preservation. `tests/differential.rs` directly compares pair IDs, classes, spans, order, statistics and gaps to the *published* Rust SG-10 library across edit/delete/reopen/cold rebuild and token/candidate/output budgets. Unit tests in `src/main.rs` cover the bounded framed encoding and terminal identity behavior. All runtime execution, performance measurements, native dependency resolution, memory qualification and Windows/Linux correctness remain **deferred to the integration owner by instruction**.
