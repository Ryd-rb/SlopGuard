internal static partial class WideCorpus
{
    internal static WideModule[] LifecycleModules()
    {
        var components = IntegratedModules().SelectMany(module => module.Components!).ToArray();
        return [Compose("Continuity", "The operations team approves one release for field scheduling, replication, publication and settlement. Each producer has its own handbook and request catalog. The continuity handbook covers replay, staged activation and effects across operating windows.",
            [.. components, ContinuityWindows(), RecoveryJournal()])];
    }

    private static WideModule ContinuityWindows() => new("ContinuityWindows", "These are reconciliation queries over already admitted events. Ticks and amounts are nonnegative, intervals are ordered and half-open unless stated, and intermediate arithmetic fits in a signed 64-bit integer.",
    [
        Fault("RestoredAllowance", "long starting, long limit, long originalSpend, long refund, long refundTick, long movedSpend, long movedTick, long close",
            "Return allowance at close. Starting is the available amount after earlier adjustments, between zero and the current limit. Admit a partial refund on or before close, limited to the original transaction amount, which can exceed current headroom. A refund cannot take allowance over limit. Then admit another spend on or before close, limited to the available allowance after that refund.",
            """
            var recovered = refundTick <= close ? Math.Min(refund, originalSpend) : 0;
            var headroomAtClose = movedTick <= close ? Math.Min(movedSpend, starting + recovered) : 0;
            var journal = Math.Min(limit, starting + recovered - headroomAtClose);
            return journal;
            """,
            """
            var recovered = refundTick <= close ? Math.Min(refund, originalSpend) : 0;
            var available = Math.Min(limit, starting + recovered);
            return movedTick <= close ? available - Math.Min(movedSpend, available) : available;
            """,
            [20, 100, 15, 8, 4, 14, 6, 9], 14, [91, 100, 25, 20, 4, 12, 6, 9], 88),
        Fault("DispatchFrame", "long baseLeft, long baseRight, long liveLeft, long liveRight, long liveEpoch, long newBaseLeft, long newBaseRight, long newEpoch, long selectedEpoch",
            "A scheduled migration supplies a replacement base frame in a new epoch. Select the requested epoch, which is either the live epoch or the new epoch. For the live epoch report liveRight minus liveLeft. For the new epoch transport each live offset relative to its own base coordinate into the replacement frame, then report right minus left. Base coordinates may differ by unequal offsets across the migration.",
            """
            if (selectedEpoch == liveEpoch) return liveRight - liveLeft;
            if (selectedEpoch != newEpoch) return -1;
            var leftOffset = liveLeft - baseLeft;
            var rightOffset = liveRight - baseRight;
            var adjustment = newBaseLeft - baseLeft;
            return (baseRight + adjustment + rightOffset) - (newBaseLeft + leftOffset);
            """,
            """
            if (selectedEpoch == liveEpoch) return liveRight - liveLeft;
            if (selectedEpoch != newEpoch) return -1;
            var left = newBaseLeft + liveLeft - baseLeft;
            var right = newBaseRight + liveRight - baseRight;
            return right - left;
            """,
            [10, 20, 11, 23, 4, 14, 24, 5, 5], 12, [10, 20, 11, 23, 4, 14, 28, 5, 5], 16),
        Fault("RecordedIdle", "long start, long stop, long scheduledStart, long scheduledStop, long unplannedStart, long unplannedStop, long monitoredStart, long monitoredStop",
            "Count available ticks within the intersection of the shift and the actual monitoring window. Exclude all ticks in either outage, counting an overlapping tick just once. An empty monitored shift has zero available ticks.",
            """
            var low = Math.Max(start, monitoredStart);
            var high = Math.Min(stop, monitoredStop);
            if (low >= high) return 0;
            var scheduled = Overlap(low, high, scheduledStart, scheduledStop);
            var incident = Overlap(low, high, unplannedStart, unplannedStop);
            var shared = Overlap(start, stop, Math.Max(scheduledStart, unplannedStart), Math.Min(scheduledStop, unplannedStop));
            return high - low - scheduled - incident + shared;

            static long Overlap(long low, long high, long from, long to) => Math.Max(0, Math.Min(high, to) - Math.Max(low, from));
            """,
            """
            var low = Math.Max(start, monitoredStart);
            var high = Math.Min(stop, monitoredStop);
            if (low >= high) return 0;
            var scheduled = Overlap(low, high, scheduledStart, scheduledStop);
            var incident = Overlap(low, high, unplannedStart, unplannedStop);
            var shared = Overlap(low, high, Math.Max(scheduledStart, unplannedStart), Math.Min(scheduledStop, unplannedStop));
            return high - low - scheduled - incident + shared;

            static long Overlap(long low, long high, long from, long to) => Math.Max(0, Math.Min(high, to) - Math.Max(low, from));
            """,
            [0, 20, 2, 7, 4, 9, 1, 14], 6, [0, 20, 2, 11, 4, 13, 9, 17], 4),
        Fault("CoverageThrough", "long head, long through, long firstFrom, long firstTo, long secondFrom, long secondTo, long droppedFrom, long droppedTo",
            "A first and second receipt are ordered by start and may overlap. They mark acknowledged ticks. Coverage starts at head, stops at through, and cannot cross an unacknowledged tick. A nonempty dropped span revokes every receipt in that span. Report the first tick outside coverage. There is no coverage before head.",
            """
            var end = head;
            if (firstFrom <= end) end = Math.Max(end, firstTo);
            end = droppedFrom < droppedTo && end > droppedFrom && head < droppedTo ? Math.Max(head, droppedFrom) : end;
            if (secondFrom <= end) end = Math.Max(end, secondTo);
            return Math.Min(end, through);
            """,
            """
            var end = head;
            if (firstFrom <= end) end = Math.Max(end, firstTo);
            if (secondFrom <= end) end = Math.Max(end, secondTo);
            if (droppedFrom < droppedTo && end > droppedFrom && head < droppedTo)
                end = Math.Max(head, droppedFrom);
            return Math.Min(end, through);
            """,
            [2, 20, 1, 8, 5, 12, 14, 16], 12, [2, 20, 1, 10, 6, 14, 8, 9], 8),
        Fault("VisibleCredit", "long oldUnits, long oldPrice, long newUnits, long newPrice, long replacedAt, long issuedAt, long refundAt, long postedAt, long refundedUnits",
            "Calculate the visible value of a receipt as of postedAt. If issuedAt is after postedAt there is no receipt. Before replacement it has oldUnits at oldPrice and at or after replacement it has newUnits at newPrice. A return at refundAt removes refundedUnits at the unit price in force at refundAt. The return always relates to this receipt and refundedUnits fit both counts.",
            """
            if (issuedAt > postedAt) return 0;
            var current = replacedAt <= postedAt;
            var units = current ? newUnits : oldUnits;
            var price = current ? newPrice : oldPrice;
            var refund = refundAt <= postedAt ? refundedUnits * price : 0;
            return units * price - refund;
            """,
            """
            if (issuedAt > postedAt) return 0;
            var current = replacedAt <= postedAt;
            var units = current ? newUnits : oldUnits;
            var price = current ? newPrice : oldPrice;
            var refundPrice = replacedAt <= refundAt ? newPrice : oldPrice;
            return units * price - (refundAt <= postedAt ? refundedUnits * refundPrice : 0);
            """,
            [8, 6, 9, 7, 11, 2, 13, 15, 3], 42, [8, 6, 9, 7, 11, 2, 7, 15, 3], 45),
        Fault("CreditedAvailability", "long shiftFrom, long shiftTo, long handoff, long oldRate, long newRate, long waivedFrom, long waivedTo, long reportFrom, long reportTo",
            "Credit each tick shared by the shift and the report, excluding the waiver interval. The old rate applies before handoff and the new rate from handoff onward. Rate inputs are units per tick.",
            """
            var low = Math.Max(shiftFrom, reportFrom);
            var high = Math.Min(shiftTo, reportTo);
            if (low >= high) return 0;
            var split = Math.Clamp(handoff, low, high);
            var waived = Math.Max(0, Math.Min(high, waivedTo) - Math.Max(low, waivedFrom));
            var waiverRate = waivedFrom < handoff ? oldRate : newRate;
            return (split - low) * oldRate + (high - split) * newRate - waived * waiverRate;
            """,
            """
            var low = Math.Max(shiftFrom, reportFrom);
            var high = Math.Min(shiftTo, reportTo);
            if (low >= high) return 0;
            var split = Math.Clamp(handoff, low, high);
            var early = Math.Max(0, Math.Min(split, waivedTo) - Math.Max(low, waivedFrom));
            var late = Math.Max(0, Math.Min(high, waivedTo) - Math.Max(split, waivedFrom));
            return (split - low - early) * oldRate + (high - split - late) * newRate;
            """,
            [0, 20, 10, 2, 4, 4, 6, 1, 16], 38, [0, 20, 10, 2, 4, 8, 12, 1, 16], 30),
        Fault("ActiveCutoff", "long tradeDay, long daySpan, long normalCutoff, long extensionEnds, long extensionIssued, long cancellation, long received, long localOffset",
            "Translate received, extensionIssued and cancellation from absolute ticks into local ticks by adding localOffset. Select the day's ordinary absolute cutoff in local coordinates as tradeDay times daySpan plus normalCutoff. An extension to the day's extensionEnds is usable only if issued by the ordinary cutoff and no cancellation occurred at or before receipt. Cancellation zero denotes that no cancellation occurred. Return 1 for an on-time receipt, including the active cutoff.",
            """
            var ordinary = tradeDay * daySpan + normalCutoff;
            var arrived = received + localOffset;
            if (arrived <= ordinary) return 1;
            if (extensionIssued + localOffset > ordinary) return 0;
            if (cancellation != 0 && cancellation <= arrived) return 0;
            return arrived <= tradeDay * daySpan + extensionEnds ? 1 : 0;
            """,
            """
            var start = tradeDay * daySpan;
            var ordinary = start + normalCutoff;
            var arrived = received + localOffset;
            if (arrived <= ordinary) return 1;
            if (extensionIssued + localOffset > ordinary) return 0;
            if (cancellation != 0 && cancellation + localOffset <= arrived) return 0;
            return arrived <= start + extensionEnds ? 1 : 0;
            """,
            [2, 100, 65, 80, 255, 0, 272, 4], 1, [2, 100, 65, 90, 253, 275, 271, 8], 1),
        Fault("AliasRelease", "long reference, long successor, long newest, long firstTick, long secondTick, long releaseTick, long source, long target",
            "As of releaseTick, follow the reference-to-successor alias if its first event is active and the successor-to-newest alias if its second event is active. These are distinct keys and secondTick is no earlier than firstTick. Resolve both the source and target independently, even if both originate as different aliases. Return 1 if the two keys designate the same object.",
            """
            var route = source;
            var wanted = target;
            if (releaseTick >= firstTick)
            {
                if (route == reference) route = successor;
                if (wanted == reference) wanted = successor;
            }
            if (releaseTick >= secondTick)
            {
                if (route == successor) route = newest;
                if (target == successor) wanted = newest;
            }
            return route == wanted ? 1 : 0;
            """,
            """
            var route = Resolve(source);
            var wanted = Resolve(target);
            return route == wanted ? 1 : 0;

            long Resolve(long key)
            {
                if (releaseTick >= firstTick && key == reference) key = successor;
                if (releaseTick >= secondTick && key == successor) key = newest;
                return key;
            }
            """,
            [31, 52, 83, 2, 9, 7, 52, 31], 1, [31, 52, 83, 2, 9, 11, 83, 31], 1),
        Fault("AvailableInstallment", "long recorded, long requested, long forecast, long paid, long paidVersion, long requestVersion, long clearVersion, long hold",
            "Forecast is additional unsettled allowance. A provisional payment lowers the recorded allowance only while it is visible at requestVersion and its clearing is still in the future. The hold also reduces recorded allowance. The resulting recorded allowance has a floor of zero, then the forecast is added. Cap payment of the request at the resulting amount.",
            """
            var visiblePayment = paidVersion <= requestVersion && requestVersion < clearVersion ? paid : 0;
            var ready = Math.Max(0, recorded - hold) + forecast;
            var remaining = Math.Max(0, ready - visiblePayment);
            return Math.Min(requested, remaining);
            """,
            """
            var visiblePayment = paidVersion <= requestVersion && requestVersion < clearVersion ? paid : 0;
            var ready = Math.Max(0, recorded - hold - visiblePayment) + forecast;
            return Math.Min(requested, ready);
            """,
            [24, 32, 7, 4, 2, 3, 5, 3], 24, [12, 29, 20, 15, 2, 3, 5, 3], 20),
        Fault("LiveRemainder", "long supply, long oldOrder, long newOrder, long beforeUsage, long observedUsage, long cutover, long observation, long exactReceipt",
            "The observedUsage counter is cumulative through observation and includes beforeUsage accrued against the old order. Before cutover the old order is still in force. At and after cutover only the incremental usage belongs to the new order. exactReceipt is a separately booked consumption against the active order. Return the minimum of supply and the nonnegative remainder against that order.",
            """
            var activeOrder = observation >= cutover ? newOrder : oldOrder;
            var usage = observation >= cutover ? observedUsage - beforeUsage : observedUsage;
            var remaining = Math.Max(0, activeOrder - usage);
            return Math.Max(0, Math.Min(supply, remaining) - exactReceipt);
            """,
            """
            var activeOrder = observation >= cutover ? newOrder : oldOrder;
            var usage = observation >= cutover ? observedUsage - beforeUsage : observedUsage;
            return Math.Min(supply, Math.Max(0, activeOrder - usage - exactReceipt));
            """,
            [40, 30, 35, 12, 18, 5, 7, 5], 24, [10, 30, 35, 12, 18, 5, 7, 5], 10),
        Fault("StagedLots", "long older, long younger, long canceled, long lateArrival, long originTick, long issuedTick, long cancelTick, long arriveTick, long asOf",
            "At originTick register the net unreserved older units. At issuedTick register the younger units. At cancelTick restore canceled units from a prior reservation against the older lot, provided that lot had been registered. The reservation can exceed the initial unreserved balance. At arriveTick fulfill lateArrival against currently registered younger units first, then against older units. Unfulfilled units are discarded. Return the older unreserved balance as of asOf. Cancellation and dispatch can occur in either order, with distinct ticks.",
            """
            var old = originTick <= asOf ? older : 0;
            var young = issuedTick <= arriveTick ? younger : 0;
            var restore = originTick <= cancelTick && cancelTick <= asOf ? canceled : 0;
            if (arriveTick > asOf || arriveTick < originTick) return old + restore;
            return Math.Max(0, old + restore - Math.Max(0, lateArrival - young));
            """,
            """
            var old = originTick <= asOf ? older : 0;
            var young = issuedTick <= arriveTick ? younger : 0;
            var restore = originTick <= cancelTick && cancelTick <= asOf ? canceled : 0;
            if (arriveTick > asOf || arriveTick < originTick) return old + restore;
            var incrementAtDispatch = cancelTick < arriveTick ? restore : 0;
            return Math.Max(0, old + incrementAtDispatch - Math.Max(0, lateArrival - young))
                + restore - incrementAtDispatch;
            """,
            [8, 4, 2, 9, 1, 3, 4, 7, 9], 5, [4, 2, 5, 12, 1, 3, 8, 5, 9], 5),
        Fault("DeferredRefresh", "long original, long refreshed, long refreshAt, long issuedFor, long servedAt, long inspectedAt, long reusedAt, long revokedAt",
            "A reservation served at servedAt for issuedFor can be reused only when both are at or before reusedAt. A revocation at or before reusedAt invalidates reuse. Inspection occurs on or after servedAt. Report the active value as of the earlier of reusedAt and inspectedAt, taking refreshed when the refresh was already active by that tick. Return -1 for invalid reuse. Revocation zero denotes no revocation.",
            """
            if (issuedFor > reusedAt || servedAt > reusedAt || revokedAt != 0 && revokedAt <= reusedAt) return -1;
            return refreshAt <= reusedAt ? refreshed : original;
            """,
            """
            if (issuedFor > reusedAt || servedAt > reusedAt || revokedAt != 0 && revokedAt <= reusedAt) return -1;
            var cutoff = Math.Min(reusedAt, inspectedAt);
            return refreshAt <= cutoff ? refreshed : original;
            """,
            [12, 17, 4, 1, 3, 8, 9, 0], 17, [12, 17, 8, 1, 3, 7, 9, 0], 12),
        Valid("RetainedDispatch", "long posted, long closed, long retained, long acknowledged, long expires",
            "Return retained units if acknowledged is on or after posted and before both closure and expiry. All ticks are absolute.",
            "return acknowledged >= posted && acknowledged < Math.Min(closed, expires) ? retained : 0;", [1, 12, 23, 7, 9], 23),
        Valid("ProtectedSpare", "long capacity, long held, long demand, long holdFrom, long holdTo, long eventAt",
            "Cap demand at nonnegative available capacity. Held units are reserved during the half-open hold window only.",
            "var available = eventAt >= holdFrom && eventAt < holdTo ? capacity - held : capacity;\nreturn Math.Min(demand, Math.Max(0, available));",
            [24, 6, 28, 6, 12, 8], 18),
        Valid("OverlappedSurcharge", "long coverFrom, long coverTo, long slotFrom, long slotTo, long rate, long maximum",
            "Apply the rate only during the intersection of coverage and the booked slot, capped at maximum.",
            "var shared = Math.Max(0, Math.Min(coverTo, slotTo) - Math.Max(coverFrom, slotFrom));\nreturn Math.Min(maximum, shared * rate);",
            [2, 12, 7, 13, 5, 21], 21)
    ]);
}
