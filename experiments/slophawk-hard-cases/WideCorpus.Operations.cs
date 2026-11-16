internal static partial class WideCorpus
{
    private static WideModule[] OperationsModules() =>
    [
        new("Turnstile", "Turnstile plans maintenance work and tracks the decisions that survive a pause, retry, or handoff.",
        [
            Fault("NextMaintenanceSlot", "long anchor, long interval, long now, long quietFrom, long quietUntil",
                "Return the earliest start at or after now on the grid anchor + k × interval for k ≥ 0. No start may fall in [quietFrom, quietUntil). Scheduling resumes on the original grid once that interval ends. Minutes are 0–10,000, interval is positive, and quietFrom ≤ quietUntil.",
                """
                var elapsed = Math.Max(0, now - anchor);
                var slot = anchor + ((elapsed + interval - 1) / interval * interval);
                if (quietFrom <= slot && slot < quietUntil)
                    slot = Math.Max(slot + ((quietUntil - slot) / interval * interval), quietUntil);
                return slot;
                """,
                """
                var elapsed = Math.Max(0, now - anchor);
                var slot = anchor + ((elapsed + interval - 1) / interval * interval);
                if (quietFrom <= slot && slot < quietUntil)
                    slot += (quietUntil - slot + interval - 1) / interval * interval;
                return slot;
                """,
                [10, 6, 16, 30, 40], 16, [10, 6, 17, 20, 29], 34),
            Fault("ReplayWorkReceipt", "long stage, long mode, long epoch, long receiptStage, long receiptEpoch, long outcome",
                "Return 2 to reuse a successful receipt, 1 to run, or 0 to stay stopped. Mode is 0 stopped, 1 running, or 2 draining. In running and draining modes, reuse a receipt with outcome 1 made in the same epoch for the same stage. Stage 4 is the legacy identifier for stage 2, on either side. A draining job with no reusable receipt stays stopped. Stages are 1–4, epochs are 0–10,000, and outcome is a predicate.",
                """
                if (mode == 0)
                    return 0;
                var requestedStage = mode == 2 ? stage : stage == 4 ? 2 : stage;
                var recordedStage = receiptStage == 4 ? 2 : receiptStage;
                if (outcome == 1 && epoch == receiptEpoch && requestedStage == recordedStage)
                    return 2;
                return mode == 1 ? 1 : 0;
                """,
                """
                if (mode == 0)
                    return 0;
                var requestedStage = stage == 4 ? 2 : stage;
                var recordedStage = receiptStage == 4 ? 2 : receiptStage;
                if (outcome == 1 && epoch == receiptEpoch && requestedStage == recordedStage)
                    return 2;
                return mode == 1 ? 1 : 0;
                """,
                [4, 1, 6, 2, 6, 1], 2, [4, 2, 6, 2, 6, 1], 2),
            Fault("ParkingDeadline", "long openedAt, long deadline, long pausedAt, long releasedAt, long cap",
                "Extend the deadline by up to cap minutes of a completed pause. Credit begins when both the task and the pause have started, and is earned only if that overlap begins before the original deadline. A qualifying pause keeps earning credit through release, including time beyond the original deadline. Inputs are 0–10,000, openedAt ≤ deadline, and pausedAt ≤ releasedAt.",
                """
                var overlapStart = Math.Max(openedAt, pausedAt);
                if (overlapStart >= deadline || releasedAt <= overlapStart)
                    return deadline;
                var duration = releasedAt - pausedAt;
                return deadline + Math.Min(cap, duration);
                """,
                """
                var overlapStart = Math.Max(openedAt, pausedAt);
                if (overlapStart >= deadline || releasedAt <= overlapStart)
                    return deadline;
                var duration = releasedAt - overlapStart;
                return deadline + Math.Min(cap, duration);
                """,
                [10, 50, 20, 24, 15], 54, [20, 50, 10, 28, 15], 58),
            Fault("NextRunnableStep", "long required, long finished, long waived, long held",
                "Each mask uses the low four bits. Return the bit of the earliest required step that is unresolved, with stages ordered from low to high bits. Completed and waived steps are resolved. An unresolved held step blocks itself and all later stages, so return 0 if the first unresolved step is held or there are no unresolved steps.",
                """
                var unresolved = required & ~(finished | waived);
                var runnable = unresolved & ~held;
                return runnable & -runnable;
                """,
                """
                var unresolved = required & ~(finished | waived);
                var first = unresolved & -unresolved;
                return first & ~held;
                """,
                [15, 3, 0, 8], 4, [15, 0, 3, 4], 0),
            Fault("PickDispatchLane", "long firstDue, long firstSetup, long firstCold, long firstEpoch, long firstOrder, long secondDue, long secondSetup, long secondCold, long secondEpoch, long secondOrder",
                "Select lane 1 or 2 by the earlier latest-start time, which is the due minute less setup minutes for a cold lane and just the due minute for a warm lane. Break a tie by earlier arrival epoch, then lower arrival order within that epoch, then lane 1. Arrival order resets each epoch. Cold flags are predicates and other inputs are 0–10,000.",
                """
                var firstStart = firstDue - (firstCold == 1 ? firstSetup : 0);
                var secondStart = secondDue - (secondCold == 1 ? secondSetup : 0);
                if (firstStart != secondStart)
                    return firstStart < secondStart ? 1 : 2;
                return (firstOrder, firstEpoch).CompareTo((secondOrder, secondEpoch)) <= 0 ? 1 : 2;
                """,
                """
                var firstStart = firstDue - (firstCold == 1 ? firstSetup : 0);
                var secondStart = secondDue - (secondCold == 1 ? secondSetup : 0);
                if (firstStart != secondStart)
                    return firstStart < secondStart ? 1 : 2;
                return (firstEpoch, firstOrder).CompareTo((secondEpoch, secondOrder)) <= 0 ? 1 : 2;
                """,
                [34, 6, 1, 4, 3, 37, 6, 0, 3, 7], 1, [44, 6, 1, 2, 1, 38, 4, 0, 1, 9], 2),
            Fault("ResolveRunbookRevision", "long baseline, long previous, long staged, long track, long previousTrack, long stagedTrack, long withdrawn",
                "Return the greatest eligible runbook version. The baseline is always eligible. A staged or previous version is eligible only on its assigned rollout track and only if it has not been withdrawn. Versions are positive and strictly ordered baseline < previous < staged ≤ 10,000. The withdrawn version is 0 or one of the two rollout versions. Track identifiers are 1–10.",
                """
                var available = baseline;
                if (previousTrack == track)
                    available = previous;
                if (stagedTrack == track)
                    available = staged;
                return available == withdrawn ? baseline : available;
                """,
                """
                var available = baseline;
                if (previousTrack == track && previous != withdrawn)
                    available = previous;
                if (stagedTrack == track && staged != withdrawn)
                    available = staged;
                return available;
                """,
                [10, 12, 15, 2, 1, 2, 0], 15, [10, 14, 17, 2, 2, 2, 17], 14),
            Fault("LeaseRenewalDelay", "long now, long expiresAt, long lead, long retryAfter, long retryActive",
                "Return the number of minutes until renewal. The clock wraps after minute 63. Each clock timestamp is 0–63 and represents an instant within 31 minutes either side of now. Renewal is due lead minutes before expiration, but an active retry hold can delay it, at most until expiration. A past deadline or hold has no remaining minutes. Lead is 0–31 and retryActive is a predicate.",
                """
                static long Remaining(long from, long to)
                {
                    var wrapped = (to - from + 64) % 64;
                    return wrapped < 32 ? wrapped : 0;
                }
                var remaining = Remaining(now, expiresAt);
                var target = Math.Max(0, remaining - lead);
                var hold = retryActive == 1 ? Math.Max(0, retryAfter - now) : 0;
                return Math.Min(remaining, Math.Max(target, hold));
                """,
                """
                static long Remaining(long from, long to)
                {
                    var wrapped = (to - from + 64) % 64;
                    return wrapped < 32 ? wrapped : 0;
                }
                var remaining = Remaining(now, expiresAt);
                var target = Math.Max(0, remaining - lead);
                var hold = retryActive == 1 ? Remaining(now, retryAfter) : 0;
                return Math.Min(remaining, Math.Max(target, hold));
                """,
                [10, 20, 4, 15, 1], 6, [61, 9, 7, 5, 1], 8),
            Fault("CancellationTargets", "long queued, long running, long checkpointed, long detached, long emergencyCapable, long force",
                "Return the mask of child jobs that can be cancelled. Queued jobs can always be cancelled unless detached. A running job can be cancelled at a checkpoint or by a forced stop if the job supports emergency stopping. Detached jobs are excluded in every mode. Masks contain at most 16 low bits, queued and running are disjoint, and force is a predicate.",
                """
                var stoppable = checkpointed & running;
                if (force == 1 && (emergencyCapable & running) != 0)
                    stoppable |= running;
                return (queued | stoppable) & ~detached;
                """,
                """
                var stoppable = checkpointed & running;
                if (force == 1)
                    stoppable |= emergencyCapable & running;
                return (queued | stoppable) & ~detached;
                """,
                [1, 6, 2, 4, 4, 0], 3, [1, 14, 0, 8, 4, 1], 5),
            Fault("ReservedCrewMinutes", "long available, long setup, long inspection, long shared, long sameCrew, long overlapApproved",
                "Return reserved minutes, limited to available minutes. Setup and inspection request their full minutes. Shared minutes represent the same work counted in both requests and are deducted once if the crew is the same and overlap was approved. Counts are 0–10,000, shared is at most either request, and the two flags are predicates.",
                """
                var overlap = sameCrew == 1 && overlapApproved == 1 ? shared : 0;
                var requested = Math.Min(available, setup + inspection);
                return Math.Max(0, requested - overlap);
                """,
                """
                var overlap = sameCrew == 1 && overlapApproved == 1 ? shared : 0;
                return Math.Min(available, setup + inspection - overlap);
                """,
                [20, 6, 5, 3, 1, 1], 8, [10, 8, 7, 4, 1, 1], 10),
            Fault("EscalationLevel", "long elapsed, long nudgeAfter, long urgentAfter, long existing, long muted, long cleared",
                "Return escalation level 0 none, 1 nudge, or 2 urgent. A cleared incident resets to 0. An open incident keeps its existing level as a floor. A threshold is reached at the given minute, and muting suppresses new nudges only. Times are 0–10,000 with nudgeAfter < urgentAfter, existing is a level, and muted and cleared are predicates.",
                """
                if (cleared == 1)
                    return 0;
                var due = elapsed >= urgentAfter ? 2 : elapsed >= nudgeAfter ? 1 : 0;
                return muted == 1 ? existing : Math.Max(existing, due);
                """,
                """
                if (cleared == 1)
                    return 0;
                var due = elapsed >= urgentAfter ? 2 : elapsed >= nudgeAfter && muted == 0 ? 1 : 0;
                return Math.Max(existing, due);
                """,
                [30, 20, 40, 0, 0, 0], 1, [52, 20, 40, 0, 1, 0], 2),
            Valid("ClosingShiftMinutes", "long shiftStart, long shiftEnd, long finishedAt, long supervisorCleared",
                "Return the shift minutes to record from shiftStart until finishedAt, at most until shiftEnd unless a supervisor cleared overtime. Minute inputs are 0–10,000 and shiftStart ≤ shiftEnd. A finish at or before shiftStart records zero. supervisorCleared is a predicate.",
                """
                var end = supervisorCleared == 1 ? finishedAt : Math.Min(shiftEnd, finishedAt);
                return Math.Max(0, end - shiftStart);
                """,
                [12, 30, 37, 1], 25),
            Valid("PendingDryRun", "long requested, long finished, long obsolete, long dryRun",
                "Return the mask of still-pending requested steps, excluding finished and obsolete steps. In dry-run mode return 0 because the call does not submit any pending steps. Masks contain at most 16 low bits. dryRun is a predicate.",
                """
                var pending = requested & ~(finished | obsolete);
                return dryRun == 0 ? pending : 0;
                """,
                [31, 3, 8, 0], 20),
        ]),
        new("Quorum", "Quorum tracks copy progress, pinned history, and subscriber cursors across storage replicas.",
        [
            Fault("ResolveReplicaVersion", "long primary, long follower, long durable, long tombstone, long readFence",
                "Return the newest available live version at or above readFence and at or below the durable watermark, or 0 if none. A durable tombstone hides all versions up to and including its version. A version or tombstone of 0 means absent. Inputs are 0–10,000.",
                """
                var visible = Math.Max(primary <= durable ? primary : 0, follower <= durable ? follower : 0);
                if (visible < readFence || visible <= tombstone)
                    return 0;
                return visible;
                """,
                """
                var visible = Math.Max(primary <= durable ? primary : 0, follower <= durable ? follower : 0);
                var committedDeletion = tombstone <= durable ? tombstone : 0;
                if (visible < readFence || visible <= committedDeletion)
                    return 0;
                return visible;
                """,
                [9, 11, 11, 8, 7], 11, [12, 10, 10, 11, 7], 10),
            Fault("ReplicaWriteDomains", "long firstDomain, long secondDomain, long thirdDomain, long currentTerm, long persisted, long witnesses",
                "Count the distinct failure domains containing at least one data replica that persisted the write in the current term. Each three-bit mask uses bit 0 for the first member. Witness members and acknowledgments from earlier terms do not count. Domain identifiers are 1–10,000 and may repeat.",
                """
                var durableMembers = currentTerm & persisted & ~witnesses;
                var domains = new[] { firstDomain, secondDomain, thirdDomain };
                return domains.Where((_, index) => (durableMembers & (1L << index)) != 0).Count();
                """,
                """
                var durableMembers = currentTerm & persisted & ~witnesses;
                var domains = new[] { firstDomain, secondDomain, thirdDomain };
                return domains.Where((_, index) => (durableMembers & (1L << index)) != 0).Distinct().Count();
                """,
                [1, 2, 3, 7, 7, 4], 2, [1, 1, 3, 7, 7, 4], 1),
            Fault("SubscriptionGap", "long cursorEpoch, long cursorOffset, long eventEpoch, long eventOffset, long last, long first, long marker",
                "Return the payload offsets strictly between the committed cursor and a later event. In each epoch offsets run from first through last inclusive, with marker reserved for metadata. The event and cursor are payload offsets. The event is either later in the cursor epoch or in the following epoch. Offsets and epochs are 0–10,000, with first < marker < last.",
                """
                var sameEpoch = cursorEpoch == eventEpoch;
                var distance = sameEpoch ? eventOffset - cursorOffset : last - cursorOffset + eventOffset - first + 1;
                var reserved = cursorOffset < marker && marker < eventOffset ? 1 : 0;
                return distance - 1 - reserved;
                """,
                """
                var sameEpoch = cursorEpoch == eventEpoch;
                var distance = sameEpoch ? eventOffset - cursorOffset : last - cursorOffset + eventOffset - first + 1;
                var reserved = sameEpoch
                    ? cursorOffset < marker && marker < eventOffset ? 1 : 0
                    : (cursorOffset < marker ? 1 : 0) + (marker < eventOffset ? 1 : 0);
                return distance - 1 - reserved;
                """,
                [2, 1, 2, 7, 9, 0, 4], 4, [2, 2, 3, 7, 9, 0, 4], 12),
            Fault("CompactionFloor", "long stable, long followerCursor, long snapshotPin, long attached, long snapshotOpen",
                "Return the earliest offset that must remain. The stable watermark always pins history, and an attached follower also pins its replay cursor. An open snapshot transfer pins its start independently until it closes, including transfers to detached followers. Offsets are 0–10,000 and attached and snapshotOpen are predicates.",
                """
                var keep = stable;
                if (attached == 1)
                {
                    keep = Math.Min(keep, followerCursor);
                    if (snapshotOpen == 1)
                        keep = Math.Min(keep, snapshotPin);
                }
                return keep;
                """,
                """
                var keep = stable;
                if (attached == 1)
                    keep = Math.Min(keep, followerCursor);
                if (snapshotOpen == 1)
                    keep = Math.Min(keep, snapshotPin);
                return keep;
                """,
                [46, 31, 20, 1, 1], 20, [46, 31, 20, 0, 1], 20),
            Fault("ReclaimableBlocks", "long retired, long live, long snapshot, long restoring",
                "Count retired blocks unreferenced by either the live index or an active restore snapshot. A block can appear in several manifests. Only during restore does the snapshot hold blocks. Masks represent up to 16 blocks, and restoring is a predicate.",
                """
                var restorePins = restoring == 1 ? snapshot : 0;
                var retained = live ^ restorePins;
                return System.Numerics.BitOperations.PopCount((ulong)(retired & ~retained));
                """,
                """
                var restorePins = restoring == 1 ? snapshot : 0;
                var retained = live | restorePins;
                return System.Numerics.BitOperations.PopCount((ulong)(retired & ~retained));
                """,
                [15, 1, 2, 1], 2, [15, 3, 6, 1], 1),
            Fault("ReplayCredit", "long firstBytes, long secondBytes, long pageSize, long firstCodec, long secondCodec, long firstDictionary, long secondDictionary",
                "Count pages charged for two replay chunks. Chunks share a partially used page only when both codec and dictionary generation match. Otherwise each chunk rounds its byte count up to whole pages on its own. Bytes and identifiers are 0–10,000 and pageSize is 1–10,000.",
                """
                long Pages(long bytes) => (bytes + pageSize - 1) / pageSize;
                if (firstCodec == secondCodec)
                    return Pages(firstBytes + secondBytes);
                return Pages(firstBytes) + Pages(secondBytes);
                """,
                """
                long Pages(long bytes) => (bytes + pageSize - 1) / pageSize;
                if (firstCodec == secondCodec && firstDictionary == secondDictionary)
                    return Pages(firstBytes + secondBytes);
                return Pages(firstBytes) + Pages(secondBytes);
                """,
                [3, 4, 8, 2, 2, 6, 6], 1, [3, 4, 8, 2, 2, 6, 7], 2),
            Fault("SnapshotPartOffset", "long part, long firstPart, long lastPart, long reversed, long payloadBytes, long headerBytes, long footerBytes",
                "Return the zero-based physical byte offset of a part's payload. The parts occupy consecutive logical identifiers from firstPart to lastPart and are stored in ascending order, or descending when reversed is 1. Each has payloadBytes. The first logical part carries a header before its payload, and the last logical part carries a footer after its payload, in either physical order. All counts and identifiers are 0–10,000 with firstPart ≤ part ≤ lastPart and payloadBytes > 0. reversed is a predicate.",
                """
                var position = reversed == 1 ? lastPart - part : part - firstPart;
                return position * payloadBytes + headerBytes;
                """,
                """
                var position = reversed == 1 ? lastPart - part : part - firstPart;
                var headerBefore = reversed == 0 || part == firstPart ? headerBytes : 0;
                var footerBefore = reversed == 1 && part < lastPart ? footerBytes : 0;
                return position * payloadBytes + headerBefore + footerBefore;
                """,
                [4, 3, 6, 0, 10, 2, 5], 12, [4, 3, 6, 1, 10, 2, 5], 25),
            Fault("RebaseReplayCursor", "long cursor, long oldBase, long newBase, long newLength",
                "The old cursor is relative to oldBase. Return that same absolute position relative to newBase, clamped to the new window [0, newLength]. The new window may start before or after the old one. A cursor at the end of the window is valid. All arguments are 0–10,000.",
                """
                var bounded = Math.Min(cursor, newLength);
                return Math.Max(0, oldBase - newBase + bounded);
                """,
                """
                var rebased = oldBase + cursor - newBase;
                return Math.Clamp(rebased, 0, newLength);
                """,
                [12, 100, 100, 18], 12, [12, 100, 90, 18], 18),
            Fault("ReadLagState", "long applied, long expected, long previous, long low, long high, long measuredTerm, long currentTerm",
                "Return 1 for lagging or 0 for healthy. A sample from a term other than currentTerm preserves the previous state. A current sample enters the lagging state at a gap of at least high, exits at a gap no greater than low, and preserves the previous state in between. Gap is the nonnegative difference expected minus applied. low and high are 0–10,000 with low < high. Counts and terms are 0–10,000 and previous is a predicate.",
                """
                var gap = Math.Max(0, expected - applied);
                if (gap <= low)
                    return 0;
                if (measuredTerm != currentTerm)
                    return previous;
                return gap >= high ? 1 : previous;
                """,
                """
                if (measuredTerm != currentTerm)
                    return previous;
                var gap = Math.Max(0, expected - applied);
                if (gap <= low)
                    return 0;
                return gap >= high ? 1 : previous;
                """,
                [45, 52, 0, 2, 6, 7, 7], 1, [50, 52, 1, 2, 6, 7, 8], 1),
            Fault("FinalizeStreamCredit", "long balance, long capacity, long rate, long lastRefill, long arrived, long settled, long cost",
                "Return the stream credit at settlement. The balance is measured at lastRefill. Refill at rate per minute up to capacity, debit cost at arrival without going below zero, then refill from arrival to settlement. All inputs are 0–10,000, balance ≤ capacity, and lastRefill ≤ arrived ≤ settled.",
                """
                var atSettlement = Math.Min(capacity, balance + (settled - lastRefill) * rate);
                return Math.Max(0, atSettlement - cost);
                """,
                """
                var atArrival = Math.Min(capacity, balance + (arrived - lastRefill) * rate);
                var remaining = Math.Max(0, atArrival - cost);
                return Math.Min(capacity, remaining + (settled - arrived) * rate);
                """,
                [2, 10, 1, 0, 2, 3, 2], 3, [8, 10, 2, 0, 1, 4, 6], 10),
            Valid("WitnessBallots", "long live, long votes, long draining, long jointTransition",
                "Count ballots from live members with set vote bits. Drain-marked members remain in the ballot set during a joint configuration transition, and are excluded otherwise. The masks use 16 or fewer low bits. jointTransition is a predicate.",
                """
                var permitted = jointTransition == 1 ? live : live & ~draining;
                return System.Numerics.BitOperations.PopCount((ulong)(permitted & votes));
                """,
                [15, 11, 2, 1], 3),
            Valid("FullSegmentsBeforePin", "long retainedFrom, long activeFrom, long pin, long segmentLength",
                "Count full segments eligible to drop starting at retainedFrom. Segments have segmentLength offsets and align to retainedFrom. Drop only segments whose offsets are all strictly before both the active segment at activeFrom and the pin. Offsets are 0–10,000, retainedFrom ≤ activeFrom, and segmentLength is 1–10,000.",
                """
                var stop = Math.Min(activeFrom, pin);
                return Math.Max(0, stop - retainedFrom) / segmentLength;
                """,
                [5, 35, 26, 7], 3),
        ]),
        new("Folio", "Folio assembles structured imports and report exports, including formatting, corrections, and summary measures.",
        [
            Fault("PackOrderedSections", "long first, long second, long third, long fourth, long capacity",
                "Return the number of pages to lay out four sections in order. A nonempty section no taller than a page must stay together and moves to a fresh page when the remaining space is too small. A larger section streams across available space, including space on the current page. Empty sections use no space. Section sizes are 0–10,000 rows and capacity is 1–10,000 rows.",
                """
                var rows = first + second + third + fourth;
                return (rows + capacity - 1) / capacity;
                """,
                """
                var rows = 0L;
                foreach (var section in new[] { first, second, third, fourth })
                {
                    var used = rows % capacity;
                    if (section <= capacity && used != 0 && used + section > capacity)
                        rows += capacity - used;
                    rows += section;
                }
                return (rows + capacity - 1) / capacity;
                """,
                [4, 3, 2, 0, 10], 1, [6, 6, 6, 11, 10], 4),
            Fault("ResolveDetailAmount", "long persistedUnits, long overrideAmount, long present, long overrideBatch, long currentBatch, long scale",
                "Return the detail value in export units. A present override from the current batch is already in export units and takes precedence, including when its value is zero. Otherwise multiply the persisted units by scale. Amounts are −10,000 to 10,000, batch identifiers are 0–10,000, scale is 1–10,000, and present is a predicate.",
                """
                var currentOverride = present == 1 && overrideBatch == currentBatch;
                return currentOverride && overrideAmount != 0 ? overrideAmount : persistedUnits * scale;
                """,
                """
                var currentOverride = present == 1 && overrideBatch == currentBatch;
                return currentOverride ? overrideAmount : persistedUnits * scale;
                """,
                [5, 21, 1, 6, 6, 100], 21, [5, 0, 1, 6, 6, 100], 0),
            Fault("AllocatedCents", "long total, long firstWeight, long secondWeight, long thirdWeight, long section",
                "Return the whole cents allocated to the selected section 1–3. First round each exact weighted share down. Give one remaining cent to each section with the largest fractional remainder until the total is allocated. Break a fractional tie in section order. Total is 0–10,000 and weights are 1–10,000.",
                """
                var weights = new[] { firstWeight, secondWeight, thirdWeight };
                var sum = weights.Sum();
                var shares = weights.Select(weight => total * weight / sum).ToArray();
                var unassigned = (int)(total - shares.Sum());
                foreach (var index in Enumerable.Range(0, weights.Length).OrderByDescending(index => weights[index]).Take(unassigned))
                    shares[index]++;
                return shares[(int)section - 1];
                """,
                """
                var numerators = new[] { total * firstWeight, total * secondWeight, total * thirdWeight };
                var sum = firstWeight + secondWeight + thirdWeight;
                var shares = numerators.Select(numerator => numerator / sum).ToArray();
                var unassigned = (int)(total - shares.Sum());
                foreach (var index in Enumerable.Range(0, numerators.Length).OrderByDescending(index => numerators[index] % sum).Take(unassigned))
                    shares[index]++;
                return shares[(int)section - 1];
                """,
                [10, 5, 3, 2, 2], 3, [7, 5, 4, 2, 2], 3),
            Fault("CountSummaryGroups", "long allSeries, long firstRegion, long firstSeries, long secondRegion, long secondSeries, long thirdRegion, long thirdSeries",
                "Count report groups among three source rows. In detailed mode use exact pairs of region code and series code. With allSeries enabled, group by region alone. Both codes are 0–10,000, and allSeries is a predicate.",
                """
                long Key(long region, long series) => allSeries == 1 ? region : region * 1000 + series;
                return new[] { Key(firstRegion, firstSeries), Key(secondRegion, secondSeries), Key(thirdRegion, thirdSeries) }.Distinct().Count();
                """,
                """
                (long, long) Key(long region, long series) => (region, allSeries == 1 ? 0 : series);
                return new[] { Key(firstRegion, firstSeries), Key(secondRegion, secondSeries), Key(thirdRegion, thirdSeries) }.Distinct().Count();
                """,
                [0, 1, 11, 1, 11, 2, 1], 2, [0, 1, 1001, 2, 1, 2, 2], 3),
            Fault("RestatementPoints", "long originalQuantity, long replacementQuantity, long originalFactor, long currentFactor, long repriced",
                "Return the change in report points when restating a previously published row. Reverse the old row at its original factor. Publish the replacement at the original factor unless repricing was approved, in which case use the current factor for the replacement. Quantities are −10,000 to 10,000, factors are 1–10,000, and repriced is a predicate.",
                """
                var factor = repriced == 1 ? currentFactor : originalFactor;
                return (replacementQuantity - originalQuantity) * factor;
                """,
                """
                var factor = repriced == 1 ? currentFactor : originalFactor;
                return replacementQuantity * factor - originalQuantity * originalFactor;
                """,
                [10, 13, 2, 3, 0], 6, [10, 13, 2, 3, 1], 19),
            Fault("ReportingPeriodIndex", "long recordedMonth, long postingLag, long fiscalAnchor, long periodMonths",
                "Return the zero-based period index containing the effective month recordedMonth minus postingLag. Period 0 starts at fiscalAnchor, and each period is periodMonths contiguous months, including periods before the anchor with negative indices. Month and lag inputs are 0–10,000, and periodMonths is 2–12.",
                """
                var relativeMonth = recordedMonth - postingLag - fiscalAnchor;
                return relativeMonth / periodMonths;
                """,
                """
                var relativeMonth = recordedMonth - postingLag - fiscalAnchor;
                var period = relativeMonth / periodMonths;
                return relativeMonth % periodMonths < 0 ? period - 1 : period;
                """,
                [18, 2, 4, 3], 4, [6, 4, 4, 3], -1),
            Fault("WeightedPercentile", "long firstValue, long firstCount, long secondValue, long secondCount, long thirdValue, long thirdCount, long percentile",
                "Return the lowest observed value whose cumulative weight reaches percentile percent of total weight. On an exact cumulative boundary, return the integer midpoint rounded down between that observed value and the next observed value. Values are strictly increasing in 0–10,000. Each count is 0–10,000, their sum is positive, and percentile is 1–99. A value with zero count is unobserved.",
                """
                var samples = new[] { (firstValue, firstCount), (secondValue, secondCount), (thirdValue, thirdCount) };
                var target = samples.Sum(sample => sample.Item2) * percentile;
                var cumulative = 0L;
                for (var index = 0; index < samples.Length; index++)
                {
                    var (value, count) = samples[index];
                    cumulative += count * 100;
                    if (cumulative > target)
                        return value;
                    if (cumulative == target)
                        return (value + samples[index + 1].Item1) / 2;
                }
                return 0;
                """,
                """
                var samples = new[] { (firstValue, firstCount), (secondValue, secondCount), (thirdValue, thirdCount) }
                    .Where(sample => sample.Item2 != 0).ToArray();
                var target = samples.Sum(sample => sample.Item2) * percentile;
                var cumulative = 0L;
                for (var index = 0; index < samples.Length; index++)
                {
                    var (value, count) = samples[index];
                    cumulative += count * 100;
                    if (cumulative > target)
                        return value;
                    if (cumulative == target)
                        return (value + samples[index + 1].Item1) / 2;
                }
                return 0;
                """,
                [10, 2, 20, 3, 40, 1, 50], 20, [10, 2, 20, 0, 50, 2, 50], 30),
            Fault("ExportColumnOrder", "long ordinal, long visible, long reverseBands",
                "Return the one-based identifier of the column at a zero-based displayed ordinal, or 0 if absent. The six columns form three consecutive bands of two columns. Reversing bands changes their order while columns within each band retain schema order. The visibility mask uses the low six bits, with bit 0 for column 1. Ordinal is 0–5, and reverseBands is a predicate.",
                """
                var schema = Enumerable.Range(0, 6);
                var ordered = reverseBands == 1 ? schema.Reverse() : schema;
                return ordered.Where(column => (visible & (1L << column)) != 0)
                    .Select(column => column + 1).ElementAtOrDefault((int)ordinal);
                """,
                """
                var bands = Enumerable.Range(0, 3);
                var ordered = reverseBands == 1 ? bands.Reverse() : bands;
                return ordered.SelectMany(band => Enumerable.Range(band * 2, 2))
                    .Where(column => (visible & (1L << column)) != 0)
                    .Select(column => column + 1).ElementAtOrDefault((int)ordinal);
                """,
                [2, 63, 0], 3, [1, 45, 1], 3),
            Fault("ResolveCarryForward", "long groupDefault, long firstValue, long firstKind, long secondValue, long secondKind, long suppressed",
                "Resolve a missing value in the current row from two earlier rows, applied in order. A divider (kind 1) resets the value to groupDefault, even if the divider is hidden from the exported report. A visible detail (kind 0) sets the value. An annotation (kind 2) has no effect. The suppression mask uses bit 0 for the first earlier row and bit 1 for the second. Values are −10,000 to 10,000.",
                """
                var result = groupDefault;
                var history = new[] { (Kind: firstKind, Value: firstValue), (Kind: secondKind, Value: secondValue) };
                for (var index = 0; index < history.Length; index++)
                {
                    if ((suppressed & (1L << index)) != 0)
                        continue;
                    var row = history[index];
                    if (row.Kind == 1)
                        result = groupDefault;
                    else if (row.Kind == 0)
                        result = row.Value;
                }
                return result;
                """,
                """
                var result = groupDefault;
                var history = new[] { (Kind: firstKind, Value: firstValue), (Kind: secondKind, Value: secondValue) };
                for (var index = 0; index < history.Length; index++)
                {
                    var row = history[index];
                    if (row.Kind == 1)
                        result = groupDefault;
                    else if (row.Kind == 0 && (suppressed & (1L << index)) == 0)
                        result = row.Value;
                }
                return result;
                """,
                [3, 8, 0, 99, 2, 0], 8, [3, 8, 0, 99, 1, 2], 3),
            Fault("ClassifyRateChange", "long previousHits, long previousCount, long currentHits, long currentCount, long basisPoints",
                "Compare the exact current and previous rates (hits divided by count). Return 1 when the increase is at least basisPoints/10,000, −1 when the decrease is at least that amount, and 0 otherwise. Counts are 1–10,000, hits are 0 through their respective count, and basisPoints is 1–10,000. Rounded display percentages do not determine the status.",
                """
                var delta = (currentHits * 100 / currentCount - previousHits * 100 / previousCount) * 100;
                return delta >= basisPoints ? 1 : delta <= -basisPoints ? -1 : 0;
                """,
                """
                var delta = (currentHits * previousCount - previousHits * currentCount) * 10_000;
                var threshold = basisPoints * previousCount * currentCount;
                return delta >= threshold ? 1 : delta <= -threshold ? -1 : 0;
                """,
                [50, 100, 55, 100, 200], 1, [149, 300, 201, 400, 75], 0),
            Valid("SortCountWithBlank", "long first, long second, long third, long excludeBlank",
                "Return the number of distinct sort categories in three cells. Codes 0–10,000 represent filled cells, and −1 represents the single blank category. Blanks are excluded when excludeBlank is 1. excludeBlank is a predicate.",
                """
                var categories = new[] { first, second, third };
                return categories.Where(code => excludeBlank == 0 || code >= 0).Distinct().Count();
                """,
                [0, -1, 0, 0], 2),
            Valid("ReportCoverage", "long covered, long expected, long adjustments, long allowSurplus",
                "Return the adjusted covered count. Signed adjustments can reduce or increase the covered count. The result is always at least zero, and is limited to the expected count unless surplus is permitted. Covered and expected are 0–10,000, adjustments is −10,000 to 10,000, and allowSurplus is a predicate.",
                """
                var adjusted = Math.Max(0, covered + adjustments);
                return allowSurplus == 1 ? adjusted : Math.Min(expected, adjusted);
                """,
                [12, 10, -1, 0], 10),
        ]),
    ];
}
