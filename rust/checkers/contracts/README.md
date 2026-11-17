# Pure integer contracts and seeded probe plans

This standalone Rust 2024 library corresponds to the pinned `ContractCases`,
`ContractExpression`, `ContractConditionCases`, and the **Plan** portion of
`ProbeReplay`. It depends only on the existing local `slophawk-probe-replay`
crate for the typed `ProbeInput`, `ProbeExpectation`, and lossless `JsonValue`
interfaces. There is no OS process launch, target adapter, CLI, IL acquisition,
publication, or external service integration.

`IntegerContract` takes the operation name, ordered parameter names, source,
expression strings, **ordered** `(local_name, expression)` bindings and
optional ordered `(parameter, values)` basis entries. `ContractCases::new` takes
these definitions and non-null public examples and reports one ordered
qualification per published operation, followed by unpublished definitions.
Only qualified rules participate in `evaluate` and `generate`. Exact expected
values are signed 64-bit JSON integers; domain and intermediate arithmetic use
the bounded signed arbitrary-precision evaluator. `generate` yields only new,
legal, distinct cases, ordered per qualified operation. The optional
`implementation_literals` argument is a *provided hint*, not an IL scanner.

`plan_inputs` ports the ordered `ProbeReplay.Plan` generator, including grouped
operation/arity alternation, original seeds, axial, relational and correlated
scale mutations, saturated signed addition and seeded .NET `Random(int)`
compatibility. Equal seeds, scales and typed inputs give equal output order.
All collections are caller-supplied, with no mutable global RNG.

## Explicit scope envelope

- C# expression grammar: precedence, ternary, lazy `&&`/`||`, signed arithmetic,
  `mod` versus `%`, bit shifts 0–127, bitwise operators, functions `abs`,
  `min`, `max`, `clamp`, `popcount`, comparisons and `true`/`false`.
- Expression text is at most 4,096 UTF-8 bytes (slightly stricter than the C#
  UTF-16 `string.Length` cap for non-ASCII whitespace); nesting at most 64;
  decimal literals at most 39 digits; checked binary results at most 1,024
  `BigInteger.GetBitLength` bits. Projection difference is capped at 1,025 bits.
- Contracts: at most 64 operations; 32 parameters per qualified operation;
  64 ordered local bindings; at most 32 basis values per parameter.
- Plans: at most 8,192 cases per operation, 65,536 cases overall, 256 seeds per
  operation, 32 arguments per probe, 16 distinct supplied positive scales,
  128,000 correlated candidate paths per individual seed, 1,000,000 candidate
  attempts per arity stream; maximum 250,000 *eagerly materialized* guided
  candidates per candidate source. Exceeding a bounded generator scope returns
  `ContractError::Scope` rather than truncating a plan silently.
- `evaluate` maps out-of-domain inputs, division by zero, invalid shifts,
  intermediate over-budget arithmetic, and signed-64 result overflow to `None`,
  matching the reference `TryExpected` fail-closed behavior. Whole-plan scope
  failures return an explicit `Err`.

For in-envelope, ASCII contract expressions, the ordering is intended to match
the pinned C# reference exactly. Inputs or rules beyond the above limits are
explicit **scope gaps**, not claims of complete unbounded C# parity.

## Validation

The integration tests in `tests/pinned_csharp_golden.rs` contain **actual**
ordered outputs from a .NET 10.0.404 execution of a C# `ProbeReplay.Plan`
reference harness for mixed arities, signed boundary/scale saturation, two
operations and reproducible seeds 0 and 11. `tests/behavior.rs` exercises
qualification, evaluated contracts, guarded arithmetic, domain failures,
conditions, boundary guidance and deterministic generation. The owner should
run the pinned Rust 1.99.1 `fmt`, `build --locked`, `test --locked` and
`clippy --all-targets -- -D warnings` commands from `INTEGRATION.md` and then
execute a **full** differential against the pinned C# assembly and integration
suite before promoting either platform. A golden test is not evidence it has
already passed without a Rust execution result.

Integration validation on 17 November 2026 used Windows x64 and Rust 1.99.1:
all 20 tests passed, including the captured C# ordering vectors. Formatting,
locked offline all-target builds and Clippy with warnings denied passed.
Linux execution and full release qualification remain open.
