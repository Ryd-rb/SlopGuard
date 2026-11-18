# SG-MIG-02: opt-in four-language native shadow checker

This is a **separate** Rust CLI. It does not replace the current C# application,
daemon, watcher, public command entry point, persistent index, or test runner.
It invokes one existing .NET **fact** worker for each requested C# document and
never starts that worker for Python, Rust, or FluxR requests.

## One-shot command

Use Rust **1.99.1** and a working native linker. This source-only delivery
intentionally does **not** include an invented `Cargo.lock`. The integration
owner must first regenerate and review the complete locked dependency closure
using the pinned manifests. From the verified SlopHawk checkout, **after**
the owner has qualified the generated lock:

```sh
export CARGO_TARGET_DIR="$(mktemp -d)" # outside the checkout
cargo +1.99.1 generate-lockfile --manifest-path rust/native-checks/Cargo.toml
# Review and commit the generated Cargo.lock in a separate, qualified integration change.
cargo +1.99.1 build --locked --manifest-path rust/native-checks/Cargo.toml
cargo +1.99.1 run --locked --manifest-path rust/native-checks/Cargo.toml \
  -- shadow --request /absolute/path/to/scoped-request.json
```

The JSON request is typed, `version: 1`, and explicitly enumerates every source;
there is no file discovery. The receipt is JSON on stdout; invalid requests and
worker failures never silently turn into a clean finding. Program exit codes are
0 for accepted non-failed scope, 1 if the report includes a failed document, and
2 for rejected input or reporting errors. An `unsupported` or `partial` rule
must not be interpreted as a clean outcome.

The request shape:

```json
{
  "version": 1,
  "workspaceEpoch": "epoch-1",
  "committedRevision": "rev-candidate",
  "workspaceRoot": "/absolute/project",
  "linkedRoots": [],
  "context": {
    "key": "logical-evaluated-context",
    "pythonMinor": 12,
    "rustEdition": "2024",
    "fluxrModule": "my-module@checked-v1",
    "targetFramework": "net10.0",
    "languageVersion": "CSharp14",
    "assemblyName": "MyAssembly",
    "referencePack": "/absolute/dotnet/packs/Microsoft.NETCore.App.Ref/10.0.15/ref/net10.0",
    "referenceClosureComplete": true,
    "sourceClosureComplete": true
  },
  "worker": {
    "dotnet": "/absolute/dotnet/dotnet",
    "assembly": "/absolute/SlopHawk.Providers.CSharp.Worker.dll"
  },
  "sources": [
    {
      "language": "python",
      "documentId": "logical:source-1",
      "logicalPath": "src/example.py",
      "physicalPath": "/absolute/project/src/example.py",
      "provenance": "normal",
      "bytesBase64": "eCA9IDEK",
      "sha256": "<lowercase SHA-256 of decoded bytes>",
      "byteMapping": "exactUtf8"
    }
  ],
  "limits": { "minCloneTokens": 4 }
}
```

For a real request replace the placeholder digest with the SHA-256 of exactly
`x = 1\n` or the source you provide. `worker` and the C# context fields may
be omitted for a request without C#; if specified, they are not launched for
non-C# jobs. `normal` files must lie lexically within `workspaceRoot`. `linked`
and `generated` files outside it require explicit `linkedRoots` grants. The
caller, not the worker, owns source enumeration, digest validation, epoch,
revision, evaluated context, and grants. Physical paths are *source identities*,
not a filesystem sandbox or source loading instruction.

## Projection and rule gates

| Provider | Source scope admitted to clone-index v1 | Exact rule | Renamed-local rule |
| --- | --- | --- | --- |
| C# / `slophawk.csharp.roslyn.experimental` | Complete, byte-mapped, untruncated **whole source** token stream only | Yes, if worker status/capabilities/terminal/exit are accepted | **Unsupported**: `localRole` is syntax-only, no semantic local binding |
| Python / Ruff 0.0.22 | Function/class regions, or complete source unit without them | Yes, when token/range capabilities are complete | Only if the provider's local-binding capability is **complete**; partial frontiers disable normalization |
| Rust / `ra_ap_syntax` 0.0.363 | Source blocks excluding parse, cfg, macro and other unresolved overlaps | Yes on admitted blocks | Only conservatively proven simple, same-body `let` occurrences; all other names remain exact |
| FluxR / `FluxR-lang` `e089fa301668c0d28821f72b08f6e4086a06ef27` | **Whole canonical checked document** only when there are no opaque native items | Yes using source-whitespace tokens | **Unsupported**: declaration byte ranges and binding IDs are not provided |

The actual clone index implements whole-block Type-1 (exact) and Type-2
(proven-local-renamed) matching. Near matching, cross-language matching,
spelling/dictionary findings, semantic call graphs, runner execution, generated
macro output, and declaration-level FluxR matching are **not** implemented.
For Python, the response advertises `sg10.python-layout-clones.v2/exact` and
`sg10.python-layout-clones.v2/renamed-proven-local`; accepted pairs use the same
v2 prefix. These rules require equal relative indentation and logical-line
boundaries in addition to the shared index's lexical comparison. Other
languages retain `sg10.token-clones.v1/exact` and
`sg10.token-clones.v1/renamed-proven-local`. The unsupported
`sg10.token-clones.v1/near` remains visible for every language.
See [Python clone qualification](../../docs/python-clone-layout.md) for the
current profile-backed resident caller and receipt-version invalidation.
Unavailable FluxR declaration spans and opaque native item spans appear as
concrete gaps. A lexical occurrence cannot turn into a semantic local binding.

### Valid Python sources without clone tokens

A nonempty Python source containing only whitespace, a UTF-8 BOM, or comments
can have a complete, empty substantive token stream. When the provider confirms
that outcome with complete token coverage and no gaps, the projection reports
the exact rule as complete over that empty token scope and submits zero blocks.
It does not fail the job or discard clone pairs from other source documents.
The renamed-local and near rules remain unsupported for that document, so the
aggregate report can still be partial. This is not a claim of repository safety.

Zero-byte source requests are still rejected by admission. Invalid syntax,
invalid encodings, resource failures, unsupported grammar or encoding, and an
unavailable original-byte mapping retain their existing dispositions. A
comment-only file is a complete provider result, whereas whitespace-only input
can be `NoRelevantFacts`; both can legitimately have zero clone tokens.

## Framing, input admission, and bounded reporting

The C# worker uses its **existing** `uint32_le length + UTF-8 JSON` framing,
not a new interoperable provider wire protocol. The caller sends only typed
argument array `dotnet`, `worker.dll` (no shell), checks every `fact` frame
scope, byte span, token value, and capability, checks complete source closure,
C# target/language and evaluated context, provider, epoch, revision, document and SHA-256 from the terminal,
then requires a successful exit before using any facts. Failed/partial worker
jobs are never misreported as complete. Facts are staged until that point.

The per-request defaults (lower ceilings are allowed; higher ceilings are
rejected) are: 24 sources; 1 MiB Rust source, 512 KiB Python/C# source,
256 KiB FluxR source; combined source 2 MiB (C# closure max 768 KiB);
4 MiB JSON request; C# frame 1 MiB; C# combined request/response 8 MiB;
C# stderr 64 KiB; C# worker deadline 10,000 ms; terminal 64 KiB; report
1 MiB; maximum 2,400 worker records and 100,000 syntax nodes. The C# fact
worker also limits references. The clone-index budgets are configurable with
bounded admission. An output too large to preserve the scope/gap evidence fails
rather than hiding unsupported work.

Rust parser execution is in-process for this **opt-in** prototype. Input/fact
budgets limit size, but not a hard native CPU or RSS quota. C# worker
transport uses private, seekable files rather than blocking stdio pipe threads;
nonreading stdin and inherited descendant stdio therefore cannot block the
supervisor's drain or reader. The complete per-worker lifecycle has a monotonic
admission/execution/exit/drain deadline, with bounded output and no published
facts on failed scope observation, terminal or exit.

On **Linux** the worker is enrolled before its entry point through a transient
user-systemd **service**, with `--service-type=exec`, cgroup-v2 population and
unit invocation verified during execution. Cancellation or overrun kills the
**entire unit** and requires evidence of drainage. A missing user manager,
cgroup v2, service identity, or delegation **fails closed**; ordinary process
kill or process groups are never accepted as containment proof. On **Windows**
`CreateProcessW` uses STARTUPINFOEX `PROC_THREAD_ATTRIBUTE_JOB_LIST` to enroll
the process atomically into a kill-on-close Job Object, with a restricted
inherit-handle set, recursive termination and active-process accounting before
facts can be consumed. Other platforms fail closed for C# work. Unsupported
kernel or API versions may reject C# work rather than falling back to an
unsupervised worker. OS scopes manage lifetime, **not** filesystem/network
isolation. The caller must not provide untrusted executables or reference-pack
paths. Scope-specific timeout paths require qualification on both OS targets;
the source-only pass performed no runtime tests.

The request uses a **public CLI JSON** envelope and the unchanged **private
C# worker frame-v1** transport. No `slophawk.migration-job/1` internal daemon
transport is introduced; this standalone executable does not change the
shipped daemon IPC, MCP or artifact schemas. A new daemon-side transport would
need independent versioned admission, grant and full terminal identity before
activation.

## Validation and promotion

```sh
manifest=rust/native-checks/Cargo.toml
# Owner-only: first produce/review rust/native-checks/Cargo.lock (not in patch).
cargo +1.99.1 generate-lockfile --manifest-path "$manifest"
cargo +1.99.1 fmt --manifest-path "$manifest" -- --check
cargo +1.99.1 build --locked --manifest-path "$manifest"
cargo +1.99.1 test --locked --manifest-path "$manifest"
cargo +1.99.1 clippy --locked --manifest-path "$manifest" --all-targets -- -D warnings
# To require built actual C# worker testing:
VARO_REQUIRE_REAL_WORKER=1 VARO_DOTNET=/abs/dotnet \
  VARO_WORKER_DLL=/abs/SlopHawk.Providers.CSharp.Worker.dll \
  VARO_REFERENCE_PACK=/abs/net10.0 cargo +1.99.1 test --locked \
  --manifest-path "$manifest" --test shadow \
  real_csharp_worker_source_unit_parity_when_explicitly_admitted -- --exact --nocapture
```

Linux worker-lifetime fixture tests require an explicitly configured user
manager and `VODA_TEST_SYSTEMD_SCOPE=1`; those tests include a nonreading
worker, descendant-held output, crash, deadline, cancellation, and kill/drain
paths. The real worker test is explicitly conditional on a **built** .NET worker and
net10.0 ref pack; skipping it must be recorded as missing C# end-to-end
parity, not as a pass. Run the existing solution's locked restore/build/test
before and after applying this patch. On the same Linux x64 host, record
five or more repeated one-shot CLI runs of the same source fixture, elapsed
wall time and concurrent process-tree PSS snapshots from `/proc/*/smaps_rollup`,
including the spawned .NET worker. Compare like with like against the incumbent
checker; this prototype does not itself establish RAM/latency promotion gates.
The repository's broader Windows/Linux and daemon cutover are separate.
