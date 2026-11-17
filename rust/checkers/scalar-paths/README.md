# Scalar paths: opt-in Rust comparison adapter

The existing C# `ScalarPaths.Explore` and the CLI `paths` plan/check/review flow remain the **only** production decision path. Nothing in this crate is wired into them. The bounded Rust solver is a pure comparison candidate planner; it neither emits/loads assemblies nor runs a fixture, verifies a replay, or attributes observed branch coverage.

`ScalarPaths.ProjectFacts(compilation, documentPath, signature)` is the narrow C# Roslyn export. It returns a typed `ScalarPathFactProjection` with the original structural `Graph` (Roslyn's method, blocks, edge roles/regions and spans) plus scalar node and edge facts. `ToWire(seeds, workLimit, caseLimit)` produces the UTF-8 `scalar-path-facts\t1` stream. Standard input to `scalar-paths` is exactly one whole stream; stdout is `scalar-path-results\t1` with `result`, `candidate` (predicted edge and args), and hex-UTF-8 `unknown` records. The process exits 2 for malformed input, not a partial prediction. Input is bounded to 16 MiB, 4096 blocks, and 65536 nodes per block. The Rust crate has no third-party dependencies.

The wire records are, in order, `limits`, `bound`, zero or more `param`, `block`, `node`, `stmt`, `branch`, `edge`, `seed`, `end`. Empty node-child lists use an empty interior field; an absent edge-local/capture list uses `-` (never a trailing tab). Node entries are in depth-first preorder, preserving C# evaluation order. Types and symbol/capture IDs are explicit; checked/lifted operators and unsupported nodes carry hex-encoded semantic holes. Every node also carries a Roslyn-sourced diagnostic for runtime-unknown operands, distinct from a static hole. Block edge facts retain destination, semantics, finally presence and leaving-region local/capture IDs. Branch predictions always refer to the same `b{ordinal}:fallThrough` / `b{ordinal}:conditional` edge IDs as the C# graph. Unknown transition semantics, overflow checked contexts, loops, uninitialized locals, and opaque operators stop the affected model path; they do not become proof of edge execution.

The C# graph is **structural**, and any Rust `candidate` is **predicted**. The owner of the future coordinator must verify the compiler-input manifest and emitted candidate SHA-256 through the existing controlled-build path before importing the graph, then preserve candidate order and compare against the original C# search on the identical facts, seeds and bounds. Graph-off must still follow the existing published/probe ordering; use `caseLimit=0` to disable Rust candidates. Only the existing original-build replay, exact comparator, repeated execution, sealed plan/review and distinct observed trace receipts can establish behavior. Stale builds, mismatched artifacts, adapter response reorderings and replay disagreement must continue to block/mark review, never be inferred from this prediction stream. Do not cut over the default planner or promote Windows/Linux without end-to-end parity and matched memory/latency measurements.

## Focused fixtures

`fixtures/*.facts` were exported with the pinned .NET 10.0.404 SDK Roslyn assemblies from the six method bodies in `tests/SlopHawk.IntegrationTests/ScalarPathModelTests.cs` (`Trace`, `Assignment`, `Opaque`, `Checked`, `Loop`, `Contradiction`). The trace seed set uses the pinned `{-1,0,1}×{-1,0,1}×{0,1}` domain. C# fixture-project and source-file positions in these independent excerpts are local to the projection harness; only block/edge topology and simulated behavior are compared, not unrelated source offsets. The fixture exporter is opt-in, outside the checkout. The baseline C# candidate sequence must be captured from an independently verified full checkout before claiming cross-language parity; the Rust fixtures alone are not that evidence.

```sh
manifest=rust/checkers/scalar-paths/Cargo.toml
export CARGO_TARGET_DIR="$(mktemp -d)"
cargo +1.99.1 fmt --manifest-path "$manifest" -- --check
cargo +1.99.1 build --locked --manifest-path "$manifest"
cargo +1.99.1 test --locked --manifest-path "$manifest"
cargo +1.99.1 clippy --locked --manifest-path "$manifest" --all-targets -- -D warnings
cargo +1.99.1 run --locked --manifest-path "$manifest" -- < rust/checkers/scalar-paths/fixtures/trace.facts
```

The locked manifest is standalone and contains only the root package. The optional adapter is not tied to any new NuGet or Cargo registry feed. Expression graphs deeper than 256 nodes are rejected before recursive evaluation.

On 17 November 2026, Windows x64 validation with Rust 1.99.1 and .NET SDK 10.0.201 compared the real C# exporter and original planner with the Rust CLI on seven methods at five budget settings. All 35 comparisons matched bound status, work counts, ordered candidates, predicted edge IDs and unknown reasons. This checks the planner boundary; original-build replay, Linux validation and release performance gates remain separate.
