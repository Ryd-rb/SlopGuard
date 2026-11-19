using System.Security.Cryptography;
using System.Text;
using System.Text.Json;
using Microsoft.CodeAnalysis;
using Microsoft.CodeAnalysis.CSharp;
using Microsoft.CodeAnalysis.CSharp.Syntax;
using Microsoft.CodeAnalysis.Text;
using SlopHawk.Providers.CSharp;
using Xunit;

namespace SlopHawk.IntegrationTests;

// Draft regression cases. This packet does not execute compilation APIs or tests.
public sealed class CSharpResourceLifetimeTests
{
    [Theory]
    [InlineData("stream.Dispose();", "stream.ReadByte()")]
    [InlineData("((IDisposable)stream).Dispose();", "((Stream)stream).ReadByte()")]
    [InlineData("IDisposable owner = stream; owner.Dispose();", "stream.ReadByte()")]
    public void ExactAllocationSupportsDirectAndInterfaceDisposal(string disposal, string read)
    {
        var body = $"var stream = new MemoryStream(); {disposal} return {read};";
        var result = Read(body);
        var use = Assert.Single(result.Uses);
        Assert.Equal(ResourceLifetimeStatus.Analyzed, result.Status);
        Assert.True(result.Complete);
        Assert.Equal(ResourceLifetimeUseState.Disposed, use.State);
        Assert.NotNull(use.Allocation);
        Assert.Single(use.Disposals);
        Assert.Equal(Source(body).IndexOf("new MemoryStream()", StringComparison.Ordinal),
            use.Allocation!.Start);
        Assert.Equal(Source(body).LastIndexOf(read, StringComparison.Ordinal), use.Span.Start);
    }

    [Fact]
    public void DisposingAnAliasIntroducesARegressionAtTheOriginalLocal()
    {
        var before = Read("var stream = new MemoryStream(); var alias = stream; int value = stream.ReadByte(); alias.Dispose(); return value;");
        var after = Read("var stream = new MemoryStream(); var alias = stream; alias.Dispose(); int value = stream.ReadByte(); return value;",
            revision: "after");
        var comparison = CSharpResourceLifetime.Compare(before, after, [Match(before, after)]);
        var change = Assert.Single(comparison.Changes);
        Assert.True(comparison.Complete);
        Assert.Equal(ResourceLifetimeChangeKind.Introduced, change.Kind);
        Assert.Equal(ResourceLifetimeUseState.Live, change.Before!.State);
        Assert.Equal(ResourceLifetimeUseState.Disposed, change.After.State);
        Assert.Single(change.After.Disposals);
    }

    [Fact]
    public void DisposalOnOnlyOneBranchRemainsUnknownAtTheJoin()
    {
        var result = Read("var stream = new MemoryStream(); if (choose) stream.Dispose(); return stream.ReadByte();");
        var use = Assert.Single(result.Uses);
        Assert.Equal(ResourceLifetimeStatus.Analyzed, result.Status);
        Assert.False(result.Complete);
        Assert.Equal(ResourceLifetimeUseState.Unknown, use.State);
        Assert.Single(use.Disposals);
        Assert.False(string.IsNullOrWhiteSpace(use.Reason));
    }

    [Fact]
    public void DisposalOnBothBranchesRetainsBothWitnesses()
    {
        var result = Read("var stream = new MemoryStream(); if (choose) stream.Dispose(); else stream.Dispose(); return stream.ReadByte();");
        var use = Assert.Single(result.Uses);
        Assert.Equal(ResourceLifetimeUseState.Disposed, use.State);
        Assert.True(result.Complete);
        Assert.Equal(2, use.Disposals.Count);
        Assert.True(use.Disposals[0].Start < use.Disposals[1].Start);
    }

    [Theory]
    [InlineData("stream", ResourceLifetimeUseState.Live)]
    [InlineData("alias", ResourceLifetimeUseState.Disposed)]
    public void ReassignmentChangesOneBindingAndPreservesTheOldAlias(
        string receiver, ResourceLifetimeUseState expected)
    {
        var body = "var stream = new MemoryStream(); var alias = stream; stream.Dispose(); " +
                   $"stream = new MemoryStream(); return {receiver}.ReadByte();";
        var result = Read(body);
        var use = Assert.Single(result.Uses);
        Assert.Equal(expected, use.State);
        Assert.True(result.Complete);
        var source = Source(body);
        var allocation = expected == ResourceLifetimeUseState.Live
            ? source.LastIndexOf("new MemoryStream()", StringComparison.Ordinal)
            : source.IndexOf("new MemoryStream()", StringComparison.Ordinal);
        Assert.Equal(allocation, use.Allocation!.Start);
        Assert.Equal(expected == ResourceLifetimeUseState.Live ? 0 : 1, use.Disposals.Count);
    }

    [Fact]
    public void ToArrayIsValidAfterDisposalAndDoesNotEraseTheReadByteContract()
    {
        var safe = Read("var stream = new MemoryStream(); stream.Dispose(); stream.ToArray(); return 0;");
        Assert.Equal(ResourceLifetimeStatus.Analyzed, safe.Status);
        Assert.True(safe.Complete);
        Assert.Empty(safe.Uses);

        var unsafeRead = Read("var stream = new MemoryStream(); stream.Dispose(); stream.ToArray(); return stream.ReadByte();");
        Assert.Equal(ResourceLifetimeUseState.Disposed, Assert.Single(unsafeRead.Uses).State);
    }

    [Fact]
    public void ADefiniteDisposedReadHasNoLaterNormalContinuation()
    {
        var body = "var stream = new MemoryStream(); stream.Dispose(); stream.ReadByte(); return stream.ReadByte();";
        var result = Read(body);
        var use = Assert.Single(result.Uses);
        Assert.Equal(ResourceLifetimeUseState.Disposed, use.State);
        Assert.Equal(Source(body).IndexOf("stream.ReadByte()", StringComparison.Ordinal), use.Span.Start);
    }

    [Fact]
    public void AThrowingBranchDoesNotPolluteTheSurvivingLivePath()
    {
        var result = Read("var stream = new MemoryStream(); if (choose) { stream.Dispose(); stream.ReadByte(); } return stream.ReadByte();");
        Assert.True(result.Complete);
        Assert.Equal(new[] { ResourceLifetimeUseState.Disposed, ResourceLifetimeUseState.Live },
            result.Uses.Select(use => use.State).ToArray());
        Assert.Single(result.Uses[0].Disposals);
        Assert.Empty(result.Uses[1].Disposals);
    }

    [Fact]
    public void AParameterHasNoProvenAllocationIdentity()
    {
        var result = Read("return input.ReadByte();");
        var use = Assert.Single(result.Uses);
        Assert.Equal(ResourceLifetimeUseState.Unknown, use.State);
        Assert.Null(use.Allocation);
        Assert.Empty(use.Disposals);
        Assert.False(result.Complete);
        Assert.False(string.IsNullOrWhiteSpace(use.Reason));

        AssertUnsupported(Read("input.Dispose(); return input.ReadByte();"));
    }

    [Theory]
    [InlineData("var stream = new MemoryStream(); Keep(stream); stream.Dispose(); return stream.ReadByte();")]
    [InlineData("var stream = new MemoryStream(); Replace(ref stream); stream.Dispose(); return stream.ReadByte();")]
    [InlineData("var stream = new MemoryStream(); Saved = stream; stream.Dispose(); return stream.ReadByte();")]
    [InlineData("var stream = new MemoryStream(); var values = new MemoryStream[] { stream }; stream.Dispose(); return stream.ReadByte();")]
    [InlineData("var stream = new MemoryStream(); stream.Flush(); stream.Dispose(); return stream.ReadByte();")]
    public void EscapeRefStorageAndUnmodeledVirtualCallsInvalidateTheMethod(string body)
    {
        AssertUnsupported(Read(body, members: """
            static MemoryStream? Saved;
            static void Keep(object value) { }
            static void Replace(ref MemoryStream value) { value = new MemoryStream(); }
            """));
    }

    [Fact]
    public void UnrelatedTypesWithTheSameMethodNamesDoNotProduceLifetimeEvidence()
    {
        var result = Read("var other = new Other(); other.Dispose(); return other.ReadByte();", members: """
            sealed class Other
            {
                public void Dispose() { }
                public int ReadByte() => 1;
            }
            """);
        AssertUnsupported(result);
    }

    [Theory]
    [InlineData("using var stream = new MemoryStream(); return stream.ReadByte();")]
    [InlineData("var stream = new MemoryStream(); try { stream.Dispose(); } finally { } return stream.ReadByte();")]
    [InlineData("var stream = new MemoryStream(); while (choose) { stream.Dispose(); break; } return stream.ReadByte();")]
    [InlineData("var stream = new MemoryStream(); Action dispose = () => stream.Dispose(); return stream.ReadByte();")]
    [InlineData("var stream = new MemoryStream(); void Later() { stream.Dispose(); } return stream.ReadByte();")]
    [InlineData("Func<int> read = input.ReadByte; return read();")]
    public void ImplicitDisposalLoopsAndDeferredBodiesAreVisibleUnsupportedBoundaries(string body)
    {
        AssertUnsupported(Read(body));
    }

    [Fact]
    public void AsyncDisposalIsAnExplicitUnsupportedBoundary()
    {
        AssertUnsupported(Read("await using var stream = new MemoryStream(); return stream.ReadByte();", isAsync: true));
    }

    [Theory]
    [InlineData("var stream = new MemoryStream(); if (choose) { stream.Dispose(); if (!choose) return stream.ReadByte(); } return 0;")]
    [InlineData("int zero = 0; int value = 1 / zero; var stream = new MemoryStream(); stream.Dispose(); return stream.ReadByte();")]
    [InlineData("int value = 2147483647; value = checked(value + 1); var stream = new MemoryStream(); stream.Dispose(); return stream.ReadByte();")]
    [InlineData("object value = \"not a stream\"; var cast = (IDisposable)value; var stream = new MemoryStream(); stream.Dispose(); return stream.ReadByte();")]
    public void CorrelatedPredicatesAndPotentiallyThrowingPrefixesRemainUnsupported(string body)
    {
        AssertUnsupported(Read(body));
    }

    [Theory]
    [InlineData(ResourceLifetimeSourceKind.Generated)]
    [InlineData(ResourceLifetimeSourceKind.Unknown)]
    public void GeneratedOrUnknownProvenanceDoesNotClaimACompletedAnalysis(ResourceLifetimeSourceKind sourceKind)
    {
        var result = Read("var stream = new MemoryStream(); stream.Dispose(); return stream.ReadByte();", sourceKind: sourceKind);
        Assert.Equal(ResourceLifetimeStatus.IncompleteInput, result.Status);
        Assert.False(result.Complete);
        Assert.Empty(result.Uses);
        Assert.False(string.IsNullOrWhiteSpace(result.Reason));
    }

    [Theory]
    [InlineData(false, true)]
    [InlineData(true, false)]
    public void MissingInputClosureCannotBecomeANegativeResult(bool sourcesComplete, bool referencesComplete)
    {
        var result = Read("return 0;", sourceClosure: sourcesComplete, referenceClosure: referencesComplete);
        Assert.Equal(ResourceLifetimeStatus.IncompleteInput, result.Status);
        Assert.False(result.Complete);
        Assert.Empty(result.Uses);
    }

    [Fact]
    public void CallerReportedCompilerErrorsKeepTheScopeIncomplete()
    {
        var result = Read("return Missing;", compilerErrorsAbsent: false, expectCompilerErrors: true);
        Assert.Equal(ResourceLifetimeStatus.IncompleteInput, result.Status);
        Assert.False(result.Complete);
        Assert.Empty(result.Uses);
    }

    [Fact]
    public void UncertifiedErrorFreedomCannotSupplyALiveBaselineEvenForValidSource()
    {
        var before = Read("var stream = new MemoryStream(); return stream.ReadByte();", compilerErrorsAbsent: false);
        var after = Read("var stream = new MemoryStream(); stream.Dispose(); return stream.ReadByte();", revision: "after");
        Assert.Equal(ResourceLifetimeStatus.IncompleteInput, before.Status);
        var comparison = CSharpResourceLifetime.Compare(before, after, []);
        Assert.False(comparison.Complete);
        Assert.False(string.IsNullOrWhiteSpace(comparison.Reason));
        Assert.Empty(comparison.Changes);
    }

    [Fact]
    public void UnsupportedSiblingPathWithdrawsAnEarlierDisposalWitness()
    {
        var result = Read("""
            var stream = new MemoryStream();
            if (choose) { stream.Dispose(); stream.ReadByte(); }
            else { Opaque(); }
            return 0;
            """, members: "static void Opaque() { }");
        AssertUnsupported(result);
    }

    [Fact]
    public void ExistingEvidenceIsSuppressedEvenWhenOffsetsMove()
    {
        const string body = "var stream = new MemoryStream(); stream.Dispose(); return stream.ReadByte();";
        var before = Read(body);
        var after = Read("/* shifted use */ " + body, revision: "after");
        var comparison = CSharpResourceLifetime.Compare(before, after, [Match(before, after)]);
        Assert.True(comparison.Complete);
        Assert.Equal(ResourceLifetimeChangeKind.Existing, Assert.Single(comparison.Changes).Kind);
    }

    [Fact]
    public void SafeReacquisitionInTheCurrentArmIsNotARegression()
    {
        var before = Read("var stream = new MemoryStream(); stream.Dispose(); return stream.ReadByte();");
        var after = Read("var stream = new MemoryStream(); stream.Dispose(); stream = new MemoryStream(); return stream.ReadByte();",
            revision: "after");
        var comparison = CSharpResourceLifetime.Compare(before, after, [Match(before, after)]);
        Assert.True(comparison.Complete);
        Assert.Equal(ResourceLifetimeChangeKind.NoRegression, Assert.Single(comparison.Changes).Kind);
    }

    [Fact]
    public void ConditionalOldEvidenceCannotEstablishIntroduction()
    {
        var before = Read("var stream = new MemoryStream(); if (choose) stream.Dispose(); return stream.ReadByte();");
        var after = Read("var stream = new MemoryStream(); stream.Dispose(); return stream.ReadByte();", revision: "after");
        var comparison = CSharpResourceLifetime.Compare(before, after, [Match(before, after)]);
        Assert.False(comparison.Complete);
        Assert.Equal(ResourceLifetimeChangeKind.Unknown, Assert.Single(comparison.Changes).Kind);
    }

    [Fact]
    public void AnUnknownEarlierReadPreventsADefinitiveIntroducedClassification()
    {
        var before = Read("var stream = new MemoryStream(); return stream.ReadByte();");
        var after = Read("input.ReadByte(); var stream = new MemoryStream(); stream.Dispose(); return stream.ReadByte();",
            revision: "after");
        Assert.False(after.Complete);
        Assert.Equal(2, after.Uses.Count);
        var oldSpan = Assert.Single(before.Uses).Span;
        var newSpan = after.Uses[1].Span;
        var match = new ResourceLifetimeUseMatch(new TextSpan(oldSpan.Start, oldSpan.Length),
            new TextSpan(newSpan.Start, newSpan.Length));
        var comparison = CSharpResourceLifetime.Compare(before, after, [match]);
        Assert.False(comparison.Complete);
        Assert.All(comparison.Changes, change => Assert.Equal(ResourceLifetimeChangeKind.Unknown, change.Kind));
    }

    [Fact]
    public void IncompleteOrUnmatchedBaselineNeverEstablishesIntroduction()
    {
        var after = Read("var stream = new MemoryStream(); stream.Dispose(); return stream.ReadByte();", revision: "after");
        var incomplete = Read("return 0;", sourceClosure: false);
        var incompleteComparison = CSharpResourceLifetime.Compare(incomplete, after, []);
        Assert.False(incompleteComparison.Complete);
        Assert.False(string.IsNullOrWhiteSpace(incompleteComparison.Reason));
        Assert.Empty(incompleteComparison.Changes);

        var unmatched = CSharpResourceLifetime.Compare(Read("return 0;"), after, []);
        Assert.False(unmatched.Complete);
        Assert.Equal(ResourceLifetimeChangeKind.Unknown, Assert.Single(unmatched.Changes).Kind);
    }

    [Fact]
    public void WrongScopeAndInvalidUseMappingsRemainUnknown()
    {
        const string live = "var stream = new MemoryStream(); return stream.ReadByte();";
        var before = Read(live);
        var after = Read("var stream = new MemoryStream(); stream.Dispose(); return stream.ReadByte();", revision: "after");
        var valid = Match(before, after);
        var wrongSpan = new ResourceLifetimeUseMatch(new TextSpan(valid.Before.Start + 1, valid.Before.Length), valid.After);
        var invalidMapping = CSharpResourceLifetime.Compare(before, after, [wrongSpan]);
        Assert.False(invalidMapping.Complete);
        Assert.False(string.IsNullOrWhiteSpace(invalidMapping.Reason));
        Assert.Empty(invalidMapping.Changes);

        var duplicateMapping = CSharpResourceLifetime.Compare(before, after, [valid, valid]);
        Assert.False(duplicateMapping.Complete);
        Assert.False(string.IsNullOrWhiteSpace(duplicateMapping.Reason));
        Assert.Empty(duplicateMapping.Changes);

        var otherContext = Read(live, context: "other|net10.0|Debug");
        var wrongScope = CSharpResourceLifetime.Compare(otherContext, after, [Match(otherContext, after)]);
        Assert.False(wrongScope.Complete);
        Assert.False(string.IsNullOrWhiteSpace(wrongScope.Reason));
        Assert.Empty(wrongScope.Changes);

        var sameSnapshot = CSharpResourceLifetime.Compare(before, before, [Match(before, before)]);
        Assert.False(sameSnapshot.Complete);
        Assert.False(string.IsNullOrWhiteSpace(sameSnapshot.Reason));
        Assert.Empty(sameSnapshot.Changes);
    }

    [Fact]
    public void CompilerOrBudgetDriftCannotBeAttributedToASourceEdit()
    {
        var before = Read("var stream = new MemoryStream(); return stream.ReadByte();");
        var after = Read("var stream = new MemoryStream(); stream.Dispose(); return stream.ReadByte();", revision: "after");
        var match = Match(before, after);
        var compilerDrift = CSharpResourceLifetime.Compare(before,
            after with { RoslynCSharpAssemblyVersion = "different-compiler" }, [match]);
        var budgetDrift = CSharpResourceLifetime.Compare(before,
            after with { Limits = new ResourceLifetimeLimits(MaxUses: 1) }, [match]);
        Assert.All(new[] { compilerDrift, budgetDrift }, comparison =>
        {
            Assert.False(comparison.Complete);
            Assert.False(string.IsNullOrWhiteSpace(comparison.Reason));
            Assert.Empty(comparison.Changes);
        });
    }

    [Theory]
    [InlineData("characters")]
    [InlineData("syntax")]
    [InlineData("blocks")]
    [InlineData("work")]
    [InlineData("uses")]
    public void ExhaustionIsVisibleAndOutputRemainsBounded(string budget)
    {
        var limits = budget switch
        {
            "characters" => new ResourceLifetimeLimits(MaxMethodCharacters: 1),
            "syntax" => new ResourceLifetimeLimits(MaxSyntaxNodes: 1),
            "blocks" => new ResourceLifetimeLimits(MaxBlocks: 2),
            "work" => new ResourceLifetimeLimits(MaxWork: 1),
            _ => new ResourceLifetimeLimits(MaxUses: 1)
        };
        var result = Read("var stream = new MemoryStream(); stream.ReadByte(); return stream.ReadByte();", limits: limits);
        Assert.Equal(ResourceLifetimeStatus.BudgetExceeded, result.Status);
        Assert.False(result.Complete);
        Assert.InRange(result.Uses.Count, 0, limits.MaxUses);
        Assert.False(string.IsNullOrWhiteSpace(result.Reason));
    }

    [Fact]
    public void WitnessOverflowIsNotSilentlyTruncatedIntoAProof()
    {
        var result = Read("var stream = new MemoryStream(); if (choose) stream.Dispose(); else stream.Dispose(); return stream.ReadByte();",
            limits: new ResourceLifetimeLimits(MaxWitnessesPerUse: 1));
        Assert.Equal(ResourceLifetimeStatus.BudgetExceeded, result.Status);
        Assert.False(result.Complete);
    }

    [Theory]
    [InlineData("identities")]
    [InlineData("depth")]
    public void IdentityAndDepthBudgetsDiscardUnfinishedEvidence(string budget)
    {
        var limits = budget == "identities"
            ? new ResourceLifetimeLimits(MaxTracked: 1)
            : new ResourceLifetimeLimits(MaxDepth: 1);
        var result = Read("var first = new MemoryStream(); var second = new MemoryStream(); return first.ReadByte();",
            limits: limits);
        Assert.Equal(ResourceLifetimeStatus.BudgetExceeded, result.Status);
        Assert.False(result.Complete);
        Assert.Empty(result.Uses);
        Assert.False(string.IsNullOrWhiteSpace(result.Reason));
    }

    [Fact]
    public void CancellationIsReportedWithoutPartialProof()
    {
        using var cancellation = new CancellationTokenSource();
        cancellation.Cancel();
        var result = Read("var stream = new MemoryStream(); stream.Dispose(); return stream.ReadByte();",
            cancellationToken: cancellation.Token);
        Assert.Equal(ResourceLifetimeStatus.Cancelled, result.Status);
        Assert.False(result.Complete);
        Assert.Empty(result.Uses);
    }

    [Fact]
    public void RepeatedAnalysisHasStableSourceOrderAndEvidence()
    {
        const string body = "var first = new MemoryStream(); var second = new MemoryStream(); first.ReadByte(); return second.ReadByte();";
        var first = Read(body);
        var again = Read(body);
        Assert.True(first.Complete);
        Assert.Equal(2, first.Uses.Count);
        Assert.True(first.Uses[0].Span.Start < first.Uses[1].Span.Start);
        Assert.All(first.Uses, use => Assert.Equal(ResourceLifetimeUseState.Live, use.State));
        Assert.Equal(first.Uses.Select(use => (use.Span.Start, use.State, use.Allocation?.Start)).ToArray(),
            again.Uses.Select(use => (use.Span.Start, use.State, use.Allocation?.Start)).ToArray());
        Assert.Equal(first.Work, again.Work);
        Assert.False(string.IsNullOrWhiteSpace(first.RoslynCommonAssemblyVersion));
        Assert.False(string.IsNullOrWhiteSpace(first.RoslynCSharpAssemblyVersion));
    }

    [Fact]
    public void FiniteBooleanGuardsRequireAnExplicitVersionedMode()
    {
        var oldInput = JsonSerializer.Deserialize<ResourceLifetimeLimits>("{\"MaxUses\":1}")!;
        Assert.Equal(ResourceLifetimeGuardMode.SingleConditionV1, oldInput.GuardMode);
        Assert.Equal(1, oldInput.MaxNonconstantConditions);
        Assert.Equal(1, oldInput.MaxBooleanParameters);
        Assert.Equal(1, oldInput.MaxUses);
        Assert.Equal(new ResourceLifetimeLimits(MaxUses: 1), oldInput);
        Assert.Equal(16, FiniteBooleanLimits.MaxNonconstantConditions);
        Assert.Equal(3, FiniteBooleanLimits.MaxBooleanParameters);
        Assert.NotEqual(new ResourceLifetimeLimits(), FiniteBooleanLimits);

        const string body = "var stream = new MemoryStream(); if (choose) stream.Dispose(); if (choose) return stream.ReadByte(); return 0;";
        AssertUnsupported(Read(body));
        AssertUnsupported(Read(body, limits: oldInput));
        Assert.Equal(ResourceLifetimeUseState.Disposed,
            Assert.Single(Read(body, limits: FiniteBooleanLimits).Uses).State);

        var restored = JsonSerializer.Deserialize<ResourceLifetimeLimits>(
            JsonSerializer.Serialize(FiniteBooleanLimits))!;
        Assert.Equal(FiniteBooleanLimits, restored);
        Assert.Throws<ArgumentOutOfRangeException>(() => Read("return 0;",
            limits: new ResourceLifetimeLimits { GuardMode = (ResourceLifetimeGuardMode)99 }));
    }

    [Theory]
    [InlineData("if (choose) stream.Dispose(); if (choose) return stream.ReadByte(); return 0;", ResourceLifetimeUseState.Disposed, 1)]
    [InlineData("if (choose) stream.Dispose(); if (!choose) return stream.ReadByte(); return 0;", ResourceLifetimeUseState.Live, 0)]
    [InlineData("if (choose) stream.Dispose(); if (!choose) stream.Dispose(); return stream.ReadByte();", ResourceLifetimeUseState.Disposed, 2)]
    public void RepeatedImmutableGuardsRetainTheirValuations(
        string suffix, ResourceLifetimeUseState expected, int witnesses)
    {
        var result = Read("var stream = new MemoryStream(); " + suffix, limits: FiniteBooleanLimits);
        var use = Assert.Single(result.Uses);
        Assert.Equal(ResourceLifetimeStatus.Analyzed, result.Status);
        Assert.True(result.Complete);
        Assert.Equal(expected, use.State);
        Assert.NotNull(use.Allocation);
        Assert.Equal(witnesses, use.Disposals.Count);
        Assert.False(result.PathFeasibilityProved);
    }

    [Fact]
    public void ContradictoryValuationsDoNotCreateADisposedUse()
    {
        var result = Read("var stream = new MemoryStream(); if (choose) { stream.Dispose(); if (!choose) return stream.ReadByte(); } return 0;",
            limits: FiniteBooleanLimits);
        Assert.Equal(ResourceLifetimeStatus.Analyzed, result.Status);
        Assert.True(result.Complete);
        Assert.Empty(result.Uses);
    }

    [Fact]
    public void IndependentGuardsRetainBothLiveAndDisposedPossibilities()
    {
        var result = Read("var stream = new MemoryStream(); if (choose) stream.Dispose(); if (other) return stream.ReadByte(); return 0;",
            limits: FiniteBooleanLimits, additionalParameters: ", bool other");
        var use = Assert.Single(result.Uses);
        Assert.Equal(ResourceLifetimeStatus.Analyzed, result.Status);
        Assert.False(result.Complete);
        Assert.Equal(ResourceLifetimeUseState.Unknown, use.State);
        Assert.NotNull(use.Allocation);
        Assert.Single(use.Disposals);
        Assert.Equal("joined_lifetime_unknown", use.Reason);
    }

    [Theory]
    [InlineData("if (choose) stream.Dispose(); if (choose && other && third) return stream.ReadByte(); return 0;")]
    [InlineData("if (!choose) stream.Dispose(); if (!(choose || !other)) return stream.ReadByte(); return 0;")]
    [InlineData("if (choose) stream.Dispose(); if (other) { if (third) { if (!!choose) return stream.ReadByte(); } } return 0;")]
    public void LoweredShortCircuitAndNestedGuardsUseTheSameBooleanAtoms(string suffix)
    {
        var result = Read("var stream = new MemoryStream(); " + suffix, limits: FiniteBooleanLimits,
            additionalParameters: ", bool other, bool third");
        Assert.Equal(ResourceLifetimeStatus.Analyzed, result.Status);
        Assert.True(result.Complete);
        Assert.Equal(ResourceLifetimeUseState.Disposed, Assert.Single(result.Uses).State);
    }

    [Theory]
    [InlineData("stream", ResourceLifetimeUseState.Live)]
    [InlineData("alias", ResourceLifetimeUseState.Disposed)]
    public void CorrelatedReacquisitionPreservesTheOldAlias(
        string receiver, ResourceLifetimeUseState expected)
    {
        var body = "var stream = new MemoryStream(); var alias = stream; " +
            "if (choose) { stream.Dispose(); stream = new MemoryStream(); } " +
            $"if (choose) return {receiver}.ReadByte(); return 0;";
        var result = Read(body, limits: FiniteBooleanLimits);
        var use = Assert.Single(result.Uses);
        Assert.True(result.Complete);
        Assert.Equal(expected, use.State);
        var source = Source(body);
        var origin = expected == ResourceLifetimeUseState.Live
            ? source.LastIndexOf("new MemoryStream()", StringComparison.Ordinal)
            : source.IndexOf("new MemoryStream()", StringComparison.Ordinal);
        Assert.Equal(origin, use.Allocation!.Start);
        Assert.Equal(expected == ResourceLifetimeUseState.Live ? 0 : 1, use.Disposals.Count);
    }

    [Fact]
    public void DifferentDisposedAllocationsDoNotBecomeOneExactIdentity()
    {
        var result = Read("var stream = new MemoryStream(); if (choose) stream = new MemoryStream(); stream.Dispose(); if (other) return stream.ReadByte(); return 0;",
            limits: FiniteBooleanLimits, additionalParameters: ", bool other");
        var use = Assert.Single(result.Uses);
        Assert.Equal(ResourceLifetimeStatus.Analyzed, result.Status);
        Assert.False(result.Complete);
        Assert.Equal(ResourceLifetimeUseState.Unknown, use.State);
        Assert.Null(use.Allocation);
        Assert.Empty(use.Disposals);
        Assert.Equal("object_identity_unknown", use.Reason);
    }

    [Fact]
    public void ThrowingValuationsDoNotReachALaterGuardedRead()
    {
        var result = Read("var stream = new MemoryStream(); if (choose) { stream.Dispose(); stream.ReadByte(); } if (other) return stream.ReadByte(); return 0;",
            limits: FiniteBooleanLimits, additionalParameters: ", bool other");
        Assert.Equal(ResourceLifetimeStatus.Analyzed, result.Status);
        Assert.True(result.Complete);
        Assert.Equal(new[] { ResourceLifetimeUseState.Disposed, ResourceLifetimeUseState.Live },
            result.Uses.Select(use => use.State).ToArray());
        Assert.Single(result.Uses[0].Disposals);
        Assert.Empty(result.Uses[1].Disposals);
    }

    [Fact]
    public void AnUnknownReadStillTerminatesOnlyItsDisposedValuations()
    {
        var result = Read("var stream = new MemoryStream(); if (choose) stream.Dispose(); stream.ReadByte(); if (other) return stream.ReadByte(); return 0;",
            limits: FiniteBooleanLimits, additionalParameters: ", bool other");
        Assert.Equal(ResourceLifetimeStatus.Analyzed, result.Status);
        Assert.False(result.Complete);
        Assert.Equal(new[] { ResourceLifetimeUseState.Unknown, ResourceLifetimeUseState.Live },
            result.Uses.Select(use => use.State).ToArray());
    }

    [Fact]
    public void CorrelatedChangesStillRequireAMatchedLiveBaseline()
    {
        const string live = "var stream = new MemoryStream(); if (choose) stream.ToArray(); if (choose) return stream.ReadByte(); return 0;";
        const string disposed = "var stream = new MemoryStream(); if (choose) stream.Dispose(); if (choose) return stream.ReadByte(); return 0;";
        var before = Read(live, limits: FiniteBooleanLimits);
        var after = Read(disposed, revision: "after", limits: FiniteBooleanLimits);
        var introduced = CSharpResourceLifetime.Compare(before, after, [Match(before, after)]);
        Assert.True(introduced.Complete);
        Assert.Equal(ResourceLifetimeChangeKind.Introduced, Assert.Single(introduced.Changes).Kind);

        var unmatched = CSharpResourceLifetime.Compare(before, after, []);
        Assert.False(unmatched.Complete);
        Assert.Equal(ResourceLifetimeChangeKind.Unknown, Assert.Single(unmatched.Changes).Kind);
        Assert.Equal("matched_live_baseline_required", unmatched.Changes[0].Reason);

        var shifted = Read("/* shifted */ " + disposed, revision: "shifted", limits: FiniteBooleanLimits);
        var existing = CSharpResourceLifetime.Compare(after, shifted, [Match(after, shifted)]);
        Assert.True(existing.Complete);
        Assert.Equal(ResourceLifetimeChangeKind.Existing, Assert.Single(existing.Changes).Kind);

        var reacquired = Read("var stream = new MemoryStream(); if (choose) { stream.Dispose(); stream = new MemoryStream(); } if (choose) return stream.ReadByte(); return 0;",
            revision: "reacquired", limits: FiniteBooleanLimits);
        var safe = CSharpResourceLifetime.Compare(after, reacquired, [Match(after, reacquired)]);
        Assert.True(safe.Complete);
        Assert.Equal(ResourceLifetimeChangeKind.NoRegression, Assert.Single(safe.Changes).Kind);
    }

    [Fact]
    public void AnOldUnknownUnderIndependentGuardsCannotEstablishIntroduction()
    {
        var before = Read("var stream = new MemoryStream(); if (choose) stream.Dispose(); if (other) return stream.ReadByte(); return 0;",
            limits: FiniteBooleanLimits, additionalParameters: ", bool other");
        var after = Read("var stream = new MemoryStream(); if (choose) stream.Dispose(); if (choose) return stream.ReadByte(); return 0;",
            revision: "after", limits: FiniteBooleanLimits, additionalParameters: ", bool other");
        Assert.Equal(ResourceLifetimeUseState.Unknown, Assert.Single(before.Uses).State);
        var comparison = CSharpResourceLifetime.Compare(before, after, [Match(before, after)]);
        Assert.False(comparison.Complete);
        var change = Assert.Single(comparison.Changes);
        Assert.Equal(ResourceLifetimeChangeKind.Unknown, change.Kind);
        Assert.Equal("incomplete_lifetime_observations", change.Reason);
    }

    [Fact]
    public void AGuardModeChangeCannotBeAttributedToASourceEdit()
    {
        var before = Read("var stream = new MemoryStream(); return stream.ReadByte();");
        var after = Read("var stream = new MemoryStream(); stream.Dispose(); return stream.ReadByte();",
            revision: "after", limits: FiniteBooleanLimits);
        var comparison = CSharpResourceLifetime.Compare(before, after, [Match(before, after)]);
        Assert.False(comparison.Complete);
        Assert.Empty(comparison.Changes);
        Assert.Equal("incompatible_compiler_or_limits", comparison.Reason);
    }

    [Theory]
    [InlineData("var stream = new MemoryStream(); if (choose) stream.Dispose(); choose = !choose; if (choose) return stream.ReadByte(); return 0;")]
    [InlineData("var stream = new MemoryStream(); if (Get()) stream.Dispose(); if (choose) return stream.ReadByte(); return 0;")]
    [InlineData("var stream = new MemoryStream(); if (Flag) stream.Dispose(); if (choose) return stream.ReadByte(); return 0;")]
    [InlineData("var stream = new MemoryStream(); bool copy = choose; if (copy) stream.Dispose(); if (choose) return stream.ReadByte(); return 0;")]
    [InlineData("var stream = new MemoryStream(); if (!choose) { stream.Dispose(); stream.ReadByte(); } else Opaque(); return 0;")]
    [InlineData("var stream = new MemoryStream(); try { if (choose) stream.Dispose(); } catch { } if (choose) return stream.ReadByte(); return 0;")]
    public void FiniteGuardsDoNotAdmitMutationOpaqueEffectsOrExceptionRegions(string body)
    {
        AssertUnsupported(Read(body, members: "static bool Get() => true; static bool Flag => true; static void Opaque() { }",
            limits: FiniteBooleanLimits));
    }

    [Fact]
    public void FiniteGuardsDoNotAdmitRefParameters()
    {
        AssertUnsupported(Read("var stream = new MemoryStream(); if (choose) stream.Dispose(); if (other) return stream.ReadByte(); return 0;",
            limits: FiniteBooleanLimits, additionalParameters: ", ref bool other"));
    }

    [Fact]
    public void AFourthBooleanAtomIsAnExplicitUnsupportedBoundary()
    {
        var result = Read("var stream = new MemoryStream(); if (choose) { if (other) { if (third) { if (fourth) return stream.ReadByte(); } } } return 0;",
            limits: FiniteBooleanLimits, additionalParameters: ", bool other, bool third, bool fourth");
        AssertUnsupported(result);
        Assert.Equal("boolean_parameter_limit", result.Reason);
    }

    [Theory]
    [InlineData(16, ResourceLifetimeStatus.Analyzed)]
    [InlineData(17, ResourceLifetimeStatus.Unsupported)]
    public void RepeatedGuardSitesHaveAFixedSliceBound(int sites, ResourceLifetimeStatus expected)
    {
        var body = "var stream = new MemoryStream(); " +
            string.Concat(Enumerable.Repeat("if (choose) stream.ToArray(); ", sites)) + "return stream.ReadByte();";
        var result = Read(body, limits: FiniteBooleanLimits);
        Assert.Equal(expected, result.Status);
        if (expected == ResourceLifetimeStatus.Analyzed)
            Assert.Equal(ResourceLifetimeUseState.Live, Assert.Single(result.Uses).State);
        else
        {
            Assert.Empty(result.Uses);
            Assert.Equal("predicate_correlation_or_shape", result.Reason);
        }
    }

    [Fact]
    public void FiniteValuationsCountDistinctUsesAndShareTheWorkBudget()
    {
        const string prefix = "var stream = new MemoryStream(); if (choose) stream.ToArray(); if (other) stream.ToArray(); if (third) stream.ToArray(); ";
        var single = Read(prefix + "return stream.ReadByte();", limits: FiniteBooleanLimits with { MaxUses = 1 },
            additionalParameters: ", bool other, bool third");
        Assert.Equal(ResourceLifetimeStatus.Analyzed, single.Status);
        Assert.True(single.Complete);
        Assert.Equal(ResourceLifetimeUseState.Live, Assert.Single(single.Uses).State);

        var second = Read(prefix + "stream.ReadByte(); return stream.ReadByte();",
            limits: FiniteBooleanLimits with { MaxUses = 1 }, additionalParameters: ", bool other, bool third");
        Assert.Equal(ResourceLifetimeStatus.BudgetExceeded, second.Status);
        Assert.Equal("use_limit", second.Reason);
        Assert.Empty(second.Uses);

        var bounded = Read(prefix + "return stream.ReadByte();", limits: FiniteBooleanLimits with { MaxWork = 256 },
            additionalParameters: ", bool other, bool third");
        Assert.Equal(ResourceLifetimeStatus.BudgetExceeded, bounded.Status);
        Assert.Equal("work_limit", bounded.Reason);
        Assert.InRange(bounded.Work, 0, 256);
        Assert.Empty(bounded.Uses);
    }

    [Fact]
    public void FiniteWitnessAggregationIsSortedAndNeverSilentlyTruncated()
    {
        const string body = "var stream = new MemoryStream(); if (choose) stream.Dispose(); else if (other) stream.Dispose(); else stream.Dispose(); return stream.ReadByte();";
        var result = Read(body, limits: FiniteBooleanLimits, additionalParameters: ", bool other");
        var repeated = Read(body, limits: FiniteBooleanLimits, additionalParameters: ", bool other");
        var use = Assert.Single(result.Uses);
        Assert.True(result.Complete);
        Assert.Equal(ResourceLifetimeUseState.Disposed, use.State);
        Assert.Equal(3, use.Disposals.Count);
        Assert.Equal(use.Disposals.OrderBy(span => span.Start).ToArray(), use.Disposals.ToArray());
        Assert.Equal(use.Disposals.ToArray(), Assert.Single(repeated.Uses).Disposals.ToArray());
        Assert.Equal(result.Work, repeated.Work);

        var exhausted = Read(body, limits: FiniteBooleanLimits with { MaxWitnessesPerUse = 2 },
            additionalParameters: ", bool other");
        Assert.Equal(ResourceLifetimeStatus.BudgetExceeded, exhausted.Status);
        Assert.Equal("disposal_witness_limit", exhausted.Reason);
        Assert.Empty(exhausted.Uses);
    }

    private static ResourceLifetimeLimits FiniteBooleanLimits => new()
    {
        GuardMode = ResourceLifetimeGuardMode.FiniteBooleanParametersV1
    };

    private static void AssertUnsupported(ResourceLifetimeAnalysis result)
    {
        Assert.Equal(ResourceLifetimeStatus.Unsupported, result.Status);
        Assert.False(result.Complete);
        Assert.Empty(result.Uses);
        Assert.False(string.IsNullOrWhiteSpace(result.Reason));
    }

    private static ResourceLifetimeUseMatch Match(ResourceLifetimeAnalysis before, ResourceLifetimeAnalysis after)
    {
        var oldUse = Assert.Single(before.Uses).Span;
        var newUse = Assert.Single(after.Uses).Span;
        return new(new TextSpan(oldUse.Start, oldUse.Length), new TextSpan(newUse.Start, newUse.Length));
    }

    private static string Source(string body, string members = "", bool isAsync = false,
        string additionalParameters = "") =>
        "using System; using System.IO; class C { public static " +
        (isAsync ? "async System.Threading.Tasks.Task<int>" : "int") +
        " Run(bool choose, MemoryStream input" + additionalParameters + ") { " + body + " } " + members + " }";

    private static ResourceLifetimeAnalysis Read(string body, string members = "", string revision = "before",
        string context = "fixture|net10.0|Debug", bool sourceClosure = true, bool referenceClosure = true,
        bool compilerErrorsAbsent = true, bool expectCompilerErrors = false,
        ResourceLifetimeSourceKind sourceKind = ResourceLifetimeSourceKind.User,
        ResourceLifetimeLimits? limits = null, bool isAsync = false, CancellationToken cancellationToken = default,
        string additionalParameters = "")
    {
        var source = Source(body, members, isAsync, additionalParameters);
        var tree = CSharpSyntaxTree.ParseText(source, new CSharpParseOptions(LanguageVersion.CSharp14), path: "src/Fixture.cs");
        var compilation = CSharpCompilation.Create("LifetimeFixture", [tree],
            [MetadataReference.CreateFromFile(typeof(object).Assembly.Location)],
            new CSharpCompilationOptions(OutputKind.DynamicallyLinkedLibrary, nullableContextOptions: NullableContextOptions.Enable));
        var errors = compilation.GetDiagnostics().Where(diagnostic => diagnostic.Severity == DiagnosticSeverity.Error).ToArray();
        if (expectCompilerErrors) Assert.NotEmpty(errors);
        else Assert.Empty(errors);
        var method = tree.GetRoot().DescendantNodes().OfType<MethodDeclarationSyntax>()
            .Single(declaration => declaration.Identifier.ValueText == "Run");
        var identity = new RoslynDocumentIdentity(context, "fixture-document", "src/Fixture.cs", "src/Fixture.cs",
            revision, "sha256:" + Convert.ToHexString(SHA256.HashData(Encoding.UTF8.GetBytes(source))), referenceClosure);
        return CSharpResourceLifetime.Analyze(compilation, tree, method,
            new ResourceLifetimeInput(identity, sourceClosure, compilerErrorsAbsent, sourceKind), limits, cancellationToken);
    }
}
