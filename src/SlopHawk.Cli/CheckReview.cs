using System.Text.Json;
using System.Text.Json.Serialization;
using SlopHawk.Core;

namespace SlopHawk.Cli;

internal sealed record CheckRun(string Format, string Run, string Candidate, string? RunVerb, string? PrepareVerb,
    ProbeCheckResult Result, JsonElement[]? Expected);
internal sealed record CheckAcknowledgment(string Run, [property: JsonRequired] int Index, string RetestRun,
    [property: JsonRequired] int RetestIndex, string Disposition, string Reason);
internal sealed record CheckAdapter(string Candidate, string? RunVerb, string? PrepareVerb);
internal sealed record CheckEvidence(CheckRun Report, ProbeInput[] Requests, JsonElement[] Observed)
{
    internal CheckAdapter Adapter => new(Report.Candidate, Report.RunVerb, Report.PrepareVerb);
}

internal static class CheckReview
{
    private sealed record RunHeader(string Format, string Run, string Candidate, string? RunVerb, string? PrepareVerb);
    private sealed record Finding(string Run, ProbeDifference Difference);
    private sealed record ReviewSource(string Operation, string Component, string Contract, string Implementation);
    private sealed record ReviewScope(string[] Operations, ProbeInput[] Ordinary, int MinimumOutsideOrdinary = 1,
        ReviewSource[]? Sources = null);

    internal static async Task<int> RunAsync(string[] args)
    {
        var progress = args[^1] == "--progress";
        if (progress)
            args = args[..^1];
        string? requestedRun = null;
        var runOption = Array.IndexOf(args, "--run");
        if (runOption >= 0)
        {
            if (runOption < 2 || runOption + 1 >= args.Length || !Guid.TryParseExact(args[runOption + 1], "N", out _))
                throw new ArgumentException("Supply a completed check run ID after --run.");
            requestedRun = args[runOption + 1];
            args = [.. args[..runOption], .. args[(runOption + 2)..]];
        }
        if (args.Length is < 2 or > 4)
            throw new ArgumentException("Usage: slophawk check-review <workspace> [acknowledgments.json [scope.json]] [--run <run>] [--progress]");
        var scope = args.Length == 4 ? await ReadAsync<ReviewScope>(args[3]) : null;
        if (scope is not null &&
            (scope.Operations is not { Length: > 0 } || scope.Operations.Any(string.IsNullOrWhiteSpace) ||
             scope.Ordinary is null || scope.Ordinary.Any(input => input is null || string.IsNullOrWhiteSpace(input.Operation) || input.Args is null) ||
             scope.MinimumOutsideOrdinary <= 0))
            throw new ArgumentException("The scope needs Operations, an Ordinary array of public examples, and a positive minimum if specified.");
        if (scope?.Sources is { } sources &&
            (sources.Any(source => source is null || !scope.Operations.Contains(source.Operation, StringComparer.Ordinal) ||
                string.IsNullOrWhiteSpace(source.Component) || string.IsNullOrWhiteSpace(source.Contract) ||
                string.IsNullOrWhiteSpace(source.Implementation)) ||
             sources.Select(source => source.Operation).Distinct(StringComparer.Ordinal).Count() != sources.Length))
            throw new ArgumentException("Each source entry needs a unique listed operation and its component, contract and implementation.");
        var ordinary = scope?.Ordinary.ToLookup(input => input.Operation, StringComparer.Ordinal);
        var directory = ForegroundCheck.ChecksPath(args[1]);
        if (!Directory.Exists(args[1]))
            throw new DirectoryNotFoundException("The workspace directory must already exist.");

        var runs = new Dictionary<string, CheckAdapter>(StringComparer.Ordinal);
        IEnumerable<string> runPaths = Directory.Exists(directory) ? Directory.EnumerateDirectories(directory).Order(StringComparer.Ordinal) : [];
        foreach (var runPath in runPaths)
        {
            var run = Path.GetFileName(runPath);
            var reportPath = Path.Combine(runPath, "report.json");
            if (!Guid.TryParseExact(run, "N", out _) || !File.Exists(reportPath))
                continue;
            var header = await ReadAsync<RunHeader>(reportPath);
            if (header.Format != ForegroundCheck.Format || header.Run != run || string.IsNullOrWhiteSpace(header.Candidate))
                throw new IOException($"Inconsistent checker evidence in run {run}.");
            runs.Add(run, new(header.Candidate, header.RunVerb, header.PrepareVerb));
        }
        if (runs.Count == 0 && requestedRun is null && (!progress || scope is null))
            throw new IOException("No completed checker reports are available.");

        var evidenceSets = runs.GroupBy(pair => pair.Value, pair => pair.Key).ToArray();
        var adapter = requestedRun is null
            ? evidenceSets.Length == 1 ? evidenceSets[0].Key : null
            : runs.TryGetValue(requestedRun, out var requested) ? requested
                : throw new ArgumentException($"Check run {requestedRun} was not found in this workspace.");
        var selectionRequired = adapter is null && evidenceSets.Length > 1;
        var selected = new Dictionary<string, CheckEvidence>(StringComparer.Ordinal);
        foreach (var (run, runAdapter) in runs)
        {
            if (requestedRun is not null && runAdapter != adapter)
                continue;
            var evidence = await ReadRunAsync(Path.Combine(directory, run), run);
            if (evidence.Adapter != runAdapter)
                throw new IOException($"Checker evidence changed during review of run {run}.");
            if (runAdapter == adapter)
                selected.Add(run, evidence);
        }
        var findings = selected.Values.SelectMany(run => run.Report.Result.Findings
            .Select(difference => new Finding(run.Report.Run, difference))).ToArray();
        CheckAcknowledgment[] acknowledged = args.Length >= 3 && !selectionRequired
            ? (await ReadAsync<CheckAcknowledgment[]>(args[2])).Where(item => item is null ||
                string.IsNullOrWhiteSpace(item.Run) || selected.ContainsKey(item.Run) || !runs.ContainsKey(item.Run)).ToArray() : [];
        var resolved = new Dictionary<(string Run, int Index), string>();
        foreach (var item in acknowledged)
        {
            if (item is null || string.IsNullOrWhiteSpace(item.Run) || string.IsNullOrWhiteSpace(item.RetestRun) ||
                !selected.TryGetValue(item.Run, out var original) || !selected.TryGetValue(item.RetestRun, out var retest) ||
                item.Run == item.RetestRun || retest.Report.Expected is null ||
                item.RetestIndex < 0 || item.RetestIndex >= retest.Requests.Length ||
                string.IsNullOrWhiteSpace(item.Reason))
                throw new ArgumentException("Each acknowledgment needs a finding, a separate retest of the same candidate and adapter, and a reason.");

            var finding = original.Report.Result.Findings.FirstOrDefault(difference => difference.Index == item.Index);
            if (finding is null || !SameInput(finding.Input, retest.Requests[item.RetestIndex]) ||
                !JsonElement.DeepEquals(finding.Candidate, retest.Observed[item.RetestIndex]))
                throw new ArgumentException($"The retest must reproduce the input and actual output for {item.Run}/{item.Index}.");
            var retestedExpectation = retest.Report.Expected[item.RetestIndex];
            var substantiated = item.Disposition switch
                {
                    "confirmed" => JsonElement.DeepEquals(finding.Reference, retestedExpectation),
                    "corrected-expectation" => JsonElement.DeepEquals(finding.Candidate, retestedExpectation),
                    _ => false
                };
            if (!substantiated)
                throw new ArgumentException($"The retest does not substantiate the disposition for {item.Run}/{item.Index}.");

            foreach (var equivalent in findings)
            {
                var difference = equivalent.Difference;
                if (!SameInput(finding.Input, difference.Input) ||
                    !JsonElement.DeepEquals(finding.Candidate, difference.Candidate) ||
                    !JsonElement.DeepEquals(finding.Reference, difference.Reference))
                    continue;
                var key = (equivalent.Run, difference.Index);
                if (resolved.TryGetValue(key, out var disposition) && disposition != item.Disposition)
                    throw new ArgumentException($"Conflicting dispositions for {equivalent.Run}/{difference.Index}.");
                resolved[key] = item.Disposition;
            }
        }

        var outstanding = findings.Where(finding => !resolved.ContainsKey((finding.Run, finding.Difference.Index))).ToArray();
        var observations = selected.Values.SelectMany(run => run.Requests.Select((input, index) =>
            (Input: input, Run: run.Report.Run, Observed: run.Observed[index], ObserveOnly: run.Report.Expected is null))).ToArray();
        var unsupportedComparisons = observations.Count(item => !item.ObserveOnly && item.Observed.ValueKind == JsonValueKind.Null);
        var unstable = observations.GroupBy(item => (item.Input.Operation, Args: string.Join(',', item.Input.Args)))
            .Where(group => group.Skip(1).Any(item => !JsonElement.DeepEquals(item.Observed, group.First().Observed)))
            .Select(group => new { group.First().Input, Observations = group.Select(item => new { item.Run, item.Observed }) }).ToArray();
        var coverage = observations.Select(item =>
                (item.Input, Supported: item.Observed.ValueKind is not JsonValueKind.Null, item.ObserveOnly))
            .GroupBy(item => item.Input.Operation, StringComparer.Ordinal).OrderBy(group => group.Key, StringComparer.Ordinal)
            .Select(group => new
            {
                Operation = group.Key,
                Compared = group.Count(item => item.Supported && !item.ObserveOnly),
                OutsideOrdinary = ordinary is null ? (int?)null : group
                    .Where(item => item.Supported && !item.ObserveOnly && !ordinary[group.Key].Any(example => SameInput(example, item.Input)))
                    .Select(item => string.Join(',', item.Input.Args)).Distinct(StringComparer.Ordinal).Count(),
                ObservedOnly = group.Count(item => item.Supported && item.ObserveOnly),
                Unsupported = group.Count(item => !item.Supported)
            }).ToArray();
        string[] missingCoverage = scope is null ? [] : scope.Operations
            .Except(coverage.Where(operation => operation.OutsideOrdinary >= scope.MinimumOutsideOrdinary)
                .Select(operation => operation.Operation), StringComparer.Ordinal)
            .Order(StringComparer.Ordinal).ToArray();
        var missingOperations = missingCoverage.ToHashSet(StringComparer.Ordinal);
        var worklist = scope?.Sources?.Where(source => missingOperations.Contains(source.Operation))
            .GroupBy(source => source.Component, StringComparer.Ordinal)
            .Select(group => new { Component = group.Key, Operations = group.ToArray() }).ToArray();
        var complete = !selectionRequired && unstable.Length == 0 && outstanding.Length == 0 &&
            unsupportedComparisons == 0 && missingCoverage.Length == 0 && coverage.Any(operation => operation.Compared > 0);
        var sets = evidenceSets.Select(set => new { Adapter = set.Key, Runs = set.ToArray() }).ToArray();
        Directory.CreateDirectory(directory);
        var review = Path.Combine(directory, "review.json");
        await ForegroundCheck.PublishJsonAsync(review, new
        {
            Format = "slophawk-check-review-v1",
            Complete = complete,
            ScopeProvided = scope is not null,
            RequiresRunSelection = selectionRequired,
            EvidenceSets = sets,
            Adapter = adapter,
            Runs = selected.Keys.ToArray(),
            Coverage = coverage,
            UnsupportedComparisons = unsupportedComparisons,
            UnstableInputs = unstable,
            MinimumOutsideOrdinary = scope?.MinimumOutsideOrdinary,
            MissingCoverage = missingCoverage,
            Worklist = worklist,
            Acknowledged = acknowledged,
            Outstanding = outstanding
        });
        Console.WriteLine(JsonSerializer.Serialize(new
        {
            Review = review, Complete = complete, RequiresRunSelection = selectionRequired, ScopeProvided = scope is not null,
            EvidenceSets = selectionRequired ? sets : null, UnstableInputs = unstable.Length,
            UnsupportedComparisons = unsupportedComparisons, Outstanding = outstanding.Length, MissingCoverage = missingCoverage.Length,
            NextComponent = worklist?.FirstOrDefault()?.Component
        }));
        return complete || progress ? 0 : 1;
    }

    private static bool SameInput(ProbeInput left, ProbeInput right) =>
        left.Operation == right.Operation && left.Args.AsSpan().SequenceEqual(right.Args);

    internal static async Task<CheckEvidence> ReadRunAsync(string path, string run)
    {
        var report = await ReadAsync<CheckRun>(Path.Combine(path, "report.json"));
        var requests = await ReadAsync<ProbeInput[]>(Path.Combine(path, "requests.json"));
        var observed = await ReadAsync<JsonElement[]>(Path.Combine(path, "observed.json"));
        if (report.Format != ForegroundCheck.Format || report.Run != run || string.IsNullOrWhiteSpace(report.Candidate) ||
            report.Result is null || requests.Any(input => input is null || string.IsNullOrWhiteSpace(input.Operation) || input.Args is null) ||
            requests.Length != observed.Length || (report.Expected is not null && requests.Length != report.Expected.Length))
            throw new IOException($"Inconsistent checker evidence in run {run}.");
        var recomputed = report.Expected is { } expected
            ? ProbeReplay.Check(requests.Select((input, index) => new ProbeExpectation(input.Operation, input.Args, expected[index]))
                .ToArray(), observed)
            : ForegroundCheck.Observe(requests.Length, observed);
        if (!JsonElement.DeepEquals(JsonSerializer.SerializeToElement(report.Result), JsonSerializer.SerializeToElement(recomputed)))
            throw new IOException($"Checker report disagrees with the saved inputs and observations in run {run}.");
        return new(report, requests, observed);
    }

    internal static async Task<T> ReadAsync<T>(string path)
    {
        await using var file = File.OpenRead(path);
        return await JsonSerializer.DeserializeAsync<T>(file, ForegroundCheck.Json)
            ?? throw new JsonException($"Missing checker data in {path}.");
    }
}
