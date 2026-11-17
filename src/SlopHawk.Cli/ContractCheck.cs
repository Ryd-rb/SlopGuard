using System.Diagnostics;
using System.Globalization;
using System.Security.Cryptography;
using System.Text.Json;
using SlopHawk.Core;

namespace SlopHawk.Cli;

internal static class ContractCheck
{
    private const int BatchSize = 2048;
    private sealed record ObservedCase(ProbeExpectation Case, JsonElement Value, string Run, int Index);
    private sealed record ReplayChange(ProbeInput Input, JsonElement Expected, JsonElement Before, JsonElement After,
        string InitialRun, int InitialIndex, int ReplayIndex);

    internal static async Task<int> QualifyAsync(string[] args)
    {
        if (args is not ["contracts", "qualify", var rules, var published])
            throw new ArgumentException("Usage: slophawk contracts qualify <rules.json> <published.json>");
        var (catalog, _, _) = await ReadCatalogAsync(rules, published);
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
        var (catalog, rulesHash, publishedHash) = await ReadCatalogAsync(args[2], args[3]);
        var workspace = Path.GetFullPath(args[4]);
        if (!Directory.Exists(workspace))
            throw new DirectoryNotFoundException("The workspace directory must already exist.");
        var search = Path.Combine(workspace, ".slophawk", "contract-searches", Guid.NewGuid().ToString("N"));
        Directory.CreateDirectory(search);
        var qualification = Path.Combine(search, "qualification.json");
        await ForegroundCheck.PublishJsonAsync(qualification, catalog.Qualification);

        var selected = new Dictionary<string, ObservedCase>(StringComparer.Ordinal);
        var matched = new Dictionary<string, ObservedCase>(StringComparer.Ordinal);
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
        if (limit > initial && remaining.Count > 0)
        {
            using var assembly = File.OpenRead(args[1]);
            var arities = remaining.ToDictionary(operation => operation, operation => catalog.ParameterCounts[operation],
                StringComparer.Ordinal);
            var literals = ManagedBodyComparison.IntegerLiterals(assembly, arities);
            await SearchAsync(limit - initial, 1, literals);
        }

        var witnesses = selected.Values.Concat(matched.Where(pair => !selected.ContainsKey(pair.Key)).Select(pair => pair.Value))
            .ToArray();
        CheckReceipt? report = null;
        CheckReceipt? replay = null;
        var replayChanges = new List<ReplayChange>();
        if (witnesses.Length > 0)
        {
            var cases = witnesses.Select(witness => witness.Case).ToArray();
            var inputs = ProbeReplay.InputsForCheck(cases);
            (replay, var observed) = await ForegroundCheck.ExecuteAsync(args[1], workspace, runVerb, prepareVerb,
                inputs, cases, candidate, search);
            var stableCases = new List<ProbeExpectation>();
            var stableObserved = new List<JsonElement>();
            for (var index = 0; index < witnesses.Length; index++)
            {
                var witness = witnesses[index];
                if (JsonElement.DeepEquals(witness.Value, observed[index]))
                {
                    stableCases.Add(witness.Case);
                    stableObserved.Add(observed[index]);
                }
                else
                    replayChanges.Add(new(inputs[index], witness.Case.Expected, witness.Value, observed[index],
                        witness.Run, witness.Index, index));
            }
            if (replayChanges.Count == 0)
                report = replay = ForegroundCheck.PromoteReplay(workspace, replay);
            else if (stableCases.Count > 0)
                report = await ForegroundCheck.PublishReplaySubsetAsync(workspace, candidate!, runVerb, prepareVerb,
                    [.. stableCases], [.. stableObserved]);
        }

        var summary = Path.Combine(search, "summary.json");
        var result = new
        {
            Format = "slophawk-contract-search-v1", Search = search, Qualification = qualification,
            QualificationBasis = "published-examples", ExpectationBasis = "supplied-rules",
            RulesSha256 = rulesHash, PublishedSha256 = publishedHash,
            Summary = summary, Report = report?.Report, Run = report?.Run, Compared = compared,
            PerOperation = limit, ElapsedMilliseconds = timer.ElapsedMilliseconds,
            Unsupported = unsupported, Findings = report?.Result.Findings.Count ?? 0,
            ObservedDiscrepancies = selected.Count, ReplayUnsupported = replay?.Result.Unsupported ?? 0,
            ReplayReport = replay?.Report, ReplayChanges = replayChanges,
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
            result.ObservedDiscrepancies, result.ReplayUnsupported, result.ElapsedMilliseconds, result.ReplayReport,
            ReplayChanges = replayChanges.Count, result.QualificationBasis, result.ExpectationBasis,
            result.RulesSha256, result.PublishedSha256,
            Unqualified = catalog.Qualification.Count(item => item.Error is not null)
        }));
        return report is not null && replayChanges.Count == 0 && selected.Count == 0 && result.Findings == 0 && unsupported == 0 &&
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
                        matched.TryAdd(test.Operation, new(test, observed[index], receipt.Run, index));
                }
                foreach (var difference in receipt.Result.Findings)
                {
                    selected.TryAdd(difference.Input.Operation,
                        new(cases[difference.Index], difference.Candidate, receipt.Run, difference.Index));
                    remaining.Remove(difference.Input.Operation);
                }
            }
        }
    }

    private static async Task<(ContractCases Catalog, string RulesHash, string PublishedHash)> ReadCatalogAsync(
        string rules, string published)
    {
        var ruleBytes = await File.ReadAllBytesAsync(rules);
        var exampleBytes = await File.ReadAllBytesAsync(published);
        var catalog = new ContractCases(Read<IntegerContract[]>(ruleBytes, rules), Read<ProbeExpectation[]>(exampleBytes, published));
        return (catalog, Convert.ToHexString(SHA256.HashData(ruleBytes)), Convert.ToHexString(SHA256.HashData(exampleBytes)));

        static T Read<T>(byte[] bytes, string path) =>
            JsonSerializer.Deserialize<T>(bytes.AsSpan().StartsWith(new byte[] { 0xEF, 0xBB, 0xBF }) ? bytes.AsSpan(3) : bytes,
                ForegroundCheck.Json) ?? throw new JsonException($"Missing checker data in {path}.");
    }
}
