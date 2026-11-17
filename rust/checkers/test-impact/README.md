# Pure evaluated-project test impact

Dependency-free Rust 1.99.1 port of the *selection and explanation decisions* in
`src/SlopHawk.Daemon/EvaluatedTestAdapter.cs` at upstream commit
`a4399a0a199cd173a5b9d2048faaabfb093d1a96`.

`select(&SelectionInput)` takes **resolved evaluated concrete target IDs**, old and new
compile membership, qualified predecessor graphs, source digests and already-approved
unit-test projects from provider boundaries. It never parses XML, determines which
project is a test project, loads coverage, evaluates MSBuild, launches tests or
admits jobs. It preserves complete/partial distinction, reason kinds and chains,
selected/budget-deferred/unapproved scope and bounded explanation omissions.

The input graph is explicitly scoped to one evaluated revision; a provider issue
remains visible in `UnresolvedFrontiers` and makes the current selection partial.
`Complete = true` on an empty selection is authoritative only if providers
supplied all relevant resolved graph facts. This Rust model cannot itself audit
whether the provider's completeness claim is truthful.

The `PathComparison::Ordinal` mode matches Linux path comparison;
`OrdinalIgnoreAsciiCase` is deliberately limited to ASCII rather than making
false claims about non-ASCII Windows case folding. Broken selected fingerprint
reference closures fail closed with `SelectionError`. Fingerprints reproduce
C# anonymous-record `System.Text.Json` structure and uppercase SHA-256; cross-
runtime parity remains an integration gate, especially for encoder edge cases.

The source observations and candidate test identities are **not** verification
receipts. A complete selection does not mean a test was executed, passed or
asserted the modified behavior. Admission, test runs, coverage evidence and
report publication stay in their original C# owners until a separately verified
cutover.

Integration validation on 17 November 2026 used Windows x64 and Rust 1.99.1:
all 18 tests passed, including C# frontier omission-count behavior when
discarded events recur. Formatting, locked offline all-target builds and
Clippy with warnings denied passed. Unicode Windows path equivalence and
full runtime cutover remain outside this pure pilot.
