# Native coordination and source discovery — SG-MIG-03

This standalone Rust 1.99.1 source tree is an **opt-in migration recipient**, not
an alteration to the shipped SlopHawk daemon. It owns a separate native
reconciliation baseline, durable C# change causes, provisional four-language
source inventory and revision-validated native results projection. **It does
not replace** the C# daemon, Roslyn provider, MCP bridge, report writer,
existing CLI, test scheduler, or any published API. No production checker or
repository-execution adapter is registered.

## Scope and behavior

- `Coordinator::open(root)` requires a canonical physical workspace directory,
  a writer OS lock and an independently observed Git HEAD. First observation
  (including startup recovery) creates **zero new automatic test causes**.
- `poll()` performs a bounded direct scan, detects byte-digest and relative-path
  changes, invalidates on Git branch/commit/binding changes, then reconciles.
  Manual `Event` hints and `recheck()` use the same scanner. Notification
  coalescing uses a 300 ms quiet window, 2 s max age and full-scan fallback on
  overflow. The CLI/server polls every 750 ms. Transient A-to-B-to-A changes
  entirely between polls cannot be proved without an additional watcher or
  transition lease; consumers must not infer such transitions were observed.
- Sources: `.cs`, `.py`, `.pyi`, `.rs` and canonical `.fluxr` files. Discovery
  also fingerprints relevant project/manifest inputs. Each language gets an
  explicit nearest-manifest or unresolved-directory context; this is **not**
  compiler-evaluated membership. No cross-language dependency edge, test
  selection or automatic execution is inferred from extensions alone.
- A bounded scan is incomplete when a path/subtree is unreadable, linked,
  too deep or over budget. Affected old entries remain as **last-known,
  uncertain**; proved deletes/edits in independent scopes can proceed. Limits
  include 50,000 directory entries, 8,192 source files/directories, depth 64,
  2 MiB/source, 64 MiB observed source bytes and 8 MiB state/report files.
- Only confirmed `.cs` deltas after the baseline create native *pending causes*
  (2,048 detailed entries, then a conservative workspace fallback marker).
  No module here resolves or executes those causes automatically. Pending
  causes and the uncertainty frontier are committed in the same checksummed,
  atomically replaced state file. Restart increments generation/epoch and
  recovers pending causes without scheduling an offline edit. An unverified
  previously launched worker stays `scope_uncertain` and blocks admission.
- Current native output uses an isolated `results.json` with an empty checker
  list, no findings, `schemaVersion: 2`, explicit inventory capabilities and
  partial/pending coverage. The validated reader rechecks report bytes,
  epoch/fence, physical root identity, Git binding, state-directory identity
  and source digest before returning it. Published bytes are not authoritative
  without the reader.

## Private state and IPC

The only workspace writes are within
`<root>/.slophawk-native-coordinator/`: `writer.lock`, `state.v2`,
`results.json` and (Linux opt-in only) `ipc.sock`.
`.slophawk/results.json`, the C# `inventory.sqlite3` and the published C#
daemon's named-pipe address are untouched. The state is specific to this native
policy and is **not** an upgrade or replacement for the C# schema 5 inventory.
There is no automatic import of native pending causes into C#.

An owner can opt into one of two standalone modes *after source-only delivery
has been integrated and verified*:

```text
slophawk-native-coordinator serve --root <existing-physical-root>
slophawk-native-coordinator serve-ipc --root <existing-physical-root>
```

`serve` exposes a **test-only line-delimited JSON reply** interface on stdout:
`hello`, `status`, `results`, `list_checkers`, `recheck`, `stop`,
`event add|change|delete|rename|overflow`, `git <branch> <commit>` and
`run complete|fail|cancel` for a controlled fake-worker fixture. Event/Git
arguments are invalidation hints, not evidence. `serve-ipc` is a separate
Linux Unix-domain socket, mode 0600 under a checked 0700 directory, with
same-UID `SO_PEERCRED`. It accepts the SG-07 private wire-v1 op bytes
`01` status, `02` results, `03` recheck, `04` stop, `05` hello and
`06 01 <01..04>` versioned requests; replies use LE signed 32-bit length,
bounded JSON and client ACK `01` before acknowledged shutdown. The socket
is **not** registered with the C# CLI or MCP bridge. The current native
server is single-request at a time, not the C# eight-connection listener.

Internal peer facts have a **separate** `slophawk.migration-job/1` protocol:
LE 32-bit JSON frame lengths, max 1 MiB/frame, 8 MiB/job and 2,000 facts;
echoed operation/job, root/context, generation epoch, committed revision,
input/tool/policy SHA-256, budget and grant refs. A valid, identity-matched
terminal and EOF are necessary to accept staged facts. Partial, unsupported,
deferred, cancelled, stale and failed terminals return **no accepted facts**.
No Roslyn frame-v1 or public CLI/MCP transport is modified.

## Executable peers and containment

`execution::read_profile` reads a bounded **user-owned** grant file (never
repository policy). `prepared_job` requires a unique configured executable
outside the checkout and checks its full SHA-256 before building a job
identity. On Linux the default *suggested* profile location is
`~/.config/slophawk-native-coordinator/execution-v1.json` (JSON field
`rootIdentity` must match this native coordinator's physical-root digest).
These functions only prepare an identity; they do **not** authorize process
launch. `require_authenticated_containment` deliberately rejects Windows Job
and Linux systemd/cgroup v2 until the owner supplies and validates an adapter
which enrolls the child before it executes repository code and can authenticate
and drain the process scope after an owner crash.

The user-wide `heavy.lock` is an OS-level single-heavy-worker reservation for
the controlled fake-worker fixture; its PID/start-time and Linux process-group
recovery are **not** an authenticated production containment primitive.
Windows source scanning uses no-follow handles and physical file IDs. The
private socket and process-backed coordinator fixtures are Linux-only.
Production execution must never call `run_fake`, treat its receipts as test
results, or substitute its process group for a systemd cgroup.

## Test coverage

`src/sha256.rs` is a reviewed SHA-256 implementation with the missing
`f = e` working-state shift corrected; tests include empty, `abc`, 56-byte
multi-block and million-byte vectors. Unit tests cover parser/framing
fail-closed behavior, checksum state loading, Git observations and event
coalescing. `tests/lifecycle.rs` contains scratch-root tests for restart,
stable saves, changed-content renames, independent-scope retention, overflow,
Git same-commit A/B/A, explicit contexts, links, root replacement and stale
publication. `tests/platform_exec.rs` is separate and **Linux-only**; it
exercises a controlled fake binary's cancellation and descendants, not
production repository execution. Run the pinned suite with `cargo test --locked`.

The portable lifecycle suite checks Windows physical IDs and stale projection
rejection. Production peer execution, writer transfer and daemon parity require
separate qualification.
