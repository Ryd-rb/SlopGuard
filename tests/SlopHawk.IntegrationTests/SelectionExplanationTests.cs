using SlopHawk.Daemon;
using Xunit;

namespace SlopHawk.IntegrationTests;

public sealed class SelectionExplanationTests
{
    [Fact]
    public async Task LinkedSourceUsesEachEvaluatedTargetAndRecordsDirectAndTransitiveReasons()
    {
        var graph = Graph(
            Target("Library/Library.csproj|net8.0", false, ["Shared/Linked.cs"]),
            Target("Library/Library.csproj|net10.0", false, ["Shared/Linked.cs"]),
            Target("Middle/Middle.csproj|net8.0", false, [], "Library/Library.csproj|net8.0"),
            Target("Tests/Tests.csproj|net8.0", true, [], "Middle/Middle.csproj|net8.0"),
            Target("Tests/Tests.csproj|net10.0", true, ["Shared/Linked.cs"], "Library/Library.csproj|net10.0"));
        var cause = Cause(newPath: "Shared/Linked.cs");
        var result = await SelectAsync(graph, [cause], "Tests/Tests.csproj");

        Assert.True(result.Complete);
        Assert.Equal(new[] { "Tests/Tests.csproj|net10.0", "Tests/Tests.csproj|net8.0" },
            result.Targets.Select(target => target.Id));
        var net8 = Explain(result, "Tests/Tests.csproj|net8.0");
        Assert.Equal(new[] { "Library/Library.csproj|net8.0", "Middle/Middle.csproj|net8.0",
            "Tests/Tests.csproj|net8.0" },
            Assert.Single(net8.Reasons).TargetReferenceChain);
        Assert.Equal(TestSelectionReasonKind.CurrentDependency, Assert.Single(net8.Reasons).Kind);
        var direct = Explain(result, "Tests/Tests.csproj|net10.0");
        Assert.Equal(TestSelectionReasonKind.CurrentMembership, Assert.Single(direct.Reasons).Kind);
        Assert.Equal(new[] { "Tests/Tests.csproj|net10.0" }, Assert.Single(direct.Reasons).TargetReferenceChain);
    }

    [Fact]
    public async Task MovePreservesBothCurrentAndPredecessorChainsAndTheSingleCauseBinding()
    {
        var test = Target("Tests/Tests.csproj|net10.0", true, [], "Library/Library.csproj|net10.0");
        var current = Graph(Target("Library/Library.csproj|net10.0", false, ["New.cs"]), test);
        var previous = Graph(Target("Library/Library.csproj|net10.0", false, ["Old.cs"]), test);
        var moved = Cause(oldPath: "Old.cs", newPath: "New.cs");
        var result = await SelectAsync(current, [moved], ["Tests/Tests.csproj"], ("graph:0", previous));

        var selected = Assert.Single(result.Targets);
        Assert.Equal(new[] { moved.Id }, selected.CauseIds);
        var reasons = Explain(result, selected.Id).Reasons;
        Assert.Equal(2, reasons.Count);
        Assert.Contains(reasons, reason => reason.Kind == TestSelectionReasonKind.CurrentDependency &&
            reason.SourcePath == "New.cs");
        Assert.Contains(reasons, reason => reason.Kind == TestSelectionReasonKind.PredecessorDependency &&
            reason.SourcePath == "Old.cs");
    }

    [Fact]
    public async Task DeletedSourceUsesOldGraphAndMissingPredecessorFallsBackConservatively()
    {
        var test = Target("Tests/Tests.csproj|net10.0", true, [], "Lib/Lib.csproj|net10.0");
        var current = Graph(Target("Lib/Lib.csproj|net10.0", false, []), test);
        var previous = Graph(Target("Lib/Lib.csproj|net10.0", false, ["Deleted.cs"]), test);
        var deleted = Cause(oldPath: "Deleted.cs");
        var found = await SelectAsync(current, [deleted], ["Tests/Tests.csproj"], ("graph:0", previous));
        Assert.Equal(TestSelectionReasonKind.PredecessorDependency, Assert.Single(Explain(found,
            "Tests/Tests.csproj|net10.0").Reasons).Kind);

        var missing = await SelectAsync(current, [deleted], "Tests/Tests.csproj");
        Assert.True(missing.Complete); // The existing conservative selection remains complete.
        Assert.Single(missing.Targets);
        Assert.True(missing.Projection!.ConservativeFallback);
        Assert.Equal(TestSelectionReasonKind.MissingPredecessorFallback,
            Assert.Single(Explain(missing, "Tests/Tests.csproj|net10.0").Reasons).Kind);
        Assert.Contains(missing.Projection.UnresolvedFrontiers, frontier => frontier.Reason == "missing_predecessor");
    }

    [Fact]
    public async Task FirstRevisionFallbackSelectsEligibleGroupWithoutInventingAPath()
    {
        var graph = Graph(Target("Tests/A.csproj|net10.0", true, []),
            Target("Tests/B.csproj|net10.0", true, []));
        var cause = new TestCause(4, "fallback", null, null, null, null, "graph_unavailable", 1);
        var result = await SelectAsync(graph, [cause], "Tests/A.csproj", "Tests/B.csproj");
        Assert.True(result.Complete);
        Assert.Equal(2, result.Targets.Count);
        Assert.All(result.Projection!.Targets, target =>
        {
            var reason = Assert.Single(target.Reasons);
            Assert.Equal(TestSelectionReasonKind.FirstRevisionFallback, reason.Kind);
            Assert.Empty(reason.TargetReferenceChain);
        });
    }

    [Fact]
    public async Task CyclesTerminateAndUnapprovedAndUnresolvedScopesStayVisible()
    {
        var library = Target("Library/Lib.csproj|net10.0", false, ["Changed.cs"], "Other/Other.csproj|net10.0");
        var other = Target("Other/Other.csproj|net10.0", false, [], "Library/Lib.csproj|net10.0");
        var approved = Target("Tests/A.csproj|net10.0", true, [], "Other/Other.csproj|net10.0");
        var unapproved = Target("Tests/B.csproj|net10.0", true, [], "Library/Lib.csproj|net10.0");
        var current = new ProjectGraph([library, other, approved, unapproved],
            [new ProjectEvaluationIssue("Other/Other.csproj", "unresolved_reference_context")]);
        var result = await SelectAsync(current, [Cause(newPath: "Changed.cs")], "Tests/A.csproj");
        Assert.False(result.Complete);
        Assert.Equal("Tests/A.csproj|net10.0", Assert.Single(result.Targets).Id);
        Assert.Equal(new[] { "Library/Lib.csproj|net10.0", "Other/Other.csproj|net10.0",
            "Tests/A.csproj|net10.0" }, Assert.Single(Explain(result, "Tests/A.csproj|net10.0").Reasons).TargetReferenceChain);
        Assert.Equal(TestSelectionTargetScope.Unapproved, Explain(result, "Tests/B.csproj|net10.0").Scope);
        Assert.Equal(1, result.Projection!.UnapprovedAffectedTargets);
        Assert.Contains(result.Projection.UnresolvedFrontiers, frontier =>
            frontier.Reason == "unresolved_reference_context");
    }

    [Fact]
    public async Task OverBudgetKeepsExactlyTheOriginalLexicographicSixteenAndShowsDeferredTargets()
    {
        var root = Target("Library/Lib.csproj|net10.0", false, ["Changed.cs"]);
        var tests = Enumerable.Range(0, 40).Select(number => Target($"Tests/T{number:00}.csproj|net10.0",
            true, [], root.Id)).ToArray();
        var result = await SelectAsync(Graph([root, .. tests]), [Cause(newPath: "Changed.cs")],
            tests.Select(target => target.ProjectPath).ToArray());

        Assert.False(result.Complete);
        Assert.Equal(ValidatedSelection.MaxTargets, result.Targets.Count);
        Assert.Equal(Enumerable.Range(0, 16).Select(number => $"Tests/T{number:00}.csproj|net10.0"),
            result.Targets.Select(target => target.Id));
        Assert.Equal(40, result.Projection!.ApprovedAffectedTargets);
        Assert.Equal(24, result.Projection.BudgetDeferredTargets);
        Assert.Equal(16, result.Projection.Targets.Count(target => target.Scope == TestSelectionTargetScope.BudgetDeferred));
        Assert.Equal(8, result.Projection.OmittedTargetExplanations);
    }

    [Fact]
    public async Task LongChainAndReasonOverflowReportTruncationAndOmissions()
    {
        var chain = new List<ProjectTarget> { Target("P00|net10.0", false, ["Changed.cs"]) };
        for (var index = 1; index < 12; index++)
            chain.Add(Target($"P{index:00}|net10.0", false, [], chain[^1].Id));
        chain.Add(Target("Tests/Tests.csproj|net10.0", true, [], chain[^1].Id));
        var causes = Enumerable.Range(0, 8).Select(index => Cause(newPath: "Changed.cs", revision: index + 1)).ToArray();
        var result = await SelectAsync(Graph([.. chain]), causes, "Tests/Tests.csproj");
        var target = Explain(result, "Tests/Tests.csproj|net10.0");
        Assert.Equal(4, target.Reasons.Count);
        Assert.Equal(4, target.OmittedReasons);
        Assert.All(target.Reasons, reason =>
        {
            Assert.True(reason.ChainTruncated);
            Assert.Equal(8, reason.TargetReferenceChain.Count);
            Assert.Equal(target.TargetId, reason.TargetReferenceChain[^1]);
        });
        Assert.Equal(causes.Select(cause => cause.Id).Order(StringComparer.Ordinal).Take(4),
            target.Reasons.Select(reason => reason.CauseId));
    }

    [Fact]
    public async Task FormerTestTargetMissingFromCurrentGraphIsAnExplicitFrontier()
    {
        var old = Graph(Target("Tests/Removed.csproj|net10.0", true, ["Removed.cs"]));
        var current = Graph(Target("Tests/StillHere.csproj|net10.0", true, []));
        var result = await SelectAsync(current, [Cause(oldPath: "Removed.cs")],
            ["Tests/StillHere.csproj"], ("graph:0", old));

        Assert.True(result.Complete); // Diagnostics must not silently change legacy admission.
        Assert.Empty(result.Targets);
        Assert.Contains(result.Projection!.UnresolvedFrontiers, frontier =>
            frontier.Predecessor == "graph:0" && frontier.Path == "Removed.cs" &&
            frontier.Reason == "predecessor_test_target_absent_current");
    }

    [Fact]
    public async Task UnresolvedFrontiersAreSortedAndBoundedWithAnOmissionCount()
    {
        var issues = Enumerable.Range(0, 24)
            .Select(number => new ProjectEvaluationIssue($"Project/{number:00}.csproj", "unresolved_reference_context"))
            .Reverse().ToArray();
        var current = new ProjectGraph([], issues);
        var result = await SelectAsync(current, [Cause(newPath: "A.cs")]);

        Assert.False(result.Complete);
        Assert.Empty(result.Targets);
        Assert.Equal(16, result.Projection!.UnresolvedFrontiers.Count);
        Assert.Equal(8, result.Projection.OmittedFrontiers);
        Assert.Equal(Enumerable.Range(0, 16).Select(number => $"Project/{number:00}.csproj"),
            result.Projection.UnresolvedFrontiers.Select(frontier => frontier.Path));
    }

    [Fact]
    public async Task CompleteEmptyAndEquivalentSelectionPreserveExistingFingerprintsAndCauses()
    {
        var graph = Graph(Target("Lib|net10.0", false, ["Owned.cs"]),
            Target("Tests/Tests.csproj|net10.0", true, [], "Lib|net10.0"));
        var unrelated = await SelectAsync(graph, [Cause(newPath: "Unowned.cs")], "Tests/Tests.csproj");
        Assert.True(unrelated.Complete);
        Assert.Empty(unrelated.Targets);
        Assert.Empty(unrelated.Projection!.Targets);

        var cause = Cause(newPath: "Owned.cs");
        var first = await SelectAsync(graph, [cause], "Tests/Tests.csproj");
        var second = await SelectAsync(graph, [cause], "Tests/Tests.csproj");
        Assert.Equal(Assert.Single(first.Targets).Id, Assert.Single(second.Targets).Id);
        Assert.Equal(Assert.Single(first.Targets).InputFingerprint, Assert.Single(second.Targets).InputFingerprint);
        Assert.Equal(Assert.Single(first.Targets).CauseIds, Assert.Single(second.Targets).CauseIds);
        Assert.Equal(new[] { cause.Id }, Assert.Single(first.Targets).CauseIds);
        Assert.Null(new TestSelection(true, []).Projection); // Legacy/fake adapter default.
    }

    [Fact]
    public async Task MissingApprovedProjectExplainsIncompleteSelection()
    {
        const string missing = "Tests/Missing.csproj";
        var current = Graph(Target("Tests/Present.csproj|net10.0", true, ["Changed.cs"]));
        var result = await SelectAsync(current, [Cause(newPath: "Changed.cs")],
            "Tests/Present.csproj", missing);

        Assert.False(result.Complete);
        Assert.Equal("Tests/Present.csproj|net10.0", Assert.Single(result.Targets).Id);
        var frontier = Assert.Single(result.Projection!.UnresolvedFrontiers);
        Assert.Equal(missing, frontier.Path);
        Assert.Equal("approved_test_project_absent_current", frontier.Reason);
    }

    private static TestTargetExplanation Explain(TestSelection result, string id) =>
        Assert.Single(result.Projection!.Targets, target => target.TargetId == id);

    private static TestCause Cause(string? oldPath = null, string? newPath = null, long revision = 1) =>
        new(revision, oldPath ?? newPath ?? "none", oldPath, oldPath is null ? null : "old-digest",
            newPath, newPath is null ? null : "new-digest", "graph:0");

    private static ProjectTarget Target(string id, bool test, string[] members, params string[] references) =>
        new(id, id.Split('|')[0], id.Split('|')[1], "Debug", "AnyCPU", "", "10.0.404", test,
            members.Select(path => new CompileMember(path, path.StartsWith("Shared/", StringComparison.Ordinal)
                ? "Linked.cs" : null)).ToArray(), references, "eval:" + id);

    private static ProjectGraph Graph(params ProjectTarget[] targets) => new(targets, []);

    private static Task<TestSelection> SelectAsync(ProjectGraph graph, TestCause[] causes, params string[] approved) =>
        SelectAsync(graph, causes, approved, []);

    private static Task<TestSelection> SelectAsync(ProjectGraph graph, TestCause[] causes, string[] approved,
        params (string Key, ProjectGraph Graph)[] predecessors)
    {
        var adapter = new EvaluatedTestAdapter(null!, approved, (_, _) => Task.FromResult(TestOutcome.Passed));
        return adapter.SelectAsync(new TestSelectionInput(null!, causes,
            new EvaluatedSelectionInputs(graph, predecessors.ToDictionary(pair => pair.Key, pair => pair.Graph),
                new Dictionary<string, string>(Workspace.PathComparer))), CancellationToken.None);
    }
}
