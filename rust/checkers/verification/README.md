# Pure typed verification rule v1

This standalone, dependency-free crate ports `VerificationRuleEvaluator.Evaluate`
from the pinned C# implementation (`a4399a0a199cd173a5b9d2048faaabfb093d1a96`).
It now advertises three rule IDs: `sg.verification.edited-branch`,
`sg.verification.public-contract`, and `sg.verification.serialization`.

The existing `evaluate(&request, None)` entry point and `VerificationChange`
shape remain unchanged and accept **edited branches only**, protecting existing
Rust callers. The additional `typed::evaluate_typed(&request, None)` accepts
`TypedVerificationChange { kind, change }` and evaluates mixed rule batches.
Both entry points preserve v1 budget, policy, contextual identity, UTF-16
ordinal ordering and current/stale/unknown/missing/configured outcomes. Contract
and serialization evidence must attest actual behavior, a relevant assertion,
a completed passed or failed run, valid receipt and exact target mapping.
An observed run never claims that the test **passed** when its outcome was
failed. `ProviderRunDisposition` additionally distinguishes zero discovery,
unadmitted, skipped and incomplete **before** mapping to the narrower v1
report enum; it cannot make a failed or incomplete input a successful result.

Callers supply *already validated* typed facts: neither entry point reads
coverage files, verifies receipt bytes, runs tests, infers symbols, normalizes
paths or decides whether detector boundaries are exhaustive. The upstream
providers retain these responsibilities. The current crate is pure and is not
integrated into the daemon/worker/publishing pipeline.

Full behavior and boundary fixtures are in `tests/behavior.rs` (existing branch
rule) and `tests/typed_contracts.rs` (new contract and serialization rules).
C# `.ToString()` names in contract action text are retained; the legacy
edited-branch action wording is preserved for backward compatibility.
Live differential execution against the pinned .NET implementation remains a
mandatory end-to-end cutover gate.

Integration validation on 17 November 2026 used Windows x64 and Rust 1.99.1:
all 48 tests passed, including the original edited-branch suite. Formatting,
locked offline all-target builds and Clippy with warnings denied passed.
