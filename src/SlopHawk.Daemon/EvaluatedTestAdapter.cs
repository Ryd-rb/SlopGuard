using System.Text.Json;

namespace SlopHawk.Daemon;

internal sealed class EvaluatedTestAdapter(ProjectEvaluator evaluator, IEnumerable<string> unitProjects,
    Func<TestAttempt, CancellationToken, Task<TestOutcome>> run) : IEvaluatedTestAdapter
{
    private readonly HashSet<string> approved = unitProjects.ToHashSet(Workspace.PathComparer);

    public Task<ProjectGraph> EvaluateAsync(WorkerLease worker, CancellationToken cancellationToken) =>
        evaluator.EvaluateAsync(worker, cancellationToken);

    public Task<TestSelection> SelectAsync(TestSelectionInput input, CancellationToken cancellationToken)
    {
        var projects = input.Projects ?? throw new IOException("Evaluated project inputs are missing.");
        var graph = new GraphIndex(projects.Current);
        var predecessors = projects.Predecessors.ToDictionary(pair => pair.Key, pair => new GraphIndex(pair.Value));
        var testTargets = projects.Current.Targets.Where(target => target.IsTestProject)
            .ToDictionary(target => target.Id, StringComparer.Ordinal);
        var eligible = testTargets.Values.Where(target => approved.Contains(target.ProjectPath))
            .Select(target => target.Id).ToHashSet(StringComparer.Ordinal);
        var missingApproved = approved.Except(testTargets.Values.Select(target => target.ProjectPath),
            Workspace.PathComparer).ToArray();
        var complete = projects.Current.Complete && missingApproved.Length == 0;
        var selected = new SortedDictionary<string, List<string>>(StringComparer.Ordinal);
        var explanations = new ExplanationBuilder(eligible, testTargets, projects.Current.Issues, missingApproved);

        foreach (var cause in input.Causes)
        {
            cancellationToken.ThrowIfCancellationRequested();
            var causeId = cause.Id;
            var current = graph.DependentsOf(cause.NewPath, cancellationToken);
            var impacted = new HashSet<string>(current.Targets, StringComparer.Ordinal);
            explanations.RecordWalk(causeId, cause.NewPath, graph, current, predecessor: null);
            if (cause.FallbackFirstRevision is not null)
            {
                impacted.UnionWith(eligible);
                explanations.RecordFallback(causeId, cause, TestSelectionReasonKind.FirstRevisionFallback);
            }
            else if (cause.OldPath is not null)
            {
                if (cause.Predecessor is { } key && predecessors.TryGetValue(key, out var prior))
                {
                    var previous = prior.DependentsOf(cause.OldPath, cancellationToken);
                    impacted.UnionWith(previous.Targets);
                    explanations.RecordWalk(causeId, cause.OldPath, prior, previous, key);
                    explanations.RecordIssues(key, prior.Issues);
                }
                else
                {
                    impacted.UnionWith(eligible);
                    explanations.RecordFallback(causeId, cause, TestSelectionReasonKind.MissingPredecessorFallback);
                }
            }
            impacted.IntersectWith(eligible);
            foreach (var target in impacted)
            {
                if (!selected.TryGetValue(target, out var causes))
                    selected.Add(target, causes = []);
                causes.Add(causeId);
            }
        }

        var targets = selected.Take(ValidatedSelection.MaxTargets)
            .Select(pair => new SelectedTarget(pair.Key, graph.Fingerprint(pair.Key, projects.SourceDigests), pair.Value))
            .ToArray();
        return Task.FromResult(new TestSelection(complete && selected.Count == targets.Length, targets)
        {
            Projection = explanations.Finish(selected.Count)
        });
    }

    public Task<TestOutcome> RunAsync(TestAttempt attempt, WorkerLease worker, CancellationToken cancellationToken) =>
        run(attempt, cancellationToken);

    private sealed class ExplanationBuilder
    {
        // Each graph is already limited by the inventory's 2 MiB serialization budget.
        // These independent presentation limits also bound explanation allocations.
        private const int MaxDeferred = 16;
        private const int MaxUnapproved = 16;
        private const int MaxReasons = 4;
        private const int MaxFrontiers = 16;
        private readonly HashSet<string> eligible;
        private readonly IReadOnlyDictionary<string, ProjectTarget> testTargets;
        private readonly SortedDictionary<string, CapturedTarget> approvedTargets = new(StringComparer.Ordinal);
        private readonly SortedDictionary<string, CapturedTarget> unapprovedTargets = new(StringComparer.Ordinal);
        private readonly SortedDictionary<string, TestSelectionFrontier> frontiers = new(StringComparer.Ordinal);
        private int frontierEvents;
        private readonly HashSet<string> unapprovedImpacted = new(StringComparer.Ordinal);
        private readonly HashSet<string> explainedPredecessors = new(StringComparer.Ordinal);
        private bool conservativeFallback;

        internal ExplanationBuilder(HashSet<string> eligible, IReadOnlyDictionary<string, ProjectTarget> testTargets,
            IReadOnlyList<ProjectEvaluationIssue> issues, IReadOnlyList<string> missingApproved)
        {
            this.eligible = eligible;
            this.testTargets = testTargets;
            RecordIssues(null, issues);
            foreach (var path in missingApproved)
                RecordFrontier(null, path, "approved_test_project_absent_current");
        }

        internal void RecordIssues(string? predecessor, IReadOnlyList<ProjectEvaluationIssue> issues)
        {
            if (predecessor is not null && !explainedPredecessors.Add(predecessor))
                return;
            foreach (var issue in issues)
                RecordFrontier(predecessor, issue.Path, issue.Reason);
        }

        internal void RecordWalk(string causeId, string? path, GraphIndex index, GraphIndex.DependencyWalk walk,
            string? predecessor)
        {
            if (path is null)
                return;
            foreach (var target in walk.Targets)
            {
                if (!testTargets.ContainsKey(target))
                {
                    if (predecessor is not null && index.IsTestTarget(target))
                        RecordFrontier(predecessor, path, "predecessor_test_target_absent_current");
                    continue;
                }
                if (Capture(target) is not { } captured)
                    continue;
                var (chain, truncated) = walk.ChainTo(target);
                var kind = (predecessor is not null, chain.Count == 1) switch
                {
                    (true, true) => TestSelectionReasonKind.PredecessorMembership,
                    (true, false) => TestSelectionReasonKind.PredecessorDependency,
                    (false, true) => TestSelectionReasonKind.CurrentMembership,
                    _ => TestSelectionReasonKind.CurrentDependency
                };
                captured.Add(new TestSelectionReason(causeId, kind, path, chain, truncated));
            }
        }

        internal void RecordFallback(string causeId, TestCause cause, TestSelectionReasonKind kind)
        {
            conservativeFallback = true;
            if (kind == TestSelectionReasonKind.MissingPredecessorFallback)
                RecordFrontier(cause.Predecessor, cause.OldPath ?? ".", "missing_predecessor");
            var reason = new TestSelectionReason(causeId, kind, cause.OldPath, [], false);
            foreach (var target in eligible)
                Capture(target)?.Add(reason);
        }

        private void RecordFrontier(string? predecessor, string path, string reason)
        {
            var key = string.Concat(predecessor ?? "", "\0", path, "\0", reason);
            if (frontiers.ContainsKey(key))
                return;
            frontierEvents++;
            if (frontiers.Count >= MaxFrontiers)
            {
                var last = frontiers.Keys.Last();
                if (string.CompareOrdinal(key, last) >= 0)
                    return;
                frontiers.Remove(last);
            }
            frontiers.Add(key, new TestSelectionFrontier(predecessor, path, reason));
        }

        private CapturedTarget? Capture(string target)
        {
            var isEligible = eligible.Contains(target);
            if (!isEligible)
                unapprovedImpacted.Add(target);
            var captured = isEligible ? approvedTargets : unapprovedTargets;
            var budget = isEligible ? ValidatedSelection.MaxTargets + MaxDeferred : MaxUnapproved;
            if (!captured.TryGetValue(target, out var explanation))
            {
                if (captured.Count >= budget)
                {
                    var last = captured.Keys.Last();
                    if (string.CompareOrdinal(target, last) >= 0)
                        return null;
                    captured.Remove(last);
                }
                captured.Add(target, explanation = new CapturedTarget());
            }
            return explanation;
        }

        internal TestSelectionProjection Finish(int selectedCount)
        {
            var targets = new List<TestTargetExplanation>();
            var index = 0;
            foreach (var (id, value) in approvedTargets)
            {
                targets.Add(value.Project(id, index++ < ValidatedSelection.MaxTargets
                    ? TestSelectionTargetScope.Selected : TestSelectionTargetScope.BudgetDeferred));
            }
            targets.AddRange(unapprovedTargets.Select(pair => pair.Value.Project(pair.Key,
                TestSelectionTargetScope.Unapproved)));
            var deferred = Math.Max(0, selectedCount - ValidatedSelection.MaxTargets);
            return new TestSelectionProjection(targets, frontiers.Values.ToArray(),
                Math.Max(0, selectedCount - approvedTargets.Count) +
                    Math.Max(0, unapprovedImpacted.Count - unapprovedTargets.Count),
                Math.Max(0, frontierEvents - frontiers.Count), selectedCount, deferred,
                unapprovedImpacted.Count, conservativeFallback);
        }

        private sealed class CapturedTarget
        {
            private readonly List<TestSelectionReason> reasons = [];
            private int total;

            internal void Add(TestSelectionReason reason)
            {
                total++;
                reasons.Add(reason);
                reasons.Sort((left, right) =>
                {
                    var order = string.CompareOrdinal(left.CauseId, right.CauseId);
                    if (order != 0) return order;
                    order = left.Kind.CompareTo(right.Kind);
                    if (order != 0) return order;
                    return string.CompareOrdinal(left.SourcePath, right.SourcePath);
                });
                if (reasons.Count > MaxReasons)
                    reasons.RemoveAt(reasons.Count - 1);
            }

            internal TestTargetExplanation Project(string id, TestSelectionTargetScope scope) =>
                new(id, scope, reasons.ToArray(), total - reasons.Count);
        }
    }

    private sealed class GraphIndex(ProjectGraph graph)
    {
        internal IReadOnlyList<ProjectEvaluationIssue> Issues => graph.Issues;
        internal bool IsTestTarget(string id) => targets.TryGetValue(id, out var target) && target.IsTestProject;

        private readonly Dictionary<string, ProjectTarget> targets = graph.Targets.ToDictionary(target => target.Id);
        private readonly ILookup<string, string> owners = graph.Targets
            .SelectMany(target => target.Compile.Select(member => (member.Path, target.Id)))
            .ToLookup(pair => pair.Path, pair => pair.Id, Workspace.PathComparer);
        private readonly ILookup<string, string> dependents = graph.Targets
            .SelectMany(target => target.References.Select(reference => (Reference: reference, target.Id)))
            .ToLookup(pair => pair.Reference, pair => pair.Id, StringComparer.Ordinal);

        internal DependencyWalk DependentsOf(string? path, CancellationToken cancellationToken)
        {
            var parents = new Dictionary<string, string?>(StringComparer.Ordinal);
            if (path is null)
                return new DependencyWalk(parents);
            var pending = new Queue<string>();
            foreach (var root in owners[path].Distinct(StringComparer.Ordinal).Order(StringComparer.Ordinal))
            {
                parents.Add(root, null);
                pending.Enqueue(root);
            }
            var traversed = 0;
            while (pending.TryDequeue(out var current))
            {
                if ((++traversed & 127) == 0)
                    cancellationToken.ThrowIfCancellationRequested();
                foreach (var related in dependents[current].Order(StringComparer.Ordinal))
                {
                    if (parents.TryAdd(related, current))
                        pending.Enqueue(related);
                }
            }
            return new DependencyWalk(parents);
        }

        internal sealed class DependencyWalk(Dictionary<string, string?> parents)
        {
            internal IEnumerable<string> Targets => parents.Keys;

            internal (IReadOnlyList<string> Chain, bool Truncated) ChainTo(string target)
            {
                const int maxChain = 8;
                var chain = new List<string>(maxChain);
                var current = target;
                // Keep the contiguous suffix ending in the selected target if the chain is too long.
                while (chain.Count < maxChain)
                {
                    chain.Add(current);
                    if (!parents.TryGetValue(current, out var parent) || parent is null)
                    {
                        chain.Reverse();
                        return (chain, false);
                    }
                    current = parent;
                }
                chain.Reverse();
                return (chain, true);
            }
        }

        internal string Fingerprint(string target, IReadOnlyDictionary<string, string> sourceDigests)
        {
            var contexts = Traverse([target], id => targets[id].References).Order(StringComparer.Ordinal)
                .Select(id => targets[id])
                .Select(context => new
                {
                    context.Id,
                    context.EvaluationFingerprint,
                    context.References,
                    Sources = context.Compile.Select(member => new
                    {
                        member.Path,
                        member.Link,
                        Digest = sourceDigests.GetValueOrDefault(member.Path)
                    })
                });
            return TestCause.Hash(JsonSerializer.Serialize(new { graph.Issues, Contexts = contexts }));
        }

        private static HashSet<string> Traverse(IEnumerable<string> roots, Func<string, IEnumerable<string>> edges)
        {
            var seen = new HashSet<string>(StringComparer.Ordinal);
            var pending = new Queue<string>(roots);
            while (pending.TryDequeue(out var current))
            {
                if (seen.Add(current))
                {
                    foreach (var related in edges(current))
                        pending.Enqueue(related);
                }
            }
            return seen;
        }
    }
}
