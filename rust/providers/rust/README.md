# Rust syntax provider (SG-RUST-5 pilot)

This **standalone library**, under `rust/providers/rust/`, consumes one Rust UTF-8
source file and an explicit Rust edition (2015/2018/2021/2024). The public
entry points are `analyze_file` (bounded read) and `analyze_source` (bounded
in-memory input). Both require a caller-owned logical document identity and
optional lower `Limits`. Nothing builds a Cargo package, loads modules or
executes macros, build scripts, tests or external tools.

The local typed boundary is `SourceFacts` in `src/lib.rs`. `Identity` binds
`slophawk.rust.syntax`, its package and schema version, `ra_ap_syntax` parser
version **0.0.363**, the declared edition, caller-supplied logical document ID,
the **SHA-256 of exact source bytes**, and the byte length. Every `Span` is an
original **zero-based half-open UTF-8 byte range** (`start`, `end`). The parser
is fed the original string without newline normalization, so CRLF, Unicode,
raw identifiers and BOM bytes, if present, are kept in the source coordinate
system. The caller must map bytes to editor line/column or UTF-16 separately.

Capabilities are explicit strings on each batch: syntax declarations and
blocks, significant lexical tokens, lexical identifiers and comments, exact
clone/spelling input, very conservative same-body `let` normalization, parse
recovery diagnostics and unresolved frontiers. These are **source syntax facts,
not typed symbols or semantic edges**. The parser's AST nodes do not escape the
call; only owned, size-capped records do. Normalized keys `$local0`, etc. are
only emitted for simple direct `let` bindings and plain in-body path reads, with
no macro/attribute/closure/complex pattern involvement and a clean parse.
Other identifier spellings stay as written, with lexical role. `Declaration`
contains syntax-level `NAME` nodes (and `None` for `impl`), not symbol IDs.

`Completeness::CompleteSyntax` means the *offered syntax capabilities* were
extracted within limits and without recovered syntax errors/frontiers; it never
means semantic resolution, evaluated cfg, generated items, or discovered tests.
`PartialSyntax` is accompanied by diagnostics or `Unresolved` reasons (macro
expansion, cfg, generated includes, method dispatch, out-of-line modules,
parse recovery, or fact limit). One output slot is reserved for `FactLimit`.
The input is rejected over 1 MiB; output is capped at 20,000 records across all
collections, with caller-specific smaller caps supported. The implementation
is read-only and does not retain parse trees between calls. These limits do
not constitute a memory ceiling for third-party parser allocations; this is
an in-process pilot, not a hostile-input worker sandbox.

This pilot intentionally does **not** supply Cargo metadata, crate/feature
context, source generation, HIR, trait resolution, diagnostics from rustc,
test discovery or execution, IPC framing, or a shared cross-provider wire
schema. The owner should review it against the C# and Python provider contracts
before introducing cross-packet types or daemon routing.

From the repository root, with a verified Rust 1.99.1 toolchain and an
independently generated lockfile / dependency feed:

```sh
manifest=rust/providers/rust/Cargo.toml
cargo +1.99.1 generate-lockfile --manifest-path "$manifest"
cargo +1.99.1 fmt --manifest-path "$manifest" -- --check
cargo +1.99.1 build --manifest-path "$manifest" --locked
cargo +1.99.1 test --manifest-path "$manifest" --locked
cargo +1.99.1 clippy --manifest-path "$manifest" --locked --all-targets -- -D warnings
```

Research: rust-analyzer syntax architecture
<https://rust-analyzer.github.io/book/contributing/architecture.html>;
`ra_ap_syntax` API <https://docs.rs/ra_ap_syntax/0.0.363/ra_ap_syntax/>;
Rust editions <https://doc.rust-lang.org/edition-guide/editions/>; Rust cfg
<https://doc.rust-lang.org/reference/conditional-compilation.html>.
