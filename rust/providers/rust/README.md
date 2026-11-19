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
Attribute frontiers cover the original source range of the attribute's owner,
including its nested bodies, rather than only the `#[...]` or `#![...]` text.
Consumers that exclude unresolved overlaps therefore omit bodies governed by an
unevaluated `cfg`, `cfg_attr`, or unresolved attribute macro. Adjacent unattached
items remain eligible for exact syntax comparison. This does not evaluate any
configuration predicate or expand an attribute; even an apparently inert
attribute retains the existing conservative unresolved treatment.
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

## Optional attested cancellation input

`cancellation::assess` remains the original opt-in consumer of trusted `ArmFact`
values. `cancellation::attested` adds a separate **single-arm receipt admission
interface** for externally resolved Tokio 1.54.1 `AsyncReadExt::read_exact`.
It validates an independently owner-approved receipt and translates its claims
to the existing consumer. It does not resolve Rust, authenticate signatures,
check semantic proofs, scan a file for arms, or qualify a producer by itself.
No new CLI/MCP request, resident setting, process, dependency or execution grant
is introduced. Syntax-only callers still cannot supply cancellation evidence.

The supported positive class is one original-source future directly selected by
the reviewed Tokio macro and implemented by the reviewed `read_exact` method.
Transparent pin/reborrow forms need explicit external attribution and owner
proof. Arbitrary helpers/composite futures, ambiguous dispatch, unmapped macro
output, unknown cfg and uncertain introduction remain unsupported. One-shot and
loop contexts retain their original distinct `Repetition` values; neither
establishes actual lost input or an application defect.

### Independent authority and source closure

The owner passes `Approval` through a trusted channel separate from the receipt.
Its producer/version, qualification digest, approved receipt digest, actual
loaded-package fingerprint, macro-closure fingerprint and reviewed definition
IDs must originate in an independent qualification/review. **Do not authorize
production input by constructing Approval from that same input.** Synthetic
test helpers do this only to exercise admission branches, and are not evidence
of a qualified producer.

The external producer or human attestor is responsible for these facts:

- Exact original UTF-8 source, logical document/module and edition, with no
  newline normalization; original macro/future ranges and an unambiguous map.
- All semantic inputs in `ContextIdentity.project_sha256`: local imports and
  modules, actual dependency and macro bytes, expanded source maps, cfg,
  target/features/toolchain, producer version and options. The Cargo graph
  context from `project-context/rust` is insufficient by itself.
- Exact baseline/candidate revisions and contexts plus introduction policy in
  `comparison_sha256`, and a retained comparison artifact identified by
  `introduction_evidence_sha256`. A touched enclosing line/function is not proof.
- Actual selected macro and method/body identities, including receiver dispatch.
  Definition IDs are producer-qualified context-bound identities, not hashes of
  display names. Renames/re-exports can preserve identity; same-spelled local
  macros or custom traits do not acquire it.
- Active cfg, transparent attribution, underlying future owner survival/drop,
  and whether a Pending poll can precede a distinct matching branch winning.
  Retain the analysis artifact identified by `semantic_evidence_sha256`.

The reviewed upstream is official, unmodified Tokio **1.54.1**, source commit
`b53b2001ed2ccfd9b87e32a0c6da1a1eebdc80b8`. `TOKIO_ARCHIVE_SHA256` is the
[crates.io registry checksum](https://github.com/rust-lang/crates.io-index/blob/d370a7d8c7ebb65d78d8ae0733db7a2984f32726/to/ki/tokio)
`b52fd3255258d26fac7264ab5a76b53c13367136dc011d3d6ad4f8ed070396a3`.
It identifies the official archive; it does not prove which extracted files an
external resolver loaded. The qualification must independently compare those
actual loaded bytes with that archive and define the fingerprint encoding.
Equal package name/version or a lockfile checksum cannot excuse patched bytes.
Tokio's `select!` also uses `tokio-macros` helpers. Bind their actual source,
configuration, expansion and mapping closure separately; a missing helper
provenance record cannot be replaced by the Tokio archive identity.

### Admission, assessment and dispositions

`attested::adapt(unit, owner_approval, external_receipt, options)` returns an
`AdaptedFacts` holder. Borrow `holder.typed_facts()` into the unchanged
`cancellation::assess(unit, facts, options)`. Keep the holder alive while reading
the report. Both calls require explicit `Options.enabled`; the default remains
false. The holder exposes read-only `unavailable()` and
`approved_receipt_sha256()` accessors. Preserve that receipt digest and the
original receipt/approval/evidence artifacts alongside the assessment: the
legacy `Report` does not carry those additional provenance artifacts itself.

| Input, after earlier validation succeeds | Disposition |
| --- | --- |
| Disabled, even with malformed inputs | `Ok(None)` without inspection |
| No approval; or approval supplied but no receipt | Empty facts with `Coverage::Unavailable` and `MissingApproval` / `MissingReceipt` |
| Matching approval, source/context, reviewed package/macro closure and direct resolved arm | One `ArmFact`; the existing classifier determines its result |
| Alias/re-export of both reviewed definitions | Same classification; spelling is never inspected |
| Positively different macro / non-Tokio custom method | `NoAdvisory(DifferentMacro)` / `NoAdvisory(DifferentCallee)` under the consumer's existing precedence |
| Unresolved macro/method, ambiguous trait or unknown configuration | Existing `Unsupported` outcome; unknown cfg cannot become `Direct` |
| Known inactive configuration | Empty unavailable facts with `InactiveConfiguration`; no active arm is assessed |
| Other resolved Tokio API | `OtherTokioApi` translates to `Unsupported(ApiSummary)`; even `write_all` is outside this adapter's positive class |
| Resolved definition ID different from owner-approved target | Atomic `DefinitionMismatch`, never inferred `Other` |
| Borrow of the same proven surviving future | Existing `NoAdvisory(RetainedFuture)`; owning a pin still has `OwnedBySelect` semantics |
| Unknown introduction | Existing `Unsupported(Introduction)`; no introduction is inferred |
| Known introduction/preexistence with zero comparison-artifact digest | Atomic `MissingIntroductionEvidence` |
| Wrong schema/producer/qualification, edited approved receipt, stale source/context, invalid spans, unreviewed archive or mismatched loaded/macro bytes | Atomic error, no translated facts |

Errors have a fixed order in `adapt`: disabled; current-unit bounds; absent
approval/receipt; approval structure; receipt schema/string bounds; producer;
qualification; source/context; approved receipt digest; archive; loaded package;
macro closure; semantic/comparison artifacts; spans; resolved definitions; cfg;
translation. Thus an unapproved edit normally fails `ReceiptMismatch` before a
more specific semantic/structural gate. A reviewer-approved but inconsistent
receipt can reach the later gate. Missing evidence takes precedence over
validating an otherwise unused approval/receipt, after current-unit validation.

### Versioning and bounds

This is an additive, in-process Rust API, not a JSON/LSP/shared wire schema.
`RECEIPT_SCHEMA = 1` and `ADAPTER_ID = slophawk.rust.attested-read-exact/1` name
the admission contract. Old `SourceFacts`, `TypedFacts`, `assess`, rule ID,
summary version and CLI/MCP schemas are unchanged. There is no permissive
deserialization/default for a missing new field, and unknown receipt schemas
are rejected. A future change to canonical encoding or admitted meanings must
use a new schema/adapter identity and fresh owner qualification.

`receipt_sha256` uses a domain prefix, length-prefixed UTF-8 text,
little-endian u32 values, fixed 32-byte digests and explicit enum tags. Every
receipt field is bound. It grants no approval and fetches no artifact. Producer
IDs are limited to 128 UTF-8 bytes, document IDs to 512, both without controls;
length validation happens before hashing. Source remains capped at 1 MiB.
`MAX_ARMS = 1` is fixed by the interface and its optional one-element array.
The existing 1..=256 `max_records` validity rule is preserved, with
no records omitted from a valid one-arm assessment. Admission plus assessment
hash source twice and take O(source bytes) time and constant-size auxiliary
storage; external analysis, proof and caller allocations are not covered.
The single-record order and digest encoding are deterministic. Complete
nomination coverage means **only this supplied arm**, never complete discovery
or whole-program safety; unsupported consumer records still make
`complete_for_supplied_facts` false.

The new fixtures are synthetic source and attestation data, not executable
Tokio programs or proof of resolver/ownership quality. Owner acceptance must
separately validate the external authority, exact package closure and each
semantic/comparison claim. See the pinned
[select contract](https://docs.rs/tokio/1.54.1/tokio/macro.select.html),
[read_exact implementation](https://github.com/tokio-rs/tokio/blob/b53b2001ed2ccfd9b87e32a0c6da1a1eebdc80b8/tokio/src/io/util/read_exact.rs),
and [rust-analyzer architecture](https://rust-analyzer.github.io/book/contributing/architecture.html).

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
