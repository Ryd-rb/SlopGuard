# Python syntax facts pilot (SG-RUST-4)

`slophawk_python_syntax_provider::analyze(SourceRequest)` is a read-only,
filesystem-independent library entry point. It accepts one bounded `.py` or
`.pyi` **original byte slice**, a logical document ID/path, a Python 3.7–3.15
target, and source/token/fact budgets. It does not import source modules,
resolve dependency graphs, invoke Python, or run tests.

## Source contract

`SourceIdentity` carries provider `slophawk.python.ruff.syntax`, schema
`python-syntax-facts/v2`, `ruff_python_parser` version `0.0.22`, path and
logical document ID, SHA-256 of the supplied bytes, original size, file kind,
target Python grammar, and an explicit encoding decision. The returned
`ByteSpan` values are **half-open offsets in original UTF-8 bytes**. No line
ending canonicalization takes place. When an initial UTF-8 BOM is present,
Ruff sees bytes after the BOM and this adapter adds three to its offsets.
Unicode identifiers and CRLF therefore retain their original byte ranges.

The adapter recognizes PEP 263 declarations from the first two *eligible*
physical lines. Only UTF-8 (and supported aliases) and UTF-8 BOM have exact
source mappings in this pilot. Any declared other encoding is an explicit
`UnsupportedEncoding` outcome, even if the input happens to contain only
ASCII; undecodable nondeclared bytes are `InvalidEncoding`. It does not silently
transcode Latin-1 or interpret unknown encodings as UTF-8.

## Coverage and scope

`SourceFacts` contains syntax `Region`s, substantive `TokenFact`s,
`IdentifierFact`s, comments, and bounded `Gap`s. The token stream preserves
lexical spellings for exact and near matching. Clone normalization changes an
identifier only when the AST supplies a local binding site **in the same
function, lambda, or comprehension scope**; importing, closure references,
attributes and dynamic calls do not receive invented symbol identities.
`IdentifierFact.spelling_candidate` is a dictionary input, *not* a finding.
Comment words and string contents are excluded from that identifier list.

`Outcome::{Complete, Partial, NoRelevantFacts, UnsupportedExtension,
UnsupportedGrammar, UnsupportedEncoding, InvalidEncoding, InvalidSyntax,
ResourceLimit}` are separate terminal dispositions. Capabilities are tracked
individually. `Partial` includes unresolved imports, dynamic/member dispatch,
free names and local binding gaps. Syntax-derived facts for other regions are
retained on `Partial`, but parse errors or resource failures yield no records;
we do not promise local recovery after invalid syntax. Version-specific parser
errors are `UnsupportedGrammar` without publishing partial AST records.
`clone_tokens` means *syntax token extraction*, not that semantic linkage
or a clone finding has been verified.

Schema v2 adds `TokenFact.layout`: indentation-stack depth and whether the
token starts a logical line. Original spelling and byte ranges stay separate.
`clone_stream` supplies candidate spellings; a Python clone consumer must also
compare `clone_layout` for the selected region. That method removes only the
region's enclosing indentation depth. It returns `None` for an invalid region
or an escaping depth, not an empty proof. Indentation width, comments, blank
lines and continuation newlines do not create structural differences. Inline
and indented suites may still differ. See [Python clone qualification](../../../docs/python-clone-layout.md)
for the native caller, versioning, budgets and remaining limits.

The 4 MiB hard source cap limits a parser invocation's initial input size,
but Ruff's allocations and execution time are **not** supervised by this
library. Production integration must run it in a bounded, cancellable worker;
there is no daemon or shared wire-contract integration in this slice.

## Dependency policy

The manifest pins `ruff_python_parser`, `ruff_python_ast`, and
`ruff_text_size` to `=0.0.22`. These Ruff crates are published as **internal
components**; AST structs, lexer token types, grammar handling, and visitor
signatures are not a stable downstream API. All direct dependencies on these
Ruff APIs live in `src/ruff_adapter.rs`. Changing the parser requires review
of the entire dependency closure, target grammar and all fixtures. Ruff's
workspace advertises the MIT license; review each transitive package and
preserve its license notices before distributing a binary. A verified
`Cargo.lock` is required before this pilot can be integrated or released.

## Local verification commands

```sh
cargo +1.99.1 generate-lockfile --manifest-path rust/providers/python/Cargo.toml
cargo +1.99.1 fmt --manifest-path rust/providers/python/Cargo.toml -- --check
cargo +1.99.1 build --manifest-path rust/providers/python/Cargo.toml --locked
cargo +1.99.1 test --manifest-path rust/providers/python/Cargo.toml --locked
cargo +1.99.1 clippy --manifest-path rust/providers/python/Cargo.toml --locked --all-targets -- -D warnings
```

The build/test commands are a **validation requirement**, not evidence that
they were run successfully when the toolchain or registry is unavailable.

## Opt-in direct-import syntax

`slophawk_python_syntax_provider::imports::extract(SourceRequest)` is a
separate typed library entry point with provider
`slophawk.python.ruff.direct-import-syntax` and schema
`python-direct-import-syntax/v1`. It preserves the existing `analyze` schema,
token/layout facts, clone behavior, defaults, and caller behavior. It adds no
CLI/MCP setting, automatic discovery, shared serialization, or execution.

This entry point admits `.py` source with a Python 3.12 target only. It emits
top-level `import package.module` references, including multiple aliases and
`as` bindings. Names must be at most 256 bytes, have ASCII identifier components,
not contain an `__init__` component, and have ASCII **original source spellings**.
Ruff's normalized name alone is insufficient: an original Kelvin sign, for
example, must not become an admitted ASCII `K`. An alias binding does not
change the named module and is not used for resolution. Each record carries
the entire original import-statement byte span; aliases in a statement share
that span. Source order and duplicate witnesses are retained deterministically.

The source hash includes original line endings and any UTF-8 BOM. Every
emitted range is nonempty, lies on UTF-8 boundaries, and includes the BOM
offset correction. The provider neither discovers nor resolves packages.
A subsequent admitted-inventory producer must validate source revision,
identity, package roots, unique module membership and policy permission before
turning these names into graph edges. Python import hooks, package execution,
`sys.modules`, filesystem presence, or runtime reachability are not proven.

`ImportFacts.unsupported` retains the unknown frontier. Absolute `from`
imports and explicit relative imports are unknown even when a matching file
exists, because attribute/re-export bindings are not proved. `if` and `try`
statements remain unresolved guards, including unbound or rebound
`TYPE_CHECKING` names and their `else` branches. No nested bodies are traversed.
Definitions, assignments, loops, calls, f-strings and every other executable
statement retain one enclosing unknown span. This deliberately catches hidden
dynamic imports instead of declaring a module complete merely because no
recognized import call appeared. Only whitespace, comments, `pass`, and
ordinary string-literal expression statements are inert in this version.
`is_complete()` means only that this conservative syntax frontier is empty;
it is not a proof of general runtime dependency completeness.

Source/path/document-ID caps are checked before encoding inspection or
copying. Caps are the lesser of caller budgets and 4 MiB of source, 131,072
parser tokens, and 32,768 records; the path is limited to 1,024 UTF-8 bytes and
the document ID to 160. A record is one import alias (including an unsupported
name) or one other non-inert top-level statement. The adapter validates and
counts all records before allocating output vectors; token/fact overflow,
invalid syntax, unsupported grammar, and encoding failure return `Err(Failure)`
without partial facts. Hashing occurs only after successful validation. Parser
allocations still precede token-count inspection, so a supervising worker is
required for a hard memory/time boundary.

`tests/imports.rs` supplies source fixtures for absolute aliases, original
byte witnesses, BOM/CRLF, relative and re-export uncertainty, type-only guard
uncertainty, hidden dynamic imports, NFKC spellings, stale byte identities,
resource limits and the independent old syntax contract. These fixtures and
the implementation require owner-side runtime validation; authored assertions
are not observed test results.
