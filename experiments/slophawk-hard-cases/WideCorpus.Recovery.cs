internal static partial class WideCorpus
{
    private static WideModule RecoveryJournal() => new("RecoveryJournal",
        "The settlement service restarts from a durable checkpoint and reconciles its remaining records and receipts. Inputs are decoded fields from valid persisted records. Quantities, identifiers, versions, and ticks fit in signed arithmetic.",
    [
        Fault("RestoreDebitBalance",
            "long postedBalance, long heldForReplay, long debit, long eventOrigin, long eventEpoch, long eventNumber, long receiptOrigin, long receiptEpoch, long receiptNumber, long receiptState",
            "Return the available balance after reconciling one pending debit. postedBalance reflects every committed debit before subtracting the pending reservation. The reservation amount heldForReplay is at most postedBalance. The event and receipt each identify an origin, the origin's sequence epoch, and a number that can restart in each epoch. A matching receipt in state 2 means the debit already posted, so release the reservation. State 0 means absent and 1 means prepared only. For an unposted debit, release the reservation and book the debit if funds suffice. If funds are insufficient, leave the reservation in place.",
            """
            var available = postedBalance - heldForReplay;
            var receiptMatches = eventOrigin == receiptOrigin && eventNumber == receiptNumber;
            if (receiptState == 2 && receiptMatches)
                return available + heldForReplay;

            var fundsAtPosting = available + heldForReplay;
            if (fundsAtPosting < debit)
                return available;
            return fundsAtPosting - debit;
            """,
            """
            var available = postedBalance - heldForReplay;
            var receiptMatches = eventOrigin == receiptOrigin && eventEpoch == receiptEpoch && eventNumber == receiptNumber;
            if (receiptState == 2 && receiptMatches)
                return available + heldForReplay;

            var fundsAtPosting = available + heldForReplay;
            if (fundsAtPosting < debit)
                return available;
            return fundsAtPosting - debit;
            """,
            [150, 20, 35, 8, 6, 40, 8, 6, 40, 1], 115,
            [150, 20, 35, 8, 7, 40, 8, 6, 40, 2], 115),

        Fault("CreditRecoveredAliases",
            "long firstBucket, long secondBucket, long firstRequested, long secondRequested, long firstAlreadyCredited, long secondAlreadyCredited, long firstRoom, long secondRoom, long pool",
            "Recover two partially posted credits in positional order and return the new credit granted to the second request. Each requested amount includes the corresponding amount already credited. Each bucket identifier designates a daily account limit and its room is the remaining capacity after all previous postings. Requests using the same identifier see the same initial room and consume that room together. Pool is the remaining recovery budget shared by both requests even across different buckets. A grant consumes both room and pool.",
            """
            var firstOutstanding = firstRequested - firstAlreadyCredited;
            var secondOutstanding = secondRequested - secondAlreadyCredited;
            var firstGrant = Math.Min(firstOutstanding, Math.Min(firstRoom, pool));
            var remainingPool = pool - firstGrant;
            var secondCapacity = firstBucket == secondBucket ? firstRoom : secondRoom;
            var secondGrant = Math.Min(secondOutstanding, secondCapacity);
            return Math.Min(secondGrant, remainingPool);
            """,
            """
            var firstOutstanding = firstRequested - firstAlreadyCredited;
            var secondOutstanding = secondRequested - secondAlreadyCredited;
            var firstGrant = Math.Min(firstOutstanding, Math.Min(firstRoom, pool));
            var remainingPool = pool - firstGrant;
            var secondCapacity = firstBucket == secondBucket ? firstRoom - firstGrant : secondRoom;
            var secondGrant = Math.Min(secondOutstanding, secondCapacity);
            return Math.Min(secondGrant, remainingPool);
            """,
            [11, 12, 45, 38, 10, 8, 40, 26, 70], 26,
            [11, 11, 45, 38, 10, 8, 40, 40, 70], 5),

        Fault("RestoreCompactedValue",
            "long capturedThrough, long rowVersion, long deleteRevision, long checkpointValue, long tailKind, long tailBase, long tailRevision, long tailValue",
            "Return the live stream value or -1 for a deleted row. capturedThrough is the checkpoint's scan position and rowVersion is the positive revision of the retained row image, at or below capturedThrough. The independently persisted tombstone deleteRevision is 0 when absent and suppresses row images through its own revision inclusive. A tail record can affect the result only if its revision exceeds both the checkpoint cursor and tombstone revision. Tail kind 1 adds its signed value to a live row at exactly tailBase. Kind 2 creates a new row with tailValue if the old row is suppressed and tailBase names the tombstone. Live values are nonnegative.",
            """
            var live = capturedThrough > deleteRevision;
            var replayAfter = Math.Max(capturedThrough, deleteRevision);
            if (tailRevision <= replayAfter)
                return live ? checkpointValue : -1;
            if (tailKind == 1 && live && tailBase == rowVersion)
                return checkpointValue + tailValue;
            if (tailKind == 2 && !live && tailBase == deleteRevision)
                return tailValue;
            return live ? checkpointValue : -1;
            """,
            """
            var live = rowVersion > deleteRevision;
            var replayAfter = Math.Max(capturedThrough, deleteRevision);
            if (tailRevision <= replayAfter)
                return live ? checkpointValue : -1;
            if (tailKind == 1 && live && tailBase == rowVersion)
                return checkpointValue + tailValue;
            if (tailKind == 2 && !live && tailBase == deleteRevision)
                return tailValue;
            return live ? checkpointValue : -1;
            """,
            [15, 12, 8, 40, 1, 12, 16, 5], 45,
            [15, 8, 12, 40, 2, 12, 16, 17], 17),

        Fault("CompactJournalThrough",
            "long checkpointBase, long checkpointCommit, long receiptBase, long receiptThrough, long firstVersion, long firstPosting, long secondVersion, long secondPosting",
            "Return the highest journal position safe to trim through. The journal is already trimmed through checkpointBase. The checkpoint includes materialized state up to checkpointCommit. The two retained records are the complete list after the base through the commit, in arbitrary order. Posting flags are 1 for records with a separate ledger effect and 0 for pure state updates. Compacted settlement receipts cover the span strictly after receiptBase through receiptThrough inclusive. A record with a separate ledger effect must stay in the journal if no covering receipt is available, even when the checkpoint includes its materialized state. Trimming can only remove a prefix.",
            """
            var trimThrough = checkpointCommit;
            var retained = new[] { (Version: firstVersion, Posting: firstPosting), (Version: secondVersion, Posting: secondPosting) };
            foreach (var entry in retained)
            {
                if (entry.Version <= checkpointBase || entry.Version > trimThrough)
                    continue;
                var posted = entry.Version <= receiptThrough;
                if (entry.Posting == 1 && !posted)
                    trimThrough = entry.Version - 1;
            }
            return trimThrough;
            """,
            """
            var trimThrough = checkpointCommit;
            var retained = new[] { (Version: firstVersion, Posting: firstPosting), (Version: secondVersion, Posting: secondPosting) };
            foreach (var entry in retained)
            {
                if (entry.Version <= checkpointBase || entry.Version > trimThrough)
                    continue;
                var posted = entry.Version > receiptBase && entry.Version <= receiptThrough;
                if (entry.Posting == 1 && !posted)
                    trimThrough = entry.Version - 1;
            }
            return trimThrough;
            """,
            [10, 21, 10, 18, 20, 0, 14, 1], 21,
            [10, 21, 16, 18, 20, 0, 14, 1], 13),

        Fault("ChooseSettlementState",
            "long firstRevision, long firstState, long firstSource, long firstObservedAt, long secondRevision, long secondState, long secondSource, long secondObservedAt, long thirdRevision, long thirdState, long thirdSource, long thirdObservedAt",
            "Return the state selected from three asynchronous reports about one settlement. A report contains its journal revision, state (1 pending, 2 settled, 3 declined), source (0 broker observation, 1 durable ledger report), and observation tick. Prefer the greatest revision. At equal revisions a durable ledger report takes precedence because broker observations can reflect tentative state. Within the same revision and source prefer the latest observation tick. Keep the earliest argument position if all selection fields tie.",
            """
            var selected = (Revision: firstRevision, State: firstState, Source: firstSource, Tick: firstObservedAt);
            var later = new[] { (Revision: secondRevision, State: secondState, Source: secondSource, Tick: secondObservedAt),
                (Revision: thirdRevision, State: thirdState, Source: thirdSource, Tick: thirdObservedAt) };
            foreach (var report in later)
            {
                if (report.Revision > selected.Revision || report.Revision == selected.Revision &&
                    (report.Tick > selected.Tick || report.Tick == selected.Tick && report.Source > selected.Source))
                    selected = report;
            }
            return selected.State;
            """,
            """
            var selected = (Revision: firstRevision, State: firstState, Source: firstSource, Tick: firstObservedAt);
            var later = new[] { (Revision: secondRevision, State: secondState, Source: secondSource, Tick: secondObservedAt),
                (Revision: thirdRevision, State: thirdState, Source: thirdSource, Tick: thirdObservedAt) };
            foreach (var report in later)
            {
                if (report.Revision > selected.Revision || report.Revision == selected.Revision &&
                    (report.Source > selected.Source || report.Source == selected.Source && report.Tick > selected.Tick))
                    selected = report;
            }
            return selected.State;
            """,
            [20, 2, 1, 3, 21, 1, 0, 5, 21, 3, 0, 8], 3,
            [20, 2, 1, 3, 21, 2, 1, 5, 21, 3, 0, 8], 2),

        Fault("ReplayableHistoryStart",
            "long nextWriteSlot, long activeCount, long nextOffset, long slotZero, long slotOne, long slotTwo, long slotThree",
            "Return the oldest offset in the continuous recoverable history immediately preceding nextOffset, or nextOffset itself when no record is recoverable. The four fields slotZero through slotThree are physical slots containing positive journal offsets or 0 for empty. nextWriteSlot is the next physical slot to fill (0 through 3). The activeCount slots (0 through 4) immediately before it, wrapping around, are the valid contents. Other slots can retain obsolete offsets after restoring a shorter history. Walk backward in physical order and stop at the first missing predecessor.",
            """
            var slots = new[] { slotZero, slotOne, slotTwo, slotThree };
            var cursor = nextOffset;
            var slot = (int)nextWriteSlot;
            for (var distance = 0; distance < slots.Length; distance++)
            {
                slot = (slot + slots.Length - 1) % slots.Length;
                if (slots[slot] == 0 || slots[slot] != cursor - 1)
                    break;
                cursor = slots[slot];
            }
            return cursor;
            """,
            """
            var slots = new[] { slotZero, slotOne, slotTwo, slotThree };
            var cursor = nextOffset;
            var slot = (int)nextWriteSlot;
            for (var distance = 0; distance < activeCount; distance++)
            {
                slot = (slot + slots.Length - 1) % slots.Length;
                if (slots[slot] == 0 || slots[slot] != cursor - 1)
                    break;
                cursor = slots[slot];
            }
            return cursor;
            """,
            [1, 3, 35, 34, 31, 30, 33], 33,
            [1, 2, 35, 34, 31, 32, 33], 33),

        Fault("RecoverDeliveryPrefix",
            "long checkpoint, long deliveredThrough, long reversedThrough, long firstOffset, long firstWithdrawn, long firstTransmitted, long secondOffset, long secondWithdrawn, long secondTransmitted",
            "Return the highest safe outbox cursor after recovering two records with increasing offsets greater than checkpoint. Only adjacent offsets extend the cursor. A record still active needs a delivery receipt. A withdrawn record can be skipped if it was never transmitted; if it was transmitted it needs both delivery and reversal receipts. Each receipt watermark covers all offsets through itself. The two flags on each record represent withdrawal and transmission history.",
            """
            var cursor = checkpoint;
            var pending = new[] { (Offset: firstOffset, Withdrawn: firstWithdrawn, Transmitted: firstTransmitted),
                (Offset: secondOffset, Withdrawn: secondWithdrawn, Transmitted: secondTransmitted) };
            foreach (var item in pending)
            {
                if (item.Offset != cursor + 1)
                    break;
                var canSkip = item.Withdrawn == 1;
                var delivered = item.Offset <= deliveredThrough && (item.Withdrawn == 0 || item.Offset <= reversedThrough);
                if (!canSkip && !delivered)
                    break;
                cursor = item.Offset;
            }
            return cursor;
            """,
            """
            var cursor = checkpoint;
            var pending = new[] { (Offset: firstOffset, Withdrawn: firstWithdrawn, Transmitted: firstTransmitted),
                (Offset: secondOffset, Withdrawn: secondWithdrawn, Transmitted: secondTransmitted) };
            foreach (var item in pending)
            {
                if (item.Offset != cursor + 1)
                    break;
                var canSkip = item.Withdrawn == 1 && item.Transmitted == 0;
                var delivered = item.Offset <= deliveredThrough && (item.Withdrawn == 0 || item.Offset <= reversedThrough);
                if (!canSkip && !delivered)
                    break;
                cursor = item.Offset;
            }
            return cursor;
            """,
            [10, 11, 10, 11, 0, 1, 12, 1, 0], 12,
            [10, 12, 11, 11, 1, 1, 12, 1, 1], 11),

        Fault("RecoveredAttemptBudget",
            "long limit, long attemptsBooked, long checkpointThrough, long firstPrepared, long firstReceipt, long firstOutcome, long secondPrepared, long secondReceipt, long secondOutcome",
            "Return the remaining attempt allowance for one delivery, floored at zero. The checkpoint's attemptsBooked counts real transport attempts whose terminal receipts were recorded at or before checkpointThrough. Preparation precedes the terminal receipt and consumes no allowance by itself. The two distinct attempts have outcome 1 if abandoned before contacting transport, 2 if transport accepted, or 3 if transport refused. An attempt that reached transport consumes one allowance at its terminal receipt.",
            """
            var available = limit - attemptsBooked;
            var attempts = new[] { (Prepared: firstPrepared, Receipt: firstReceipt, Outcome: firstOutcome),
                (Prepared: secondPrepared, Receipt: secondReceipt, Outcome: secondOutcome) };
            foreach (var attempt in attempts)
            {
                if (attempt.Prepared <= checkpointThrough || attempt.Outcome == 1)
                    continue;
                available--;
            }
            return Math.Max(0, available);
            """,
            """
            var available = limit - attemptsBooked;
            var attempts = new[] { (Prepared: firstPrepared, Receipt: firstReceipt, Outcome: firstOutcome),
                (Prepared: secondPrepared, Receipt: secondReceipt, Outcome: secondOutcome) };
            foreach (var attempt in attempts)
            {
                if (attempt.Receipt <= checkpointThrough || attempt.Outcome == 1)
                    continue;
                available--;
            }
            return Math.Max(0, available);
            """,
            [5, 2, 40, 41, 42, 3, 43, 44, 1], 2,
            [5, 2, 40, 37, 42, 3, 43, 44, 1], 2),

        Fault("SettledFeeAdjustment",
            "long settledGross, long recoveredDelta, long bookedFee, long threshold, long lowerBasisPoints, long upperBasisPoints, long minimumFee",
            "Return the signed fee correction in cents. Add the recovered signed delta to the originally settled gross to obtain a nonnegative revised gross. The fee is zero for zero gross. Otherwise apply lowerBasisPoints through threshold cents and upperBasisPoints to the excess, then round the combined fee to the nearest cent with half cents rounding up. Raise the rounded fee to at least minimumFee. bookedFee is what the ledger actually charged before recovery and must be subtracted from the revised fee.",
            """
            var revisedGross = settledGross + recoveredDelta;
            if (revisedGross == 0)
                return -bookedFee;
            var lowerNumerator = Math.Min(revisedGross, threshold) * lowerBasisPoints;
            var upperNumerator = Math.Max(0, revisedGross - threshold) * upperBasisPoints;
            var roundedFee = (lowerNumerator + 5_000) / 10_000 + (upperNumerator + 5_000) / 10_000;
            return Math.Max(minimumFee, roundedFee) - bookedFee;
            """,
            """
            var revisedGross = settledGross + recoveredDelta;
            if (revisedGross == 0)
                return -bookedFee;
            var lowerNumerator = Math.Min(revisedGross, threshold) * lowerBasisPoints;
            var upperNumerator = Math.Max(0, revisedGross - threshold) * upperBasisPoints;
            var roundedFee = (lowerNumerator + upperNumerator + 5_000) / 10_000;
            return Math.Max(minimumFee, roundedFee) - bookedFee;
            """,
            [1000, 600, 25, 1000, 250, 100, 15], 6,
            [950, 946, 24, 1016, 250, 50, 15], 6),

        Fault("RecoverTransferReserve",
            "long heldAtCheckpoint, long releasedUnits, long successorUnits, long transferVersion, long releasePersistedThrough, long successorPersistedThrough, long successorCancelled",
            "Return the total held units after recovering a reservation transfer. heldAtCheckpoint already includes every applied change to either leg. The transfer releases releasedUnits from the old hold and would add successorUnits to the new hold. The release and successor ledgers advance independently. Each leg is already applied if its own persisted watermark reaches transferVersion. Always complete the old release. If successorCancelled is 1, skip an outstanding successor hold or remove it if it was already applied. Otherwise complete the outstanding successor hold.",
            """
            var bothPersisted = Math.Min(releasePersistedThrough, successorPersistedThrough) >= transferVersion;
            var outstandingRelease = bothPersisted ? 0 : releasedUnits;
            var outstandingSuccessor = bothPersisted ? 0 : successorUnits;
            var afterRelease = heldAtCheckpoint - outstandingRelease;
            if (successorCancelled == 1)
                return afterRelease - (successorUnits - outstandingSuccessor);
            return afterRelease + outstandingSuccessor;
            """,
            """
            var outstandingRelease = releasePersistedThrough >= transferVersion ? 0 : releasedUnits;
            var outstandingSuccessor = successorPersistedThrough >= transferVersion ? 0 : successorUnits;
            var afterRelease = heldAtCheckpoint - outstandingRelease;
            if (successorCancelled == 1)
                return afterRelease - (successorUnits - outstandingSuccessor);
            return afterRelease + outstandingSuccessor;
            """,
            [100, 30, 18, 9, 8, 7, 0], 88,
            [118, 30, 18, 9, 8, 10, 1], 70),

        Fault("RecoveredWindowNet",
            "long postedUnits, long postedThrough, long windowOpen, long windowClose, long correctionVersion, long previousTick, long replacementTick, long previousUnits, long replacementUnits",
            "Return the revised booked units for the half-open effective-time window. postedUnits already reflects all records through postedThrough. If the correction is newer, the earlier settlement is included in postedUnits exactly when previousTick lies in the window. The correction atomically replaces that settlement with the specified amount and effective tick. Remove its old units from whichever window contains previousTick and add its replacement units to whichever window contains replacementTick. Report only the supplied window.",
            """
            if (correctionVersion <= postedThrough)
                return postedUnits;
            var oldInWindow = previousTick >= windowOpen && previousTick < windowClose;
            var newInWindow = replacementTick >= windowOpen && replacementTick < windowClose;
            if (!newInWindow)
                return postedUnits;
            var reversal = oldInWindow ? previousUnits : 0;
            return postedUnits - reversal + replacementUnits;
            """,
            """
            if (correctionVersion <= postedThrough)
                return postedUnits;
            var oldInWindow = previousTick >= windowOpen && previousTick < windowClose;
            var newInWindow = replacementTick >= windowOpen && replacementTick < windowClose;
            var reversal = oldInWindow ? previousUnits : 0;
            var replacement = newInWindow ? replacementUnits : 0;
            return postedUnits - reversal + replacement;
            """,
            [90, 100, 10, 20, 105, 17, 19, 30, 24], 84,
            [90, 100, 10, 20, 105, 17, 23, 30, 24], 60),

        Fault("RetryClockRemaining",
            "long dispatchTick, long now, long activeDelay, long firstOutageOpen, long firstOutageClose, long secondOutageOpen, long secondOutageClose",
            "Return how many more active ticks must elapse before a failed settlement dispatch is eligible to retry. dispatchTick is at or before now. Each outage is an ordered half-open interval reported by a monitor. The two monitors can describe the same downtime. Count elapsed time in the half-open interval from dispatchTick to now, excluding any tick covered by either outage, then subtract active elapsed time from activeDelay and floor the result at zero.",
            """
            var firstBegin = Math.Max(dispatchTick, firstOutageOpen);
            var firstEnd = Math.Min(now, firstOutageClose);
            var secondBegin = Math.Max(dispatchTick, secondOutageOpen);
            var secondEnd = Math.Min(now, secondOutageClose);
            var firstPause = Math.Max(0, firstEnd - firstBegin);
            var secondPause = Math.Max(0, secondEnd - secondBegin);
            var active = Math.Max(0, now - dispatchTick - firstPause - secondPause);
            return Math.Max(0, activeDelay - active);
            """,
            """
            var firstBegin = Math.Max(dispatchTick, firstOutageOpen);
            var firstEnd = Math.Min(now, firstOutageClose);
            var secondBegin = Math.Max(dispatchTick, secondOutageOpen);
            var secondEnd = Math.Min(now, secondOutageClose);
            var firstPause = Math.Max(0, firstEnd - firstBegin);
            var secondPause = Math.Max(0, secondEnd - secondBegin);
            var overlap = Math.Max(0, Math.Min(firstEnd, secondEnd) - Math.Max(firstBegin, secondBegin));
            var active = now - dispatchTick - (firstPause + secondPause - overlap);
            return Math.Max(0, activeDelay - active);
            """,
            [100, 140, 35, 105, 112, 125, 135], 12,
            [100, 140, 35, 95, 118, 110, 130], 25),

        Fault("RestoredAssignmentSlot",
            "long now, long handoffTick, long priorSlot, long priorGrant, long priorExpires, long successorSlot, long successorGrant, long successorExpires, long renewalGrant, long renewalRecordedAt, long renewedUntil",
            "Return the assigned worker slot, or zero if unassigned. The prior slot is scheduled before handoffTick and the successor slot from handoffTick inclusive. Their positive slot identifiers and their unique grant revisions address separate reservations. Each expires at its own expiry tick exclusive. A renewal receipt may arrive asynchronously after the handoff. Apply it only once recorded by now and only to the scheduled reservation whose grant it names, extending that reservation to at least renewedUntil. If the scheduled reservation has expired, leave the slot unassigned.",
            """
            var successorActive = now >= handoffTick;
            var slot = successorActive ? successorSlot : priorSlot;
            var expires = successorActive ? successorExpires : priorExpires;
            var recordedInCurrentPhase = successorActive == (renewalRecordedAt >= handoffTick);
            if (renewalRecordedAt <= now && recordedInCurrentPhase)
                expires = Math.Max(expires, renewedUntil);
            return now < expires ? slot : 0;
            """,
            """
            var successorActive = now >= handoffTick;
            var slot = successorActive ? successorSlot : priorSlot;
            var expires = successorActive ? successorExpires : priorExpires;
            var grant = successorActive ? successorGrant : priorGrant;
            if (renewalRecordedAt <= now && renewalGrant == grant)
                expires = Math.Max(expires, renewedUntil);
            return now < expires ? slot : 0;
            """,
            [53, 40, 4, 21, 44, 9, 22, 50, 22, 43, 75], 9,
            [53, 40, 4, 21, 44, 9, 22, 50, 21, 43, 75], 0),

        Fault("ReplayDisputeReserve",
            "long checkpointHeld, long checkpointThrough, long ceiling, long firstRevision, long firstDelta, long firstConfirmed, long secondRevision, long secondDelta, long secondConfirmed",
            "Return the held dispute reserve after replaying two changes supplied in arbitrary order with distinct revisions. checkpointHeld is the amount at checkpointThrough and lies between zero and ceiling. Apply confirmed changes (flag 1) at strictly later revisions in increasing revision order. Signed deltas add or remove held units. Amounts beyond the ceiling cannot be held, and a release can remove only the units held at the time of that individual change. Ignore unconfirmed changes.",
            """
            var held = checkpointHeld;
            var changes = new[] { (Revision: firstRevision, Delta: firstDelta, Confirmed: firstConfirmed),
                (Revision: secondRevision, Delta: secondDelta, Confirmed: secondConfirmed) };
            foreach (var change in changes.OrderBy(change => change.Revision))
            {
                if (change.Confirmed == 1 && change.Revision > checkpointThrough)
                    held += change.Delta;
            }
            return Math.Clamp(held, 0, ceiling);
            """,
            """
            var held = checkpointHeld;
            var changes = new[] { (Revision: firstRevision, Delta: firstDelta, Confirmed: firstConfirmed),
                (Revision: secondRevision, Delta: secondDelta, Confirmed: secondConfirmed) };
            foreach (var change in changes.OrderBy(change => change.Revision))
            {
                if (change.Confirmed == 1 && change.Revision > checkpointThrough)
                    held = Math.Clamp(held + change.Delta, 0, ceiling);
            }
            return held;
            """,
            [60, 20, 100, 23, -25, 1, 21, 30, 1], 65,
            [88, 20, 100, 23, -25, 1, 21, 30, 1], 75),

        Valid("PendingReceiptUnits",
            "long originalUnits, long shadowUnits, long committedUnits, long commitRevision, long shadowBase, long shadowRevision, long brokerSettled",
            "Return the outstanding units on a settlement with a current committed amount and an optional shadow balance. The shadow is a proposed absolute amount. Use it when based on commitRevision and newer than the commit; otherwise use committedUnits. Cap the provisional amount at the original issued units. brokerSettled is the quantity already paid, regardless of whether it came from the shadow or committed state. Outstanding units cannot be negative.",
            """
            var issued = committedUnits;
            if (shadowBase == commitRevision && shadowRevision > commitRevision)
                issued = shadowUnits;
            var applicable = Math.Min(originalUnits, issued);
            return Math.Max(0, applicable - brokerSettled);
            """,
            [80, 90, 60, 15, 15, 17, 55], 25),

        Valid("SettlementLeaseReviewTick",
            "long clock, long lastActivity, long quietTicks, long leaseStart, long leaseExpires, long statementDeadline",
            "Return the earliest review tick permitted by the inactivity rule and the active review lease. Use the greatest of clock, lastActivity plus quietTicks, and leaseStart. The lease expires at leaseExpires exclusive while statementDeadline is inclusive. Return -1 when the earliest tick cannot meet both limits.",
            """
            var quietAt = lastActivity + quietTicks;
            var scheduled = Math.Max(Math.Max(clock, quietAt), leaseStart);
            if (scheduled > statementDeadline || scheduled >= leaseExpires)
                return -1;
            return scheduled;
            """,
            [51, 45, 12, 56, 60, 57], 57),

        Valid("PreferredRecoveryLane",
            "long firstLane, long firstAvailable, long firstQueued, long secondLane, long secondAvailable, long secondQueued, long now",
            "Return the next lane for an eligible dispatch or zero when no lane is available. Positive lane identifiers are distinct. Each lane becomes available from its given tick inclusive. After filtering lanes by now, prefer the smaller already queued count, breaking a tie by smaller lane identifier.",
            """
            var firstOpen = firstAvailable <= now;
            var secondOpen = secondAvailable <= now;
            if (!firstOpen)
                return secondOpen ? secondLane : 0;
            if (!secondOpen)
                return firstLane;
            if (firstQueued == secondQueued)
                return Math.Min(firstLane, secondLane);
            return firstQueued < secondQueued ? firstLane : secondLane;
            """,
            [9, 57, 3, 5, 58, 3, 58], 5)
    ]);
}
