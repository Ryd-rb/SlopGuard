using SlopHawk.Core;
using Xunit;

namespace SlopHawk.Core.Tests;

public sealed class ManagedBodyComparisonTests
{
    [Fact]
    public void IdenticalManagedBodiesProduceNoChanges()
    {
        using var left = File.OpenRead(typeof(ManagedBodyComparison).Assembly.Location);
        using var right = File.OpenRead(typeof(ManagedBodyComparison).Assembly.Location);
        Assert.Empty(ManagedBodyComparison.Compare(left, right));
        Assert.True(left.CanRead);
        Assert.True(right.CanRead);
    }

    [Fact]
    public void DistinguishesMethodsPresentOnlyOnOneSide()
    {
        using var left = File.OpenRead(typeof(ManagedBodyComparison).Assembly.Location);
        using var right = File.OpenRead(typeof(ManagedBodyComparisonTests).Assembly.Location);
        var differences = ManagedBodyComparison.Compare(left, right);

        Assert.Contains(differences, row => row.Name == nameof(ProbeReplay.Plan) &&
            row.Change == "added" && row.Candidate.Count > 0 && row.Reference.Count == 0);
        Assert.Contains(differences, row => row.Name == nameof(IdenticalManagedBodiesProduceNoChanges) &&
            row.Change == "removed" && row.Reference.Count > 0 && row.Candidate.Count == 0);
    }
}
