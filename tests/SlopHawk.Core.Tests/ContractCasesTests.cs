using System.Text.Json;
using SlopHawk.Core;
using Xunit;

namespace SlopHawk.Core.Tests;

public sealed class ContractCasesTests
{
    [Fact]
    public void GeneratesOnlyLegalNewInputsAndDerivesResultsWithoutAnImplementation()
    {
        var rule = new IntegerContract("Window", ["first", "last", "period", "gate"],
            "period > 0 && first <= last && gate >= 0 && gate <= 1",
            "gate == 0 ? 0 : tail + 2 * span", "windows.md",
            new() { ["span"] = "last - first", ["tail"] = "mod(first, period)" });
        var known = Example("Window", [-2, 4, 5, 1], 15);
        var catalog = new ContractCases([rule], [known]);

        Assert.Null(Assert.Single(catalog.Qualification).Error);
        var cases = catalog.Generate(120, 11).ToArray();
        Assert.Equal(120, cases.Length);
        Assert.Equal(120, cases.Select(test => string.Join(',', test.Args)).Distinct().Count());
        Assert.Contains(cases, test => test.Args[0] < 0 && test.Args[3] == 1);
        foreach (var test in cases)
        {
            var (first, last, period, gate) = (test.Args[0], test.Args[1], test.Args[2], test.Args[3]);
            Assert.True(first <= last && period > 0 && gate is 0 or 1);
            Assert.False(test.Args.SequenceEqual(known.Args));
            Assert.Equal(gate == 0 ? 0 : (first % period + period) % period + 2 * (last - first),
                test.Expected.GetInt64());
        }
    }

    [Fact]
    public void QualifiesLazyExactArithmeticAndRejectsUnqualifiedOperations()
    {
        var lazy = new IntegerContract("Guarded", ["value", "slot"], "slot == 0 || value / slot >= 0",
            "slot == 0 ? (value * value) / value : popcount((value >> slot) & 255)", "guards.md");
        var wrong = new IntegerContract("Wrong", ["a"], "true", "a + 1", "guards.md");
        var missing = new IntegerContract("Unpublished", ["a"], "true", "a", "guards.md");
        var catalog = new ContractCases([lazy, wrong, missing],
            [Example("Guarded", [long.MaxValue, 0], long.MaxValue), Example("Wrong", [1], 1), Example("Unmodeled", [0], 0)]);

        Assert.Null(catalog.Qualification[0].Error);
        Assert.All(catalog.Qualification.Skip(1), item => Assert.NotNull(item.Error));
        Assert.All(catalog.Generate(25, 4), item => Assert.Equal("Guarded", item.Operation));
    }

    [Fact]
    public void GuidedSearchExercisesDeclaredDistantBoundaries()
    {
        var rule = new IntegerContract("Threshold", ["units"], "0 <= units && units <= 910",
            "units >= 811 ? 2 : 1", "threshold.md");
        var catalog = new ContractCases([rule], [Example("Threshold", [3], 1)]);
        var cases = catalog.Generate(40, 1, guideWithBoundaries: true).ToArray();

        Assert.Contains(cases, item => item.Args[0] == 910 && item.Expected.GetInt64() == 2);
        Assert.Contains(cases, item => item.Args[0] == 810 && item.Expected.GetInt64() == 1);
        Assert.Contains(cases, item => item.Args[0] == 811 && item.Expected.GetInt64() == 2);
        Assert.All(cases, item => Assert.InRange(item.Args[0], 0, 910));
    }

    private static ProbeExpectation Example(string operation, long[] args, long value) =>
        new(operation, args, JsonSerializer.SerializeToElement(value));
}
