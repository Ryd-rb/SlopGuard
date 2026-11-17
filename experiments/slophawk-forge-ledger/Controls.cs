partial class Catalog
{
    static IEnumerable<Op> Controls()
    {
        yield return new("staging_bin_index", "ChooseShopBin", ["row", "station", "offset", "bins"],
            "row >= 0 && row <= 3000 && station >= 0 && station <= 15 && offset >= -60 && offset <= 60 && bins >= 5 && bins <= 97",
            "mod(17 * row + station + offset, bins)",
            "Assign a staging bin by taking the nonnegative remainder of seventeen times the row plus station and signed offset.",
            "return mod(row * 17 + station + offset, bins);",
            "var slot = (row * 17 + offset + station) % bins; return slot < 0 ? slot + bins : slot;",
            [["0", "0", "-58", "31"], ["10", "1", "0", "21"], ["2999", "15", "60", "97"]], []);

        yield return new("gauge_zero_adjustment", "QuantizeGaugeDrift", ["reference", "reading", "step"],
            "reference >= -2000 && reference <= 2000 && reading >= -2000 && reading <= 2000 && step >= 1 && step <= 120",
            "reading >= reference ? (reading - reference + step / 2) / step : -((reference - reading + step / 2) / step)",
            "Return the difference from reference measured in whole gauge steps. Round to the nearest whole step. An exact tie is rounded away from zero.",
            "var delta = reading - reference; return delta >= 0 ? (delta + step / 2) / step : -((-delta + step / 2) / step);",
            "var delta = reading - reference; var result = Math.DivRem(Math.Abs(delta), step); var magnitude = result.Quotient + (2 * result.Remainder >= step ? 1 : 0); return Math.Sign(delta) * magnitude;",
            [["103", "125", "4"], ["241", "235", "4"], ["-117", "-114", "7"], ["-1200", "-1200", "3"]], []);

        yield return new("powder_tote_fill", "FillBatchContainers", ["mass", "linerMass", "capacity", "sealedFill"],
            "mass >= 0 && mass <= 1700000 && linerMass >= 0 && linerMass <= 500 && capacity >= linerMass + 100 && capacity <= 8000 && sealedFill >= 0 && sealedFill <= 1",
            "mass == 0 ? 0 : max(sealedFill + 1, (mass + capacity - linerMass - 1) / (capacity - linerMass))",
            "A tote's usable capacity is gross capacity minus liner mass. Issue enough whole totes to contain the mass. A nonzero sealed fill requires at least two totes.",
            "var payload = capacity - linerMass; return mass == 0 ? 0 : max(sealedFill + 1, (mass + payload - 1) / payload);",
            "if (mass == 0) return 0; var fill = Math.DivRem(mass, capacity - linerMass); var count = fill.Quotient + (fill.Remainder == 0 ? 0 : 1); return sealedFill == 1 && count == 1 ? 2 : count;",
            [["0", "10", "500", "1"], ["990", "10", "1000", "1"], ["2020", "60", "560", "0"]], []);

        yield return new("reader_rotation", "ShiftReaderAddress", ["current", "stride", "assignments", "readers"],
            "current >= 0 && current < readers && stride >= -15 && stride <= 15 && assignments >= 0 && assignments <= 100000 && readers >= 2 && readers <= 80",
            "mod(current + stride * assignments, readers)",
            "Advance the reader address by the signed stride for each assignment, wrapping into the range from zero through one below reader count.",
            "return mod(current + stride * assignments, readers);",
            "var remainder = stride * assignments % readers; var advance = current + remainder; if (advance >= readers) return advance - readers; if (advance < 0) return advance + readers; return advance;",
            [["11", "-9", "17", "20"], ["0", "3", "50", "10"], ["1", "-2", "100000", "2"]], []);

        yield return new("laser_bed_capacity", "NestFixtureRectangles", ["width", "height", "partWidth", "partHeight", "turn"],
            "width >= 10 && width <= 8000 && height >= 10 && height <= 8000 && partWidth >= 1 && partWidth <= 1000 && partHeight >= 1 && partHeight <= 1000 && turn >= 0 && turn <= 1",
            "turn == 0 ? (width / partWidth) * (height / partHeight) : max((width / partWidth) * (height / partHeight), (width / partHeight) * (height / partWidth))",
            "Count complete rectangles in a uniform grid with the supplied orientation. If turning is allowed, also count a grid using exclusively turned rectangles and retain the larger count.",
            "var straight = width / partWidth * (height / partHeight); return turn == 0 ? straight : max(straight, width / partHeight * (height / partWidth));",
            "var countX = width / partWidth; var countY = height / partHeight; long count = countX * countY; if (turn == 1) { var cross = (width / partHeight) * (height / partWidth); if (cross > count) count = cross; } return count;",
            [["130", "240", "145", "42", "1"], ["213", "777", "50", "251", "0"], ["10", "10", "11", "11", "1"]], []);

        yield return new("manifest_label_count", "QueueRequiredLabels", ["offered", "mandatory", "excluded", "copies"],
            "offered >= 0 && offered <= 1023 && mandatory >= 0 && mandatory <= 1023 && excluded >= 0 && excluded <= 1023 && copies >= 1 && copies <= 8 && (mandatory & excluded) == 0",
            "copies * popcount((offered | mandatory) & (1023 ^ excluded))",
            "Union offered and mandatory label flags, omit excluded flags, and print the requested number of copies for each remaining label. Valid manifests never exclude a mandatory label.",
            "return copies * popcount((offered | mandatory) & (1023 ^ excluded));",
            "long kinds = 0; for (long bit = 1; bit <= 512; bit <<= 1) { if (((offered & bit) != 0 || (mandatory & bit) != 0) && (excluded & bit) == 0) kinds++; } return kinds * copies;",
            [["21", "32", "13", "3"], ["1023", "256", "0", "8"], ["0", "512", "511", "1"]], []);
    }
}
