# Edited-branch verification evaluator

This standalone, dependency-free crate ports **only**
`sg.verification.edited-branch`, rule version **1**, from the pinned
`VerificationRuleEvaluator.Evaluate` implementation at SlopHawk commit
`978a6464a6e5013fb1ad3a56c4e2877e62c18ac4`.

Call `evaluate(&request, None)` to use the C# default count and nested-list
limits, or pass `Some(&limits)` for a smaller positive budget. Inputs are typed,
prevalidated facts: this crate does **not** read coverage files, verify receipts,
execute tests, infer symbols, normalize paths, or decide whether a detector's
claims are honest. Those responsibilities stay with the producers.

The input `VerificationChange` has no change-kind field: every change here is
an edited branch. Other rule IDs in policy/search/evidence are ignored for
matching but **still count** toward the whole-request budgets, as in C#.
`SUPPORTED_RULE_IDS` advertises only this rule.

`VerificationDecision` includes the complete C# v1 output contract, retaining
run failures even when measured coverage was observed. All sortable identifier
fields use UTF-16 code-unit ordinal order, matching `.NET StringComparer.Ordinal`
instead of Rust's UTF-8/Unicode scalar ordering. The C# comparison fixtures are
transcribed as typed Rust regression cases in `tests/behavior.rs`; differential
execution against the pinned .NET suite is a separate integration check.

No integration into the daemon or C# worker is provided by this slice.
