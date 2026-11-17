using System.Text.Json;
using System.Text.Json.Serialization;
using SlopHawk.Core;

namespace SlopHawk.Cli;

internal static class CheckRetest
{
    private sealed record Decision([property: JsonRequired] int Index, string Reason, JsonElement Expected);

    internal static async Task<int> RunAsync(string[] args)
    {
        if (args.Length != 6 || !Guid.TryParseExact(args[3], "N", out _))
            throw new ArgumentException("Usage: slophawk check-retest <assembly.dll> <workspace> <run> <decisions.json> <acknowledgments.json>");

        var run = args[3];
        var evidence = await CheckReview.ReadRunAsync(Path.Combine(ForegroundCheck.ChecksPath(args[2]), run), run);
        var original = evidence.Report;
        if (original.Expected is null)
            throw new IOException("Provide a completed comparison report for retesting.");
        var decisions = await CheckReview.ReadAsync<Decision[]>(args[4]);
        if (decisions.Length == 0 || decisions.Any(item => item is null || string.IsNullOrWhiteSpace(item.Reason)) ||
            decisions.Select(item => item.Index).Distinct().Count() != decisions.Length)
            throw new ArgumentException("Provide unique finding indices and a reason for each decision.");

        var findings = original.Result.Findings.ToDictionary(finding => finding.Index);
        var cases = new ProbeExpectation[decisions.Length];
        var dispositions = new string[decisions.Length];
        for (var index = 0; index < decisions.Length; index++)
        {
            var decision = decisions[index];
            if (!findings.TryGetValue(decision.Index, out var finding))
                throw new ArgumentException($"Index {decision.Index} is not a finding in run {run}.");
            var corrected = decision.Expected.ValueKind != JsonValueKind.Undefined;
            if (corrected && !JsonElement.DeepEquals(decision.Expected, finding.Candidate))
                throw new ArgumentException($"The corrected expectation at index {decision.Index} must agree with the observed result.");
            dispositions[index] = corrected ? "corrected-expectation" : "confirmed";
            cases[index] = new(finding.Input.Operation, finding.Input.Args, corrected ? decision.Expected : finding.Reference);
        }

        var attempts = Path.Combine(Path.GetFullPath(args[2]), ".slophawk", "retest-attempts");
        var (attempt, observed) = await ForegroundCheck.ExecuteAsync(args[1], args[2], original.RunVerb, original.PrepareVerb,
            ProbeReplay.InputsForCheck(cases), cases, original.Candidate, attempts);
        for (var index = 0; index < decisions.Length; index++)
        {
            if (!JsonElement.DeepEquals(observed[index], findings[decisions[index].Index].Candidate))
                throw new IOException($"The retest did not reproduce the previously observed value. See {attempt.Report}.");
        }
        var receipt = ForegroundCheck.PromoteReplay(args[2], attempt);

        var destination = Path.GetFullPath(args[5]);
        await using var writeLock = new FileStream(destination + ".lock", FileMode.OpenOrCreate, FileAccess.Write, FileShare.None);
        CheckAcknowledgment[] existing = File.Exists(destination)
            ? await CheckReview.ReadAsync<CheckAcknowledgment[]>(destination) : [];
        var updated = existing.Where(item => item.Run != run || !decisions.Any(decision => decision.Index == item.Index))
            .Concat(decisions.Select((item, index) =>
                new CheckAcknowledgment(run, item.Index, receipt.Run, index, dispositions[index], item.Reason))).ToArray();
        await ForegroundCheck.PublishJsonAsync(destination, updated);
        Console.WriteLine(JsonSerializer.Serialize(new { receipt.Run, receipt.Report, Acknowledgments = destination, Added = decisions.Length }));
        return 0;
    }
}
