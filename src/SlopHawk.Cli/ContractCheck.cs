using System.Diagnostics;
using System.Globalization;
using System.Text.Json;
using SlopHawk.Core;

namespace SlopHawk.Cli;

internal static class ContractCheck
{
    private const int BatchSize = 2048;

    internal static async Task<int> QualifyAsync(string[] args)
    {
        if (args is not ["contracts", "qualify", var rules, var published])
            throw new ArgumentException("Usage: slophawk contracts qualify <rules.json> <published.json>");
        var catalog = await ReadCatalogAsync(rules, published);
        Console.WriteLine(JsonSerializer.Serialize(catalog.Qualification));
        return catalog.Qualification.All(rule => rule.Error is null) ? 0 : 1;
    }

    internal static async Task<int> RunAsync(string[] args)
    {
        if (args is not ["check-contracts", _, _, _, _, ..] || args.Length > 8)
            throw new ArgumentException("Usage: slophawk check-contracts <assembly.dll> <rules.json> <published.json> <workspace> [per-operation [run-verb [prepare-verb]]]");
        var limit = 512;
        if (args.Length >= 6 && (!int.TryParse(args[5], NumberStyles.None, CultureInfo.InvariantCulture, out limit) ||
            limit is < 1 or > 8192))
            throw new ArgumentException("The per-operation budget must be between 1 and 8192.");

        var timer = Stopwatch.StartNew();
        var catalog = await ReadCatalogAsync(args[2], args[3]);
        var workspace = Path.GetFullPath(args[4]);
        if (!Directory.Exists(workspace))
            throw new DirectoryNotFoundException("The workspace directory must already exist.");
        var search = Path.Combine(workspace, ".slophawk", "contract-searches", Guid.NewGuid().ToString("N"));
        Directory.CreateDirectory(search);
        var qualification = Path.Combine(search, "qualification.json");
        await ForegroundCheck.PublishJsonAsync(qualification, catalog.Qualification);

        var selected = new Dictionary<string, ProbeExpectation>(StringComparer.Ordinal);
        var matched = new Dictionary<string, ProbeExpectation>(StringComparer.Ordinal);
        var tested = new HashSet<(string Operation, string Args)>();
        string? candidate = null;
        var compared = 0;
        var unsupported = 0;
        var remaining = catalog.Qualification.Where(item => item.Error is null).Select(item => item.Operation)
            .ToHashSet(StringComparer.Ordinal);
        var processed = remaining.ToDictionary(operation => operation, _ => 0, StringComparer.Ordinal);
        var runVerb = args.Length >= 7 ? args[6] : null;
        var prepareVerb = args.Length == 8 ? args[7] : null;
        var initial = Math.Min(64, limit);
        await SearchAsync(initial, 0, null);
        if (limit > initial)
        {
            using var assembly = File.OpenRead(args[1]);
            var literals = ManagedBodyComparison.IntegerLiterals(assembly, remaining);
            await SearchAsync(limit - initial, 1, literals);
        }

        var witnesses = selected.Values.Concat(matched.Where(pair => !selected.ContainsKey(pair.Key)).Select(pair => pair.Value))
            .ToArray();
        CheckReceipt? report = null;
        if (witnesses.Length > 0)
            (report, _) = await ForegroundCheck.ExecuteAsync(args[1], workspace, runVerb, prepareVerb,
                ProbeReplay.InputsForCheck(witnesses), witnesses, candidate);

        var summary = Path.Combine(search, "summary.json");
        var result = new
        {
            Format = "slophawk-contract-search-v1", Search = search, Qualification = qualification,
            Summary = summary, Report = report?.Report, Run = report?.Run, Compared = compared,
            PerOperation = limit, ElapsedMilliseconds = timer.ElapsedMilliseconds,
            Unsupported = unsupported, Findings = report?.Result.Findings.Count ?? 0,
            ObservedDiscrepancies = selected.Count, ReplayUnsupported = report?.Result.Unsupported ?? 0,
            Operations = catalog.Qualification.Select(item => new
            {
                item.Operation, item.Source, item.Error, Compared = processed.GetValueOrDefault(item.Operation),
                Difference = selected.ContainsKey(item.Operation)
            }).ToArray()
        };
        await ForegroundCheck.PublishJsonAsync(summary, result);
        Console.WriteLine(JsonSerializer.Serialize(new
        {
            result.Format, result.Summary, result.Report, result.Run, result.Compared, result.Unsupported, result.Findings,
            result.ObservedDiscrepancies, result.ReplayUnsupported, result.ElapsedMilliseconds,
            Unqualified = catalog.Qualification.Count(item => item.Error is not null)
        }));
        return report is not null && result.ReplayUnsupported == 0 && selected.Count == 0 && result.Findings == 0 && unsupported == 0 &&
            catalog.Qualification.All(item => item.Error is null && processed[item.Operation] > 0) ? 0 : 1;

        async Task SearchAsync(int count, int seed, IReadOnlyDictionary<string, long[]>? literals)
        {
            foreach (var batch in catalog.Generate(count, seed, remaining, guideWithBoundaries: seed != 0,
                implementationLiterals: literals).Chunk(BatchSize))
            {
                var cases = batch.Where(item => remaining.Contains(item.Operation) &&
                    tested.Add((item.Operation, string.Join(',', item.Args)))).ToArray();
                if (cases.Length == 0)
                    continue;
                candidate ??= await ForegroundCheck.CandidateHashAsync(args[1]);
                var (receipt, observed) = await ForegroundCheck.ExecuteAsync(args[1], workspace, runVerb, prepareVerb,
                    ProbeReplay.InputsForCheck(cases), cases, candidate, search);
                compared += receipt.Result.Compared;
                unsupported += receipt.Result.Unsupported;
                for (var index = 0; index < cases.Length; index++)
                {
                    if (observed[index].ValueKind == JsonValueKind.Null)
                        continue;
                    var test = cases[index];
                    processed[test.Operation]++;
                    if (JsonElement.DeepEquals(test.Expected, observed[index]))
                        matched.TryAdd(test.Operation, test);
                }
                foreach (var difference in receipt.Result.Findings)
                {
                    selected.TryAdd(difference.Input.Operation, cases[difference.Index]);
                    remaining.Remove(difference.Input.Operation);
                }
            }
        }
    }

    private static async Task<ContractCases> ReadCatalogAsync(string rules, string published) =>
        new(await CheckReview.ReadAsync<IntegerContract[]>(rules), await CheckReview.ReadAsync<ProbeExpectation[]>(published));
}
