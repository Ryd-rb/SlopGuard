partial class Catalog
{
    static IEnumerable<Op> Orders()
    {
        yield return new("plate_bundle_charge", "PackCutPlates", ["plates", "size", "rate", "tier"],
            "plates >= 1 && plates <= 20000 && size >= 2 && size <= 100 && rate >= 10 && rate <= 500 && tier >= 0 && tier <= 2",
            "bundles * rate + (tier == 2 && bundles >= 40 ? 180 : 0)",
            "Cut plates travel in complete charging bundles. Partial bundles are rounded up. Tier 2 adds 180 cents at forty charged bundles.",
            "var bundles = (plates + size - 1) / size; return bundles * rate + (tier == 2 && bundles >= 40 ? 180 : 0);",
            "var bundles = plates / size; if (plates % size != 0) bundles++; var surcharge = tier == 2 && bundles >= 40 ? 180 : 0; return rate * bundles + surcharge;",
            [["13", "5", "40", "0"], ["83", "4", "71", "1"]],
            [
                V("var bundles = (plates + size) / size; return bundles * rate + (tier == 2 && bundles >= 40 ? 180 : 0);", 72, 9, 40, 0),
                V("var bundles = (plates + size - 1) / size; return bundles * rate + (tier == 2 && bundles > 40 ? 180 : 0);", 795, 20, 55, 2),
                V("var bundles = (plates + size - 1) / size; var allowance = tier == 1 && plates >= 5100 && plates % size >= size - 2 ? 1 : 0; return (bundles - allowance) * rate + (tier == 2 && bundles >= 40 ? 180 : 0);", 5289, 23, 45, 1)
            ], new() { ["bundles"] = "(plates + size - 1) / size" });

        yield return new("coating_issue_grams", "DecantPowder", ["pieces", "dry", "carry", "humidity"],
            "pieces >= 1 && pieces <= 3000 && dry >= 2 && dry <= 800 && carry >= 0 && carry <= 350 && humidity >= 0 && humidity <= 98",
            "((pieces * dry + carry) * (100 + humidity) + 99) / 100",
            "Add setup carry to the total dry grams, apply the humidity percent, and issue the smallest whole gram count covering the result.",
            "var material = pieces * dry + carry; return (material * (100 + humidity) + 99) / 100;",
            "var scaled = (dry * pieces + carry) * (humidity + 100); return scaled / 100 + (scaled % 100 > 0 ? 1 : 0);",
            [["5", "20", "0", "10"], ["8", "25", "0", "25"]],
            [
                V("var material = pieces * dry + carry; return material * (100 + humidity) / 100;", 17, 21, 18, 14),
                V("var material = pieces * dry; return (material * (100 + humidity) + 99) / 100 + carry;", 100, 19, 270, 43),
                V("var material = pieces * dry + carry; var effective = humidity >= 83 && carry > 0 ? humidity - 1 : humidity; return (material * (100 + effective) + 99) / 100;", 31, 67, 1, 84)
            ]);

        yield return new("bend_sample_count", "OpenBrakeInspection", ["pieces", "profile", "critical", "shift"],
            "pieces >= 1 && pieces <= 4000 && profile >= 0 && profile <= 31 && critical >= 0 && critical <= 3 && shift >= 1 && shift <= 8",
            "1 + pieces / 120 + popcount(profile & 21) + (critical == 3 && shift >= 6 ? 2 : 0)",
            "Reserve one inspection, one per 120 pieces, and one for each of profile flags 1, 4, and 16. Critical class 3 on shift 6 or later adds two.",
            "return 1 + pieces / 120 + popcount(profile & 21) + (critical == 3 && shift >= 6 ? 2 : 0);",
            "var count = 1 + pieces / 120; foreach (var flag in new long[] { 1, 4, 16 }) if ((profile & flag) == flag) count++; if (critical == 3 && shift > 5) count += 2; return count;",
            [["15", "0", "0", "1"], ["273", "1", "2", "3"]],
            [
                V("return 1 + pieces / 120 + popcount(profile & 17) + (critical == 3 && shift >= 6 ? 2 : 0);", 271, 4, 0, 1),
                V("return 1 + pieces / 120 + popcount(profile & 21) + (critical == 3 && shift > 6 ? 2 : 0);", 75, 0, 3, 6),
                V("var configured = critical == 2 && shift == 8 ? profile ^ 16 : profile; return 1 + pieces / 120 + popcount(configured & 21) + (critical == 3 && shift >= 6 ? 2 : 0);", 95, 31, 2, 8)
            ]);

        yield return new("rework_stock_allowance", "ReserveTrimMaterial", ["length", "coil", "scrap", "reclaim"],
            "length >= 100 && length <= 250000 && coil >= 1 && coil <= 5000 && scrap >= 0 && scrap <= 400 && reclaim >= 0 && reclaim <= scrap",
            "max(0, (length * scrap + 9999) / 10000 - reclaim + (mod(coil, 17) == 0 ? 2 : 0))",
            "Reserve the rounded-up scrap basis-point share of the line length. Subtract the reclaim, add two for coils divisible by 17, and floor the result at zero.",
            "var allowance = (length * scrap + 9999) / 10000 - reclaim + (coil % 17 == 0 ? 2 : 0); return max(0, allowance);",
            "var scaled = scrap * length; long reserve = scaled / 10000; if (scaled % 10000 != 0) reserve++; if (coil % 17 == 0) reserve += 2; return Math.Max(reserve - reclaim, 0);",
            [["500", "3", "0", "0"], ["10000", "19", "100", "0"]],
            [
                V("var allowance = (length * scrap + 9999) / 10000 - reclaim + (coil % 17 == 1 ? 2 : 0); return max(0, allowance);", 10000, 34, 9, 0),
                V("var allowance = (length * scrap + 5000) / 10000 - reclaim + (coil % 17 == 0 ? 2 : 0); return max(0, allowance);", 501, 31, 3, 0),
                V("var allowance = (length * scrap + 9999) / 10000 - reclaim; return max(0, allowance) + (coil % 17 == 0 ? 2 : 0);", 100, 17, 200, 195)
            ]);

        yield return new("dispatch_lane_day", "ChooseOutboundDay", ["day", "minute", "lane", "sealedLoad"],
            "day >= 0 && day <= 6000 && minute >= 0 && minute <= 1439 && lane >= 0 && lane <= 2 && sealedLoad >= 0 && sealedLoad <= 1",
            "day + (minute >= cutoff ? 1 : 0) + (lane == 2 && sealedLoad == 0 ? 1 : 0)",
            "Count days from the planning epoch. Lane cutoffs are 1110, 960 and 840 minutes, respectively. Arrival at the cutoff rolls to the following day. An unsealed lane 2 load requires one additional day.",
            "var cutoff = lane == 0 ? 1110 : lane == 1 ? 960 : 840; return day + (minute >= cutoff ? 1 : 0) + (lane == 2 && sealedLoad == 0 ? 1 : 0);",
            "long[] cutoffByLane = [1110, 960, 840]; if (minute >= cutoffByLane[lane]) day++; if (lane == 2 && sealedLoad != 1) day++; return day;",
            [["507", "660", "0", "1"], ["1443", "1200", "1", "1"]],
            [
                V("var cutoff = lane == 0 ? 1110 : lane == 1 ? 960 : 840; return day + (minute > cutoff ? 1 : 0) + (lane == 2 && sealedLoad == 0 ? 1 : 0);", 581, 960, 1, 1),
                V("var cutoff = lane == 0 ? 1110 : lane == 1 ? 960 : 870; return day + (minute >= cutoff ? 1 : 0) + (lane == 2 && sealedLoad == 0 ? 1 : 0);", 83, 855, 2, 1),
                V("var cutoff = lane == 0 ? 1110 : lane == 1 ? 960 : 840; return day + (minute >= cutoff ? 1 : 0) + (lane == 2 && sealedLoad == 0 && minute < cutoff ? 1 : 0);", 4417, 1025, 2, 0)
            ], new() { ["cutoff"] = "lane == 0 ? 1110 : (lane == 1 ? 960 : 840)" });

        yield return new("kiln_ramp_minutes", "EstimateHeatUp", ["start", "finish", "pace", "soak"],
            "start >= -100 && start <= 900 && finish >= start && finish <= 1800 && pace >= 5 && pace <= 80 && soak >= 0 && soak <= 720",
            "((finish - start) * 10 + pace - 1) / pace + soak",
            "Temperature inputs are whole degrees, while ramp pace is measured in tenths of one degree per minute. Round ramp time up to whole minutes and add soak time.",
            "return ((finish - start) * 10 + pace - 1) / pace + soak;",
            "var tenths = 10 * (finish - start); if (tenths % pace != 0) soak++; return soak + tenths / pace;",
            [["200", "350", "10", "40"], ["350", "450", "20", "67"]],
            [
                V("return ((finish - start) * 10 + pace / 2) / pace + soak;", 100, 177, 67, 23),
                V("var reference = start >= 0 ? start : 0; return ((finish - reference) * 10 + pace - 1) / pace + soak;", -43, 782, 23, 65),
                V("var minimum = finish - start <= 3 && soak >= 390 ? 1 : 0; return max(minimum, ((finish - start) * 10 + pace - 1) / pace) + soak;", 881, 881, 73, 390)
            ]);

        yield return new("liner_trim_mm", "CutProtectiveLiner", ["width", "sides", "lip", "grade"],
            "width >= 20 && width <= 1500 && sides >= 1 && sides <= 8 && lip >= 0 && lip <= 50 && grade >= 0 && grade <= 4",
            "clamp(width - sides * lip + (grade == 4 ? 25 : 0), 12, 1200)",
            "Subtract the lip allowance once per side from the liner width. Grade 4 uses a 25 mm extension. Clamp the resulting cut to the fixture's 12 to 1,200 mm range.",
            "return clamp(width - sides * lip + (grade == 4 ? 25 : 0), 12, 1200);",
            "var cut = width - lip * sides; if (grade == 4) cut += 25; if (cut < 12) return 12; if (cut > 1200) return 1200; return cut;",
            [["500", "2", "0", "1"], ["840", "6", "0", "2"]],
            [
                V("return clamp(width - (sides - 1) * lip + (grade == 4 ? 25 : 0), 12, 1200);", 31, 8, 3, 4),
                V("return clamp(width - sides * lip, 12, 1200) + (grade == 4 ? 25 : 0);", 1380, 3, 44, 4),
                V("return clamp(width - sides * lip + (grade == 4 ? 25 : 0), grade == 2 && sides >= 7 ? 16 : 12, 1200);", 35, 7, 10, 2)
            ]);

        yield return new("weld_coupon_route", "RouteSampleTray", ["primaryFlags", "backupFlags", "workcell", "manual"],
            "primaryFlags >= 0 && primaryFlags <= 63 && backupFlags >= 0 && backupFlags <= 63 && workcell >= 0 && workcell <= 6 && manual >= 0 && manual <= 1",
            "workcell * 20 + popcount((primaryFlags ^ backupFlags) & 45) + (manual == 1 ? 10 : 0)",
            "Each workcell occupies a 20-position block. For flags 1, 4, 8 and 32, add one position per flag whose primary and backup settings disagree. Add ten positions for manual routing.",
            "return workcell * 20 + popcount((primaryFlags ^ backupFlags) & 45) + (manual == 1 ? 10 : 0);",
            "var route = workcell * 20 + manual * 10; for (var flag = 1; flag <= 32; flag *= 2) if ((45 & flag) != 0 && (primaryFlags & flag) != (backupFlags & flag)) route++; return route;",
            [["5", "0", "0", "0"], ["9", "32", "3", "1"]],
            [
                V("return workcell * 20 + popcount((primaryFlags | backupFlags) & 45) + (manual == 1 ? 10 : 0);", 55, 37, 3, 0),
                V("return workcell * 20 + popcount((primaryFlags ^ backupFlags) & (workcell == 6 ? 29 : 45)) + (manual == 1 ? 10 : 0);", 46, 0, 6, 0),
                V("var positions = popcount((primaryFlags ^ backupFlags) & 45); return workcell * 20 + positions + (manual == 1 && positions > 0 ? 10 : 0);", 63, 63, 3, 1)
            ]);

        yield return new("remnant_credit_cents", "ValueReusableOffcut", ["piece", "usable", "rate", "grade"],
            "piece >= 50 && piece <= 1000000 && usable >= 0 && usable <= piece && rate >= 1 && rate <= 9500 && grade >= 0 && grade <= 3",
            "usable * 10 >= piece * 7 && usable >= 45 ? (usable * rate * (10 - grade)) / 100000 : 0",
            "An offcut qualifies at a reusable area of at least 45 and at least 70 percent of the piece. Multiply the usable area by the rate and the grade factor (10 minus grade), then divide by 100,000 and round down.",
            "if (usable * 10 < piece * 7 || usable < 45) return 0; return usable * rate * (10 - grade) / 100000;",
            "if (usable < 45) return 0; if (10 * usable - 7 * piece < 0) return 0; var weightedArea = (10 - grade) * usable; return Math.DivRem(weightedArea * rate, 100000).Quotient;",
            [["900", "840", "711", "0"], ["6000", "1200", "48", "2"]],
            [
                V("if (usable * 10 <= piece * 7 || usable < 45) return 0; return usable * rate * (10 - grade) / 100000;", 1500, 1050, 130, 2),
                V("if (usable * 10 < piece * 7 || usable < 45) return 0; return usable * rate * (10 - (grade == 3 ? 2 : grade)) / 100000;", 7200, 6730, 240, 3),
                V("if (usable * 10 < piece * 7 || usable < 50) return 0; return usable * rate * (10 - grade) / 100000;", 60, 48, 7000, 1)
            ]);

        yield return new("carrier_rack_count", "StageCarrierFrames", ["upright", "flat", "stops", "slots"],
            "upright >= 0 && upright <= 1000 && flat >= 0 && flat <= 1000 && stops >= 0 && stops <= 12 && slots >= 4 && slots <= 70 && (stops == 0 || upright + flat >= stops)",
            "(upright + 2 * flat + stops + slots - 1) / slots",
            "One upright piece consumes one slot and one flat piece consumes two. Reserve one slot per stop. Charge the resulting count in full or partially occupied racks.",
            "var positions = upright + 2 * flat + stops; return (positions + slots - 1) / slots;",
            "var positions = flat + upright + stops + flat; var wholeRacks = Math.DivRem(positions, slots); return wholeRacks.Quotient + (wholeRacks.Remainder != 0 ? 1 : 0);",
            [["43", "0", "0", "10"], ["5", "1", "0", "8"]],
            [
                V("var positions = upright + 2 * flat + stops; return (positions + slots) / slots;", 8, 19, 4, 25),
                V("var positions = upright + 2 * flat + stops; return (positions + slots - 1) / slots + (upright > 0 && flat > 0 && stops >= 6 ? 1 : 0);", 20, 30, 8, 25),
                V("var positions = upright + 2 * flat + (flat != 0 ? stops : 0); return (positions + slots - 1) / slots;", 15, 0, 3, 5)
            ]);
    }
}
