using System.Diagnostics;
using System.Text.Json;
using SlopHawk.Core;
using SlopHawk.Providers.CSharp;

namespace SlopHawk.Cli;

internal sealed record ScalarBuildReceipt(string CompilerSha256, string RuntimeVersion, string CandidateSha256,
    ScalarCompilerInputs CompilerInputs);
internal sealed record PathCase(ProbeExpectation Expectation, string Origin, string? PredictedGoal, int? BaselineOrder);
internal sealed record PathMethodPlan(string Operation, string Signature, bool Bound, string? Reason, int Work,
    IReadOnlyList<string> Unknown, int RejectedByDomain, int BaselineCases, int GraphProposals);
internal sealed record PathPlan(PathInputs Inputs, string Scratch, string? Build, string? BuildSha256,
    string Mode, int PerOperation, int RetestLimit, long PlanMilliseconds, PathMethodPlan[] Methods,
    PathCase[] Cases, string? Blocker);
internal sealed record PathViolation(int OriginalIndex, string RetestRun, PathCase Case, JsonElement Observed, string Source);
internal sealed record PathScope(string Operation, string Signature, bool Bound, string? Reason,
    int UnknownFrontiers, int PlannedCases);
internal sealed record PathReview(string Plan, string PlanSha256, string Mode, string Outcome,
    int Cases, int Compared, int Unsupported, int Retests, int Unconfirmed, int Work, long PlanMilliseconds,
    long ExecutionMilliseconds, string? InitialRun, IReadOnlyList<PathViolation> Reproduced, int Unstable,
    string? Blocker, PathScope[] Scope);

internal static class PathCheck
{
    private const string CandidateName = "candidate.dll";
    private const string RuntimeConfig = "candidate.runtimeconfig.json";

    internal static Task<int> RunAsync(string[] args) => args switch
    {
        ["paths", "build", var spec, var scratch] => BuildAsync(spec, Path.GetFullPath(scratch)),
        ["paths", "plan", var spec, var scratch, var build, var mode] when mode is "graph-off" or "graph-on" =>
            PlanAsync(spec, Path.GetFullPath(scratch), build, mode),
        ["paths", "check", var plan] => CheckAsync(Path.GetFullPath(plan)),
        ["paths", "review", var review] => ReviewAsync(Path.GetFullPath(review)),
        _ => throw new ArgumentException("See slophawk --help for the path build, plan, check and review commands.")
    };

    private static async Task<int> BuildAsync(string spec, string scratch)
    {
        var context = PathContext.Open(spec, scratch);
        var binary = context.Compilation.Emit();
        var artifacts = new PathArtifacts(scratch, "path-builds");
        await File.WriteAllBytesAsync(Path.Combine(artifacts.Staging, CandidateName), binary);
        await artifacts.WriteAsync(RuntimeConfig, new
        {
            runtimeOptions = new
            {
                tfm = "net10.0", rollForward = "Disable",
                framework = new { name = "Microsoft.NETCore.App", version = context.Inputs.RuntimeVersion }
            }
        });
        await artifacts.WriteAsync("build.json", new ScalarBuildReceipt(context.Inputs.CompilerSha256,
            context.Inputs.RuntimeVersion, PathContext.Hash(binary), context.Compilation.Inputs));
        var manifest = await artifacts.PublishAsync("build");
        return Print(new { Build = manifest, Candidate = Path.Combine(Path.GetDirectoryName(manifest)!, CandidateName) });
    }

    private static async Task<int> PlanAsync(string spec, string scratch, string build, string mode)
    {
        var timer = Stopwatch.StartNew();
        var context = PathContext.Open(spec, scratch);
        string? candidate = null;
        string? blocker = "No controlled build was supplied.";
        string? buildHash = null;
        if (build != "-")
        {
            build = Path.GetFullPath(build);
            try
            {
                candidate = VerifiedCandidate(build, scratch, context, proveCompilation: true);
                buildHash = PathArtifacts.Hash(build);
                blocker = null;
            }
            catch (Exception ex) when (ex is IOException or ArgumentException or JsonException)
            {
                blocker = ex.Message;
            }
        }

        var methods = new List<PathMethodPlan>();
        var cases = new List<PathCase>();
        var graphs = new Dictionary<string, object?>(StringComparer.Ordinal);
        var baseline = new Dictionary<string, ProbeExpectation[]>(StringComparer.Ordinal);
        var proposals = new Dictionary<string, IReadOnlyList<ScalarPathCandidate>>(StringComparer.Ordinal);
        var arities = context.Catalog.ParameterCounts.Where(pair =>
            context.Spec.Targets.Any(target => target.Operation == pair.Key)).ToDictionary();
        IReadOnlyDictionary<string, long[]> literals = new Dictionary<string, long[]>();
        if (candidate is not null)
        {
            using var assembly = File.OpenRead(candidate);
            literals = ManagedBodyComparison.IntegerLiterals(assembly, arities);
        }
        using var analysisDeadline = new CancellationTokenSource(TimeSpan.FromSeconds(20));
        foreach (var target in context.Spec.Targets)
        {
            var seeds = context.Published.Where(item => item.Operation == target.Operation).ToArray();
            var qualified = arities.ContainsKey(target.Operation);
            var tryGraph = candidate is not null && qualified && mode == "graph-on";
            ScalarPathResult result;
            try
            {
                result = context.Compilation.Explore(target.Document, target.Signature,
                    seeds.Select(item => item.Args).ToArray(), context.Spec.WorkLimit,
                    tryGraph ? context.Spec.PerOperation : 0, analysisDeadline.Token);
            }
            catch (OperationCanceledException) when (analysisDeadline.IsCancellationRequested)
            {
                result = new(false, "Graph analysis reached its wall-time limit.", null, [], 0, ["wall-time limit"]);
            }
            graphs[target.Operation] = result.Graph;
            proposals[target.Operation] = result.Candidates;
            var supported = result.ScalarEligible && result.ParameterTypes?.Count == arities.GetValueOrDefault(target.Operation);
            if (candidate is null || !qualified || !supported)
            {
                methods.Add(new(target.Operation, target.Signature, result.Bound,
                    result.Reason ?? blocker ?? (!qualified ? "No qualified public rule." : "Unsupported scalar signature or rule arity."),
                    result.Work,
                    result.Unknown, 0, 0, result.Candidates.Count));
                continue;
            }

            var original = OriginalCases(context, target.Operation, seeds, literals, result.ParameterTypes!);
            baseline[target.Operation] = original;
            var ordinary = seeds.Select(CaseKey).ToHashSet(StringComparer.Ordinal);
            var graph = new List<(ProbeExpectation Case, string Goal)>();
            foreach (var proposal in result.Candidates)
            {
                var expected = context.Catalog.Evaluate(target.Operation, proposal.Args);
                if (expected is not null && Compatible(expected, result.ParameterTypes!))
                    graph.Add((expected, proposal.Goal));
            }
            cases.AddRange(Schedule(original, graph, ordinary, context.Spec.PerOperation));
            methods.Add(new(target.Operation, target.Signature, true, result.Reason, result.Work,
                result.Unknown, result.Candidates.Count - graph.Count, original.Length, result.Candidates.Count));
        }
        if (cases.Count == 0)
            blocker ??= "No bound target with admitted qualified cases.";
        var plan = new PathPlan(context.Inputs, scratch, candidate is null ? null : build, buildHash, mode,
            context.Spec.PerOperation, context.Spec.RetestLimit, timer.ElapsedMilliseconds, [.. methods], [.. cases], blocker);
        var artifacts = new PathArtifacts(scratch, "path-checks");
        await artifacts.WriteAsync("graph.json", new { BuildAttested = candidate is not null, Graphs = graphs });
        await artifacts.WriteAsync("plan.json", plan);
        await artifacts.WriteAsync("candidates.json", new { Baseline = baseline, Graph = proposals });
        var manifest = await artifacts.PublishAsync("plan");
        return Print(new { Plan = manifest, plan.Mode, Cases = cases.Count, plan.Blocker, plan.Methods },
            blocker is null ? 0 : 1);
    }

    private static ProbeExpectation[] OriginalCases(PathContext context, string operation, ProbeExpectation[] seeds,
        IReadOnlyDictionary<string, long[]> literals, IReadOnlyList<string> parameterTypes)
    {
        var limit = context.Spec.PerOperation;
        var selected = new HashSet<string>(StringComparer.Ordinal) { operation };
        var warmup = Math.Min(64, limit / 2);
        var ordinarySearch = warmup == 0 ? [] : context.Catalog.Generate(warmup, 0, selected);
        var guidedSearch = context.Catalog.Generate(limit, context.Spec.Seed, selected,
            guideWithBoundaries: true, implementationLiterals: literals);
        return seeds.Concat(ordinarySearch).Concat(guidedSearch)
            .Where(item => Compatible(item, parameterTypes)).DistinctBy(CaseKey).Take(limit).ToArray();
    }

    private static IEnumerable<PathCase> Schedule(ProbeExpectation[] baseline,
        List<(ProbeExpectation Case, string Goal)> graph, HashSet<string> ordinary, int limit)
    {
        var baselineIndices = baseline.Select((item, index) => (Key: CaseKey(item), Index: index))
            .ToDictionary(item => item.Key, item => item.Index, StringComparer.Ordinal);
        var seen = new HashSet<string>(StringComparer.Ordinal);
        var original = 0;
        while (original < baseline.Length && ordinary.Contains(CaseKey(baseline[original])))
        {
            var item = baseline[original];
            seen.Add(CaseKey(item));
            yield return new(item, "ordinary", null, original++);
        }

        var graphIndex = 0;
        while (seen.Count < limit && (original < baseline.Length || graphIndex < graph.Count))
        {
            if (graphIndex < graph.Count)
            {
                var proposal = graph[graphIndex++];
                var key = CaseKey(proposal.Case);
                if (seen.Add(key))
                {
                    var inBaseline = baselineIndices.TryGetValue(key, out var rank);
                    yield return new(proposal.Case, inBaseline ? "graph-shared" : "graph-new", proposal.Goal,
                        inBaseline ? rank : null);
                }
            }
            for (var added = 0; added < 2 && seen.Count < limit && original < baseline.Length; original++)
            {
                var item = baseline[original];
                if (!seen.Add(CaseKey(item)))
                    continue;
                yield return new(item, "baseline", null, original);
                added++;
            }
        }
    }

    private static async Task<int> CheckAsync(string manifest)
    {
        var plan = ReadPlan(manifest);
        var context = CurrentContext(plan);
        var candidate = VerifiedCandidate(plan.Build!, plan.Scratch, context, proveCompilation: false);
        var candidateHash = PathArtifacts.Hash(candidate);
        var artifacts = new PathArtifacts(plan.Scratch, "path-checks");
        var cases = plan.Cases.Select(item => item.Expectation).ToArray();
        var timer = Stopwatch.StartNew();
        var stable = new List<PathViolation>();
        var changes = new List<object>();
        CheckReceipt? initial = null;
        var retests = 0;
        string? blocker = null;
        using var runDeadline = new CancellationTokenSource(TimeSpan.FromSeconds(60));
        try
        {
            (initial, _) = await RunAsync(cases);
            var retestOrder = initial.Result.Findings.GroupBy(finding => finding.Input.Operation, StringComparer.Ordinal)
                .SelectMany(group => group.Select((finding, rank) => (Finding: finding, Rank: rank)))
                .OrderBy(item => item.Rank).ThenBy(item => item.Finding.Index);
            foreach (var (discrepancy, _) in retestOrder.Take(plan.RetestLimit))
            {
                var item = plan.Cases[discrepancy.Index];
                var (replay, values) = await RunAsync([item.Expectation]);
                retests++;
                if (JsonElement.DeepEquals(discrepancy.Candidate, values[0]))
                {
                    var source = context.Catalog.Qualification.Single(rule => rule.Operation == item.Expectation.Operation).Source!;
                    stable.Add(new(discrepancy.Index, replay.Run, item, values[0], source));
                }
                else
                    changes.Add(new { discrepancy.Index, Before = discrepancy.Candidate, After = values[0], Retest = replay.Run });
            }
            VerifiedCandidate(plan.Build!, plan.Scratch, CurrentContext(plan), proveCompilation: false);
        }
        catch (Exception ex) when (ex is IOException or ArgumentException or JsonException or UnauthorizedAccessException
                                   or System.ComponentModel.Win32Exception or OverflowException)
        {
            blocker = ex.Message;
            stable.Clear();
        }
        var pending = (initial?.Result.Findings.Count ?? 0) - stable.Count - changes.Count;
        var unsupported = initial?.Result.Unsupported ?? 0;
        var outcome = blocker is not null || unsupported != 0 || pending != 0 || changes.Count != 0
            ? "incomplete" : stable.Count != 0 ? "reproduced-discrepancy" : "no-discrepancy-in-scope";
        var review = new PathReview(manifest, PathArtifacts.Hash(manifest), plan.Mode, outcome, cases.Length,
            initial?.Result.Compared ?? 0, unsupported, retests, pending, plan.Methods.Sum(method => method.Work),
            plan.PlanMilliseconds, timer.ElapsedMilliseconds, initial?.Run, stable, changes.Count, blocker,
            plan.Methods.Select(method => new PathScope(method.Operation, method.Signature, method.Bound, method.Reason,
                method.Unknown.Count, cases.Count(item => item.Operation == method.Operation))).ToArray());
        await artifacts.WriteAsync("review.json", review);
        await artifacts.WriteAsync("unstable.json", changes);
        var published = await artifacts.PublishAsync("review");
        return Print(new { Review = published, Result = review }, outcome == "no-discrepancy-in-scope" ? 0 : 1);

        Task<(CheckReceipt Receipt, JsonElement[] Observed)> RunAsync(ProbeExpectation[] input) =>
            ForegroundCheck.ExecuteAsync(candidate, plan.Scratch, context.Spec.RunVerb, context.Spec.PrepareVerb,
                ProbeReplay.InputsForCheck(input), input, candidateHash, artifacts.Staging, runDeadline.Token);
    }

    private static Task<int> ReviewAsync(string manifest)
    {
        var path = PathArtifacts.Read(manifest, "review", "review.json", "unstable.json");
        var review = PathArtifacts.ReadJson<PathReview>(Path.Combine(path, "review.json"));
        if (PathArtifacts.Hash(review.Plan) != review.PlanSha256)
            throw new IOException("The review does not match the referenced plan.");
        var plan = ReadPlan(review.Plan);
        string? stale = null;
        try
        {
            var context = CurrentContext(plan);
            VerifiedCandidate(plan.Build!, plan.Scratch, context, proveCompilation: false);
        }
        catch (Exception ex) when (ex is IOException or ArgumentException or JsonException)
        {
            stale = ex.Message;
        }
        return Task.FromResult(Print(new { Review = manifest, Current = stale is null, HistoricalReason = stale, Result = review },
            stale is null && review.Outcome == "no-discrepancy-in-scope" ? 0 : 1));
    }

    private static PathPlan ReadPlan(string manifest)
    {
        var path = PathArtifacts.Read(manifest, "plan", "plan.json", "graph.json", "candidates.json");
        var plan = PathArtifacts.ReadJson<PathPlan>(Path.Combine(path, "plan.json"));
        if (plan.Blocker is not null || plan.Build is null || plan.Cases.Length == 0 ||
            !PathArtifacts.IsWithin(Path.Combine(plan.Scratch, ".slophawk", "path-checks"), path))
            throw new IOException(plan.Blocker ?? "The path plan has no runnable cases in this workspace.");
        return plan;
    }

    private static PathContext CurrentContext(PathPlan plan)
    {
        var context = PathContext.Open(plan.Inputs.Spec, plan.Scratch);
        if (context.Inputs != plan.Inputs || PathArtifacts.Hash(plan.Build!) != plan.BuildSha256)
            throw new IOException("The compiler, build, runtime, rules or plan inputs changed. Create a fresh plan.");
        return context;
    }

    private static string VerifiedCandidate(string manifest, string scratch, PathContext context, bool proveCompilation)
    {
        var path = PathArtifacts.Read(manifest, "build", "build.json", CandidateName, RuntimeConfig);
        var receipt = PathArtifacts.ReadJson<ScalarBuildReceipt>(Path.Combine(path, "build.json"));
        var candidate = Path.Combine(path, CandidateName);
        if (!PathArtifacts.IsWithin(Path.Combine(scratch, ".slophawk", "path-builds"), path) ||
            receipt.CompilerSha256 != context.Inputs.CompilerSha256 || receipt.RuntimeVersion != context.Inputs.RuntimeVersion ||
            receipt.CandidateSha256 != PathArtifacts.Hash(candidate) ||
            (proveCompilation && receipt.CandidateSha256 != PathContext.Hash(context.Compilation.Emit())))
            throw new IOException("The candidate must be the verified output of this controlled compilation.");
        return candidate;
    }

    private static string CaseKey(ProbeExpectation item) => string.Join(',', item.Args);

    private static bool Compatible(ProbeExpectation test, IReadOnlyList<string> types)
    {
        if (test.Args.Length != types.Count)
            return false;
        for (var index = 0; index < test.Args.Length; index++)
            if (test.Args[index] is < int.MinValue or > int.MaxValue ||
                types[index] == "bool" && test.Args[index] is not (0 or 1))
                return false;
        return true;
    }

    private static int Print<T>(T value, int exitCode = 0)
    {
        Console.WriteLine(JsonSerializer.Serialize(value));
        return exitCode;
    }
}
