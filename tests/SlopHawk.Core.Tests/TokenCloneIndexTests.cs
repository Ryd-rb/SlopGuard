using SlopHawk.Core;
using Xunit;

namespace SlopHawk.IntegrationTests;

public sealed class TokenCloneIndexTests
{
    private static readonly CloneIndexSettings Small = new(MinimumTokens: 4, MaximumTokensPerBlock: 64);

    [Fact]
    public void ExactAndConsistentlyRenamedPairsHaveCanonicalLocationsAndCounts()
    {
        var index = new TokenCloneIndex(Small);
        index.Upsert(Document("beta", "rev1", Block("body", 10, Pattern("counter"))));
        index.Upsert(Document("gamma", "rev1", Block("body", 20, Pattern("value"))));
        index.Upsert(Document("alpha", "rev4", Block("body", 0, Pattern("counter"))));

        var query = index.Query(Uncapped());
        Assert.False(query.IsTruncated);
        Assert.Equal(3, query.Pairs.Count);
        Assert.Equal(TokenCloneIndex.RuleVersion, query.Pairs[0].NormalizationRuleVersion);
        Assert.All(query.Pairs, pair =>
        {
            Assert.Equal(1.0, pair.Score);
            Assert.Equal(6, pair.NormalizedMatchedTokens);
            Assert.Equal(6, pair.Left.TokenCount);
            Assert.Equal(6, pair.Right.TokenCount);
            Assert.True(string.CompareOrdinal(pair.Left.DocumentId, pair.Right.DocumentId) < 0);
        });

        var exact = Assert.Single(query.Pairs, pair => pair.MatchClass == CloneMatchClass.Exact);
        Assert.Equal(("alpha", "beta"), (exact.Left.DocumentId, exact.Right.DocumentId));
        Assert.Equal(("rev4", "rev1"), (exact.Left.DocumentVersion, exact.Right.DocumentVersion));
        Assert.Equal(new CloneSourceSpan(0, 12), exact.Left.Span);
        Assert.Equal(new CloneSourceSpan(10, 22), exact.Right.Span);
        Assert.Equal(6, exact.ExactMatchedTokens);

        var renamed = query.Pairs.Where(pair => pair.MatchClass == CloneMatchClass.RenamedLocalIdentifiers).ToArray();
        Assert.Equal(2, renamed.Length);
        Assert.All(renamed, pair => Assert.Equal(4, pair.ExactMatchedTokens));
        Assert.Equal(CloneMatchClass.Exact, query.Pairs[0].MatchClass);
        Assert.Equal(query.Pairs.Select(pair => pair.PairId).Distinct().Count(), query.Pairs.Count);
    }

    [Fact]
    public void NormalizationRetainsLiteralBoundNameAndOperatorDistinctions()
    {
        var index = new TokenCloneIndex(Small);
        index.Upsert(Document("source", "1", Block("body", 0, Pattern("a"))));
        index.Upsert(Document("renamed", "1", Block("body", 0, Pattern("b"))));
        index.Upsert(Document("literal", "1", Block("body", 0, Pattern("b", literal: "2"))));
        index.Upsert(Document("bound", "1", Block("body", 0, Pattern("b", bound: "OtherType"))));
        index.Upsert(Document("operator", "1", Block("body", 0,
            [Local("b"), Other("="), Literal("1"), Other(";"), Local("b"), Other("-")])));

        var pair = Assert.Single(index.Query(Uncapped()).Pairs);
        Assert.Equal(("renamed", "source"), (pair.Left.DocumentId, pair.Right.DocumentId));
        Assert.Equal(CloneMatchClass.RenamedLocalIdentifiers, pair.MatchClass);
    }

    [Fact]
    public void LocalIdentifierMappingPreservesEqualityPattern()
    {
        var index = new TokenCloneIndex(Small);
        index.Upsert(Document("base", "1", Block("m", 0,
            [Local("x"), Other("+"), Local("x"), Other("*"), Local("y")])));
        index.Upsert(Document("positive", "1", Block("m", 0,
            [Local("a"), Other("+"), Local("a"), Other("*"), Local("b")])));
        index.Upsert(Document("negative", "1", Block("m", 0,
            [Local("a"), Other("+"), Local("b"), Other("*"), Local("a")])));

        var pair = Assert.Single(index.Query(Uncapped()).Pairs);
        Assert.Equal(("base", "positive"), (pair.Left.DocumentId, pair.Right.DocumentId));
        Assert.Equal(2, pair.ExactMatchedTokens);
    }

    [Fact]
    public void DuplicateBlocksInSameDocumentAreSeparateEndpoints()
    {
        var index = new TokenCloneIndex(Small);
        index.Upsert(Document("folder/code.cs|target", "rev1",
            Block("method/1", 10, Pattern("id")),
            Block("method/2", 80, Pattern("id"))));

        var pair = Assert.Single(index.Query(Uncapped()).Pairs);
        Assert.Equal(pair.Left.DocumentId, pair.Right.DocumentId);
        Assert.Equal(("method/1", "method/2"), (pair.Left.BlockId, pair.Right.BlockId));
        Assert.NotEqual(pair.Left.Span, pair.Right.Span);
        Assert.StartsWith(TokenCloneIndex.RuleVersion, pair.PairId);
    }

    [Fact]
    public void ReplacementsAndDeletesRecomputeBothSidesWithoutStalePairs()
    {
        var index = new TokenCloneIndex(Small);
        var unchanged = Pattern("x").ToList();
        index.Upsert(Document("A", "v1", new CloneTokenBlock("one", new CloneSourceSpan(0, 9), unchanged)));
        index.Upsert(Document("B", "v1", Block("one", 10, Pattern("x"))));
        index.Upsert(Document("C", "v1", Block("one", 20, Pattern("x"))));
        Assert.Equal(3, index.Query(Uncapped()).Pairs.Count);

        // The index must not retain a caller's mutable token collection.
        unchanged[0] = Local("accidental-change");
        Assert.Equal(3, index.Query(Uncapped()).Pairs.Count);

        index.Upsert(Document("B", "v2", Block("one", 30, Pattern("x", "9"))));
        var afterEdit = Assert.Single(index.Query(Uncapped()).Pairs);
        Assert.Equal(("A", "C"), (afterEdit.Left.DocumentId, afterEdit.Right.DocumentId));
        Assert.Equal(new CloneSourceSpan(20, 32), afterEdit.Right.Span);

        index.Upsert(Document("A", "v2", Block("one", 40, Pattern("x", "9"))));
        var now = Assert.Single(index.Query(Uncapped()).Pairs);
        Assert.Equal(("A", "B"), (now.Left.DocumentId, now.Right.DocumentId));
        Assert.Equal("v2", now.Left.DocumentVersion);
        Assert.Equal(new CloneSourceSpan(30, 42), now.Right.Span);
        Assert.True(index.Delete("B"));
        Assert.Empty(index.Query(Uncapped()).Pairs);
        Assert.False(index.Delete("B"));
        index.Upsert(Document("C", "v2", Block("one", 50, Pattern("x", "9"))));
        Assert.Single(index.Query(Uncapped()).Pairs);
    }

    [Fact]
    public void InvalidReplacementIsRejectedWithoutLosingTheOldPostings()
    {
        var index = new TokenCloneIndex(Small);
        index.Upsert(Document("a", "v1", Block("m", 0, Pattern("id"))));
        index.Upsert(Document("b", "v1", Block("m", 0, Pattern("id"))));
        var before = Assert.Single(index.Query(Uncapped()).Pairs);

        Assert.Throws<ArgumentException>(() => index.Upsert(Document("a", "v2",
            Block("duplicate", 0, Pattern("id")), Block("duplicate", 10, Pattern("id")))));
        Assert.Throws<ArgumentException>(() => index.Upsert(Document("a", "v2",
            Block("bad", -1, Pattern("id")))));
        Assert.Equal(before, Assert.Single(index.Query(Uncapped()).Pairs));
    }

    [Fact]
    public void WarmAndColdResultsAgreeIndependentOfInsertionOrder()
    {
        var fixture = new[]
        {
            Document("c", "r3", Block("z", 0, Pattern("v"))),
            Document("a", "r1", Block("x", 0, Pattern("x")), Block("y", 30, Pattern("v"))),
            Document("b", "r4", Block("x", 0, Pattern("x", "2"))),
            Document("d", "r3", Block("z", 0, Pattern("value")))
        };
        var warm = new TokenCloneIndex(Small);
        foreach (var doc in fixture)
            warm.Upsert(doc);
        // Include a removal and new insertion to exercise reverse posting maintenance.
        warm.Upsert(Document("temporary", "r1", Block("x", 0, Pattern("v"))));
        warm.Delete("temporary");

        var cold = new TokenCloneIndex(Small);
        foreach (var doc in fixture.Reverse())
            cold.Upsert(doc);
        Assert.Equal(cold.Query(Uncapped()).Pairs.ToArray(), warm.Query(Uncapped()).Pairs.ToArray());
        Assert.Equal(cold.Query(Uncapped()).Gaps.ToArray(), warm.Query(Uncapped()).Gaps.ToArray());

        warm.Upsert(Document("b", "r5", Block("x", 70, Pattern("x"))));
        cold = new TokenCloneIndex(Small);
        foreach (var doc in fixture.Where(doc => doc.DocumentId != "b"))
            cold.Upsert(doc);
        cold.Upsert(Document("b", "r5", Block("x", 70, Pattern("x"))));
        Assert.Equal(cold.Query(Uncapped()).Pairs.ToArray(), warm.Query(Uncapped()).Pairs.ToArray());
    }

    [Fact]
    public void CandidateOutputAndTokenBudgetsExposeStablePairFrontiers()
    {
        var index = new TokenCloneIndex(Small);
        foreach (var name in new[] { "d", "a", "c", "b" })
            index.Upsert(Document(name, "1", Block("m", 0, Pattern("x"))));
        Assert.Equal(6, index.Query(Uncapped()).Pairs.Count);

        var candidate = index.Query(new CloneQueryLimits(MaximumCandidatePairs: 1));
        Assert.True(candidate.IsTruncated);
        Assert.Equal(CloneScopeGapReason.CandidatePairLimit, Assert.Single(candidate.Gaps).Reason);
        Assert.Equal(1, candidate.Statistics.CandidatePairsExamined);
        Assert.Single(candidate.Pairs);

        var output = index.Query(new CloneQueryLimits(MaximumOutputPairs: 1));
        Assert.Equal(CloneScopeGapReason.OutputPairLimit, Assert.Single(output.Gaps).Reason);
        Assert.Single(output.Pairs);

        var token = index.Query(new CloneQueryLimits(MaximumComparedTokens: 11));
        Assert.Empty(token.Pairs);
        Assert.Equal(0, token.Statistics.CandidatePairsExamined);
        Assert.Equal(CloneScopeGapReason.ComparedTokenLimit, Assert.Single(token.Gaps).Reason);
        Assert.Equal(12, token.Gaps[0].AffectedCount);
        Assert.Equal(candidate.Pairs[0].PairId, token.Gaps[0].ScopeKey);
        Assert.Equal(output.Pairs[0].PairId, candidate.Pairs[0].PairId);
    }

    [Fact]
    public void RepeatedBoilerplateAndOversizeInputAreExplicitScopeGaps()
    {
        var limited = new TokenCloneIndex(new CloneIndexSettings(
            MinimumTokens: 4, MaximumTokensPerBlock: 6, MaximumFingerprintPostings: 2));
        limited.Upsert(Document("a", "1", Block("m", 0, Pattern("x"))));
        limited.Upsert(Document("b", "1", Block("m", 0, Pattern("x"))));
        Assert.Single(limited.Query(Uncapped()).Pairs);
        limited.Upsert(Document("c", "1", Block("m", 0, Pattern("x"))));
        var query = limited.Query(Uncapped());
        Assert.Empty(query.Pairs);
        Assert.Equal(2, query.Gaps.Count(gap => gap.Reason == CloneScopeGapReason.BoilerplatePostingLimit));
        Assert.All(query.Gaps, gap => Assert.Equal(3, gap.AffectedCount));
        Assert.True(query.IsTruncated);

        limited.Upsert(Document("too-large", "1", Block("big", 0,
            [.. Pattern("x"), Other("additional")])));
        query = limited.Query(Uncapped());
        Assert.Contains(query.Gaps, gap => gap.Reason == CloneScopeGapReason.OversizedBlock &&
            gap.ScopeKey.Contains("too-large", StringComparison.Ordinal) && gap.AffectedCount == 7);
        Assert.Equal(1, query.Statistics.OversizedBlocks);
    }

    [Fact]
    public void GeneratedExcludedAndSmallBlocksStayOutOfCandidatePostings()
    {
        var index = new TokenCloneIndex(Small);
        index.Upsert(new CloneTokenDocument("generated", "1", true, false,
            [Block("m", 0, Pattern("x"))]));
        index.Upsert(new CloneTokenDocument("excluded", "1", false, true,
            [Block("m", 0, Pattern("x"))]));
        index.Upsert(Document("tiny", "1", Block("m", 0, [Local("x")])));
        index.Upsert(Document("first", "1", Block("m", 0, Pattern("x"))));
        var before = index.Query(Uncapped());
        Assert.Empty(before.Pairs);
        Assert.False(before.IsTruncated);
        Assert.Equal(1, before.Statistics.GeneratedDocuments);
        Assert.Equal(1, before.Statistics.ExcludedDocuments);
        Assert.Equal(1, before.Statistics.BelowMinimumBlocks);
        Assert.Equal(1, before.Statistics.IndexedBlockCount);

        index.Upsert(new CloneTokenDocument("generated", "2", false, false,
            [Block("m", 0, Pattern("x"))]));
        Assert.Single(index.Query(Uncapped()).Pairs);
    }

    private static CloneQueryLimits Uncapped() => new(int.MaxValue, int.MaxValue, long.MaxValue);

    private static CloneTokenDocument Document(string id, string version, params CloneTokenBlock[] blocks) =>
        new(id, version, false, false, blocks);

    private static CloneTokenBlock Block(string id, int start, params CloneToken[] tokens) =>
        new(id, new CloneSourceSpan(start, start + tokens.Length * 2), tokens);

    private static CloneToken Local(string name) => new(CloneTokenKind.LocalIdentifier, name);
    private static CloneToken Other(string text) => new(CloneTokenKind.Other, text);
    private static CloneToken Literal(string text) => new(CloneTokenKind.Literal, text);
    private static CloneToken Bound(string text) => new(CloneTokenKind.BoundIdentifier, text);

    private static CloneToken[] Pattern(string name, string literal = "1", string bound = "Console") =>
        [Local(name), Other("="), Literal(literal), Other(";"), Local(name), Bound(bound)];
}
