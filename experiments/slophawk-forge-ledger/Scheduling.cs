partial class Catalog
{
    static IEnumerable<Op> Scheduling()
    {
        yield return new("cold_hold_minutes", "CompareFreezerClocks", ["loadMinute", "clearMinute", "night", "purge"],
            "loadMinute >= 0 && loadMinute <= 1439 && clearMinute >= 0 && clearMinute <= 1439 && night >= 0 && night <= 1 && purge >= 0 && purge <= 90 && (night == 1 || clearMinute >= loadMinute)",
            "max(0, clearMinute - loadMinute + 1440 * night - purge)",
            "Count minutes from loading until clearance. A night crossing means clearance is on the next day. Exclude purge minutes and floor the result at zero.",
            "var elapsed = clearMinute + 1440 * night - loadMinute; return max(0, elapsed - purge);",
            "if (night == 1) clearMinute += 24 * 60; return clearMinute - loadMinute >= purge ? clearMinute - loadMinute - purge : 0;",
            [["610", "771", "0", "30"], ["1380", "90", "1", "0"]],
            [
                V("var elapsed = clearMinute + 1440 * (clearMinute < loadMinute ? 1 : 0) - loadMinute; return max(0, elapsed - purge);", 210, 314, 1, 27),
                V("var elapsed = clearMinute + 1440 * night - loadMinute; return elapsed - purge;", 19, 31, 0, 20),
                V("var elapsed = clearMinute + 1440 * night - loadMinute; return max(0, elapsed - (night == 1 ? 0 : purge));", 1401, 59, 1, 12)
            ]);

        yield return new("rush_priority_score", "RankExpeditedOrders", ["contract", "owed", "elapsed", "accountClass"],
            "contract >= 100 && contract <= 1000000000 && owed >= 0 && owed <= contract && elapsed >= 0 && elapsed <= 7200 && accountClass >= 0 && accountClass <= 3",
            "owed * 10000 / contract + elapsed / 30 + (accountClass == 3 ? 120 : 0)",
            "Score the owed portion of the contract in whole basis points, add one point per complete 30 minutes, and add 120 points for account class 3.",
            "var share = owed * 10000 / contract; return share + elapsed / 30 + (accountClass == 3 ? 120 : 0);",
            "var result = Math.DivRem(10000 * owed, contract).Quotient; result += (elapsed - elapsed % 30) / 30; if (accountClass == 3) result += 120; return result;",
            [["1000", "150", "90", "1"], ["5600", "280", "300", "2"]],
            [
                V("var share = unchecked((int)(owed * 10000)) / contract; return share + elapsed / 30 + (accountClass == 3 ? 120 : 0);", 735123000, 688195422, 1117, 2),
                V("var share = owed * 10000 / contract; return share + elapsed / 30 + (accountClass == 3 && owed != 0 ? 120 : 0);", 71555, 0, 841, 3),
                V("var share = contract >= 10000000 ? owed / (contract / 10000) : owed * 10000 / contract; return share + elapsed / 30 + (accountClass == 3 ? 120 : 0);", 10009999, 9501234, 901, 2)
            ]);

        yield return new("trace_stamp_slots", "IssueLotMarkers", ["lot", "station", "reel", "line", "overrideFlag"],
            "lot >= 90000 && lot <= 99999 && station >= 3 && station <= 44 && reel >= 0 && reel <= 63 && line >= 1 && line <= 12 && overrideFlag >= 0 && overrideFlag <= 1 && (mod(lot + reel, 9) == mod(station, 9) || overrideFlag == 1)",
            "2 + popcount(reel & 43) + (station == mod(lot, 47) && line >= 8 ? 3 : 0) + overrideFlag",
            "Issue two trace stamps and one for each set reel flag 1, 2, 8 and 32. On lines 8 through 12, issue three additional stamps when the lot number leaves the station number as its remainder on division by 47. Overrides require one extra stamp. The normal station and reel pairing follows the admitted trace-routing domain.",
            "var initial = 2 + popcount(reel & 43); return initial + (station == lot % 47 && line >= 8 ? 3 : 0) + overrideFlag;",
            "var count = overrideFlag + 2; foreach (long flag in new long[] { 1, 2, 8, 32 }) count += (reel & flag) != 0 ? 1 : 0; if (line > 7 && lot - lot / 47 * 47 == station) count += 3; return count;",
            [["95501", "12", "20", "2", "1"], ["95602", "25", "23", "9", "1"]],
            [
                V("var initial = 2 + popcount(reel & 43); return initial + (station == lot % 47 && line > 8 ? 3 : 0) + overrideFlag;", 90102, 3, 0, 8, 1),
                V("var initial = 2 + popcount(reel & 43); return initial + (station == lot % 47 && line >= 8 && overrideFlag == 1 ? 3 : 0) + overrideFlag;", 90106, 7, 9, 9, 0),
                V("var initial = 2 + popcount((reel >= 56 && overrideFlag == 1 && line == 12 ? reel ^ 8 : reel) & 43); return initial + (station == lot % 47 && line >= 8 ? 3 : 0) + overrideFlag;", 95591, 33, 58, 12, 1)
            ]);

        yield return new("cure_release_day", "PublishCureSchedule", ["batchDay", "baseCure", "recipe", "repeat"],
            "batchDay >= 0 && batchDay <= 8000 && baseCure >= 1 && baseCure <= 140 && recipe >= 0 && recipe <= 6 && repeat >= 0 && repeat <= 1 && (repeat == 0 || baseCure >= recipe + 1)",
            "batchDay + clamp(baseCure + recipe * 2 - (repeat == 1 ? 4 : 0), 3, 120)",
            "Start on the batch day. Add two cure days per recipe index and subtract four days for a repeat run. Schedule between 3 and 120 cure days inclusive.",
            "return batchDay + clamp(baseCure + recipe * 2 - (repeat == 1 ? 4 : 0), 3, 120);",
            "var cure = baseCure + recipe + recipe; if (repeat != 0) cure -= 4; if (cure < 3) cure = 3; else if (cure > 120) cure = 120; return batchDay + cure;",
            [["263", "50", "2", "0"], ["643", "35", "0", "0"]],
            [
                V("return batchDay + clamp(baseCure + recipe * 2, 3, 120) - (repeat == 1 ? 4 : 0);", 201, 2, 1, 1),
                V("return clamp(batchDay + baseCure + recipe * 2 - (repeat == 1 ? 4 : 0), batchDay + 3, batchDay + 120 - (recipe == 6 ? 1 : 0));", 210, 125, 6, 0),
                V("return batchDay + clamp(baseCure + recipe * 2 - (repeat == 1 && recipe != 4 ? 4 : 0), 3, 120);", 440, 12, 4, 1)
            ]);

        yield return new("shop_quote_cents", "PriceSetupAndLabor", ["setup", "rate", "minutes", "passThrough"],
            "setup >= 0 && setup <= 50000000 && rate >= 0 && rate <= 200000 && minutes >= 0 && minutes <= 10000 && passThrough >= 0 && passThrough <= 2000000 && (minutes != 0 || rate == 0)",
            "setup + (rate * minutes + 30) / 60 + passThrough",
            "Working time is billed at the cents-per-hour rate. Round the labor component to the nearest cent with half a cent rounded up. Add setup and pass-through charges in cents.",
            "return setup + (rate * minutes + 30) / 60 + passThrough;",
            "var labor = Math.DivRem(minutes * rate, 60); if (labor.Remainder < 30) return setup + labor.Quotient + passThrough; return setup + labor.Quotient + passThrough + 1;",
            [["1200", "2500", "60", "0"], ["1375", "3600", "23", "400"]],
            [
                V("return setup + (rate * minutes + 29) / 60 + passThrough;", 7433, 2415, 10, 38),
                V("var billable = minutes >= 9000 ? minutes - minutes % 5 : minutes; return setup + (rate * billable + 30) / 60 + passThrough;", 49100000, 190221, 9313, 45112),
                V("var labor = (rate * minutes + 30) / 60; return setup + labor + (setup == 0 && passThrough > 500000 ? 500000 : passThrough);", 0, 25700, 900, 754293)
            ]);

        yield return new("caster_bank_slots", "AllocateLoadWheelBays", ["load", "wheelLimit", "wheels", "frame", "paired"],
            "load >= 1 && load <= 48000 && wheelLimit >= 150 && wheelLimit <= 7000 && wheels >= 2 && wheels <= 16 && frame >= 0 && frame <= 3 && paired >= 0 && paired <= 1 && (paired == 0 || mod(wheels, 2) == 0) && load <= wheelLimit * wheels + 800",
            "load > wheelLimit * wheels + (frame == 3 && paired == 1 ? 300 : 0) ? 0 : (paired == 1 ? wheels / 2 : wheels) + (frame == 2 ? 2 : 0)",
            "Reject a load above the wheel capacity. A paired frame of class 3 extends capacity by 300. An accepted paired frame needs one bay per wheel pair. An accepted single frame needs one bay per wheel. Class 2 adds two bays.",
            "var capacity = wheelLimit * wheels + (frame == 3 && paired == 1 ? 300 : 0); if (load > capacity) return 0; return (paired == 1 ? wheels / 2 : wheels) + (frame == 2 ? 2 : 0);",
            "var wheelCapacity = wheels * wheelLimit; if (frame == 3 && paired == 1) wheelCapacity += 300; if (wheelCapacity < load) return 0; if (paired != 0) wheels /= 2; return frame == 2 ? wheels + 2 : wheels;",
            [["4431", "1101", "5", "1", "0"], ["4000", "1100", "4", "0", "1"]],
            [
                V("var capacity = wheelLimit * wheels + (frame == 3 && paired == 1 ? 300 : 0); if (load >= capacity) return 0; return (paired == 1 ? wheels / 2 : wheels) + (frame == 2 ? 2 : 0);", 4200, 700, 6, 0, 0),
                V("var capacity = wheelLimit * wheels + (frame == 3 || paired == 1 ? 300 : 0); if (load > capacity) return 0; return (paired == 1 ? wheels / 2 : wheels) + (frame == 2 ? 2 : 0);", 9883, 1211, 8, 1, 1),
                V("var capacity = wheelLimit * wheels + (frame == 3 && paired == 1 ? 300 : 0); if (load > capacity) return 0; return paired == 1 ? wheels / 2 : wheels + (frame == 2 ? 2 : 0);", 13040, 2263, 6, 2, 1)
            ]);

        yield return new("keyway_clearance_score", "ReviewKeyBlank", ["blank", "die", "index", "bothEnds"],
            "blank >= 0 && blank <= 255 && die >= 0 && die <= 255 && index >= 0 && index <= 3 && bothEnds >= 0 && bothEnds <= 1 && (bothEnds == 0 || (blank & 1) == (die & 1))",
            "popcount(((blank ^ die) >> index) & 15) + (bothEnds == 1 ? popcount(((blank ^ die) >> (4 - index)) & 15) : 0)",
            "Count mismatches across four consecutive keyway positions beginning at the index. If both ends are checked, also count four consecutive positions beginning at 4 minus the index. A position common to both windows is counted in both inspections.",
            "var differences = blank ^ die; return popcount((differences >> (int)index) & 15) + (bothEnds == 1 ? popcount((differences >> (int)(4 - index)) & 15) : 0);",
            "long count = 0; for (var offset = 0; offset < 4; offset++) { var near = 1L << (int)(index + offset); if ((blank & near) != (die & near)) count++; if (bothEnds == 1) { var far = 1L << (int)(4 - index + offset); if ((blank & far) != (die & far)) count++; } } return count;",
            [["211", "71", "2", "0"], ["16", "16", "2", "1"]],
            [
                V("var differences = blank ^ die; return popcount((differences >> (int)index) & 15) + (bothEnds == 1 ? popcount((differences >> (int)(3 - index)) & 15) : 0);", 64, 0, 1, 1),
                V("var differences = blank ^ die; var near = ((differences >> (int)index) & 15); if (bothEnds == 0) return popcount(near); var mask = (15L << (int)index) | (15L << (int)(4 - index)); return popcount(differences & mask);", 70, 8, 2, 1),
                V("var differences = blank ^ die; return popcount((differences >> (int)index) & (index == 3 ? 7 : 15)) + (bothEnds == 1 ? popcount((differences >> (int)(4 - index)) & 15) : 0);", 65, 1, 3, 0)
            ]);

        yield return new("stock_projection_trays", "ProjectStagingBalance", ["opening", "receipts", "issues", "traySize", "returns"],
            "opening >= 0 && opening <= 50000 && receipts >= 0 && receipts <= 50000 && issues >= 0 && issues <= 120000 && traySize >= 2 && traySize <= 140 && returns >= 0 && returns <= 250 && returns <= issues",
            "net >= 0 ? net / traySize : -((-net + traySize - 1) / traySize)",
            "Add receipts and returns to opening items, then subtract issues. Express the balance in trays and round toward negative infinity. A shortage smaller than one tray is one tray short.",
            "var net = opening + receipts + returns - issues; return net >= 0 ? net / traySize : -((-net + traySize - 1) / traySize);",
            "var (full, partial) = Math.DivRem(opening + receipts - issues + returns, traySize); return partial < 0 ? full - 1 : full;",
            [["560", "90", "511", "20", "0"], ["0", "100", "140", "20", "0"]],
            [
                V("var net = opening + receipts + returns - issues; return net / traySize;", 83, 13, 111, 25, 2),
                V("var net = opening + receipts - issues; var trays = net >= 0 ? net / traySize : -((-net + traySize - 1) / traySize); return trays + returns / traySize;", 94, 11, 107, 25, 2),
                V("var net = opening + receipts + returns - issues; return net >= 0 ? net / traySize : -min(100, (-net + traySize - 1) / traySize);", 913, 119, 9300, 8, 19)
            ], new() { ["net"] = "opening + receipts + returns - issues" });

        yield return new("freight_yard_surcharge", "ComputeYardInvoice", ["entered", "exited", "peakStart", "peakEnd", "rate", "permit"],
            "entered >= 0 && entered <= 1439 && exited >= entered && exited <= 1440 && peakStart >= 0 && peakStart <= 1439 && peakEnd >= peakStart && peakEnd <= 1440 && rate >= 1 && rate <= 850 && permit >= 0 && permit <= 1",
            "max(0, min(exited, peakEnd) - max(entered, peakStart) - 15 * permit) * rate",
            "Compute the overlap in minutes between the stay and the peak window. A permit discounts the first 15 overlapping minutes. Invoice the rest at the peak rate per minute.",
            "var overlap = min(exited, peakEnd) - max(entered, peakStart); return max(0, overlap - 15 * permit) * rate;",
            "if (entered < peakStart) entered = peakStart; if (exited > peakEnd) exited = peakEnd; var minutes = exited - entered; if (permit == 1) minutes -= 15; return minutes > 0 ? minutes * rate : 0;",
            [["420", "540", "600", "720", "14", "0"], ["585", "720", "585", "735", "90", "0"]],
            [
                V("var overlap = min(exited, peakEnd) - max(entered, peakStart); return max(0, overlap + (overlap >= 0 && entered != peakStart ? 1 : 0) - 15 * permit) * rate;", 400, 500, 500, 600, 110, 0),
                V("var overlap = min(exited, peakEnd) - max(entered, peakStart); return max(0, overlap) * rate - 15 * permit;", 810, 890, 815, 881, 52, 1),
                V("var overlap = min(exited, peakEnd) - max(entered, peakStart); var protectedMinutes = permit == 1 && entered >= peakStart ? 15 : 0; return max(0, overlap - protectedMinutes) * rate;", 500, 650, 555, 725, 89, 1)
            ]);

        yield return new("seam_alignment_offset", "AlignPeriodicJig", ["datum", "seam", "pitch", "tolerance", "reverse"],
            "datum >= -20000 && datum <= 20000 && seam >= -20000 && seam <= 20000 && pitch >= 4 && pitch <= 400 && tolerance >= 0 && tolerance <= pitch / 2 && reverse >= 0 && reverse <= 1",
            "min(phase, pitch - phase) <= tolerance ? 0 : (reverse == 1 ? mod(-phase, pitch) : phase)",
            "The seam phase is the nonnegative remainder of seam minus datum divided by pitch. Ignore the offset within tolerance of a registration point in either direction. Otherwise return the forward phase, or the phase in the opposite direction when the jig is reversed.",
            "var phase = mod(seam - datum, pitch); if (min(phase, pitch - phase) <= tolerance) return 0; return reverse == 1 ? mod(-phase, pitch) : phase;",
            "var phase = (seam - datum) % pitch; if (phase < 0) phase += pitch; if (phase <= tolerance || phase >= pitch - tolerance) return 0; if (reverse == 1) return pitch - phase; return phase;",
            [["2", "8", "10", "2", "0"], ["20", "157", "15", "1", "1"]],
            [
                V("var phase = (seam - datum) % pitch; if (min(abs(phase), pitch - abs(phase)) <= tolerance) return 0; return reverse == 1 ? mod(-phase, pitch) : phase;", 125, 8, 64, 1, 0),
                V("var phase = mod(seam - datum, pitch); if (min(phase, pitch - phase) < tolerance) return 0; return reverse == 1 ? mod(-phase, pitch) : phase;", 410, 471, 32, 3, 0),
                V("var phase = mod(seam - datum, pitch); if (min(phase, pitch - phase) <= tolerance) return 0; return reverse == 1 && phase < pitch / 2 ? mod(-phase, pitch) : phase;", 410, 430, 24, 2, 1)
            ], new() { ["phase"] = "mod(seam - datum, pitch)" });
    }
}
