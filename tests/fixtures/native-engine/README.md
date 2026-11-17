# SG-MIG-14 native-engine fixtures (source-only)

This directory contains negative and boundary inputs for the *future owner-run*
controlled migration peer and qualification harness. Nothing here is an
observed native performance result or a compatibility receipt. Each negative
input must be rejected *before* writer publication; the owner must use the
same exact digest-pinned binary for positive imports and rollback.

`unsupported-terminal.json` is a syntactically complete *shape* of a terminal
with an unsupported status but a deliberately mismatched identity and digest.
A test harness must replace it with the request's identity **only when testing
status handling**; any use verbatim must fail identity validation first.
`missing-scope.json` deliberately lacks the full matrix and complete samples;
qualifying it may never produce a promotable release receipt.

Additional owner tests: truncate the four-byte prefix, advertise a length
above 128 KiB, omit a terminal, duplicate a terminal, reorder chunks, corrupt
an export payload digest, change the root/epoch/committed revision, kill the
worker after admission and before receipt, and send `partial`, `stale`,
`cancelled` and `failed` terminals with bounded omission sets. Confirm
predecessor graph commitments, pending causes, receipts and interrupted worker
identities survive native-to-managed rollback with a new database handle.

`ControlledPeer.csproj` implements the bounded migration stdio wire with a
native-host-shaped .NET test executable. It accepts only the private job verb,
validates the received input chunk stream and provides controlled terminal
modes selected through `SLOPHAWK_PEER_FIXTURE_MODE`: `partial`, `unsupported`,
`missing-terminal`, `truncated`, `oversized`, `echo-mismatch`, `bad-digest`,
`duplicate-terminal`. The integration test activates these cases only when the
owner sets `SLOPHAWK_NATIVE_TEST_PEER_BINARY` to a fully built apphost **outside
the test checkout**. This is a fake peer and cannot satisfy native semantic
recovery, process-tree drainage or promotion acceptance.
