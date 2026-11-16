using System.Text.Json;
using System.Text.Json.Serialization;
using SlopHawk.Core;

namespace SlopHawk.Cli;

internal sealed record CheckRun(string Format, string Run, string Candidate, string? RunVerb, string? PrepareVerb,
    ProbeCheckResult Result, JsonElement[] Expected);

internal static class CheckReview
{
    private sealed record Evidence(CheckRun Report, ProbeInput[] Requests, JsonElement[] Observed);
    private sealed record Acknowledgment(string Run, [property: JsonRequired] int Index, string RetestRun,
        [property: JsonRequired] int RetestIndex, string Disposition, string Reason);
    private sealed record Finding(string Run, ProbeDifference Difference);

    internal static async Task<int> RunAsync(string[] args)
    {
        if (args.Length is < 2 or > 3)
            throw new ArgumentException("Usage: slophawk check-review <workspace> [acknowledgments.json]");
        var directory = ForegroundCheck.ChecksPath(args[1]);
        if (!Directory.Exists(directory))
            throw new DirectoryNotFoundException("Run slophawk check before reviewing its reports.");

        var runs = new Dictionary<string, Evidence>(StringComparer.Ordinal);
        foreach (var runPath in Directory.EnumerateDirectories(directory).Order(StringComparer.Ordinal))
        {
            var run = Path.GetFileName(runPath);
            var reportPath = Path.Combine(runPath, "report.json");
            if (!Guid.TryParseExact(run, "N", out _) || !File.Exists(reportPath))
                continue;
            var report = await ReadAsync<CheckRun>(reportPath);
            var requests = await ReadAsync<ProbeInput[]>(Path.Combine(runPath, "requests.json"));
            var observed = await ReadAsync<JsonElement[]>(Path.Combine(runPath, "observed.json"));
            if (report.Format != ForegroundCheck.Format || report.Run != run ||
                report.Result.Cases != requests.Length || requests.Length != observed.Length ||
                requests.Length != report.Expected.Length)
                throw new IOException($"Inconsistent checker evidence in run {run}.");
            runs.Add(run, new(report, requests, observed));
        }
        if (runs.Count == 0)
            throw new IOException("No completed checker reports are available.");

        var findings = runs.Values.SelectMany(run => run.Report.Result.Findings
            .Select(difference => new Finding(run.Report.Run, difference))).ToArray();
        Acknowledgment[] acknowledged = args.Length == 3 ? await ReadAsync<Acknowledgment[]>(args[2]) : [];
        var resolved = new Dictionary<(string Run, int Index), string>();
        foreach (var item in acknowledged)
        {
            if (item is null || string.IsNullOrWhiteSpace(item.Run) || string.IsNullOrWhiteSpace(item.RetestRun) ||
                !runs.TryGetValue(item.Run, out var original) || !runs.TryGetValue(item.RetestRun, out var retest) ||
                item.Run == item.RetestRun || !SameAdapter(original, retest) ||
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
                if (!SameAdapter(original, runs[equivalent.Run]) || !SameInput(finding.Input, difference.Input) ||
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
        var coverage = runs.Values.SelectMany(run => run.Requests.Select((input, index) =>
                (input.Operation, Compared: run.Observed[index].ValueKind is not JsonValueKind.Null)))
            .GroupBy(item => item.Operation, StringComparer.Ordinal).OrderBy(group => group.Key, StringComparer.Ordinal)
            .Select(group => new
            {
                Operation = group.Key,
                Compared = group.Count(item => item.Compared),
                Unsupported = group.Count(item => !item.Compared)
            }).ToArray();
        var complete = outstanding.Length == 0 && coverage.Any(operation => operation.Compared > 0);
        var review = Path.Combine(directory, "review.json");
        await ForegroundCheck.PublishJsonAsync(review, new
        {
            Format = "slophawk-check-review-v1",
            Complete = complete,
            Coverage = coverage,
            Acknowledged = acknowledged,
            Outstanding = outstanding
        });
        Console.WriteLine(JsonSerializer.Serialize(new { Review = review, Complete = complete, Outstanding = outstanding.Length }));
        return complete ? 0 : 1;
    }

    private static bool SameInput(ProbeInput left, ProbeInput right) =>
        left.Operation == right.Operation && left.Args.AsSpan().SequenceEqual(right.Args);

    private static bool SameAdapter(Evidence left, Evidence right) =>
        left.Report.Candidate == right.Report.Candidate && left.Report.RunVerb == right.Report.RunVerb &&
        left.Report.PrepareVerb == right.Report.PrepareVerb;

    private static async Task<T> ReadAsync<T>(string path)
    {
        await using var file = File.OpenRead(path);
        return await JsonSerializer.DeserializeAsync<T>(file, ForegroundCheck.Json)
            ?? throw new JsonException($"Missing checker data in {path}.");
    }
}
