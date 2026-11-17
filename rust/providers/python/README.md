# Python syntax facts pilot (SG-RUST-4)

`slophawk_python_syntax_provider::analyze(SourceRequest)` is a read-only,
filesystem-independent library entry point. It accepts one bounded `.py` or
`.pyi` **original byte slice**, a logical document ID/path, a Python 3.7–3.15
target, and source/token/fact budgets. It does not import source modules,
resolve dependency graphs, invoke Python, or run tests.

## Source contract

`SourceIdentity` carries provider `slophawk.python.ruff.syntax`, schema
`python-syntax-facts/v1`, `ruff_python_parser` version `0.0.22`, path and
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
