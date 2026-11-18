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
its budget-gap explanations. Rust, C# and FluxR pair/rule identities stay v1;
near-clone matching remains unsupported.

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
selection, and lack of cross-language or near matching remain. Debug f-string
source text and broader semantic effects are not qualified by this layout
change. In particular, matching tokens and layout are not a proof of matching
interpolated-string output or alpha-equivalence across scopes.

## Owner validation (not run by packet)

Focused provider and native tests are drafted in the existing test files.
Use an independently qualified Rust 1.99.1 toolchain and existing reviewed
dependency caches; these commands are for the integration owner:

```sh
cargo +1.99.1 test --locked --offline --manifest-path rust/providers/python/Cargo.toml --test facts python_layout_
cargo +1.99.1 test --locked --offline --manifest-path rust/native-checks/Cargo.toml --test shadow python_layout_
cargo +1.99.1 test --locked --offline --manifest-path rust/native-checks/Cargo.toml --test shadow python_exact_renamed_and_near_miss_are_separate -- --exact
cargo +1.99.1 test --locked --offline --manifest-path rust/native-checks/Cargo.toml --test shadow rust_simple_local_renaming_and_near_miss -- --exact
```

Also qualify the resident restart/reuse path and CLI/MCP agreement on the same
input, and hold out nested try/except/else/finally or match suites for an
independent structural oracle. Compare baseline/proposal latency and memory
on the same admitted corpus, especially deeply nested regions and repetitive
lexical candidates. Compiled behavior, effectiveness and performance remain
unverified by this source-only proposal.
