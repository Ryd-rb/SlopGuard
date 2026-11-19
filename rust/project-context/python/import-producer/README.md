# Admitted Python direct-import producer

This standalone, opt-in Rust library bridges
`slophawk_python_syntax_provider::imports::extract` to the existing
`slophawk_python_project_context::import_layers::compare` input. It has no CLI,
MCP registration, filesystem discovery, Python execution or result publication.
Existing callers do not acquire this dependency. The source provider's old
`analyze(SourceRequest)` API, syntax-facts/v2, tokens and clone layout are unchanged.

## Admitted meaning

The producer accepts top-level Python 3.12 `import package.module`, including
multiple names and `as` aliases. A target becomes `Internal` only on an exact
ASCII dotted-name match in a declared unique regular-package inventory. An
unused alias still creates the same declared dependency. The import witness is
the whole AST statement's nonempty half-open range in the original UTF-8 bytes;
BOM and CRLF are preserved. Package `p` is `p/__init__.py`, including an empty
initializer, and every parent package must be present.

Blank/comment-only files, `pass`, and inert string-expression statements are
known no-import controls. Every other executable statement is outside this
first slice. Definitions, assignments, calls and compound statements therefore
leave a frontier even if their text contains no recognizable import spelling.
This deliberately catches hidden dynamic mechanisms such as an aliased
`__import__` call without pretending to perform binding or call analysis.

This is a structural graph of declared direct imports. It does not certify
runtime import success, imported objects, initialization order, implicit parent
loader edges or execution reachability. The existing comparison semantics and
novelty rules are unchanged.

## Required coordinator evidence

Construct `Admission`, `CapturedModule` entries and `Request` explicitly, then
call `produce(request, independently_expected_revision, limits)`. The admission
schema is `python-unique-regular-inventory/v1`; there is no default admission or
implicit conversion from project-context's `Inventory::Complete` or package
hints. All four declarations must be true:

- Read access was authorized and remains valid for this immutable capture.
- Every regular `.py` module in every admitted package was captured, including
  package initializers. Missing, unreadable, generated-only or extension-only
  modules cannot be silently omitted and declared complete.
- The roots have unique regular-package meanings in the declared environment,
  with no competing/shadowing roots or namespace portions.
- Physical containment and absence of symlinks, hard-link aliases and other
  path aliases were checked by the coordinator.

This library validates lexical structure, not physical filesystem facts. A
caller can lie about its permission or inventory; hashes do not authenticate
those assertions. Supply independently captured lower-case SHA-256 hashes for
every module. The producer recomputes them over the original bytes and checks
the independently supplied revision before parsing. It never consults `sys.path`,
`find_spec`, an interpreter, a virtual environment or installed packages.

Root package names must be unique without ASCII case aliases; effective package
directories must not overlap. Distinct package directories may share a source
root. Module names and paths must be
unique, case-distinct, canonical ASCII and exactly agree with the declared root
and package/module role. Namespace or missing parent initializers are refused.

The output `SnapshotIdentity.source_sha256` streams domain-separated,
length-prefixed sorted module names, paths, package flags and original bytes.
The context digest binds this producer schema (`python-admitted-imports/v1`),
syntax schema/provider, pinned Ruff parser identity, Python 3.12, import
semantics, workspace identity, environment/admission-policy digests and sorted
root mapping. It excludes revision and module contents so genuine module
additions/deletions remain comparable. A change to that meaning requires a new
schema and comparison context. There is no wire format or persistence change.

## Evidence dispositions

| Evidence | Result |
| --- | --- |
| Exact admitted absolute import | Attributed `Internal` edge, `Runtime` use. |
| Missing target, including unproven standard-library/external names | `Unknown(UnresolvedTarget)` and `complete = false`. |
| `from p import x` (even if `p/x.py` exists) | `AmbiguousReexport` frontier. The package attribute can win over the submodule. |
| Explicit relative `from` import | `UnsupportedSyntax` frontier; no guessed base or unchecked dot traversal. |
| `if`, `try`, type-only-looking or unbound `TYPE_CHECKING` guard | `UnresolvedGuard` frontier; never `ProvenTypeOnly`. |
| Other executable syntax, including dynamic import expressions | `UnsupportedSyntax` frontier over the enclosing statement. |
| Unsupported/NFKC name spelling | `UnsupportedName` frontier. |
| Denied, incomplete or unattested inventory | `Err(AdmissionRequired(...))`, no graph. |
| Stale revision / original-byte hash mismatch | `Err(StaleRevision)` / `Err(HashMismatch(...))`, no graph. |
| Duplicate/case-aliased roots or modules, bad paths, absent initializers | Structured `Err`, no graph. |
| Invalid syntax, unsupported grammar/encoding, stub input | Extraction error or inventory-path rejection, no graph. |
| Invalid or exhausted budgets | Structured `Err`, no truncated graph. |

An `Ok` result may be incomplete and carry unknowns. Pass it unchanged to
`compare` with a separately admitted before/after `SnapshotPair`, explicit layer
policy and comparison budgets. The existing core returns `Unknown` and zero
findings for any input frontier, even in an unrelated module; a source-only
empty graph is not a substitute. Only budget exhaustion during the core's graph
search may preserve findings already proven by that search.

## Limits and deterministic work

Defaults per snapshot: 2,048 modules, 16 MiB original bytes, 16,384 combined
import/unknown records; per file 512 KiB, 65,536 parser tokens and 16,384 records.
Hard limits are the same aggregate limits, 128 roots, 256-byte names,
1,024-byte paths, 160-byte identity labels, and per file 4 MiB, 131,072 tokens,
32,768 records. Limits above these maxima or zero limits are refused. The
comparison core still applies its independent combined-before/after string,
root, graph-work, finding and frontier budgets; valid producer output can
exceed a comparison budget. In particular, compare permits only 128 roots
combined, so matching snapshots with more than 64 roots return `InputLimit`.

Record and source byte counts are checked before copying metadata or hashing.
The syntax producer pre-counts records before allocating its output vectors.
Aggregate fact caps are checked after each bounded file extraction and before
extending the graph; a failure discards the entire provisional snapshot.
Modules/roots are ordered lexically; occurrences follow source/alias order.
Reordering the supplied inventory does not change successful identities or
facts. Error selection for multiple malformed inputs follows validation order.

Excluding Ruff parsing, work is linear in captured bytes plus ordered inventory
and target lookups: `O(B + (M + I + M*D) log M + R^2*P)`, with bounded name/path
comparison costs. Here B is source bytes, M modules, I import occurrences,
D dotted-name depth, R roots and P maximum path length. Range checks use one
UTF-8 validation per file, then constant-time boundary checks. Live additional
storage is the bounded graph/maps plus one file's parser/facts; aggregate source
hashing streams bytes rather than duplicating the inventory. These are input
and record bounds, not a proof of Ruff's worst-case parser complexity, hard RSS,
wall-clock time or cancellation. Production needs a supervised parser worker.

## Regression tests

The fixture source uses actual captured bytes and hashes and exercises the
producer before the existing pure comparison. Its in-memory admission is an
explicit development assumption; it does not qualify filesystem capture or a
production trust channel. The old comparison fixture remains useful for pure
graph rules, including type-only/optional facts this producer never claims.

```sh
cargo +1.99.1 test --locked --manifest-path rust/providers/python/Cargo.toml
cargo +1.99.1 test --locked --manifest-path rust/project-context/python/Cargo.toml
cargo +1.99.1 test --locked --manifest-path rust/project-context/python/import-producer/Cargo.toml
dotnet test SlopHawk.slnx
```

The lock reuses the provider's pinned registry versions and checksums.
The .NET solution remains governed by `global.json`: 10.0.100 with
`latestFeature` roll-forward and prereleases disabled.

## Production boundary

Native clone admission supplies single-source syntax context, not two complete
module inventories, physical-root/alias evidence, an independent expected
snapshot pair and layer policy. CLI/MCP import-layer checking therefore stays
unavailable. Publishing findings requires an admitted evidence channel and
supervised parser execution.

Primary semantic references: [Python 3.12 imports](https://docs.python.org/3.12/reference/import.html),
[import statements](https://docs.python.org/3.12/reference/simple_stmts.html#the-import-statement),
and the existing `import_layers` typed contract. Broader
[Grimp behavior](https://grimp.readthedocs.io/en/stable/usage.html) and
[Import Linter layer contracts](https://import-linter.readthedocs.io/en/stable/contract_types/layers/)
are alternatives, not evidence of runtime resolution for this slice.
