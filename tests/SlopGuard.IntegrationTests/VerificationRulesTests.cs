using System.Text.Json;
using SlopGuard.Core.Verification;
using Xunit;

namespace SlopGuard.IntegrationTests;

public sealed class VerificationRulesTests
{
    private static readonly VerificationScope BranchScope =
        new("src/app|net10.0|Debug", "app/net10.0", "Parse(string)", "if-true@42", "source-v2", "build-v2");

    private static VerificationChange Change(VerificationChangeKind kind = VerificationChangeKind.EditedBranch,
        VerificationScope? scope = null, string id = "change") =>
        new(id, kind, scope ?? BranchScope, "src/Parse.cs:42:5-42:25", ["edited validation branch"]);

    private static string Rule(VerificationChangeKind kind) => kind switch
    {
        VerificationChangeKind.EditedBranch => VerificationRuleEvaluator.EditedBranchRule,
        VerificationChangeKind.PublicContract => VerificationRuleEvaluator.PublicContractRule,
        VerificationChangeKind.Serialization => VerificationRuleEvaluator.SerializationRule,
        _ => throw new ArgumentOutOfRangeException(nameof(kind))
    };

    private static VerificationEvidence Fact(VerificationEvidenceKind kind,
        VerificationChangeKind ruleKind = VerificationChangeKind.EditedBranch,
        VerificationScope? scope = null, string id = "fact") =>
        new()
        {
            Id = id, RuleId = Rule(ruleKind), DetectorId = "adapter", Kind = kind,
            Scope = scope ?? BranchScope, Binding = VerificationBinding.Exact
        };

    private static VerificationEvidence Counter(int? hits, VerificationScope? scope = null,
        VerificationCounterMapping mapping = VerificationCounterMapping.ExactBranch,
        VerificationRunOutcome outcome = VerificationRunOutcome.Passed,
        string id = "counter") =>
        Fact(VerificationEvidenceKind.BranchCounter, scope: scope, id: id) with
        {
            ValidReceipt = true, ExactTargetMapping = true, CounterMapping = mapping,
            HitCount = hits, RunOutcome = outcome
        };

    private static VerificationSearchFact Search(VerificationChangeKind kind = VerificationChangeKind.EditedBranch,
        VerificationScope? scope = null, bool complete = true, string detector = "adapter",
        string id = "search") =>
        new(id, Rule(kind), detector, scope ?? BranchScope, complete,
            [(scope ?? BranchScope).TargetId], [], []);

    private static VerificationRequest Request(VerificationChange change,
        IEnumerable<VerificationEvidence>? facts = null,
        IEnumerable<VerificationSearchFact>? searches = null,
        IEnumerable<string>? detectors = null, VerificationPolicyMode mode = VerificationPolicyMode.Required)
    {
        var ruleId = Rule(change.Kind);
        return new(new VerificationPolicy("repo-policy-v7", VerificationPolicyMode.Advisory,
                [new VerificationRuleSetting(ruleId, mode, (detectors ?? ["adapter"]).ToArray())]),
            [change], (searches ?? []).ToArray(), (facts ?? []).ToArray());
    }

    private static VerificationDecision Only(VerificationRequest request, VerificationLimits? limits = null) =>
        Assert.Single(VerificationRuleEvaluator.Evaluate(request, limits).Decisions);

    [Fact]
    public void ExactBranchHitIsObservedButDoesNotClaimAnAssertion()
    {
        var decision = Only(Request(Change(), [Counter(3)], [Search()]));
        Assert.Equal(VerificationState.Observed, decision.State);
        Assert.Equal(VerificationStrength.MeasuredExecution, decision.Strength);
        Assert.False(decision.RelevantAssertionEstablished);
        Assert.Equal(1, decision.RuleVersion);
        Assert.Equal(VerificationPolicyMode.Required, decision.AppliedPolicy);
        Assert.Equal("repo-policy-v7", decision.PolicyVersion);
        Assert.Equal(VerificationRuleEvaluator.EditedBranchRule, decision.RuleId);
        Assert.Equal(new[] { "counter" }, decision.EvidenceIds);
        Assert.Equal(new[] { "adapter" }, decision.ConsultedDetectors);
        Assert.NotEmpty(decision.NextAction);
    }

    [Fact]
    public void CompleteExactZeroCounterProvesUncoveredBranch()
    {
        var decision = Only(Request(Change(), [Counter(0)], [Search()]));
        Assert.Equal(VerificationState.Uncovered, decision.State);
        Assert.Equal(VerificationStrength.VerifiedMeasurement, decision.Strength);
        Assert.Contains("if-true@42", decision.NextAction);
    }

    [Fact]
    public void PartialBranchCountersNeverProveZeroOrHits()
    {
        var decision = Only(Request(Change(),
            [Counter(0, mapping: VerificationCounterMapping.PartialBranch)], [Search()]));
        Assert.Equal(VerificationState.Unknown, decision.State);
        Assert.Contains(decision.Frontier, s => s.Contains("exact verified", StringComparison.Ordinal));
    }

    [Theory]
    [InlineData(VerificationCounterMapping.LineOnly)]
    [InlineData(VerificationCounterMapping.Ambiguous)]
    [InlineData(VerificationCounterMapping.None)]
    public void NonBranchMappingsAreUnknown(VerificationCounterMapping mapping)
    {
        var decision = Only(Request(Change(), [Counter(3, mapping: mapping)], [Search()]));
        Assert.Equal(VerificationState.Unknown, decision.State);
    }

    [Fact]
    public void UnverifiedReceiptsAndIncompleteRunsAreNotMeasurements()
    {
        var unverified = Counter(2) with { ValidReceipt = false };
        var incomplete = Counter(2, id: "partial") with { RunOutcome = VerificationRunOutcome.Incomplete };
        var decision = Only(Request(Change(), [unverified, incomplete], [Search()]));
        Assert.Equal(VerificationState.Unknown, decision.State);
        Assert.Contains(VerificationRunOutcome.Incomplete, decision.RunOutcomes);
        Assert.DoesNotContain(VerificationRunOutcome.Passed, decision.RunOutcomes);
    }

    [Fact]
    public void FailedTestCanStillContributeValidMeasuredHits()
    {
        var decision = Only(Request(Change(),
            [Counter(2, outcome: VerificationRunOutcome.Failed)], [Search()]));
        Assert.Equal(VerificationState.Observed, decision.State);
        Assert.Contains(VerificationRunOutcome.Failed, decision.RunOutcomes);
        Assert.Contains("failed test", decision.NextAction);
    }

    [Fact]
    public void StaticExactPathAndPassingTestAreOnlyConfiguredReachability()
    {
        var path = Fact(VerificationEvidenceKind.StaticPath, id: "path");
        var testRun = Fact(VerificationEvidenceKind.TestRun, id: "run") with
        {
            RunOutcome = VerificationRunOutcome.Passed, ValidReceipt = true
        };
        var decision = Only(Request(Change(), [path, testRun], [Search()]));
        Assert.Equal(VerificationState.Configured, decision.State);
        Assert.Equal(VerificationStrength.ExactStaticPath, decision.Strength);
        Assert.False(decision.RelevantAssertionEstablished);
        Assert.Contains(VerificationRunOutcome.Passed, decision.RunOutcomes);
    }

    [Fact]
    public void PossibleStaticPathKeepsUncertaintyFrontier()
    {
        var path = Fact(VerificationEvidenceKind.StaticPath) with { Binding = VerificationBinding.Possible };
        var decision = Only(Request(Change(), [path], [Search()]));
        Assert.Equal(VerificationState.Configured, decision.State);
        Assert.Equal(VerificationStrength.PossibleStaticPath, decision.Strength);
        Assert.Contains(decision.Frontier, s => s.Contains("possible binding", StringComparison.Ordinal));
    }

    [Fact]
    public void MisleadingNamesNeverMatchWithoutSemanticIdentityAndExactBinding()
    {
        var wrongBehavior = BranchScope with { BehaviorId = "if-false@42" };
        var wrongTarget = BranchScope with { TargetId = "other/net10.0" };
        var namedOnly = Counter(99, id: "lookalike") with { Binding = VerificationBinding.NameCandidate };
        var decision = Only(Request(Change(),
            [Counter(4, wrongBehavior), Counter(9, wrongTarget), namedOnly], [Search()]));
        Assert.Equal(VerificationState.Missing, decision.State);
        Assert.DoesNotContain("lookalike", decision.EvidenceIds);
    }

    [Fact]
    public void SeparateTargetsDoNotSupplyCoverageToTheChangedTarget()
    {
        var wrongTarget = BranchScope with { TargetId = "cli/net10.0" };
        var decision = Only(Request(Change(), [Counter(100, wrongTarget)], [Search()]));
        Assert.Equal(VerificationState.Missing, decision.State);
    }

    [Fact]
    public void StaleSourceDigestIsNotCurrentCoverage()
    {
        var old = BranchScope with { SourceDigest = "old-digest" };
        var decision = Only(Request(Change(), [Counter(19, old)], [Search()]));
        Assert.Equal(VerificationState.Stale, decision.State);
        Assert.Contains(decision.Frontier, s => s.Contains("obsolete", StringComparison.Ordinal));
        Assert.Contains("source-v2", decision.NextAction);
    }

    [Fact]
    public void StaleBuildIdentityIsAlsoStale()
    {
        var old = BranchScope with { BuildId = "old-build" };
        Assert.Equal(VerificationState.Stale,
            Only(Request(Change(), [Counter(1, old)], [Search()])).State);
    }

    [Fact]
    public void ACurrentCompleteNegativeSearchSupersedesHistoricalSearchReceipts()
    {
        var older = Search(scope: BranchScope with { SourceDigest = "previous" }, id: "old");
        var current = Search(id: "current");
        var decision = Only(Request(Change(), [], [older, current]));
        Assert.Equal(VerificationState.Missing, decision.State);
        Assert.DoesNotContain(decision.Frontier, s => s.Contains("obsolete", StringComparison.Ordinal));
    }

    [Fact]
    public void AnIncompleteCurrentSearchRemainsUnknownDespiteHistoricalCompleteness()
    {
        var old = Search(scope: BranchScope with { SourceDigest = "previous" }, id: "old");
        var current = Search(complete: false, id: "current");
        var decision = Only(Request(Change(), [], [old, current]));
        Assert.Equal(VerificationState.Unknown, decision.State);
    }

    [Fact]
    public void IncompleteSearchCannotEstablishMissingEvidence()
    {
        var search = Search(complete: false) with { Frontier = ["unindexed dynamic caller"] };
        var decision = Only(Request(Change(), [], [search]));
        Assert.Equal(VerificationState.Unknown, decision.State);
        Assert.Contains(decision.Frontier, s => s.Contains("unindexed dynamic caller", StringComparison.Ordinal));
    }

    [Fact]
    public void ACompleteNegativeSearchMayEstablishMissing()
    {
        var decision = Only(Request(Change(), [], [Search()]));
        Assert.Equal(VerificationState.Missing, decision.State);
        Assert.Equal(new[] { BranchScope.TargetId }, decision.SearchedTargets);
        Assert.NotEmpty(decision.NextAction);
    }

    [Fact]
    public void MissingOneRequiredDetectorPreventsMissing()
    {
        var decision = Only(Request(Change(), [], [Search()], ["adapter", "second-adapter"]));
        Assert.Equal(VerificationState.Unknown, decision.State);
        Assert.Contains(decision.Frontier, s => s.Contains("second-adapter", StringComparison.Ordinal));
    }

    [Fact]
    public void ACompleteSearchThatExcludedTargetIsNotACompleteNegativeSearch()
    {
        var search = Search() with { SearchedTargets = ["other/net10.0"], ExcludedTargets = [BranchScope.TargetId] };
        var decision = Only(Request(Change(), [], [search]));
        Assert.Equal(VerificationState.Unknown, decision.State);
        Assert.Equal(new[] { BranchScope.TargetId }, decision.ExcludedTargets);
    }

    [Fact]
    public void BranchReceiptMissingTargetMappingCannotClaimUncovered()
    {
        var counter = Counter(0) with { ExactTargetMapping = false };
        Assert.Equal(VerificationState.Unknown, Only(Request(Change(), [counter], [Search()])).State);
    }

    [Theory]
    [InlineData(VerificationChangeKind.PublicContract)]
    [InlineData(VerificationChangeKind.Serialization)]
    public void ContractEvidenceRequiresExecutedBehaviorAndRelevantAssertion(VerificationChangeKind kind)
    {
        var executed = Fact(VerificationEvidenceKind.ContractExecution, kind) with
        {
            ValidReceipt = true, ExactTargetMapping = true, ExecutedBehavior = true,
            RelevantAssertion = true, RunOutcome = VerificationRunOutcome.Failed
        };
        var decision = Only(Request(Change(kind), [executed], [Search(kind)]));
        Assert.Equal(VerificationState.Observed, decision.State);
        Assert.Equal(VerificationCategory.ContractTests, decision.Category);
        Assert.True(decision.RelevantAssertionEstablished);
        Assert.Contains(VerificationRunOutcome.Failed, decision.RunOutcomes);
    }

    [Theory]
    [InlineData(VerificationChangeKind.PublicContract)]
    [InlineData(VerificationChangeKind.Serialization)]
    public void ContractWithoutRelevantAssertionRemainsUnknown(VerificationChangeKind kind)
    {
        var executed = Fact(VerificationEvidenceKind.ContractExecution, kind) with
        {
            ValidReceipt = true, ExactTargetMapping = true, ExecutedBehavior = true,
            RelevantAssertion = false, RunOutcome = VerificationRunOutcome.Passed
        };
        var decision = Only(Request(Change(kind), [executed], [Search(kind)]));
        Assert.Equal(VerificationState.Unknown, decision.State);
        Assert.False(decision.RelevantAssertionEstablished);
    }

    [Fact]
    public void BudgetExhaustionExplicitlyFailsClosedWithoutInputOrderSampling()
    {
        var request = Request(Change(), [Counter(2), Counter(3, id: "extra")], [Search()]);
        var batch = VerificationRuleEvaluator.Evaluate(request, new VerificationLimits(MaxEvidence: 1));
        Assert.True(batch.BudgetExhausted);
        Assert.Equal(VerificationState.Unknown, Assert.Single(batch.Decisions).State);
        Assert.Contains(batch.Frontier, reason => reason.Contains("evidence-fact budget", StringComparison.Ordinal));
    }

    [Fact]
    public void DetectorBudgetExhaustionIsVisibleEvenWithACompleteFlag()
    {
        var query = Search() with { Complete = true, BudgetExhausted = true };
        var batch = VerificationRuleEvaluator.Evaluate(Request(Change(), [], [query]));
        Assert.True(batch.BudgetExhausted);
        var decision = Assert.Single(batch.Decisions);
        Assert.Equal(VerificationState.Unknown, decision.State);
        Assert.Contains(decision.Frontier, f => f.Contains("budget-exhausted", StringComparison.Ordinal));
    }

    [Fact]
    public void NestedSearchScopeBudgetFailsClosedWithoutSampling()
    {
        var query = Search() with { SearchedTargets = [BranchScope.TargetId, "extra/net10.0"] };
        var batch = VerificationRuleEvaluator.Evaluate(Request(Change(), [], [query]),
            new VerificationLimits(MaxListItems: 1));
        Assert.True(batch.BudgetExhausted);
        Assert.Empty(batch.Decisions);
        Assert.Contains(batch.Frontier, f => f.Contains("nested scope-list budget", StringComparison.Ordinal));
    }

    [Fact]
    public void ChangeBudgetExhaustionReportsNoUnexaminedDecisionsAsFact()
    {
        var original = Request(Change());
        var many = original with { Changes = [Change(id: "b"), Change(id: "a")] };
        var batch = VerificationRuleEvaluator.Evaluate(many, new VerificationLimits(MaxChanges: 1));
        Assert.True(batch.BudgetExhausted);
        Assert.Empty(batch.Decisions);
        Assert.Contains(batch.Frontier, text => text.Contains("change budget", StringComparison.Ordinal));
    }

    [Fact]
    public void OutputIsDeterministicAcrossFactOrderAndSerializesAsTypedData()
    {
        var change = Change();
        var c1 = Counter(1, id: "z");
        var c2 = Counter(0, id: "a");
        var s1 = Search(id: "z");
        var s2 = Search(id: "a");
        var first = VerificationRuleEvaluator.Evaluate(Request(change, [c1, c2], [s1, s2]));
        var second = VerificationRuleEvaluator.Evaluate(Request(change, [c2, c1], [s2, s1]));
        var options = new JsonSerializerOptions(JsonSerializerDefaults.Web);
        var json = JsonSerializer.Serialize(first, options);
        Assert.Equal(json, JsonSerializer.Serialize(second, options));
        Assert.Contains("\"state\":\"observed\"", json);
        var decoded = JsonSerializer.Deserialize<VerificationBatch>(json, options)
            ?? throw new InvalidOperationException("Round-trip did not produce a verification batch.");
        Assert.Equal(VerificationState.Observed, Assert.Single(decoded.Decisions).State);
        Assert.Equal(new[] { "a", "z" }, Assert.Single(decoded.Decisions).EvidenceIds);
    }

    [Fact]
    public void DisabledRulesRemainExplicitlyNonClaims()
    {
        var decision = Only(Request(Change(), [Counter(9)], [Search()], mode: VerificationPolicyMode.Disabled));
        Assert.Equal(VerificationPolicyMode.Disabled, decision.AppliedPolicy);
        Assert.Equal(VerificationState.Unknown, decision.State);
        Assert.Contains("policy.disabled", decision.Frontier);
    }
}
