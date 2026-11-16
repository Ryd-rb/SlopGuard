# Robustness contracts

The [architecture](architecture.md) owns process and execution policy. These contracts define what consumers may infer from persisted state, job results, and publication. Treat proposed interfaces here as design until the relevant [roadmap](roadmap.md) gate passes.

## State transitions and durable ownership

A confirmed source batch commits its observations, completeness, changed paths, predecessor evidence and test causes together. A full known inventory may withdraw deleted paths. An uncertain subtree retains the last-known entries and carries an uncertainty frontier. Read errors in one subtree may delay verification there while independent readable scope advances. A repository metadata change can invalidate a graph or report without creating a C# test cause.

Keep four identities distinct: physical checkout, durable source revision, execution authority and publication epoch. For each scalable job or partition, add tool and policy identities and the exact input fingerprint. The physical checkout must match throughout a transaction that consumes its observations. A source revision and policy can survive a daemon restart. Publication tokens expire at restart. An empty current field requires a complete query over its recorded scope.

| Failure or transition | Durable state and retry |
| --- | --- |
| Interrupted input enumeration | Keep the old confirmed facts in uncertain scope. Retry a bounded scan and publish the frontier. |
| SQLite busy, full or I/O failure before commit | Keep the prior revision and intents. Normalize the failure to a recoverable status or explicit blocker. Retry with backoff after revalidating the root and inputs. |
| Unknown database schema or untrustworthy durable state | Leave the file owned by its current format and surface recovery instructions. Invalidate current claims. A separately identified derived partition can be rebuilt independently. |
| Completed work after scope supersession | Retain only bounded historical outcomes. Replan outstanding causes against the current graph and policy. |
| Death between admission and receipt | Consult the persisted worker scope and attempt. Wait for proof of containment drainage before releasing outputs and retrying. |
| Daemon death during report replacement | After reacquiring writer ownership, reject the old incarnation and validate fresh state before publishing. |

Version schema migrations transactionally. Assert preservation of pending causes, graph predecessor commitments and output recovery identities across represented versions. At each fault boundary, reopen with a fresh process or database handle and verify the durable revision, attempt disposition and retry obligation. Bound retention by configured size and count. Reserve space for durable progress and deletion before accepting optional history. [SQLite atomic commit](https://sqlite.org/atomiccommit.html)

## Repository execution

An authorized operation still requires a supported toolchain, allowed paths and a supervisor that meets the operation's lifetime contract. Resolve authority outside the checkout. Keep decisions about evaluation, build, test, restore, network and compiler extensions independent. Supersession or revocation discards future publication rights immediately. Release process-owned output roots only after confirmed drainage. Repository text, test output and analyzer diagnostics carry no authority to add grants.

For Linux execution, evaluate a transient service in a user-managed systemd cgroup v2 as the first containment candidate. Require a stable, persisted scope ID and a manager that enrolls the process before it executes repository code. Query the same scope during restart and validate its ownership. Test recursive termination and containment drainage with nested subprocesses and an owner crash. Missing delegation, unknown service identity, disabled user manager or ambiguous drain status blocks admission or recovery as applicable. File-system and network isolation remain properties of a separately configured OS boundary. [Linux cgroup v2](https://docs.kernel.org/admin-guide/cgroup-v2.html), [MSBuild execution guidance](https://learn.microsoft.com/en-us/visualstudio/msbuild/msbuild-security-best-practices)

For reusable test inputs, fingerprint the evaluated target, toolchain, runner and adapter, settings, ordered compilation inputs, build outputs, runtime assets and trusted path mapping. Record collection or snapshot mode. Compare owned file bytes and digests before accepting a receipt. Unknown required inputs yield a non-reusable result, with a reason. When the consumer independently identifies a runtime file, check that the declared closure includes it. An omission invalidates reuse and the observation grants no read authority. Detect build graph changes that can alter outputs without changing C# content. Use distinct output directories per workspace and incompatible build context where the project permits it.

## Input and protocol bounds

| Boundary | Initial policy |
| --- | --- |
| Local daemon IPC | Limit request bytes before deserialization. Use a bounded set of active requests, per-request read and operation deadlines, validated fields and typed errors. Status remains available under slow clients. Negotiate optional features at handshake. |
| Repository-controlled documents | Read with byte and structural limits, from verified paths, and with cancellation. XML parsing prohibits DTD and external entities. Reject ambiguous identity and unsupported schema with a local reason. |
| Runner output | Read bounded streams and require a fresh artifact under the attempt's owned directory. Validate the runner, artifact digest, counters and completion evidence before resolving a cause. A process exit alone is insufficient evidence. |
| Logs, telemetry and imported receipts | Retain attributed summaries under per-workspace quotas. Redact credential-like values and raw environment data. Keep import and network sources behind independent configuration. |
| Graph scans and queries | Apply bounded depth, nodes, time and decoded bytes. Cursors pin a context and revision and expire after a bounded lifetime. Show the actual query frontier and omitted scope. |

Separate the stdio MCP session handshake from the private local daemon wire handshake. An older public client can use a supported common protocol or receive a clear restart/upgrade error. Unsupported capabilities stay unavailable at the bridge. Keep stdout protocol-only. [MCP lifecycle](https://modelcontextprotocol.io/specification/2025-11-25/basic/lifecycle)

## Verification semantics

Report run health, scope completeness, test outcome and observation strength as independent fields. Failed tests can provide valid measured coverage. A passing test does not establish coverage without a matching measurement. Aggregate line hits carry run-level attribution. A branch requires a stable identity and an exact mapping to the recorded counter. Report unknown for ambiguous mappings, disabled instrumentation and incomplete graphs.

Define evidence rules as versioned predicates over normalized facts and explicitly searched scope. Return the triggering change, the detector, applied policy, consulted sources, uncertainty frontier and a scoped next action when evidence is missing. Baseline comparisons require equivalent coverage and identity schemes. A source finding can remain applicable across line shifts if its stable semantic anchor and owning context still match. Treat production claims as scoped import facts whose originating time and applicability are recorded.

## Failure-oriented acceptance

Exercise each material boundary on both sides of a fence: before and after worker admission, receipt commit, report staging and publication. Include denial, truncation, timeouts, malformed artifacts, stale readers and deliberate restart with durable pending work. Use deterministic fixtures for interleavings and actual platform fixtures for watcher, storage and process semantics. Base portability claims on tests in the advertised environment. Compare incremental output to cold rebuild where ordering and identity matter. Keep outcome summaries with the change discussion and keep generated artifacts out of the repository.
