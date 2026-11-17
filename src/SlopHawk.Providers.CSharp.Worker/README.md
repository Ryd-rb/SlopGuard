# C# on-demand Roslyn fact worker — experimental frame v1

Provider identity: `slophawk.csharp.roslyn.experimental`; parser version:
`roslyn-extractor/1;worker/1`. Not a shared native-provider wire contract.

Launch the **worker itself**, once per request, as
`dotnet SlopHawk.Providers.CSharp.Worker.dll < framed-request.bin > framed-response.bin`.
It reads exactly one request, writes zero or more `fact` frames and exactly one
terminal `result` frame when the output channel is healthy, then exits. The
process has no commands to restore, build or run the target, execute analyzers
or generators, or evaluate MSBuild. stderr is separate from the framed stdout.

## Frame shape

Each frame is `uint32_le length` followed by exactly `length` bytes of UTF-8
JSON. `1 <= length <= 1,048,576`; the length is checked before allocation.
JSON properties are case-sensitive and duplicate/unknown properties are errors
for the typed request. An incomplete header/body, wrong version or invalid
encoding causes a `failed` terminal and nonzero process exit when writable.
Only one request is consumed. The **total** input/output wire byte budget is
8 MiB, with 64 KiB reserved for the terminal result. No frame is emitted
partially. A successful job exits 0, including honest `partial` scope;
invalid/failed jobs exit nonzero.

Request example (encode this object as one length-prefixed UTF-8 JSON frame):

```json
{
  "version": 1,
  "kind": "analyze",
  "workspaceEpoch": "epoch-3",
  "committedRevision": "rev-abcd",
  "workspaceRoot": "/w/project",
  "linkedRoots": ["/w/shared"],
  "documentId": "logical:A",
  "context": {
    "key": "project|net10.0|Debug",
    "targetFramework": "net10.0",
    "assemblyName": "Fixture",
    "languageVersion": "CSharp14",
    "referencePack": "/dotnet/packs/Microsoft.NETCore.App.Ref/10.0.x/ref/net10.0",
    "defines": ["FEATURE"],
    "checked": false,
    "optimize": false,
    "referenceClosureComplete": true,
    "sourceClosureComplete": true
  },
  "sources": [{
    "documentId": "logical:A",
    "logicalPath": "src/A.cs",
    "physicalPath": "/w/project/src/A.cs",
    "provenance": "normal",
    "bytesBase64": "Y2xhc3MgQSB7IH0=",
    "sha256": "PRECOMPUTED_SHA256_OF_THE_DECODED_SOURCE_BYTES",
    "byteMapping": "exactUtf8"
  }],
  "limits": { "maxRecords": 1200, "maxSyntaxNodes": 80000,
    "maxFrontiers": 32, "maxDiagnostics": 24 }
}
```

`sha256` is a 64-character hexadecimal SHA-256 of **decoded original bytes**,
not an arbitrary claimed source fingerprint. Source text must be strict UTF-8,
optionally with BOM. Exact UTF-8 byte spans account for the BOM, supplementary
Unicode, and original CRLF; the native span is Roslyn's zero-based UTF-16
`start`, `length`, `startLine`, `startColumn`, `endLine`, `endColumn`.
`byteMapping: "unavailable"` suppresses byte coordinates and carries an explicit
`{ "known":false, "gap":"caller_declared_unmapped" }` even though the input
bytes are decoded. A UTF-16 offset inside a surrogate pair is also unknown.
No text normalization is applied to source. The worker neither opens nor
materializes source files on disk.

`normal` physical paths must be within `workspaceRoot`. `linked`/`generated`
paths outside that root must fall within a caller-authorized `linkedRoots`
entry. Paths are lexical identities (sources are never read from those paths),
not proof of real filesystem containment. This request is a trusted boundary
from the future native orchestrator, not a sandbox for arbitrary tenants.
The referenced framework `net10.0` pack is read from a caller-supplied
absolute path and independently digested. Closure completeness is a claim
**by the upstream evaluator**, not inferred from a successful Roslyn parse.
The context's set of source files and defines represents a single evaluated
compilation. No project, solution or workspace evaluation occurs here.

Each fact frame uses `version=1`, `kind="fact"`, `provider`, and `scope`
(`workspaceEpoch`, `committedRevision`, `contextKey`, `documentId`,
`sourceSha256`), plus `capability`, `span` and `value`. Capabilities are
`declarations`, `references`, `calls` and `tokens`. Reference and call symbol
IDs/binding kinds come from the existing `RoslynDocumentFactExtractor`; a
static call is not proof of runtime execution. Tokens have category and a
**local syntax** role only (`declaration_name`, `invocation_target_syntax`,
`unknown`); they are never asserted to resolve a symbol. Token text over 128
UTF-16 units is visibly truncated; the span still locates the exact source.
The terminal `kind="result"` includes actual SDK/Roslyn versions, target,
defines, all source identities/digests, the digested reference pack, evaluated
capabilities (`complete`/`partial`/`failed`), bounded compiler diagnostics,
extractor frontier, gaps, record counts, and `complete`/`partial`/`failed`
status. A limitation or unresolved/dynamic edge is **not** silently promoted
to a complete static call graph. Generated and linked provenance are as
supplied by the caller; this worker does not run generators.

## Resource envelope

- One request frame at most 1 MiB; up to 24 C# files, 512 KiB each and
  768 KiB decoded source bytes total; up to 16 authorized linked roots
  and 64 distinct defines.
- Reference pack: 2–256 DLLs, at most 64 MiB of reference bytes, with
  `System.Runtime.dll` inside a directory named `net10.0`.
- Up to 2,400 fact/token records, 100,000 syntax nodes examined by the
  extractor, 32 frontier entries and 32 diagnostics; smaller client budgets
  are accepted. Only one frame (at most 1 MiB) is held for output encoding.
- At most 8 MiB combined input/output wire bytes; 64 KiB reserved for the
  terminal frame. Limits and omissions are included in the terminal scope.
- This is a bounded **source pass**, not a security or CPU/time sandbox. The
  native orchestrator will enforce process time/memory admission separately.

The wire shape is intentionally not a stable cross-provider protocol. Shared
native scheduling, transport and publication require independent integration
review alongside the Python pilot.
