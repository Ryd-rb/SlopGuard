using System.Text.Json;
using SlopHawk.Core;
using Xunit;

namespace SlopHawk.Core.Tests;

public sealed class ProbeReplayTests
{
    [Fact]
    public void ForegroundCheckKeepsEveryMismatchAndSeparatesUnsupportedCases()
    {
        var cases = JsonSerializer.Deserialize<ProbeExpectation[]>("""
            [{"Operation":"quota","Args":[1],"Expected":3},{"Operation":"quota","Args":[2],"Expected":4},
            {"Operation":"quota","Args":[3],"Expected":5},{"Operation":"status","Args":[],"Expected":{"ready":true}}]
            """)!;
        var actual = JsonSerializer.Deserialize<JsonElement[]>("""[2,1,null,{"ready":true}]""")!;

        var result = ProbeReplay.Check(cases, actual);

        Assert.Equal(4, result.Cases);
        Assert.Equal(3, result.Compared);
        Assert.Equal(1, result.Unsupported);
        Assert.Equal(new[] { 0, 1 }, result.Findings.Select(item => item.Index));
        Assert.Equal(4, result.Findings[1].Reference.GetInt64());
        Assert.Throws<ArgumentException>(() => ProbeReplay.Check(cases, actual[..3]));
        Assert.Throws<ArgumentException>(() => ProbeReplay.InputsForCheck([cases[0] with { Expected = default }]));
    }

    [Fact]
    public void PlanIncludesSeedsAndNearbyRelationsWithoutDuplicates()
    {
        ProbeInput[] seed = [new("fee", [11, 23]), new("fee", [11, 23]), new("revision", [long.MaxValue])];
        var generated = ProbeReplay.Plan(seed, 120).ToArray();
        Assert.Equal(JsonSerializer.Serialize(generated), JsonSerializer.Serialize(ProbeReplay.Plan(seed, 120)));
        Assert.Contains(generated, item => item.Operation == "fee" && item.Args.SequenceEqual(new long[] { 11, 23 }));
        Assert.Contains(generated, item => item.Operation == "fee" && item.Args.SequenceEqual(new long[] { 11, 12 }));
        Assert.Contains(generated, item => item.Operation == "revision" && item.Args.SequenceEqual(new[] { long.MaxValue - 1 }));
        Assert.All(generated.GroupBy(item => item.Operation), group =>
        {
            Assert.InRange(group.Count(), 1, 120);
            Assert.Equal(group.Count(), group.Select(item => string.Join(',', item.Args)).Distinct().Count());
        });
    }

    [Fact]
    public void PlanUsesDeclaredScalesForCompensatedChangesInEitherDirection()
    {
        ProbeInput[] seed = [new("sequence", [5, 71, 8])];
        var generated = ProbeReplay.Plan(seed, 100, scales: [37]).ToArray();
        Assert.Contains(generated, item => item.Args.SequenceEqual(new long[] { 4, 108, 8 }));
        Assert.Contains(generated, item => item.Args.SequenceEqual(new long[] { 5, 34, 9 }));
    }

    [Fact]
    public void PlanSharesTheOperationBudgetAcrossDifferentInputShapes()
    {
        ProbeInput[] seed = [new("history", []), new("history", [42]), new("history", [10, 2])];
        var generated = ProbeReplay.Plan(seed, 9).ToArray();
        Assert.Equal(9, generated.Length);
        Assert.Equal(new[] { 0, 1, 2 }, generated.Take(3).Select(item => item.Args.Length));
        Assert.Contains(generated.Skip(3), item => item.Args.Length == 2);
    }

    [Fact]
    public void CompareRecordsEvidenceWithoutTreatingUnsupportedAsAValue()
    {
        ProbeInput[] input = [new("fee", [2]), new("fee", [3]), new("fee", [4]), new("status", [])];
        var candidate = JsonSerializer.Deserialize<JsonElement[]>("""[{"cents":4},null,0,{"ready":true,"code":2}]""")!;
        var reference = JsonSerializer.Deserialize<JsonElement[]>("""[{"cents":5},null,null,{"code":2,"ready":true}]""")!;
        var result = ProbeReplay.Compare(input, candidate, reference);

        Assert.Equal(1, result[0].Compared);
        Assert.Equal(2, result[0].Unsupported);
        Assert.Equal(1, result[0].OneSided);
        Assert.Equal(1, result[0].Different);
        Assert.Equal(0, result[0].FirstDifference!.Index);
        Assert.Equal(4, result[0].FirstDifference!.Candidate.GetProperty("cents").GetInt64());
        Assert.Null(result[1].FirstDifference);
        Assert.Throws<ArgumentException>(() => ProbeReplay.Compare(input, candidate[..1], reference));
    }
}
