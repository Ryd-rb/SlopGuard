# Rust release, state migration and qualification

SlopHawk uses the installed Rust executable for the CLI, daemon and MCP. A fresh workspace starts with a private native inventory. C# operations use the installed Roslyn, MSBuild, scalar and VSTest workers.

## Import an existing schema-v5 workspace

1. Stop the previous daemon using its installed binary and the checkout's absolute path.
2. From a trusted installation, run the offline converter with the checkout root, installed Rust executable and its SHA-256.

```text
dotnet SlopHawk.StateImport.dll <checkout> <absolute slophawk executable> <SHA-256>
slophawk start --root <checkout>
slophawk status --root <checkout>
```

The converter holds the previous writer mutex and checks that admitted worker scopes drained. It reads the schema-v5 inventory in one transaction and transfers the bounded typed snapshot. The Rust importer checks the physical checkout and snapshot schema, then installs native state and publishes its writer marker. The original SQLite inventory and the native backup provide recovery evidence. Pending work and historical receipts carry forward. Previously running attempts become interrupted after drainage. To adopt an earlier committed Rust transfer, stop its daemon and start the new executable. Its durable workspace epoch carries forward.

A transition in progress blocks startup. Preserve both inventories for recovery. The [pre-cutover qualification](https://github.com/Ryd-rb/SlopHawk/blob/fea822dc7637dfb6d3b3908aea1c9dff39240693/docs/rust-migration-qualification.md) documents the earlier transfer and rollback protocol.

## Pin C# foreground workers

Install the .NET host and worker files at physical paths outside the checkout. For project evaluation set `SLOPHAWK_NATIVE_DOTNET` and `SLOPHAWK_PROJECT_WORKER` to the host and Roslyn worker assembly. For scalar path checks set `SLOPHAWK_SCALAR_WORKER` to the scalar worker assembly, and supply the path-check authority and seal variables described in the path checker documentation. Set the corresponding `_SHA256` variables to the exact tool digests.

### Scheduled source checks

To enable the bounded resident source checker, put `native-checkers.json` in
the private state directory that contains `migration-engine.json`. The
version-1 document has `version`, `rootIdentity`, `context` and optional
`worker` and `nearMissClones` members. Set `nearMissClones` to `true` to opt into bounded near-miss alignment for eligible C#, Rust and FluxR blocks. Copy `rootIdentity` from the current writer marker and use
the documented shadow checker context. If the source closure contains C#,
provide `worker.dotnet` and `worker.assembly` as `{path, sha256}` pins and an
external reference pack in `context.referencePack`. Keep the profile readable
and writable by the owner only. The other source checkers are built into Rust.

The scheduler binds all input paths and digests, the Git observation, profile
bytes and workspace revision before admitting an attempt. It checks this
binding again before publishing the receipt. An interrupted attempt becomes
eligible to rerun only after the writer and worker scopes are recovered. An
unchanged completed or partial receipt is reused after restart. View the
current attempt in `status` or MCP `workspace_status`. The same current report is available through `results` and MCP `get_findings`. The latest receipt lives in `native-<epoch>.checks.json`.
On Linux, a bounded Git HEAD probe can wake the reconciliation loop on a
branch switch between full source scans. The full observation still validates
the Git context and source digests before publishing.

This scheduler covers the existing shadow checker's bounded source syntax
and clone rules. Larger or incomplete source closures are explicitly blocked.
Executable test selection, scalar replay and test receipts have their own
qualification requirements.

### Scheduled executable tests

The native resident writer reads `native-tests.json` in the same private
state directory as the migration marker. The version-1 registration contains
`version`, `rootIdentity`, the pinned language `worker` with its `path` and
`sha256`, a worker `budget`, and a `providers` array.
The [live scheduler fixture](../rust/frontends/public-shell/src/scheduled_tests.rs)
shows registrations for C#, Python, Cargo and FluxR.

C# maps registered projects and frameworks to evaluated MSBuild test targets.
The registration supplies pinned `dotnet` and execution-peer `assembly`,
authorized results roots, explicit runtime closures and VSTest runner pins.
The peer uses the ordinary `execution.json` build and test grants. The native
writer binds those grants and checks the verified build and TRX receipt
against the same target and attempt. On Linux the managed peer runs inside
the native owner's authenticated user cgroup.

Python selects the registered project and collects its full pytest scope.
Cargo evaluates the root workspace manifest and conservatively selects up
to sixteen workspace packages. Use a variant valid for every selected
package. FluxR executes the registered checked modules and ordered cases.
Pin the worker and tools and set the provider-specific grants as described
in the [worker contract](../rust/workers/language-execution/README.md).

The writer persists a running attempt before launching a worker. Restart
recovers the worker scope first and marks an unfinished attempt interrupted.
It records provider receipts only for the same root, Git context, source
inventory and grant. View the latest batch under `testScheduler` in daemon
status. Its ledger lives in `native-<epoch>.tests.json` and provider captures
remain in private attempt directories. The native owner resolves managed
schema-v5 C# causes only after complete project selection and a passed or
failed VSTest receipt for every eligible target. Partial selection, skipped
tests and zero discovery leave those causes pending. Every projected batch keeps its original
receipts in a private `native-<epoch>.tests-<batch>.json` recovery file.
