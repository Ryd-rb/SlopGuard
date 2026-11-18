# Native clone query evidence

The native source checker receipt adds `cloneQueries` to
`slophawk.native-shadow/1`. Each requested language has one row, ordered by the
existing `Language` enum. There are at most four rows. This is evidence about
the existing exact and renamed-local index query; it adds no checker rule,
source parser, dependency, index, configuration, or execution route.

## Three different scopes

`requestedDocuments` counts the request's admitted source documents for that
language. `statistics.documentCount` counts documents actually supplied to the
index. Sources without a mapped clone block are absent from that second count.
`statistics.indexedBlockCount` counts blocks retained by the index after its
generated/excluded-source and minimum/maximum-token policies. These numbers
are not interchangeable. The legacy per-document `blocksIndexed` and
`tokensIndexed` fields continue to describe provider-projected blocks before
index policy; their meaning is unchanged.

| Query status | Meaning |
| --- | --- |
| `not_run` | No index query was made for this requested language. `statistics` is omitted. `notRunReason` is `failed_batch` after staged facts were discarded, otherwise `no_admitted_blocks`. |
| `complete` | The existing index query returned without query gaps over eligible indexed blocks. Policy exclusions remain visible in statistics. An empty eligible index can have this status. |
| `partial` | The index query reported at least one budget or oversized-block gap. Returned pairs are a bounded subset of its possible results. |

Query completion does not establish provider completeness, whole-source
semantic coverage, source correctness, or near-clone support. Consult the
unchanged document/rule statuses and original gaps as well. In particular, a
query can be complete while a requested source has an unresolved frontier or
an unavailable byte mapping. The receipt's overall status is not promoted.

## Row fields

Every row includes the language, `ruleVersion` (currently
`sg10.token-clones.v1`), requested document count, query status, `gapReasons`,
`pairsReturned`, `pairsAccepted`, and `pairsRetained`. A query that ran also includes a
field-for-field copy of the index's statistics:

| Statistic | Meaning |
| --- | --- |
| `documentCount` | Documents passed to the index, including generated/excluded documents. |
| `indexedBlockCount` | Blocks eligible for fingerprint lookup. |
| `generatedDocuments`, `excludedDocuments` | Documents suppressed by index policy. The current native adapter always supplies `is_excluded: false`. |
| `belowMinimumBlocks`, `oversizedBlocks` | Block-size exclusions among ordinary, nonexcluded documents. Oversized blocks also generate query gaps. |
| `candidatePairsExamined` | Candidate comparisons admitted by the bounded query; not the total number of possible pairs. |
| `comparedTokensCharged` | The index's charged token-work count, not measured CPU time or parser memory. |

`gapReasons` contains at most five distinct typed values:
`oversized_block`, `boilerplate_posting_limit`, `candidate_pair_limit`,
`compared_token_limit`, and `output_pair_limit`. It is a compact, ordered
summary. The pre-existing global gaps retain their detailed scope keys and
affected counts. No consumer needs to parse those strings just to distinguish
the five query-limit categories.

`pairsReturned` counts lexical pairs returned by the bounded index query.
`pairsAccepted` counts pairs that pass language-specific qualification.
Python layout qualification can reject a lexical pair. `pairsRetained`
counts the accepted pairs still present in the serialized receipt for that
language. The report byte cap decreases only the retained count, records
`report:pair_budget`, and leaves query status and statistics unchanged.
Scope, gap and query metadata must all fit for serialization to succeed.

## Hand-worked example

Two ordinary Python sources containing `def f():\n    return 1\n` each have
seven substantive tokens: `def`, `f`, `(`, `)`, `:`, `return`, `1`. Each has
one function block. With the existing default limits the independent expected
accounting is two requested/index documents, two eligible blocks, one
candidate comparison, fourteen charged tokens, and one exact pair. The query
can be `complete` while near-clone support remains `unsupported` in both
document reports. A third identical source declared generated changes the
document and generated counts; it does not add an eligible block or pair.

In contrast, `x = 1\n` has three tokens and is below the default minimum of
four. Its index query ran over zero eligible blocks, with
`belowMinimumBlocks: 1`. That is different from a macro-obscured Rust source
whose provider supplies no admitted block and whose query is `not_run`.

## Integration and compatibility

`shadow` creates the rows around its existing per-language index queries;
`bounded_report` accounts for its existing pair trimming. No analysis is
repeated. The public shell's scheduled checker already serializes this report
and retains it as JSON; resident `checkerReport` and the existing CLI/MCP
projection receive the additional field through their current route. No
profile or worker requirement is removed.

The JSON field is additive and the schema tag remains unchanged. External
consumers that reject unknown response fields must be updated before using
this producer; JSON compatibility has not been tested here. Direct Rust
consumers constructing a `Report` literal must supply `clone_queries`. The
pinned repository has one producer of that literal. Receipts produced before
this addition have no query rows; absence is not evidence that a query ran.

The additional work is a count over at most 24 requested documents, copying
eight counters per queried language, and one pass over existing query gaps to
collect at most five distinct kinds per row. It adds at most four bounded
metadata rows. Report trimming checks at most four rows for each pair already
being removed. Parser/index work and the existing serialization algorithm are
unchanged. The new metadata consumes the existing report-byte budget, so a
tight cap can retain fewer pairs or reject a receipt that previously fit.
No measured latency, memory, correctness, or detection-performance claim is
made.

## Owner validation — not run by packet

With the repository's pinned Rust 1.99.1 toolchain and independently qualified
existing locked dependencies, run from the integration checkout:

```sh
cargo +1.99.1 test --locked --manifest-path rust/native-checks/Cargo.toml --test query_coverage
cargo +1.99.1 test --locked --manifest-path rust/native-checks/Cargo.toml --test shadow
```

These commands were **not run by packet**. The proposed `query_coverage`
source contains hand-counted exact-pair and token-work cases, generated and
minimum-size exclusions, all five query-gap kinds, a failed mixed-language
batch, an unsupported macro block, unavailable byte mapping, enumeration
permutations, and report trimming that must preserve original query work.
Its ordinary Python/Rust fixtures do not request a C# worker. Existing `shadow`
tests include separate conditional C# worker controls; skipped controls must
not be recorded as passed. Run future owner validation on supported Linux and
Windows targets before integration. No compilation, builds, test execution,
or CI was performed for this source-only proposal.
