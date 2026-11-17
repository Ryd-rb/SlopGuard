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

    [Fact]
    public void ImplementationConstantsGuideInputsWithoutChangingTheExpectedRule()
    {
        var rule = new IntegerContract(nameof(EncodeTuple), ["a", "b", "c", "d"],
            "min(min(a, b), min(c, d)) >= 0 && max(max(a, b), max(c, d)) <= 100",
            "a == c && b == d ? 0 : 1", "grouping.md");
        var catalog = new ContractCases([rule], [Example(nameof(EncodeTuple), [2, 4, 6, 8], 1)]);
        using var assembly = File.OpenRead(typeof(ContractCasesTests).Assembly.Location);
        var literals = ManagedBodyComparison.IntegerLiterals(assembly, new HashSet<string> { nameof(EncodeTuple) });
        Assert.Contains(31L, literals[nameof(EncodeTuple)]);

        var cases = catalog.Generate(128, 1, implementationLiterals: literals).ToArray();
        Assert.Contains(cases, item => item.Expected.GetInt64() == 1 &&
            EncodeTuple(item.Args[0], item.Args[1]) == EncodeTuple(item.Args[2], item.Args[3]));
    }

    [Fact]
    public void GuidedSearchCanEstablishLinkedEqualitiesInOneInput()
    {
        var rule = new IntegerContract("Shared", ["key1", "key2", "room1", "room2"],
            "key1 >= 0 && key2 >= 0 && room1 >= 0 && room2 >= 0 && (key1 != key2 || room1 == room2)",
            "key1 == key2 ? room1 : room1 + room2", "shared.md");
        var catalog = new ContractCases([rule], [Example("Shared", [400, 500, 89, 99], 188)]);
        var cases = catalog.Generate(24, 1, guideWithBoundaries: true).ToArray();
        Assert.Contains(cases, item => item.Args.SequenceEqual([400L, 400, 89, 89]) && item.Expected.GetInt64() == 89);
    }

    [Fact]
    public void GuidedSearchProjectsDerivedComparisonsTogether()
    {
        var rule = new IntegerContract("Combined", ["left", "padding", "total", "stride", "right", "gain"],
            "left >= 0 && padding >= 0 && total >= 0 && stride >= 0 && right >= 0 && gain >= 0",
            "sum == total && stride % 82 == 61 && right == gain * 7 ? 9 : 2", "combined.md",
            new() { ["sum"] = "left + padding" });
        var catalog = new ContractCases([rule], [Example("Combined", [52, 92, 1247, 422, 166, 97], 2)]);

        var cases = catalog.Generate(48, 1, guideWithBoundaries: true).ToArray();

        Assert.Contains(cases, item => item.Expected.GetInt64() == 9 && item.Args[0] + item.Args[1] == item.Args[2] &&
            item.Args[3] % 82 == 61 && item.Args[4] == item.Args[5] * 7);
        Assert.All(cases, item => Assert.All(item.Args, value => Assert.True(value >= 0)));
    }

    [Fact]
    public void ImplementationLiteralsCanComeFromAnUnmatchedNumericMethod()
    {
        const string operation = "published-route";
        using var assembly = File.OpenRead(typeof(ContractCasesTests).Assembly.Location);
        var operationArities = new Dictionary<string, int> { [operation] = 4 };

        var literals = ManagedBodyComparison.IntegerLiterals(assembly, operationArities);

        Assert.Contains(67L, literals[operation]);
        Assert.DoesNotContain(7949L, literals[operation]);
        var rule = new IntegerContract(operation, ["a", "b", "c", "d"],
            "min(min(a, b), min(c, d)) >= 0 && max(max(a, b), max(c, d)) <= 100",
            "a == c && b == d ? 0 : 1", "routes.md");
        var catalog = new ContractCases([rule], [Example(operation, [5, 9, 12, 20], 1)]);
        var cases = catalog.Generate(256, 1, implementationLiterals: literals);
        Assert.Contains(cases, item => item.Expected.GetInt64() == 1 &&
            OpaqueRoute(item.Args[0], item.Args[1], item.Args[2], item.Args[3]) == 0);
    }

    private static long OpaqueRoute(long first, long second, long third, long fourth) => first * 67 + second - (third * 67 + fourth);
    private static long UnrelatedRoute(long first, long second, long third) => first * 7949 + second - third;

    private static long EncodeTuple(long major, long minor)
    {
        long Key(long value) => 31 * value + minor;
        return Key(major);
    }

    private static ProbeExpectation Example(string operation, long[] args, long value) =>
        new(operation, args, JsonSerializer.SerializeToElement(value));
}
