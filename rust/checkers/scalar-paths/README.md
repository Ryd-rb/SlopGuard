# Scalar paths: bounded Rust candidate planner

The Rust foreground `paths` planner calls this crate's `parse_wire` and `search` after the C# scalar worker exports Roslyn facts. The C# `ScalarPaths.Explore` implementation remains a comparison baseline. This crate is a pure candidate planner; it neither emits/loads assemblies nor runs a fixture, verifies a replay, or attributes observed branch coverage.

`ScalarPaths.ProjectFacts(compilation, documentPath, signature)` is the narrow C# Roslyn export. It returns a typed `ScalarPathFactProjection` with the original structural `Graph` (Roslyn's method, blocks, edge roles/regions and spans) plus scalar node and edge facts. `ToWire(seeds, workLimit, caseLimit)` produces the UTF-8 `scalar-path-facts\t1` stream. Standard input to `scalar-paths` is exactly one whole stream; stdout is `scalar-path-results\t1` with `result`, `candidate` (predicted edge and args), and hex-UTF-8 `unknown` records. The process exits 2 for malformed input, not a partial prediction. Input is bounded to 16 MiB, 4096 blocks, and 65536 nodes per block. The Rust crate has no third-party dependencies.

The wire records are, in order, `limits`, `bound`, zero or more `param`, `block`, `node`, `stmt`, `branch`, `edge`, `seed`, `end`. Empty node-child lists use an empty interior field; an absent edge-local/capture list uses `-` (never a trailing tab). Node entries are in depth-first preorder, preserving C# evaluation order. Types and symbol/capture IDs are explicit; checked/lifted operators and unsupported nodes carry hex-encoded semantic holes. Every node also carries a Roslyn-sourced diagnostic for runtime-unknown operands, distinct from a static hole. Block edge facts retain destination, semantics, finally presence and leaving-region local/capture IDs. Branch predictions always refer to the same `b{ordinal}:fallThrough` / `b{ordinal}:conditional` edge IDs as the C# graph. Unknown transition semantics, overflow checked contexts, loops, uninitialized locals, and opaque operators stop the affected model path; they do not become proof of edge execution.

The C# graph is **structural**, and any Rust `candidate` is **predicted**. The foreground coordinator verifies the compiler-input closure and emitted candidate SHA-256 through the controlled-build path before using the graph. Graph-off follows the existing published/probe ordering and sets `caseLimit=0` to disable Rust candidates. Only original-build replay, the exact comparator, repeated execution, sealed plan/review and distinct observed trace receipts can establish behavior. Stale builds, mismatched artifacts, adapter response reorderings and replay disagreement must continue to block/mark review, never be inferred from this prediction stream. Planner policy changes require owner-side parity and matched memory/latency evaluation on Windows and Linux.

## Experimental correlated equality policy

`search(problem)` and the standalone CLI retain the existing search policy.
`search_with_policy(problem, SearchPolicy::CorrelatedEqualitiesV1)` is an explicit
opt-in for owner evaluation; no foreground caller selects it in this proposal.
The fact/result wire schemas and `SearchResult` fields are unchanged. The owner
must bind `correlated-equalities-v1`, its caps and the planner revision to the
sealed plan's policy identity before adding external configuration or wiring.

For a supported prefix such as `x == y`, `y == z`, followed by
`unchecked(x + y + z) == 3`, this policy proposes moving all three inputs together.
It recognizes only already-satisfied, single-comparison prefix equalities whose
coefficient difference has exactly one +1 and one -1 (or is identically zero).
Affine offsets are allowed. Every prior nonzero row must have that form and use
int parameters. Common wrapping shifts preserve those differences modulo 2^32;
other predicates and the remaining path still require normal interpretation.

The extension is capped at 16 total parameters and 32 preserved comparisons,
and moves one connected component of at least three int parameters at a time.
Components/members use ascending parameter ordinals. Difference targets are
0, -1, +1, followed by left/right operand minima then maxima, using the existing
at-most-two modular representatives. Proposals precede legacy proposals at the
same depth/flip priority, so opt-in candidate ordering intentionally changes.
There is no Cartesian product, solver process, dependency, interval proof or
infeasibility verdict. Non-unit relations, larger inputs and unsupported facts
remain limitations, visible through existing unknown records.

Preparation, each inspected equality row, each target equation (including an
unsolvable one) and each representative (including duplicates/no-ops) consume
one credit from the shared `64 * workLimit` proposal cap. `SearchResult.work`
still counts only distinct interpreted inputs; queued but uninterpreted inputs
are never returned. Existing prefix-expansion, work and case caps remain in
force. Zero work/cases and other early-disabled results are unchanged. Counters
bound search effort, not wall-clock latency; measure preparation costs as well.
Skipped correlated attempts also spend credits, so legacy fallback in opt-in mode
means the old rules still run within the remaining budget, not sequence parity.
Unknown records describe actual cap, unsupported-row or budget events; a policy
label alone is not added as a semantic frontier. Existing interpreter holes and
unexplored-edge reasons remain authoritative.

`tests/correlated.rs` contains synthetic, hand-authored planner regressions, not
Roslyn exports or observed execution evidence. This proposal was reviewed only
as source and a text patch. Its draft tests have not been compiled or executed.
Owner qualification must reconcile the synthetic facts with real Roslyn output,
use an independent oracle and replay the original build before accepting any
behavioral claim.

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
