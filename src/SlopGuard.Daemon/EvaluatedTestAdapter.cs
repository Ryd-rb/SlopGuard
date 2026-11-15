using System.Text.Json;

namespace SlopGuard.Daemon;

internal sealed class EvaluatedTestAdapter(ProjectEvaluator evaluator, IEnumerable<string> unitProjects,
    Func<TestAttempt, CancellationToken, Task<TestOutcome>> run) : IEvaluatedTestAdapter
{
    private readonly HashSet<string> approved = unitProjects.ToHashSet(Workspace.PathComparer);

    public Task<ProjectGraph> EvaluateAsync(CancellationToken cancellationToken) =>
        evaluator.EvaluateAsync(cancellationToken);

    public Task<TestSelection> SelectAsync(TestSelectionInput input, CancellationToken cancellationToken)
    {
        var projects = input.Projects ?? throw new IOException("Evaluated project inputs are missing.");
        var graph = new GraphIndex(projects.Current);
        var predecessors = projects.Predecessors.ToDictionary(pair => pair.Key, pair => new GraphIndex(pair.Value));
        var eligible = projects.Current.Targets.Where(target => target.IsTestProject && approved.Contains(target.ProjectPath))
            .Select(target => target.Id).ToHashSet(StringComparer.Ordinal);
        var complete = projects.Current.Complete &&
            approved.IsSubsetOf(projects.Current.Targets.Where(target => target.IsTestProject).Select(target => target.ProjectPath));
        var selected = new SortedDictionary<string, List<string>>(StringComparer.Ordinal);
        foreach (var cause in input.Causes)
        {
            cancellationToken.ThrowIfCancellationRequested();
            var impacted = graph.DependentsOf(cause.NewPath);
            if (cause.FallbackFirstRevision is not null)
                impacted.UnionWith(eligible);
            else if (cause.OldPath is not null)
            {
                if (cause.Predecessor is { } key && predecessors.TryGetValue(key, out var prior))
                    impacted.UnionWith(prior.DependentsOf(cause.OldPath));
                else
                    impacted.UnionWith(eligible);
            }
            impacted.IntersectWith(eligible);
            foreach (var target in impacted)
            {
                if (!selected.TryGetValue(target, out var causes))
                    selected.Add(target, causes = []);
                causes.Add(cause.Id);
            }
        }

        var targets = selected.Take(ValidatedSelection.MaxTargets)
            .Select(pair => new SelectedTarget(pair.Key, graph.Fingerprint(pair.Key, projects.SourceDigests), pair.Value))
            .ToArray();
        return Task.FromResult(new TestSelection(complete && selected.Count == targets.Length, targets));
    }

    public Task<TestOutcome> RunAsync(TestAttempt attempt, CancellationToken cancellationToken) =>
        run(attempt, cancellationToken);

    private sealed class GraphIndex(ProjectGraph graph)
    {
        private readonly Dictionary<string, ProjectTarget> targets = graph.Targets.ToDictionary(target => target.Id);
        private readonly ILookup<string, string> owners = graph.Targets
            .SelectMany(target => target.Compile.Select(member => (member.Path, target.Id)))
            .ToLookup(pair => pair.Path, pair => pair.Id, Workspace.PathComparer);
        private readonly ILookup<string, string> dependents = graph.Targets
            .SelectMany(target => target.References.Select(reference => (Reference: reference, target.Id)))
            .ToLookup(pair => pair.Reference, pair => pair.Id, StringComparer.Ordinal);

        internal HashSet<string> DependentsOf(string? path) =>
            path is null ? new HashSet<string>(StringComparer.Ordinal) : Traverse(owners[path], id => dependents[id]);

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
