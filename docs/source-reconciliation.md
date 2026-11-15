# Source reconciliation and automatic test intents

The first [0.2 implementation slice](version-0.2.md) turns watcher notifications into durable source changes. A source scanner supplies observations, a coordinator commits them and records outstanding test work, and the report exporter projects the current snapshot. Project evaluation and test execution consume the committed work independently. Inventory status can publish while those consumers are pending.

## Current implementation

The development daemon records source inventory and test intents in `SlopGuard/state/<workspace-id>/inventory.sqlite3` under the user's local application data directory. The database stores verified fingerprints, last-known fingerprints for uncertain paths, and the enumeration frontier. Source facts and pending work share one transaction. The single writer uses a rollback journal and a 64 MiB database page cap. The journal fits inside the checkout's larger database and journal allowance. [SQLite rollback journaling](https://sqlite.org/lockingv3.html)

The `revision` field is the daemon's invalidation sequence. `committedRevision` identifies the durable observation and advances across restarts. `pendingTestIntents` gives the detailed count, trigger revision range and any workspace fallback marker. At most 2,048 detailed intents are retained. Additional causes extend one fallback marker. A replacement root retires the previous pending records and publishes the retirement revision and reason. Inventory write failures appear as `recordingError` while the durable revision stays at the prior commit.

The schema-2 migration adds attempt and per-target evidence records and preserves source facts and pending intents. A scheduled attempt pins the selected target, input fingerprint and exact cause identifiers. The daemon records the attempt before launch and commits current evidence and resolution together. A receipt from a superseded attempt stays in bounded history. Transient failures retry after a recorded deadline, and startup marks unfinished attempts as interrupted. The same target, fingerprint and cause set gets up to three attempts. A current passing or failing test result satisfies its selected target. Partial selection keeps intents pending, even when its known targets completed.

Schema 3 retains up to eight evaluated graphs, with priority for the latest graph and predecessors referenced by pending intents. A source transaction pins the complete graph for its prior committed revision when available. Selection reverse-walks both graphs, filters to explicitly eligible test projects, and versions each target's scope from the evaluated graph and its source digests. Unknown predecessors widen selection to the approved group. An incomplete current graph keeps intents outstanding.

The preview CLI exposes the source inventory report, scheduler status and explicit project evaluation. The integration suite supplies the real evaluator and fake runner through the normal daemon entry point. Production shows `awaiting_adapter` until supervised execution is available.

## Confirm a change

| Identity | Purpose |
| --- | --- |
| Daemon incarnation and publication epoch | Fence workers and report writes across daemon restarts and Git transitions. Switches between branches at the same commit still replace the epoch. |
| Invalidation sequence | Advance when a relevant event arrives. Withdraw current results and discard scans crossed by an event. This counter lives in the daemon. |
| Committed workspace revision | Identify a reconciled snapshot in the store. Test intents carry the revision that confirmed their source cause. |

Subscribe before inventory. The first stable observation in an incarnation establishes a baseline. On restart, validate cached facts against that observation and recover committed intents for the same worktree. Differences discovered while establishing the baseline create zero new automatic intents. Changes after the baseline use normal reconciliation, including events queued during startup that still differ from the committed baseline. If the root was replaced, establish a new baseline and retire the previous root's evidence and pending work with a recorded reason.

The scanner reports a proven presence with a byte digest or a proven absence under an enumerated scope. Unreadable files, incomplete directory enumeration and paths omitted by limits leave the affected facts uncertain. Retain their last known state and exclude them from current coverage until verified. An incomplete scan cannot turn an omitted entry into a deletion. If no prior state was established, the first verified observation of that path or scope establishes one. Continue reconciling independently verifiable paths and report the remaining scope as pending or partial.

Compare path presence, canonical relative spelling and byte digest against the last committed observation. Use the workspace path comparer for lookup and preserve observed spelling so a case-only rename on Windows counts. A move may be represented by two endpoints. Rename notifications and matching digests can help explain the pair. Impact selection uses both endpoints even when the scanner cannot pair them. Equal bytes at the same path collapse repeated saves, including replace-by-rename. A create followed by a delete between observations has no net delta. A known path changing from readable to unreadable leaves an unresolved observation until a read or enumeration confirms its state.

Carry the inventory policy identity with the frontier. A newly included path establishes presence under the new policy. A newly excluded path retires its active facts. Neither creates a source cause by itself. Check the previously committed enumeration frontier to distinguish a new path from one that had never been observed. A previous complete enumeration that covered the path can prove absence. Record a prior unknown for unverified subtrees and paths. Compare within the unchanged policy scope even when another part of the inventory policy changes.

Keep physical C# changes and evaluated membership changes as separate inputs. A `Compile` item or reference change on its own refreshes the project graph and can replan existing work. It creates zero automatic test intents. A source path or digest change creates a source cause even if its owning project is still unknown. A same-commit checkout or explicit recheck follows the same comparison. Rechecking unchanged inputs creates zero automatic intents. A C# delta confirmed after a rescan for watcher overflow can create one.

## Commit the observation and the work together

1. Capture the incarnation, epoch, event sequence and prior committed revision. Reconcile outside the writer transaction. Re-read Git context and reject a candidate crossed by an invalidation before handing it to the single writer.
2. At the writer, check that the expected base still matches. Sample the live fence again before accepting the transaction. Commit the new source facts, coverage frontier, workspace revision and any source intents together. Record old path and digest and any validated old membership needed for deletes or moves. Store an explicit unknown when the predecessor graph is unavailable.
3. An event arriving after writer acceptance invalidates publication immediately. A transaction that commits across that event still records the observation and its intent. Reconcile the next observation against that committed revision and hold job admission and report publication until the current fence is reconciled. Keep watcher callbacks independent of the database writer. Check the fence again at final report replacement.

Store the confirmed baseline and pending work in the same database. The first slice needs four record families.

| Record | Minimum committed contents |
| --- | --- |
| Workspace | Worktree and root identity, inventory policy revision, committed workspace revision and last reconciled Git context. |
| Source observation | Lookup path key, canonical relative spelling, last known presence and digest, and revision last verified. Keep uncertainty and the covered enumeration scopes alongside these records. |
| Intent | Trigger revision, old and new endpoints where known, predecessor graph reference or missing-graph reason, and current per-target dispositions. |
| Attempt | Attempt ID, reconciled fence, profile and input fingerprints, target and runner, exact intent versions, supervision and lease identity, and eventual receipt or interruption state. |

Keep a compact source intent keyed to its trigger revision and endpoints. A scheduler can coalesce several causes into one attempt. Until selection completes, preserve the affected paths and predecessor context references. After selection, record unresolved work by test project and evaluated target. Keep manual-only, disabled, excluded and authoritatively empty selections as explicit dispositions. Incomplete evaluation keeps a pending scope or uses a configured conservative unit-test group with a recorded reason. A metadata change can alter the plan and the build inputs, so re-evaluate an outstanding cause against the latest graph while retaining its predecessor membership.

Include unresolved intents in the index quota. Reserve space for a bounded fallback marker for confirmed changes that exceed the detailed pending budget. The marker carries the affected scope and covered trigger revisions. The scheduler can resolve it by evaluating the configured unit-test group, or expose a blocker when policy, graph availability or budget prevents doing so. Unknown predecessor facts widen the reported scope. Extend an existing fallback marker transactionally and advance its version. An attempt can clear only the marker version it planned against after all eligible targets in that scope have a terminal disposition. When the store cannot commit even that marker, keep the prior committed frontier and surface degraded recording before accepting any more revisions. Keep read transactions short so checkpoints can reclaim WAL space. [SQLite transactions and WAL](https://www.sqlite.org/wal.html)

## Admit attempts and record outcomes

Only the scheduler admits a test attempt. It waits for a current reconciliation, checks the effective execution profile, obtains the required worker and output leases, and snapshots the selected target, runner, input fingerprint and exact intent versions covered by the attempt. Recheck the fence and profile before launch. Record the attempt before starting the runner. Identical current plans may share one attempt. A newer plan leaves its intents outstanding when an older attempt ends.

A completion transaction stores the receipt and resolves only the matching intent versions for that target. At writer acceptance, check the latest reconciled fence, the post-run input verification and the currently selected intent versions. A result that fails these checks can be recorded as history. Both passing and failing test outcomes can satisfy the requirement to run the target. Zero discovery, build failure, adapter failure, cancellation and timeout leave a visible incomplete or blocked disposition. Eligibility changes and explicit retries can reconsider those dispositions. An authoritative graph can also show that a formerly eligible test target was removed. Record that disposition separately from a test outcome.

A relevant input, policy or Git epoch change during a run supersedes its current-result claim. Keep a completed obsolete receipt as history and leave the affected intent outstanding. After reconciliation, plan against the latest inputs and retain the original cause in the selection explanation. A metadata-only edit or unchanged-source checkout can therefore resume outstanding work while creating zero new source triggers. A change to unrelated inputs may allow a result to be projected into the latest workspace revision after the scheduler verifies its scoped inputs and policy in the same epoch. Uncertain scope requires conservative invalidation.

If the daemon exits after launching a runner and before recording its completion, recover an interrupted attempt. Wait until its process tree and output lease are clear before retrying. Revalidate authorization and current inputs. A crash in that window can cause a repeated execution. Persisting receipt and resolution together prevents a restart after that commit from repeating a completed intent. Record a bounded retry reason and make admission wait for a relevant change, an explicit retry or a scheduled retry with backoff. A persistent runner or policy error should remain visible without creating a launch loop. If the cache must be rebuilt and committed intents cannot be recovered, expose the lost verification state and establish a new baseline. Preserve pending work across compatible schema upgrades independently of disposable syntax and semantic facts.

## First implementation and acceptance

The reconciliation coordinator, evaluated selector and durable scheduler are implemented and exercised with an in-process fake runner. Source file matching and fingerprinting are independent of parsing. The [next implementation stages](delivery.md#next-implementation-stages) add supervised execution before enabling process-backed tests.

| Fixture | Expected result |
| --- | --- |
| Cold start, offline edit before restart, duplicate notifications and unchanged atomic save | Zero new automatic intents. A previously committed unresolved intent survives restart. |
| Create, edit, delete, directory move or Windows case-only rename | A new source cause after reconciliation, with both endpoints available for moves and old membership available for deletes when indexed. |
| Locked source, incomplete enumeration or inventory cap | Affected coverage is incomplete. Omitted entries never become deletions. A later successful observation uses the retained baseline. |
| Edit followed by a props change before test admission | One source cause, evaluated against the current graph with predecessor membership retained. A props-only conditional membership change produces zero new source causes. |
| Invalidation during source scan or after writer acceptance | The first candidate is discarded. The accepted transaction can commit its cause, but publication and test admission wait for reconciliation of the newer event. |
| Edit while a previous attempt is running, then old attempt completes | The earlier receipt can be inspected as history. The latest attempt covers the outstanding source causes and only its own target. |
| Same-commit branch switch, then a different-commit switch with a source delta | Both transitions invalidate old publication authority. Only the confirmed C# delta creates a new source cause. |
| Crash before source transaction commits, after commit, or after receipt commit | Recovery sees neither half of an uncommitted observation, both source facts and intent after observing a commit, and no retry of a durably resolved intent. |
| Runner exits before its outcome is committed and the daemon restarts | The attempt is interrupted, with the intent still outstanding. Retry requires current inputs, authorization and cleared process and output ownership. |
| Quota pressure or database write failure | A confirmed cause is represented by detail or by the fallback marker. If neither can commit, the durable frontier stays put and status shows degraded recording. |
