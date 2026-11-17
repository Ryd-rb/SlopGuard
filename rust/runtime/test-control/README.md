# Native durable test control (SG-MIG-11)

This is a **new standalone Rust 1.99.1 library**, not a modification of the
shipped daemon, inventory schema-v2, MCP, public argv, or Roslyn frame-v1.
There is deliberately **no default execution adapter** and no daemon hook in
this recipient patch. Without a trusted `Authority` and `ScopedHost`, the
library cannot grant, start, or publish worker execution.

## Consumer flow

1. Acquire an external, single-writer workspace lease and physical root/context.
   Use an isolated, protected `test-control-v1.sqlite3` outside repository-owned
   content. Call `Store::open`, `Store::recoveries`, and `Store::bind_and_drain` with
   a monotonically increasing writer generation and fresh epoch after restart.
   `bind` commits the invalidation **before** returning scopes to revoke; never
   release an output root until `revoke_and_drain` and `confirm_drained` succeed.
2. Confirm source reconciliation, path digests, tool/policy/profile/grant and
   concrete evaluated graphs in the native owner. `Store::bind` commits all
   changed `Cause`s and the exact fence. A metadata-only commit supplies no
   causes. An incomplete scan must **not** delete previous pending causes.
3. Supply the **whole** pending cause set to `Store::select` in
   `slophawk_test_impact::SelectionInput`. The published pure selector computes
   old/new graph reachability, missing-predecessor and first-revision fallbacks,
   approved-unit-project policy, diagnostics and the 16-target budget. `select`
   rejects omitted/altered pending causes. Complete empty selection retires
   its covered causes; partial selection retires none, even after selected
   targets execute. There is no implied test run in source-only or zero work.
4. `drive_one` obtains an output scope from the supervisor, durably admits the
   exact selected outstanding causes, then calls `peer::FramedExecutor`.
   `slophawk.migration-job/1` is a little-endian 32-bit length plus UTF-8 JSON
   body with a 1 MiB **preallocation** limit. Full operation, attempt, scope,
   root/context, incarnation/sequence, committed revision, input/tool/policy/
   profile digests, grant/budget references, and selected target identity are
   echoed by the terminal. Missing, malformed or unmatched terminals never
   provide evidence. The external host enforces execution duration, stream
   limits, privilege, output roots, recursive containment, cancellation, and
   independently verifies source/artifact bytes before the commit.
5. Drain the process tree before accepting a receipt or releasing output leases.
   `Store::finish` commits the exact terminal, unique receipt, evidence and cause
   retirement in one SQLite transaction. A completed **failed** test is valid
   run evidence, *not* a passing assertion. Zero discovery, skipped, incomplete,
   unadmitted, blocked and retry exhausted are distinct from completed evidence.
   Three attempts per unchanged plan; retries use 250/500 ms backoff. A crash
   leaves running work in `recovery` until the owning supervisor proves drainage.
6. Call `Store::pending` and `Store::export` for diagnostic/recovery transfer.
   `Store::import` requires an empty native store with matching root/context;
   imported selections and coverage are intentionally invalidated. Re-bind
   with a *new* writer generation and independently re-select/revalidate before
   publication. The export does not carry active authority or grant rights.
   A live SQLite file must never be copied without its journal. Native v1
   serialization is **not** a migration of .NET SourceInventory schema-v2.

## Transaction and platform limits

All state transitions are SQLite `BEGIN IMMEDIATE` commits in a dedicated
schema (`user_version=1`, `synchronous=FULL`, rollback journal). This depends
on functioning local filesystem locking and flush semantics, per
<https://www.sqlite.org/atomiccommit.html>. Read/write quota and import size
limits fail closed rather than silently pruning pending obligations. Receipt
validity depends on the native supervisor's actual owned-file and runner
verification, not a claimed boolean from worker JSON. The `Authority` must
serialize physical-source checks with its source-commit publication boundary.

On Linux the owner must provide a qualified process-tree scope (e.g. delegated
user-managed systemd/cgroup v2); on Windows, an eligible Job Object or
an equivalently proven recursive supervisor. Direct child termination alone is
not sufficient. See <https://docs.kernel.org/admin-guide/cgroup-v2.html>.
`ScopedHost::connect` must enforce a deadline and total output/diagnostic
byte budget outside `Read + Write`. Do not enable execution without such a
host. Native runtime activation, profile parsing, root probes, daemon bridge,
C# rollback compatibility and Windows/Linux containment qualification are
**deferred to the integration owner**.

## Dependencies and deferred validation

Path-depend on the published unmodified `slophawk-test-impact` and
`slophawk-verification` crates. `serde_json =1.0.154`, `serde =1.0.229`, and
`sha2 =0.10.9` match published Rust-project lock entries; `rusqlite =0.37.0`
is a newly pinned dependency with `bundled` SQLite. No lockfile is fabricated.
The integration owner must regenerate and review `Cargo.lock`, then run the
Rust all-target build, tests, Clippy and formatting checks on Windows and Linux
with Rust 1.99.1. No such commands are run in this source-only recipient task.
