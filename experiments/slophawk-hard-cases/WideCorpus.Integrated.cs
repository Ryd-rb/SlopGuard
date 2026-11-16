internal static partial class WideCorpus
{
    internal static WideModule[] IntegratedModules()
    {
        var original = Modules().ToDictionary(module => module.Name, StringComparer.Ordinal);
        return
        [
            Compose("FieldService", "Coordinates calendar plans, maintenance workflows, route availability and live handoffs.",
                [original["Juniper"], original["Turnstile"], original["Rook"], FieldHandoffs()]),
            Compose("DataPlatform", "Accepts telemetry, synchronizes versioned documents and publishes committed replica state.",
                [original["Quorum"], original["Quill"], original["Switchyard"], StateHandoffs()]),
            Compose("ReportingHub", "Combines billing calculations, catalog transformations, and editorial publication into one release.",
                [original["Quarry"], original["Lattice"], original["Folio"], original["Council"], ReportHandoffs()])
        ];
    }

    private static WideModule Compose(string name, string introduction, WideModule[] sources) => new(name, introduction,
        sources.SelectMany(source => source.Operations.Select(operation => operation with { Source = source.Name })).ToArray(), sources);

    private static WideModule FieldHandoffs() => new("FieldHandoffs", "These operations reconcile the final reservation with the routing lease. Clock and depot fields are nonnegative.",
    [
        Fault("TransferBalance", "long room, long held, long source, long destination, long requestedDepot, long approved",
            "An approved move releases held units at source and reserves them at destination. A move within one depot leaves its room unchanged. An unapproved move changes nothing. Room is a signed before-move capacity balance.",
            "if (approved == 0) return room;\nif (requestedDepot == destination) return room - held;\nreturn requestedDepot == source ? room + held : room;",
            "if (approved == 0 || source == destination) return room;\nif (requestedDepot == destination) return room - held;\nreturn requestedDepot == source ? room + held : room;",
            [30, 4, 2, 3, 3, 1], 26, [30, 4, 3, 3, 3, 1], 30),
        Fault("PublishWindow", "long now, long requested, long leaseOpen, long leaseClose, long pauseOpen, long pauseClose, long due",
            "Pick the earliest tick no earlier than now, requested, or leaseOpen. If it lands in the half-open pause, defer to pauseClose. Return -1 unless the final tick is before leaseClose and at or before due. Intervals are ordered.",
            "var first = Math.Max(Math.Max(now, requested), leaseOpen);\nif (first >= leaseClose || first > due) return -1;\nreturn first >= pauseOpen && first < pauseClose ? pauseClose : first;",
            "var first = Math.Max(Math.Max(now, requested), leaseOpen);\nif (first >= pauseOpen && first < pauseClose) first = pauseClose;\nreturn first >= leaseClose || first > due ? -1 : first;",
            [7, 5, 3, 40, 20, 25, 35], 7, [7, 5, 3, 11, 6, 13, 15], -1)
    ]);

    private static WideModule StateHandoffs() => new("StateHandoffs", "A node can replay speculative updates from snapshots or combine independently measured windows. Versions and time inputs are nonnegative and fit signed arithmetic.",
    [
        Fault("SnapshotResult", "long originalVersion, long originalValue, long firstBase, long firstVersion, long firstDelta, long lastBase, long lastVersion, long lastDelta",
            "Start with originalValue. The first delta can apply only if it names the original version and advances it. The last delta can extend that first result by naming its resulting version and advancing it. It can also replace an earlier result if it names the original version and has a strictly higher version than the existing result. A replacement applies to the original value.",
            "var version = originalVersion;\nvar result = originalValue;\nif (firstBase == version && firstVersion > version) { version = firstVersion; result += firstDelta; }\nif (lastVersion > version && (lastBase == version || lastBase == originalVersion)) result += lastDelta;\nreturn result;",
            "var version = originalVersion;\nvar result = originalValue;\nif (firstBase == version && firstVersion > version) { version = firstVersion; result += firstDelta; }\nif (lastVersion > version && lastBase == version) return result + lastDelta;\nif (lastVersion > version && lastBase == originalVersion) return originalValue + lastDelta;\nreturn result;",
            [3, 100, 3, 4, 8, 4, 5, 12], 120, [3, 100, 3, 4, 8, 3, 5, 12], 112),
        Fault("AcknowledgedPrefix", "long floor, long ceiling, long firstStart, long firstEnd, long secondStart, long secondEnd, long prechecked",
            "Return the greatest acknowledged boundary reachable from floor after considering two unordered half-open spans. Initially prechecked is already acknowledged through its end, which lies between floor and ceiling. A span that starts before or at the current boundary may extend it. Cap the answer at ceiling.",
            "var end = prechecked;\nif (firstStart <= end) end = Math.Max(end, firstEnd);\nif (secondStart <= end) end = Math.Max(end, secondEnd);\nreturn Math.Clamp(end, floor, ceiling);",
            "var end = prechecked;\nif (firstStart <= end) end = Math.Max(end, firstEnd);\nif (secondStart <= end) end = Math.Max(end, secondEnd);\nif (firstStart <= end) end = Math.Max(end, firstEnd);\nreturn Math.Clamp(end, floor, ceiling);",
            [0, 20, 2, 8, 8, 12, 3], 12, [0, 20, 8, 12, 2, 8, 3], 12)
    ]);

    private static WideModule ReportHandoffs() => new("ReportHandoffs", "The publisher converts upstream acceptance and settlement facts into release counters before rendering reports.",
    [
        Fault("SharedSuccess", "long totalA, long passedA, long totalB, long passedB, long common, long commonPassed, long minimumPermille",
            "Return whether the distinct underlying items meet the passing ratio. Overlap is reported twice in upstream totals and pass counts. Remove common from the combined total and commonPassed from the combined passed count. A zero distinct total fails.",
            "var items = totalA + totalB - common;\nvar passed = passedA + passedB;\nreturn items > 0 && passed * 1000 >= items * minimumPermille ? 1 : 0;",
            "var items = totalA + totalB - common;\nvar passed = passedA + passedB - commonPassed;\nreturn items > 0 && passed * 1000 >= items * minimumPermille ? 1 : 0;",
            [10, 6, 10, 6, 0, 0, 500], 1, [10, 5, 10, 5, 5, 5, 500], 0),
        Fault("RecordCharge", "long priorAmount, long priorTax, long replacementAmount, long newTax, long amountAlreadyBooked, long taxAlreadyReversed, long priorVoided",
            "Return the signed adjustment against today's journal. Book the replacement amount and new tax, except for the replacement amount already booked. If the prior record is still live, reverse priorAmount and priorTax; otherwise the prior amount was reversed. In both cases, the specified part of priorTax was already reversed and the rest still needs reversal.",
            "var posting = replacementAmount + newTax - amountAlreadyBooked;\nif (priorVoided == 1) return posting;\nreturn posting - priorAmount - (priorTax - taxAlreadyReversed);",
            "var posting = replacementAmount + newTax - amountAlreadyBooked;\nvar oldPrincipal = priorVoided == 1 ? 0 : priorAmount;\nreturn posting - oldPrincipal - (priorTax - taxAlreadyReversed);",
            [40, 8, 60, 12, 10, 0, 0], 14, [40, 8, 60, 12, 10, 3, 1], 57)
    ]);
}
