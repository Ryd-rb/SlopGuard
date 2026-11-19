# Owned assertion receipts, version 1

`assertion_receipts::validate` is an explicit library seam for
`slophawk-owned-i64-equality-v1`: one owned invocation, one exact logical case/data
row, and one signed 64-bit equality assertion. It consumes bounded captured JSON
and separately supplied owner/supervisor facts, then returns `Complete(Attempt)`
or `Unknown(Reason)`. It does not run a fixture, authenticate a producer, publish a
finding, alter review, or grant permission to execute anything.

There is no production caller in the CLI, MCP, foreground reader or VSTest runner.
Those surfaces remain unavailable for precise paired assertion evidence until an
owner supplies a qualified producer and an independently admitted capture route.
Do not place an extra JSON file in the existing VSTest results directory: its
current reader admits exactly one `results.trx` file.

## Independent inputs and producer obligations

`Qualification` is a current owner review of the exact instrumentation/adapter
manifest, complete test/helper/instrumentation source closure, fixture code/data
and selected inputs, logical test/row/assertion mapping, and an independently
specified expected `i64`. Its `target.oracle` must equal `oracle_id(expected)`.
The expected value must come from the intended contract, not candidate output.
The record digest identifies the retained review; it does not prove a review took
place. `Qualification` is deliberately not deserialized from the receipt.

`Supervisor` must come from outside the candidate-authored payload. It supplies
the predeclared arm and repetition, actual selected logical case, fresh attempt
and isolation, plan, source snapshot, loaded build closure, actual environment,
current complete production-substitution review, unchanged-input manifest and
the SHA-256 of the captured bytes. These are the existing `Provenance` fields.
The actual adapter must match the qualified adapter, and test/fixture identities
must match the qualified closures. A configuration file's SDK version is not the
identity of the SDK that actually loaded.

The supervisor's `capture_complete` must attest a lossless, single-invocation
capture through teardown, with a closed sink and no outstanding descendant,
hidden in-process retry, dropped assertion or omitted terminal result. The owner
must qualify how instrumentation observes the assertion and final case outcome.
A process exit, a `complete: true` field, an ordinary TRX summary, a filename or
hash equality cannot supply this fact. A full transcript can record a failed
setup/teardown; that transcript is complete as a capture but is still `Unknown`
as a successful assertion-evidence binding.

This API is a trust boundary, not a cryptographic verifier. Rust callers can
construct its public facts. If the purported supervisor obtains them from the
same untrusted JSON, or approves an arbitrary self-report, validation supplies no
independent provenance. A dishonest qualified producer can fabricate a fully
consistent receipt. Signatures would authenticate a speaker, not establish that
the speaker observed execution. Keep the checker unavailable without an
independent authority and retain the readable supporting manifests.

## Closed JSON contract

Names use the exact lower-case/snake-case spelling below. Every identity is an
array of exactly 32 JSON integers in `0..=255`. Do not use text hashes, display
names, row positions or timestamps as substitute identity. Integers must decode
as `i64` (or `u8` for repetition), without floating-point conversion.

| Object | Fields and meaning |
| --- | --- |
| Root | `format`, `header`, `case`, `complete`, `setup`, `teardown`, `assertions`, `terminal` |
| `header` | `plan`, `attempt`, `arm`, `repetition`, `isolation`, `snapshot`, `build`, `substitution_review`, `unchanged_inputs`, `qualification`, `adapter`, `tests`, `fixture`; each must match the independently supplied supervisor fact. `arm` is `baseline` or `candidate`. |
| `case` | `test`, `row`, `row_key`; `row` is `single` or `data`. A `single` case requires an all-zero `row_key`; a `data` key identifies owner-qualified input values under its retained mapping, never an ordinal. |
| Each assertion | `attempt`, `case`, `assertion`, `oracle`, `event`, `actual`, `result`; child attempt and case must match independently, target IDs must match qualification, and `event` must equal `event_id(attempt, target)`. `actual` is the exact observed signed integer. `result` is `passed` or `failed` and must agree with recomputed equality. |
| `setup`, `teardown` | One of `completed`, `failed`, `not_run`, `incomplete`. Only both `completed` can produce `Complete`. |
| `terminal` | `kind` and optional `event`. `kind` is `passed`, `assertion_failed`, `setup_failed`, `teardown_failed`, `exception`, `build_failed`, `runner_failed`, `timeout`, `cancelled`, `skipped`, `not_run` or `incomplete`. `event` is required and must name the exact captured event for `assertion_failed`; for `passed`, it must be omitted or null. Other kinds remain unknown. |

All fields other than `terminal.event` are mandatory. Duplicate fields, unknown
fields (including nested fields), unknown enum values, invalid digest widths,
wrong types, trailing documents and malformed JSON are rejected. There are no
aliases or an extensibility map. A document matching this schema but naming a
different `format` gets `UnsupportedFormat`. Generic TRX, old foreground JSON,
and future documents whose fields differ get `MalformedReceipt` at decoding.
They remain valid inputs to their original readers, which this module does not
change. These parser expectations require the owner's runtime tests.

The original byte slice is capped at 16 KiB before SHA-256 or JSON decoding.
The trusted receipt digest binds those same bytes; no reserialization is used for
integrity checking. No normalization of line endings, whitespace or JSON member
order is assumed. Equivalent serialization can be independently captured with a
different receipt digest and the same semantic result.

The event cap is eight, but exactly one assertion is accepted. Zero means
`AssertionNotReached`; two through eight mean `AssertionAmbiguous`, even if one
appears relevant; more than eight mean `TooManyAssertions` once earlier lifecycle
checks have passed. This deliberately excludes soft/multiple assertions,
assertion loops, custom equality, floating point, object comparison and general
exception assertions. The bounded decoder can materialize an event list before
the semantic count check; the 16 KiB input cap bounds that allocation. There is
no unbounded stream, file traversal, execution or diagnostic-text retention.

## Deterministic identities and outcome binding

Hashes use SHA-256 with these exact byte encodings. `\0` denotes one NUL byte;
integer payloads are signed two's-complement i64 in big-endian byte order.

| Identity | Bytes hashed, in order |
| --- | --- |
| Oracle | UTF-8 `slophawk-owned-i64-equality-v1/oracle\0`, then expected integer |
| Observation | UTF-8 `slophawk-owned-i64-equality-v1/observation\0`, then expected integer, then actual integer |
| Event | UTF-8 `slophawk-owned-i64-equality-v1/event\0`, attempt digest, test digest, row tag byte (`0` for single, `1` for data), data-row digest only for data, assertion digest, oracle digest |
| Receipt | The unmodified received JSON bytes, without an additional prefix |

Of the derived oracle, observation and event keys, only the event key includes
the attempt: semantic observations must remain comparable across repetitions.
The observed equality verdict is recomputed from
`actual == qualification.expected`. A passed case must have a passed assertion
and no causal event; an assertion-failed case must have a failed equality and
name that exact event. A copied child event cannot acquire a new attempt merely
through a rewritten outer header. The deterministic event hash still does not
prove that an occurrence happened; complete qualified capture does.

Validation rejects in a fixed source order: independent qualification/provenance
and repetition bounds, independent completeness and qualification bindings,
selection, missing/oversized bytes, byte digest, decoding/version, header, case,
receipt completeness, setup/teardown, non-assertion terminal failures, assertion
count, event bindings, recomputed verdict, and terminal causality. Only the first
problem is returned. This precedence intentionally reports setup/teardown faults
before a missing or malformed semantic event binding. No receipt text is echoed.

## From a receipt to a paired comparison

The caller must keep the predeclared plan and all attempts for both arms. Validate
every attempt against its own independent supervisor facts and current owner
qualification. If any returns `Unknown`, retain that failed capture and keep the
pair unavailable. Do not filter it out or replace it with a later passing retry.
Use the resulting complete `Attempt` values in the existing `PairEvidence`, then
call `paired_regression::compare`. Validation alone is not a paired verdict.

The comparator still requires complete two-to-eight repetition sets, current
qualified substitution, exact target/preserved inputs, matching actual SDK,
runner, adapter, target, dependencies/settings/environment, stable builds within
each arm, and distinct attempt/receipt/isolation identities across both arms.
It compares stable observations and rejects a stale plan, snapshot or substitution
review. If both independent metadata and receipt repeat the same stale manifest,
the owner must detect that staleness against the current plan: hashes cannot do
it. The existing comparator's supplied-facts limitations remain in force.

| Evidence after complete validation | Existing paired result |
| --- | --- |
| All repetitions pass on both arms | `NoDiscrimination` for this exact assertion/input |
| Every old-arm repetition fails exactly the target; every candidate repetition passes; reviewed production-only delta and distinct loaded builds | `Discriminating` for this target only |
| A repeat's value/verdict changes | `Unknown(UnstableObservations)` |
| Reused attempt, receipt or isolation; changed source/fixture/environment; stale substitution | The existing specific `Unknown` reason, without support |
| Candidate fails or both arms fail | `Unknown`, not evidence that the fix is correct |

Finite repetition does not prove determinism, and either successful pair outcome
still requires semantic review of the assertion and the bug claim. A pure equality
assertion may be irrelevant to the intended bug even when its receipt is perfect.
Foreground review and execution admission retain their existing responsibilities.

## Scope, cost and acceptance

Validation takes O(B) time and O(B) bounded decoding storage for B <= 16 KiB;
fixed digest work is independent of repository size. The unchanged pair comparator
uses at most sixteen attempts, at most sixteen production edits and 512-byte edit
paths, with bounded quadratic identity scans. No dependency or lockfile changes
are needed. A producer pays for the existing two-to-eight repetitions per arm and
retains all capture and supporting provenance; this patch schedules none of them.

`tests/assertion_receipts.rs` is synthetic fixture source exercising the receipt
boundary and passing accepted attempts into the original comparator. It does not
qualify a real producer. Qualify the admitted producer and its independent
capture and provenance before connecting a user-facing surface. Qualify cross-language hash encodings
with independent golden byte/digest vectors; the synthetic receipt fixtures use
the library's hash helpers and do not establish that producer compatibility.
Rich stateful/multi-assertion tests
may require a different qualified adapter instead of rewriting their behavior to
fit this narrow format.
