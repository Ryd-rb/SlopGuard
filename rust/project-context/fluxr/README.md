# FluxR checked module/test + ordered replay pilot

This **opt-in Rust library and supervised worker** is not wired into SlopHawk's
CLI, daemon or project resolver. The existing `rust/providers/fluxr/` passive
checked-source provider remains passive. Public cutover stays blocked on parity,
source/worker provenance, environment admission, Windows/Linux system testing,
and end-to-end process-tree memory/latency measurements.

The crate reuses the provider's pinned FluxR compiler at
`e089fa301668c0d28821f72b08f6e4086a06ef27` / tree
`20f89b64aa479efd19e9e60d367eaa9a83e804d3` and the shared
`slophawk-probe-replay` library for exact ordered check and comparison semantics.
Every test invocation declares its module IDs, the original `.fluxr` source
paths, source SHA-256 digests, canonical checked codec, operation-to-export
bindings, input and argument layouts, scalar result type, VM limits, stdout-only
output scope and an externally qualified worker executable SHA-256. The worker
must be built with Rust `1.99.1`, with native/JIT features disabled. It only
executes the portable checked VM and has no host-service registry or filesystem
access. The parent rereads declared sources immediately before and after the
supervised child process. Changed files, changed binaries and altered worker
outputs yield no accepted observations.

## Coverage boundary

- **Supported:** canonical checked FluxR 1, compiler-accepted explicit module
  exports, portable checked execution in bounded fresh workspaces, integer probe
  arguments mapped explicitly to FluxR `i64`, `u64` and `bool`, exact comparison
  against caller-supplied independent scalar expectations, and repeated ordered
  replay through the shared comparator. A boolean maps `0=false`, `1=true`;
  other integers are individually unsupported. Unsigned inputs map only
  nonnegative signed integers (the probe format cannot represent `u64 > i64::MAX`).
  Results are bare JSON scalar values in the shared probe protocol.
- **Individually unsupported:** unknown operation, invalid case argument layout,
  out-of-domain scalar mapping, explicit native Rust items, host services,
  native execution, compact source, native-host or other source codecs,
  structured/list/float/nominal values. Unsupported never passes a check or
  qualifies a missing capability. Native items remain opaque to passive analysis.
- **Hard failures:** invalid compiler/mapping/source IDs, failed source parsing or
  type checking, stale source or worker digest, checked runtime errors, worker
  crashes/nonzero status, missing/wrong-order output, malformed or forged
  unsupported output, timeout, and cancellation. No failed run is converted to
  `null` or a clean comparison. Records and diagnostics have explicit limits.

## Trust and limitations

The worker has no shell or native service support, but this is **not a sandbox**.
The caller/coordinator must independently qualify the worker binary, its build
inputs and the Rust 1.99.1 compiler and keep worker directories under trusted
ownership. The API checks the declared build identity and executable hash; it
cannot attest to a compiler merely from a supplied version string. There is a
remaining file digest/spawn TOCTOU window on mutable untrusted files, so do not
run against attacker-controlled paths. The worker is single-process checked VM
code and never spawns descendants; cancellation/timeout kills that process. A
future privileged coordinator must enforce a full process-tree scope for any
broader runtime or host-service admission.

Per-run maxima: 16 modules, 64 operations, 256 ordered cases, 64 input arguments,
256 KiB per declared file, 1 MiB total source, 2 MiB worker job, 128 KiB worker
stdout, 4 KiB stderr, 60-second child execution deadline, 1,000,000 VM instructions, 262,144
cells, 256 frames and at most 16 retained check findings (exact total retained
separately). The shared replay parser retains its own strict JSON limits.
Output is stdout JSON only: this crate writes no receipts or project files.
The owner decides whether/where to publish an accepted receipt. `observe_only`
marks unaudited observations; a passing replay means repeatability within the
same job identity, **not** correctness against independent expectations.

The 60-second deadline covers direct child execution only. Source admission,
file hashing, and checked parsing/compilation in the caller run before that
deadline starts; this pilot has no end-to-end time or process-memory bound.
Source and executable reads retain at most their byte cap plus one rejection
byte even if a file grows after its metadata check. The public file-digest
utility hashes incrementally without retaining the complete file.

## Build and focused tests

Integration validation on 17 November 2026 passed all 10 available tests,
formatting, locked offline all-target builds and Clippy with warnings denied
on Windows x64 with Rust 1.99.1. This includes the real checked worker and
bounded-read regression. Two Unix-only supervision cases and Linux execution
remain unverified; no production migration or performance claim follows.

From the verified pinned SlopHawk checkout and with the approved Rust compiler,
Git dependency, locked registry cache, linker and `CARGO_TARGET_DIR` outside the
checkout:

```sh
manifest=rust/project-context/fluxr/Cargo.toml
cargo +1.99.1 fmt --manifest-path "$manifest" -- --check
cargo +1.99.1 build --locked --manifest-path "$manifest"
cargo +1.99.1 test --locked --manifest-path "$manifest"
cargo +1.99.1 clippy --locked --manifest-path "$manifest" --all-targets -- -D warnings
```

The integration tests resolve the freshly built worker through Cargo's
`CARGO_BIN_EXE_slophawk-fluxr-checked-worker`, hash it and supervise real child
processes. Separate Unix-only shell fixtures verify malformed stdout, timeout
and pre-cancel handling; these are intentionally untrusted *test* executables.
No network, native execution or service is used by these fixtures. The tests
exercise pinned upstream checked affine, Boolean, unsigned and declared-input
sources, an independently incorrect expectation, repeated replay, unsupported
cases, stale source, compiler refusal, arithmetic trap and bounded failures.

To consume this contract, a coordinator must produce a `Request` and a
`WorkerProfile` from an authorized checkout and call `check`, `observe` or
`replay_ordered`; the caller supplies expectation or input JSON using the
shared `ProbeInput` / `ProbeExpectation` shapes. Treat only a checked (not `observe_only`) `Outcome::Passed`
as an expectation-verified result within the declared scope. No automatic baseline promotion occurs.
