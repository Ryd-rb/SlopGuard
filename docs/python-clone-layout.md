# Python clone qualification with significant layout

The native Python exact and renamed-local rules require matching logical-line
boundaries and suite nesting as well as matching substantive tokens. This is
a structural clone relation. It does not establish equivalent behavior, a
defect in either source, or that replacing one fragment with the other is safe.

## Why layout participates

Python's [lexical rules](https://docs.python.org/3.12/reference/lexical_analysis.html)
make logical NEWLINE and INDENT/DEDENT significant. For example, the two
functions below have the same ordinary token spellings:

```python
def choose(flag):
    result = 0
    if flag:
        result = 1
        return result
```

```python
def choose(flag):
    result = 0
    if flag:
        result = 1
    return result
```

For false `flag`, the first falls off the function and the second returns zero.
This is a hand-derived example, not an executed result. The `return` token has
relative indentation depth two in the first function and one in the second.
The layout gate rejects the pair even though the lexical index nominates it.
Logical-line starts also distinguish `value = 1` followed by a separate `+ 2`
expression statement from `value = 1 + 2` on one logical line.

## Data and invocation

`slophawk_python_syntax_provider::analyze(SourceRequest)` still consumes the
caller's bounded original bytes. Schema `python-syntax-facts/v2` adds a
`TokenLayout` to each substantive `TokenFact`: indentation-stack depth and a
logical-line-start flag. It counts indentation transitions, not spaces or tab
columns. Only retained substantive tokens clear the line-start flag. Comments,
blank lines and nonlogical continuation newlines leave it unchanged.

The provider does not insert artificial token text or assign source spans to
zero-width dedents. `TokenFact.exact`, SHA-256 provenance and original UTF-8
byte ranges keep their existing meaning. `clone_layout(region_id)` yields
one layout entry per token in `clone_stream`, subtracting the region's first
token depth with checked arithmetic. An invalid region ID or escaping
indentation depth returns `None`.

`rust/native-checks/src/providers.rs::python` stores this sequence for each
admitted function/class block, or the whole document when neither is present.
`shadow` uses the unchanged in-memory `TokenCloneIndex` to nominate matches,
then requires equal layout lengths and entries before publishing a Python
pair. Missing or inconsistent sidecar metadata fails the job. Existing
invalid-syntax, incomplete-token, missing-byte-map and peer-failure paths
continue to withhold facts or findings as appropriate.

There are two existing entry paths: the explicit native `shadow --request`
command, and the public shell's `native-checkers.json` profile-backed
`scheduled_checks` caller. The latter publishes through
`resident_store::results` as `checkerReport`; CLI results and MCP
`get_findings` consume that report. No new command, analyzer worker, dependency
or automatic activation is added.

## Identity and compatibility

Accepted Python rule IDs and pair prefixes use
`sg10.python-layout-clones.v2`. The failure/unsupported paths use the same
language-specific rule IDs. Existing v1 Python baselines or suppressions must
be reviewed rather than silently transferred to v2. The native report schema
and input envelope stay unchanged; the Python fact struct's added field and
schema version require source consumers to update together.

The public scheduler includes `slophawk_native_checks::SEMANTICS_VERSION` in
its scope hash. Restarting with this implementation makes old receipts
ineligible for reuse and publication, including when source and profile bytes
did not change. All native profiles are invalidated once by that stamp. The
unchanged lexical index still uses its v1 identity internally, including in
its budget-gap explanations. Rust, C# and FluxR pair/rule identities stay v1.
The separate, explicitly enabled Python near-miss relation below has its own
identity; it does not replace either exact or renamed-local matching.

## Bounds and deliberate limits

All existing admission and candidate, compared-token, posting, output and
report-size limits remain. Rejected lexical pairs consume those same limits;
there is no refill or second unbounded search. Therefore, an output limit
can be reached before layout rejection and leave fewer accepted pairs than
the requested limit. The original gap is retained and the report remains
partial. A complete untruncated search can legitimately return no pairs.

Extraction adds two scalar values per retained source token. For T source
tokens, R selected regions and P total tokens across projected blocks,
`clone_layout` adds O(R*T) scanning and O(P) sidecar storage; nested regions
can duplicate tokens. The existing projection already scans and duplicates
these blocks. Post-query layout equality adds work proportional to returned
candidate token counts, within the already charged comparison envelope. No
hard parser CPU/RSS isolation or measured performance improvement is claimed.

Formatting controls retain equality for equivalent indentation widths,
LF/CRLF, BOMs, blank/comment-only lines and implicit/explicit continuation.
Inline versus multiline suites can be conservative misses. Existing
same-spelling binding normalization, unresolved-name gates, whole-block
selection, and lack of cross-language matching remain. Debug f-string
source text and broader semantic effects are not qualified by this layout
change. In particular, matching tokens and layout are not a proof of matching
interpolated-string output or alpha-equivalence across scopes.

## Opt-in Python near-miss proposal

`nearMissClones: true` now selects `sg10.python-layout-near-misses.v1` for
admitted Python blocks. Omission and `false` keep the existing behavior: no
near query, and unchanged Python exact/renamed v2 results. C#, Rust and FluxR
continue to use `sg10.token-near-misses.v1`. The request and report schemas,
provider version, dependencies and execution grants are unchanged.
`SEMANTICS_VERSION` becomes `slophawk.native-checks/4`, so the existing resident
scope hash invalidates old native receipts once, across all native profiles.
This is a proposed structural relation, not evidence of behavioral equivalence
or edit history. Compilation, tests, effectiveness and performance are unverified.

### Relation and scoring

The lexical candidate filter, normalized local ordinals, 80 percent threshold
and maximum 64 insertion/deletion edits remain. Replacing one token costs two.
Layout is neither fake token text nor a contribution to the token count.

1. Derive nonempty logical lines from the sidecar. The first token must start
   a logical line at relative depth zero; depth cannot change inside a line
   or increase by more than one at a line boundary. Assign each indentation
   suite a preorder ID and record its parent. A line records its owner suite
   and, if the following line opens a suite, that child's ID. Simple-statement
   insertion does not change these suite IDs.
   Withhold blocks containing a standalone semicolon token, a colon on a line
   that does not open an indentation suite, or an opening line without a final
   colon. The sidecar cannot establish inline-suite/semicolon ownership. This
   is an ambiguity gate, not an assertion that every such colon is a suite;
   body annotations, dictionaries, slices and lambdas are conservative misses.
   Colons inside a complete indentation-opening header remain admissible,
   including annotated parameters.
2. Require equal suite-parent trees. Pair logical lines only when their owner
   and opening-child IDs correspond. Suite-opening lines cannot be skipped.
   This distinguishes equal-depth sibling suites without treating absolute
   logical-line ordinals as identity.
3. Compute the ordinary minimum normalized token distance `d`. Distance zero
   stays in the existing exact/renamed domain even when layout differs. For
   nonzero `d`, search for a compatible line alignment at that same distance.
   Token alignment takes place separately inside each paired logical line.
4. Permit whole non-opening logical-line gaps only if `d == abs(m - n)`, where
   `m` and `n` count substantive tokens. Gaps then run only toward the longer
   stream. Every token in the shorter stream must survive in an accepting
   alignment. Otherwise there are no whole-line gaps: all lines correspond in
   order and edits must remain within them. A new statement plus an independent
   replacement is deliberately outside this first relation.
5. Require the constrained distance to equal `d`. The verifier cannot spend
   extra deletions/insertions to hide a moved return or split/merged surviving
   logical line. Among permitted minimum-distance alignments, retain the
   greatest exact-token match count; do not postqualify just one lexical trace.

For each accepted pair, `normalizedMatchedTokens = (m + n - d) / 2` and
`score = 2 * normalizedMatchedTokens / (m + n)`. The exact count is for a
layout-compatible optimum and can be lower than the unconstrained exact tie
maximum. Whole source spans and token counts remain the indexed spans/counts.
The native endpoint includes the verified original UTF-8 byte span as well as
the existing UTF-16 offsets. No smaller source span is inferred from the LCS.

For example, `def f():` followed by `value = 1` and `return value + 2`
has 12 substantive tokens. Replacing the final `2` with `3` preserves its
layout, has distance 2 and 11 matching tokens, and scores `22/24` (about
0.9167). Renaming a proven local preserves the normalized count while its
occurrences cease to contribute to the exact count. These are hand-derived
expectations; the fixture source is not an executed observation.

### Evidence and unavailable boundaries

The native caller passes the existing sidecar map that `shadow` already used
for exact/renamed qualification. The core calls a bounded lookup only after
admitting a block under its metadata, token and token-byte caps. It binds the
returned layout to that document version and whole-block span and requires
exactly one layout entry per substantive token. Missing layout, stale version,
span/length mismatch, invalid first line, impossible depth jump or mid-line
depth change returns `InvalidLayout`; there is no lexical fallback. The native
producer derives tokens, spans and layout together from the same digest-checked
source; the callback uses the producer's span, independently of the index span.
The index alone cannot authenticate a caller-supplied parser or sidecar, or
detect a caller reusing the same version/span for different content. Native
document versions are recomputed SHA-256 digests; generic callers must maintain
the same provenance discipline.

Structurally valid but unqualified punctuation produces an `UnsupportedLayout`
coverage gap and withholds the entire block. It still consumes token/token-byte
preprocessing budgets. If every otherwise admitted block is withheld, the native
row is unsupported with reason `unqualified_statement_layout`; a mixture of
eligible and withheld blocks is partial. Other budget gaps also keep an
all-withheld admitted prefix partial rather than imply the rest was examined.
In particular, changing `return value`
to `if flag: return value` is unavailable, even though the inline suite does not
produce a new indentation event. No pair may use the withheld block.

Unavailable byte mapping and incomplete token extraction continue to remove
all affected blocks. Invalid syntax or failed peer analysis discards the staged
batch. A known-empty source contributes no block. Missing eligible blocks is
unsupported, not a successful negative search. Complete tokens with partial
local evidence remain exact-token-only inputs: the producer strips all local
normalization and the document remains partial. A query can be complete over
those admitted blocks while source/renaming coverage remains partial.

An ordinary incompatible layout yields no pair. Query work caps retain their
existing gap and partial-result semantics. The core validates only admitted
blocks; blocks omitted by a cap remain outside the reported search, including
their unexamined sidecars. Cancellation and unknown byte-span lookup fail before
publication. One-shot epoch/revision labels remain caller claims; resident
freshness still belongs to the existing scope and publication checks.

### Work bounds and conservative misses

Existing defaults remain 4096 admitted blocks, 262144 tokens, 16 MiB token text,
16 KiB metadata per block, 64 postings per token, 100000 posting visits,
20000 candidates and 2000000 alignment cells. The native output cap stays
32 pairs per language. No retry refills a truncated search. The per-block index
token cap and generated/excluded filtering are unchanged. The core index's
minimum-token default remains 40; the native request's existing default is 4.

Layout preparation is linear in admitted tokens, with O(T log B) worst-case
integer comparisons to intern exact suite-parent vectors across B blocks and
O(T) extra storage. Identical vectors share an ID, making the per-pair shape
gate constant work. The outer logical-line DP uses a band no wider than the
token edit budget allows; fixed-line edits use only its diagonal. All outer
rows and inner token rows, including their initial/boundary cells, charge the
same alignment-cell limit as the initial lexical DP. Inner line-pair token
rectangles are disjoint parts of the full m-by-n token grid. Worst-case work
per pair remains O(m*n), additionally bounded by the shared remaining cell cap;
structural active DP rows require O(d+1) memory. No traceback or mapping-state expansion
is retained. The initial lexical pass uses its effective cutoff k (which can
exceed the optimum d), so combined active-row memory is O(k+1). The native
span lookup adds O(R) borrowed-key entries for R projected Python blocks under
existing source/provider limits. Existing provider parsing and O(R*T) nested
projection work are outside the query's alignment-cell cap and remain unchanged.
A result can truncate earlier than lexical-only v1 at the same cap.

This smaller relation intentionally misses inserted/deleted indentation suites,
mixed whole-line and replacement edits, and cases whose valid structural path
needs more edits than the flat optimum. First-occurrence local normalization
can also miss renamed clones when inserting a new local changes later ordinals;
that inherited limitation is unchanged. Inline suites, semicolon-separated
statements and other unqualified colon-bearing body lines are withheld because
their statement ownership is not represented by the sidecar. There is no AST
statement identity or control-flow proof. Identical repeated statements cannot
establish which historical statement moved, and completely replaced tokens can
support more than one edit history.

Strict whole-sidecar equality is a smaller comparator but rejects any token or
line insertion that changes sidecar length. Checking one arbitrary lexical LCS
trace also loses alternatives when locals repeat. This proposal searches all
permitted line/token alignments under the same cost and work bound. A broader
AST or parser-normalized approach could recover suite edits but needs a new
producer/ownership contract and separate cost/accuracy qualification. The
[Python lexical reference](https://docs.python.org/3.12/reference/lexical_analysis.html),
[SourcererCC](https://arxiv.org/abs/1512.06448) and
[NiCad method](https://research.cs.queensu.ca/home/cordy/Papers/MRC_NiCadModern.pdf)
motivate these distinctions; their reported results do not validate this patch.
The 2024-11-19 through 2026-11-19 survey found no direct primary validation of
this precise Python-sidecar relation. See the accompanying integration packet
for query history, versioned evidence, alternatives and review limitations.

## Regression tests

Use Rust 1.99.1 for the provider, clone index and native checker suites:

```sh
cargo +1.99.1 test --locked --offline --manifest-path rust/providers/python/Cargo.toml --test facts python_layout_
cargo +1.99.1 test --locked --offline --manifest-path rust/native-checks/Cargo.toml --test shadow python_layout_
cargo +1.99.1 test --locked --offline --manifest-path rust/native-checks/Cargo.toml --test shadow python_exact_renamed_and_near_miss_are_separate -- --exact
cargo +1.99.1 test --locked --offline --manifest-path rust/native-checks/Cargo.toml --test shadow rust_simple_local_renaming_and_near_miss -- --exact
cargo +1.99.1 test --locked --manifest-path rust/checkers/clone-index/Cargo.toml --test layout_near_miss
cargo +1.99.1 test --locked --manifest-path rust/native-checks/Cargo.toml --test python_near
cargo +1.99.1 test --locked --manifest-path rust/checkers/clone-index/Cargo.toml
cargo +1.99.1 test --locked --manifest-path rust/native-checks/Cargo.toml
dotnet test SlopHawk.slnx
```

The public-shell suite also checks opt-in behavior and CLI/MCP agreement
against a running resident daemon.
