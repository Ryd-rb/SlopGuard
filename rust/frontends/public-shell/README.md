# SlopHawk native public compatibility shell (SG-MIG-05)

This **source-only** Rust CLI implements the published daemon lifecycle and MCP
stdio tools without depending on an unpublished native daemon. The wire contract
is frozen to **local IPC version 1, report schema 2** at SlopHawk upstream
`a367ef58076f7e29d2ae475a3b593daad04c4998`. There is no new worker job
transport in this crate, and the report contains no roadmap-only result marks.

## What this shell actually supports

On **Windows**, `slophawk-native start|status|results|recheck|stop` speaks to the
existing C# daemon's current-user named pipe. Root validation rejects reparse
points in all ancestors; the pipe name is the C# `Workspace` SHA-256 identity.
Each request negotiates `05` on a fresh connection; operations use `06 01 NN`,
then read at most 8 MiB + 4096 bytes and send the required single-byte `01`
ACK. The server PID's user SID is compared to the caller's SID. Read, write,
connect and acknowledgement operations have deadlines. A version/schema or
capability mismatch is an error: **never kill or replace the running daemon**.
`results` additionally checks the report against status before and after the
read, rejecting a changed epoch/revision/root as stale.

`slophawk-native mcp [--root PATH]` starts (or reuses) the compatible daemon
and serves newline-delimited MCP 2025-11-25 JSON-RPC on stdout. The MCP
`initialize`/`notifications/initialized` lifecycle is separate from private
IPC, with `tools/list` and `tools/call`. Tool registration comes only from the
connected daemon's capabilities: `workspace_status`, `get_findings`,
`run_checks`, and `list_checkers`. `list_checkers` returns `[]` in this schema-2
preview, never invented native checker IDs. Every list/call re-negotiates the
peer and emits `notifications/tools/list_changed` if capability projection
changes. Only JSON-RPC appears on stdout; diagnostics go to stderr.

For Windows roots or account names requiring Unicode-specific invariant casing,
the shell likewise uses the verified C# launcher rather than guessing a pipe
identity. Qualification must cover these routed identity cases.

On **other operating systems**, the .NET implementation's Unix named-pipe
addressing is not a published native transport contract. This CLI therefore
**fails closed** for direct native IPC and delegates lifecycle/MCP to a
verified, owner-pinned C# CLI process; MCP uses the published C# SDK bridge.
Without a trusted launcher it fails with a concrete capability gap rather
than claiming native support. The shell runs Rust foreground checks and review
for the `check`, `check-observe`, `check-retest` and `check-review` commands.
`slophawk-native shadow --request <request.json>` runs the native C#, Python,
Rust and FluxR checker against the explicitly enumerated inputs in a
[shadow request](../../native-checks/README.md). Projects, contract/probe and
path compatibility commands use the configured C# launcher.

A `trusted_peer` launcher may be selected explicitly in the registry and must
still present the **same** published handshake. No dependency on SG-MIG-03 or
later native daemon APIs is assumed. This crate does not implement the
`slophawk.migration-job/1` protocol because it creates **no new internal job
transport**: it uses only existing local IPC and MCP stdio.

## Owner-managed executable and argv registry

A launcher registry is required to start a daemon, use a forwarded command,
or delegate to the C# SDK on an unsupported OS. Store it **outside the
checkout**, normally at `%LOCALAPPDATA%\SlopHawk\launchers.json` on Windows or
`$XDG_CONFIG_HOME/slophawk/launchers.json` on Unix (falling back to
`$HOME/.config/slophawk/launchers.json`). An owner may set the absolute
`SLOPHAWK_LAUNCHER_REGISTRY` path. No `.slophawk` settings or repository file
is consulted for process authority. Example (replace paths and both digests
with the exact actual shipped bytes):

```json
{
  "schema": "slophawk.launcher-registry/1",
  "daemon": "legacy",
  "legacy": {
    "executable": {
      "path": "C:\\Program Files\\dotnet\\dotnet.exe",
      "sha256": "<64 hex characters for dotnet.exe>"
    },
    "argvPrefix": ["C:\\Apps\\SlopHawk\\SlopHawk.Cli.dll"],
    "attestedFiles": [
      {
        "path": "C:\\Apps\\SlopHawk\\SlopHawk.Cli.dll",
        "sha256": "<64 hex characters for the deployed CLI assembly>"
      }
    ]
  }
}
```

Alternatively set `argvPrefix: []`, `attestedFiles: []` and pin the standalone
C# apphost executable. A selected `"daemon": "trusted_peer"` requires a
separate `"trustedPeer"` launcher of one of these two shapes. Arbitrary
shell arguments, unpinned `.dll` files, relative/linked paths, registry files
inside the workspace, invalid digests or unknown registry formats are refused.
Digests are checked again immediately before process start; residual
filesystem replace-between-check-and-exec races require an owner-controlled
read-only installation directory, and should be included in qualification.

**Registry SHA-256 values are not bundled**, because a repository checkout
cannot grant permission to a machine executable. Owners must hash the actual
deployed apphost, or both the `dotnet` executable and shipped CLI assembly.

## Owner-only verification (not executed in this recipient pass)

- Regenerate the **new crate's** lockfile from pinned package manifests with
  the approved Rust 1.99.1 toolchain, then check in the reviewed result.
  Existing published dependency pins are `serde =1.0.229`,
  `serde_json =1.0.154`, and `sha2 =0.10.9`; no lock was fabricated here.
- Source tests cover length limits, partial reads/ACK, unsupported wire/schema,
  missing capability, typed legacy upgrade, stale reports, MCP lifecycle and
  capability replacement, stdout framing, registry rejection and failed launch.
- On Windows, set `SLOPHAWK_TEST_CSHARP_APPHOST` to an actually built, trusted
  `SlopHawk.Cli.exe` and run the ignored `real_daemon` tests. They cover live
  hello/operations/results/stop, real MCP projections, a slow pipe peer, and
  a late/missing ACK; they intentionally spawn a daemon **only in the owner's
  explicit test pass**.
- Qualify Windows x64 pipe security, root normalization, named-pipe timeout
  cancellation, and launch race behavior against the pinned .NET 10 output.
  Linux/macOS users must explicitly qualify the C# forwarding mode, not infer
  that the unimplemented native socket path is supported.

The checked-in source has **not** been built, restored, executed or tested by
the SG-MIG-05 recipient, as instructed.
