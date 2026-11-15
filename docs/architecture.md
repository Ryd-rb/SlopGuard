# Daemon architecture

## Processes and storage

`slopguard start --root <checkout>` resolves the checkout and starts or attaches to a background daemon for that canonical root and OS user. A workspace lock protects startup. A per-user registry records the process identity, protocol version and endpoint. Validate the handshake before reusing an endpoint. Two Git worktrees have distinct roots, findings and queues. Two sessions in the same worktree share a daemon, with separate MCP sessions.

The daemon owns the watcher, scheduler, report store, and local IPC endpoint. A short-lived `slopguard mcp --root <checkout>` process bridges MCP over stdio to the daemon. Use user-restricted named pipes on Windows and Unix domain sockets in a user-owned directory elsewhere. Keep process diagnostics off MCP stdout. Start with explicit shutdown and an idle timeout after the last session and job. On shutdown, expire the registry entry, stop accepting work and drain or cancel children within a deadline. [MCP transport specification](https://modelcontextprotocol.io/specification/2025-11-25/basic/transports)

Use a per-user state directory outside the checkout for the [shared SQLite index](semantic-index.md), job journal and diagnostic artifacts. Tie cache keys to the canonical root, checker and rule versions, relevant configuration, build context and input hashes. Persist hashes, compact graph edges, fingerprints, spans and outcomes. Read source on demand. Version derived facts separately from durable pending work. Upgrade pending work through a compatible migration or surface an explicit recovery blocker. On recovery mark interrupted jobs as such and reconcile against disk before reporting results as current. Publish the bounded current-results view in [`.slopguard/results.json`](result-publication.md).

## Watcher and reconciliation

1. Attach a recursive watcher for file and directory names, last writes and size. Watch the root’s parent for root deletion or replacement. Subscribe before initial inventory and reconcile events received during that scan. Startup establishes an inventory and may run cheap checks. It creates zero new automatic test intents. Recover pending intents already committed for this worktree. [Source reconciliation and automatic test intents](source-reconciliation.md)
2. Callback threads offer paths and rename pairs to a bounded queue. Apply default exclusions for `.git`, `.slopguard`, build outputs, tool output and cache directories. Watch Git metadata separately to invalidate report publication. Honor repository and SlopGuard ignore rules with explicit includes for legitimately tracked inputs in excluded directories. Resolve paths under the canonical root and skip link traversal by default. Directory events are eligible even without a C# extension.
3. Coalesce events with a proposed 300 ms quiet window and a 2 s maximum batch age. Handle atomic saves by comparing stable end state with the previous inventory. If a file is locked mid-save, retry within a deadline and mark the affected scope pending until readable. Directory insertion, removal and rename reconcile the affected subtree using both old and current inventory. Convert results into additions, content changes, deletions and renames. Ignore timestamp-only churn with identical path and contents. A C# path move still matters because it can change project membership or path-sensitive compilation.
4. On a watcher overflow or a full event queue, mark the scope dirty and reconcile. Rehash eligible sources if metadata alone cannot establish whether they changed. A rescan can trigger tests if it proves a C# delta against the prior inventory. Lost or unreadable inputs are reported as uncertain until reconciled. Use a bounded, backing-off poller only when the platform watcher cannot provide reliable coverage. If the root disappears, mark it unavailable, stop affected jobs and reattach when it returns. Reconcile against the prior inventory when it is trustworthy. If the root identity changed, invalidate the old results and project graph before reconciling.
5. Classify stable deltas by input kind. C# paths invalidate source checks and may trigger automatic tests. Project files, props, targets, editor configuration, analyzer settings, package inputs and relevant additions or deletions invalidate dependent analysis or discovery. Those inputs alone schedule zero automatic tests. Use prior and current project membership for deleted, renamed and linked files. Explicit linked inputs outside the root require owner-approved paths before watching or reading them.

Filesystem notifications are hints. Windows can report several events for a single operation, and a moved-in folder may produce a directory event without events for its children. [FileSystemWatcher behavior](https://learn.microsoft.com/en-us/dotnet/api/system.io.filesystemwatcher?view=net-10.0)

## Revisions and outcomes

Assign each reconciled batch a monotonic workspace revision. Checkers capture their input digests and effective policy. For a source-local check, read against its captured hashes. For a tool that reads live disk, record relevant hashes before and after execution, reject publication as current if the watcher reports a relevant change during execution, and revalidate the inputs before publication. Treat unknown tool inputs conservatively. This detects common races. It provides weaker evidence than an immutable snapshot since a transient edit can be missed. Use a materialized snapshot for explicit verification where the project can build outside the live checkout.

A result includes checker and tool versions, rule identifier, severity, primary and related spans, project/target context, and a stable fingerprint. Store runs separately from findings. A run reports `queued`, `running`, `completed`, `superseded`, `cancelled`, `timed_out`, `failed_to_run`, or `skipped`, with a reason and coverage. Inspection findings and failing tests are outcomes of a completed run. A successful empty report replaces the previous report for its validated scope. Partial and failed runs retain last-known results marked stale. Deletions clear current reports for removed files and invalidate peers and projects that depended on them.

Agents may capture a baseline before editing. Classify findings as introduced, resolved, pre-existing or uncertain within equivalent tool coverage. Use a fingerprint based on rule, symbol or normalized local anchor, and project context so that line shifts preserve identity. Keep tool-native rule IDs, spans and provenance when grouping duplicate reports. A cached outcome is current only when all required inputs match. Missing coverage is explicit.

## Scheduling and resource use

| Work | Default trigger and scope |
| --- | --- |
| Source checks | Stable C# delta, limited to changed files and indexed peers. |
| Semantic checks | Short idle period after a batch. Reuse compiler diagnostics from a build already needed for impacted tests. Analyze affected project contexts and reverse dependencies when the rule requires them. |
| Related tests | Stable C# delta with discovered or conservatively inferred affected test projects. Debounce more aggressively than source checks. |
| Verification needs | Relevant source or evidence changes. Query the shared semantic index for applicable checks and coverage gaps. |
| Solution inspection | Explicit request or an owner-enabled idle policy. Respect provider-specific minimum intervals. |
| Dependency metadata | Local lock or assets changes. Refresh remote metadata only under the configured network policy. |

Give the daemon one event consumer, bounded queues and caches, and initially one heavy worker slot per machine user across worktrees. Coalesce pending jobs by checker and scope while preserving unverified changes from previous batches. Run requested checks ahead of queued idle inspections, apply fairness across worktrees, and reserve capacity for quick feedback. Cancel obsolete queued work and cancel cancellable running work at safe boundaries. Serialize writers sharing an MSBuild output directory, a test resource, or a provider cache. Watcher threads do no parsing or build work.

Use out-of-process workers for MSBuild, test hosts, and heavyweight external inspections. Pass typed arguments without a shell and supervise worker process trees with time, memory and output limits where supported. Prefer process-local build servers or disable reuse for workers whose children must exit within their budget. Recycle expensive workers at idle or under memory pressure. Use bounded syntax and metadata caches, with LRU eviction and on-disk spill for the clone index. Cap candidate fan-out and report truncation. Avoid an idle polling or parsing loop on supported local filesystems. Measure process-tree resource use and report timings, queue depth, overflow and fallback counts, cache hit rate and peak worker memory.

## Toolchain resolution

Use `global.json`, project format, installed SDKs, evaluated project properties, build configuration and actual test adapters to choose a worker. Evaluate each target framework separately where references, defines or rules differ. Include configuration, platform and runtime identifier when they affect inputs. Never derive language version solely from a TFM. Project discovery is cheap and provisional until evaluation resolves SDK imports, conditions, linked files, generators and project references.

| Project type | Planned worker and missing prerequisites |
| --- | --- |
| SDK-style .NET Core and .NET 5–10 | Use the SDK selected from the project context. Report exact SDK, runtime, workloads and test adapter requirements. Record unbuilt target frameworks separately. |
| SDK-style .NET Framework | Use the required .NET Framework reference assemblies for compilation. Route tests to a compatible Windows runner. |
| Classic .NET Framework 4.6–4.8.1 | Resolve compatible Windows MSBuild/Build Tools, reference assemblies, imports and the repository’s restore and test adapters. Keep 4.6 and 4.6.1 explicit in the compatibility matrix because toolset availability varies and these targets are retired. |
| .NET Standard and multi-target libraries | Compile each selected target. Run tests in compatible executable test projects and report their actual runtime. |
| Unsupported toolset or incomplete restore | Run eligible file-local checks and mark build, semantic, or test coverage blocked with the specific prerequisite. |

Respect directory SDK selection and evaluated build inputs. Keep the daemon on .NET 10 so supporting an old project does not require installing an old daemon runtime. [Microsoft reference assembly guidance](https://learn.microsoft.com/en-us/dotnet/framework/migration-guide/reference-assemblies), [Microsoft .NET Framework lifecycle](https://learn.microsoft.com/en-us/lifecycle/products/microsoft-net-framework), [MSBuild command-line reference](https://learn.microsoft.com/en-us/visualstudio/msbuild/msbuild-command-line-reference)

## Worker admission and recovery

The scheduler owns heavy-work admission. Snapshot the selected operation, authority revision, evaluated target, input fingerprint and required output roots. Coalesce equivalent work by those identities. Track automatic demand independently from each MCP session's demand, so closing a client cannot cancel outstanding automatic verification. Choose the oldest eligible worktree in each scheduling round. Within it, prioritize the current explicit request, then pending verification and idle analysis. Age waiting work so a stream of explicit requests cannot indefinitely defer verification.

Acquire the user worker slot and the full set of output leases in canonical path order. Include the output and intermediate roots of invoked project references. Overlapping roots conflict, including aliases. A lease coordinates SlopGuard workers. A normal developer build only participates if it uses the same lease, so prefer private worker outputs and expose shared-output mode as an explicit choice. Revalidate inputs and authority after waiting. Commit an attempt with the lease and supervision identity before admitting any repository code.

On Windows, prototype a Job Object that enrolls the worker before it can spawn children and terminates its process tree when its owning handle closes. On Linux, require an available supervisor with equivalent cleanup evidence for automatic heavy work. Validate the platform against nested child and daemon-exit fixtures. A root process exit alone cannot clear a lease. If recovery cannot establish that the old process scope is empty, leave the outputs reserved and surface the blocker. [Windows Job Objects](https://learn.microsoft.com/en-us/windows/win32/procthread/job-objects), [.NET process termination](https://learn.microsoft.com/en-us/dotnet/api/system.diagnostics.process.kill)

## Repository execution

Authorize execution through a per-user profile keyed to the canonical checkout and worktree identity. A repository can suggest checker settings and unit-test scopes. Only the user profile can grant an operation, an external path, a toolchain or a shared output location. Expose the effective capability and its source without returning private paths or environment values. Replacing the checkout root requires revalidating the profile binding. Revocation fences new launches and current-result claims and requests cancellation of affected running jobs.

| Operation | Authority and prerequisites |
| --- | --- |
| File inventory, parsing and local fact queries | Available by default within verified read roots and configured limits. |
| Project evaluation | Explicit grant, including design-time builds and MSBuild imports. Inspecting project filenames and configuration hints can proceed before the grant. |
| Build and compiler extensions | Explicit grants for build and for repository-provided analyzers and generators. A pipeline that cannot disable a denied extension reports a blocker. |
| Discovery and tests | Explicit grant before loading test modules or adapters. Automatic admission also requires an eligible unit-test scope under the effective profile. |
| Restore, downloads and remote providers | Separate restore and network grants. Validate the configured toolchain and dependencies before launch. Tool installation stays a user operation. |

Resolve executables from approved installations. Pass typed argument lists to the worker. Use a controlled environment and record behavior-affecting inputs, including toolchain overrides, by identity or hash. Deny external reads through SlopGuard unless the path is authorized. Scope inherited credentials to explicitly configured operations. With network disabled, SlopGuard avoids network-dependent commands and built-in provider calls. Enforced network denial and isolation for untrusted code require an independently configured OS boundary. Worker process supervision manages resources and lifecycle. [Secure MSBuild usage](https://learn.microsoft.com/en-us/visualstudio/msbuild/msbuild-security-best-practices)

Keep diagnostic artifacts and telemetry under the per-user state directory. Use isolated output paths where the toolchain supports them, and surface projects that need their normal build paths. Avoid overwriting a developer’s build in flight. Filter build-generated events out of the default watcher inputs. Keep raw logs bounded and avoid retaining credentials, environment dumps or source bodies in reports. Network-backed analysis and feeds require independent owner configuration. [SDK artifacts output layout](https://learn.microsoft.com/en-us/dotnet/core/sdk/artifacts-output)
