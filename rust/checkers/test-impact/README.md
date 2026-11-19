# Pure evaluated-project test impact

Dependency-free Rust 1.99.1 port of the *selection and explanation decisions* in
`src/SlopHawk.Daemon/EvaluatedTestAdapter.cs` at upstream commit
`a4399a0a199cd173a5b9d2048faaabfb093d1a96`.

`select(&SelectionInput)` takes **resolved evaluated concrete target IDs**, old and new
compile membership, qualified predecessor graphs, source digests and already-approved
unit-test projects from provider boundaries. It never parses XML, determines which
project is a test project, loads coverage, evaluates MSBuild, launches tests or
admits jobs. It preserves complete/partial distinction, reason kinds and chains,
selected/budget-deferred/unapproved scope and bounded explanation omissions.

The input graph is explicitly scoped to one evaluated revision; a provider issue
remains visible in `UnresolvedFrontiers` and makes the current selection partial.
`Complete = true` on an empty selection is authoritative only if providers
supplied all relevant resolved graph facts. This Rust model cannot itself audit
whether the provider's completeness claim is truthful.

For deletions and renames, an issue in any predecessor graph actually consulted
for an old path also makes the selection partial. Known targets, fingerprints,
reason chains and predecessor frontiers remain available. Unused predecessor
snapshots do not change completeness; the first-revision and missing-predecessor
conservative fallbacks retain their existing behavior. This intentionally tightens
the legacy C# selector's completeness decision for explicitly partial predecessors.
It does not detect missing facts that a provider incorrectly declares complete.

The standalone `slophawk-test-control` consumer retains all pending causes while
selection is partial, including when there are no selected targets or every known
target has completed. Its owner must repair or re-evaluate the relevant graph
facts before relying on a complete selection. This change adds no resident CLI,
MCP or execution wiring. The new predecessor fixtures require owner validation;
the historical validation results below apply to the earlier baseline only.

On adoption, an owner of an existing native test-control database must establish
a fresh valid authority fence, drain prior running work under the existing host
protocol, and reselect the full authoritative pending-cause set before consuming
old plans. This patch does not rewrite persisted selections or reconstruct causes
already retired by an earlier version.

The `PathComparison::Ordinal` mode matches Linux path comparison;
`OrdinalIgnoreAsciiCase` is deliberately limited to ASCII rather than making
false claims about non-ASCII Windows case folding. Broken selected fingerprint
reference closures fail closed with `SelectionError`. Fingerprints reproduce
C# anonymous-record `System.Text.Json` structure and uppercase SHA-256; cross-
runtime parity remains an integration gate, especially for encoder edge cases.

The source observations and candidate test identities are **not** verification
receipts. A complete selection does not mean a test was executed, passed or
asserted the modified behavior. Admission, test runs, coverage evidence and
report publication stay in their original C# owners until a separately verified
cutover.

Integration validation on 17 November 2026 used Windows x64 and Rust 1.99.1:
all 18 tests passed, including C# frontier omission-count behavior when
discarded events recur. Formatting, locked offline all-target builds and
Clippy with warnings denied passed. Unicode Windows path equivalence and
full runtime cutover remain outside this pure pilot.


## Opt-in lost-route comparison (SG-CHECK-R01)

`lost_routes::check(&lost_routes::Input)` is a separate, pure advisory over two
already captured graphs. It does not call `select`, change its output, admit work,
read files, evaluate MSBuild, execute tests, persist state, or publish a result.
The caller supplies every affected supported logical item as a `Request`.

### Required owner evidence

- Both captures must match the expected `Identity` values. Root-qualified,
  immutable source and graph snapshot tokens must bind the actual observations.
  The checker checks equality; it does not authenticate the provider or recompute
  graph digests. Root, evaluation universe, configured-group policy and path mode
  must agree across the pair. Graph/evaluation input fingerprints may change.
- `Capture.complete` must mean the entire evaluated universe. Any graph issue or
  dangling reference makes the comparison unknown. `evaluation_sufficient` must
  be false when relevant execution-created items or references could be missing.
  An issue-free evaluation alone does not establish this capability.
- `previous_configured_tests` and `current_configured_tests` are the complete
  resolved configured groups for each capture, including unapproved and deferred
  targets. They are independent of selected-test output. New configured tests can
  preserve an alternate route. A changed group policy requires a different
  `configured_scope` and yields `ScopeMismatch`; never relabel an approval list as
  a complete configured group. Group enumeration completeness remains an owner
  attestation.
- A request names the exact predecessor target and `(physical path, logical Link)`.
  Its current counterpart matches canonical project, framework, configuration,
  platform and RID. Shared target IDs with changed contexts and matching contexts
  with changed SDKs yield unknown, even outside a particular request's closure.
  IDs may differ if the full context is identical. Source-target replacements
  across contexts have no implicit successor mapping in this first version.
- `SurvivingSource` binds an independently observed current path and uppercase
  SHA-256 to the current source snapshot. This function does not read or hash that
  file. Physical or logical item renames require a current-bound `Declaration`;
  mappings are one-to-one within the supplied request set. A retirement similarly
  requires a current-bound policy provenance record and returns `Retired`.

### Decisions and evidence

An old logical item present in a supported target but absent from its matching
current target is `CompileMembershipLost` when the physical source still survives.
Another target or another logical Link cannot hide that loss. Membership loss has
precedence over route comparison.

For retained items, reverse traversal starts at that exact owner, reusing
`GraphIndex` adjacency without its physical-path owner union. A prior configured
route and an exhaustively empty current configured frontier yield
`LastConfiguredRouteLost`. Any current alternate route yields `Preserved`.
No prior configured route yields `NoPriorConfiguredRoute`. Unknowns are explicit,
and report completeness applies only to the supplied requests.

`MissingFact` identifies the first missing item, target, stored project-reference
edge, test classification or configured membership on one deterministic old
witness. Reference fields use the actual referencing-to-referenced direction.
This attribution is computed before witness truncation and is emitted as a loss
only after current absence is established. It is not necessarily the sole cause
or a cut covering every former path. The current graph model has no defining XML
file/line; the checker does not guess an import or declaration location.

### Limits and platform scope

Defaults admit at most 256 targets, 40,000 Compile records, 16,384 references and
256 requests across the two graphs; both configured inventories together are
capped by the target limit. Each string is at most 2,048 bytes. The 8 MiB admission
budget includes string bytes plus repeated target-ID copies made by GraphIndex.
A shared budget permits 262,144 reverse edge visits, charging duplicate edges and
already visited nodes. It is consumed in UTF-16 ordinal request-key order. Reaching
the exact limit is allowed; needing another edge returns `Unknown(EdgeBudget)`.
Zero-edge ownership by a configured test needs no edge budget.

At most 64 results and 16 nodes per old/current witness are retained by default.
Omitted results make `complete` false; witness omission counts do not invalidate a
completed decision. Full witnesses are inspected before presentation is capped.
All limits are explicit inputs and affect only this opt-in comparison. Allocation
and validation are also bounded by input counts and bytes, not by the output cap.

Paths must already be canonical under the supplied provider policy. Ordinal paths
support case-sensitive inputs; ASCII ignore-case supports only ASCII physical and
project paths. Logical Link names compare exactly. There is no filesystem probing,
separator normalization, Unicode Windows case folding or symlink resolution here.
Current C# capture also rejects platform-specific target frameworks; unsupported
provider contexts must remain unknown instead of being represented as empty facts.

### Status and owner gates

This SG-CHECK-R01 addition and `tests/lost_routes.rs` are a source-only proposal.
No compilation, builds, test execution, or CI was performed for this packet.
Source applicability is weaker than type, runtime, parity or performance validation.
Owner gates are `cargo +1.99.1 test --locked --manifest-path
rust/checkers/test-impact/Cargo.toml`, the direct consumer's test-control test gate,
and the repository completion gate `dotnet test SlopHawk.slnx`; all are unrun here.

The pinned live public-shell scheduler currently calls its separate C# scheduling
path, not test-control/test-impact. Pair capture, persistence, support/group policy,
source-survival/rename evidence and publication freshness remain external owner
handoffs. This addition is not enabled by a daemon or a default configuration.

## Capture verification

`lost_routes::capture` adds a versioned, pure integrity boundary before the existing
comparison. It does not turn the live `projects` JSON into an authenticated capture.
The normal CLI, MCP, managed worker, scheduler, selection and admission paths are
unchanged. There is no new command, configuration default, file reader or process.

The owner supplies two `CapturedEvaluation` values, separately retained
`ExpectedCapture` values, independent `SourceObservation` bytes, a `SupportPolicy`,
and an explicit bounded list of `OwnerRequest` mappings. `verify_pair` checks their
binding and constructs a private `VerifiedPair`. Its `check` method requires a
fresh independent current `Stamp` and calls the existing bounded graph comparison.

### One supported policy

Version 1 requires canonical relative paths with `PathComparison::Ordinal`, one
concrete target per project in each independently declared complete target roster,
and explicit owner qualification of immutable, evaluation-sufficient inputs.
Per-capture rosters may differ; target replacements still have no implicit mapping
in the core. Actual SDK version, provider and toolchain closure digests, complete
global properties, environment identity and fixed external-input closure must be
comparable. Changing in-root source/import contents belong to the source snapshot.
Unsupported path modes, incomplete graphs, unqualified target-time inputs, and
missing or mismatching evidence remain unavailable.

The supported configured-group policy is explicitly `AllEvaluatedTests`. The
verifier derives every `IsTestProject` endpoint from each complete graph. It accepts
no approval or selection list. A reachable unapproved or budget-deferred test can
therefore preserve a configured route. This policy grants no execution authority.
Other configured-group policies need a separately specified resolver and version.

Graph and observation fingerprints are domain-separated and length-prefixed,
with deterministic ordering and bounded inputs. They bind actual graph facts and
independently supplied source bytes, preserving physical paths and logical Link
names separately. A source missing from current Compile membership must still be
observed independently to establish survival. The verifier derives its SHA-256;
absence from Compile is never interpreted as physical deletion or retirement.
Physical or logical renames and retirements require current-bound owner declarations.

### Trust and lifecycle prerequisites

An `ExpectedCapture` must originate in trusted owner-controlled capture management,
retained separately from the candidate envelope. Do not copy an envelope's claims
into an expectation at verification time. The fingerprint helpers establish byte
consistency; they do not authenticate a collector or prove completeness of its
input observations. `SupportPolicy` records the owner's qualification, not a
capability inferred from `ProjectGraph.complete` or a nonempty digest.

A source snapshot must bind immutable contents and all relevant metadata/read
inputs, including untracked inputs, missing-file probes and directory membership
when applicable. Equal pre/post root, source and Git labels cannot rule out mixed
reads or change-and-revert races. The owner must evaluate and observe the same
immutable snapshot or provide an equivalent qualified atomic observation contract.
Currentness must be revalidated under the publication boundary; a pure method
cannot prevent a later filesystem/Git change after it returns.

The existing evaluator supplies neither this lifecycle nor the independent policy
and source observations. It uses target-free MSBuild item/property queries, hashes
selected properties and discovered in-root metadata, and checks physical root
replacement. SDK targets can add Compile items during execution, external inputs
can affect evaluation, and `MSBuildAllProjects` is not a complete read manifest.
Those gaps require an owner producer, persistence, qualification, complete request
enumeration and publication revalidation before any live lost-route opt-in.

`Report.complete` still means that every supplied request was decided within the
comparison caps. It does not prove the owner enumerated every affected item,
establish test execution or coverage, or attribute an edge to a defining XML line.
The existing raw `lost_routes::check` remains available under its original
owner-attestation contract; the new seam does not silently upgrade old inputs.
