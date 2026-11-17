# Rust project context and test receipts (SG-PROVIDER-08)

**Scope: standalone contract; no daemon routing or public cutover.** This crate
consumes an *already admitted* worker capture of Cargo metadata
`--format-version 1 --locked --offline --filter-platform TARGET`, plus the exact
workspace manifest and lockfile bytes, and constructs a bounded evaluated
Cargo package/target/feature graph. It cannot start Cargo or execute repository
code. `Binding` carries the canonical Linux workspace root, monotonic workspace
revision, epoch, Git commit, target triple, Rust 1.99.1 toolchain labels and
hashes of rustc verbose output, the selected Cargo executable and the controlled
environment/config. `CargoGraph.context_sha256` binds those inputs, every
workspace-member `Cargo.toml` digest, the root `Cargo.lock` digest and captured
metadata bytes. A digest mismatch or wrong worker argv is **stale/blocked**, not
an empty graph. The worker must independently attest these facts and revalidate
source revision and relevant bytes before publication; caller-provided booleans
are not cryptographic admission tokens.

Each `CargoPackage` has only Cargo-declared targets and direct dependency
**declarations**. `CargoEdge` uses the resolved `resolve.nodes[].deps` for the
selected feature/target variant and retains raw dependency `dep_kinds` target
expressions and optionality when unambiguously matched. Inactive optional
edges are not reachability edges. Target `src_path` is a *root*, not an
exhaustive module/file membership. `Coverage::EvaluatedCargoGraph` means only
that Cargo graph evidence is evaluated for one context; it does not mean
semantic binding, macro expansion, generated modules, cfg expansion, test
coverage, or complete source membership. Unknown files of any extension (including
macro assets and build-script inputs), missing previous
state, removed packages, variant mismatch and global Cargo inputs yield
conservative candidate selection with unresolved frontiers. `select_tests`
returns a bounded diagnostic explanation, not permission to run or a native
scheduler integration. Membership is checked separately in each revision; a
recognized rename endpoint cannot hide unknown membership at the other endpoint.
The owner must review and adapt it to its common graph
and fence contract before hookup.

## Admitted execution boundary

`metadata_argv` and `test_argv` construct exact argument arrays. Workers must
pin the Cargo executable, target/std/toolchain, repository snapshot, lock and
registry closure; reject any other flags; and use an isolated `CARGO_HOME`,
`CARGO_TARGET_DIR` outside the checkout, a closed environment, resource/timeout
caps, scoped process-group termination, and bounded stdout/stderr **while
reading**, not after an unbounded capture. Metadata is non-executing, but Cargo
may still consult registry/config/target data. `cargo test` **runs** build
scripts, proc macros, test binaries and doctests; only a separately granted,
controlled worker may call it. The library makes **no** sandboxing claim.
Proc-macro/build dependency selection may differ between host and target and
needs platform parity fixtures. No automatic tests arise from a metadata-only
file event in this crate.

The admitted test route is `cargo test --locked --offline --manifest-path PATH
--target TRIPLE --package NAME@VERSION --message-format=json --color=never
[feature flags] -- --format=pretty --color=never`. It consumes bounded Cargo
`build-finished` JSON records and stable libtest **summary text**; it never
assumes Cargo JSON alone contains test results (libtest's JSON output is
unstable on the stable Rust toolchain). An exact graph hash, revision, epoch,
attempt ID and fence sequence are necessary to accept test output. The parser
requires a successful Cargo `build-finished` record and paired `running N test(s)` and `test result` lines and checks count arithmetic, exit code,
incomplete/truncated output and compiler-status contradictions. It distinguishes pass, test failure, all ignored, zero selected,
filtered-out, build failure, runner failure, timeout, cancellation and malformed
or incomplete output. There are no per-test IDs, run-level source coverage or
complete-discovery claims. Failures require valid observed summaries; otherwise
they are adapter failures or malformed evidence. It is a **bounded best-effort
text adapter**, not a secure proof against a deliberately forged test stream.
Production integration must use a trust-bound execution receipt and tighten the
harness protocol as necessary before relying on individual test identities.

Limits: 4 MiB metadata JSON, 256 KiB per worker stream, 1 MiB per manifest or
lock, 4 MiB total manifest bytes, 512 packages, 2,048 declared target roots,
8,192 declared/resolved edges, 100,000 aggregated test cases, 256 harnesses,
4,096 change paths and 512 selection packages. Worker-level caps apply before
allocation; these are only the adapter's second-line validation caps. Record
limits and any frontier instead of claiming full coverage.

## Checks

The standalone dependency pins originate from the pinned
`rust/providers/fluxr/Cargo.lock` dependency closure: `serde_json =1.0.154`,
`sha2 =0.10.9` and transitive checksums. The lock was regenerated offline with
Cargo 1.99.1 during integration, removing an unused serde derive feature edge
without changing package versions or checksums. With the approved offline
Cargo source closure, run:

```sh
export CARGO_TARGET_DIR="$(mktemp -d)" # outside the checkout
manifest=rust/project-context/rust/Cargo.toml
cargo +1.99.1 fmt --manifest-path "$manifest" -- --check
cargo +1.99.1 build --locked --offline --manifest-path "$manifest"
cargo +1.99.1 test --locked --offline --manifest-path "$manifest"
cargo +1.99.1 clippy --locked --offline --manifest-path "$manifest" --all-targets -- -D warnings
# Only in an admitted worker with an isolated Cargo home/target dir:
cargo +1.99.1 test --locked --offline --manifest-path "$manifest" \
  --test locked_cargo_fixture -- --ignored
```

The opt-in fixture is a real locked Cargo workspace with two packages, an
optional Linux target edge, and a passing unit test. `tests/adapter.rs` also
models missing dependencies, unsupported flags, moved membership, variants,
invalid and stale snapshots, zero/skipped/failing receipts, malformed output,
truncation and a failed Cargo build without executing repository code. The
live fixture is intentionally `#[ignore]` in the default suite because nested
`cargo test` executes code and needs independent admission.

Integration validation on 17 November 2026 passed all 12 default tests,
formatting, locked offline all-target builds and Clippy with warnings denied
on Windows x64 with Rust 1.99.1. The live fixture requires Linux: an explicit
Windows attempt was refused with `only canonical Linux absolute roots are
qualified`. It remains unqualified on Linux, and passing synthetic adapter
tests does not establish Windows Cargo execution support.

Sources: [Cargo metadata](https://doc.rust-lang.org/cargo/commands/cargo-metadata.html),
[Cargo test](https://doc.rust-lang.org/cargo/commands/cargo-test.html),
[Build scripts](https://doc.rust-lang.org/cargo/reference/build-scripts.html),
[rustc test-output formats](https://doc.rust-lang.org/nightly/rustc/tests/index.html),
[SlopHawk migration](../../../docs/rust-checker-migration.md),
[SlopHawk checker contract](../../../docs/checkers.md),
[SlopHawk receipt boundary](../../../src/SlopHawk.Daemon/TestRunReceiptDetails.cs).
