using System.Diagnostics;
using System.Security.Cryptography;
using System.Text.Json;

namespace SlopHawk.RustMigrationQualification;

internal static class CheckerBenchmark
{
    private const int Cases = 2048;
    private const int Runs = 8;
    internal sealed record Work(string Binary, string BinaryDigest, string Adapter, string AdapterDigest,
        string Input, string InputDigest, string Workspace, string Receipt);

    internal static async Task<int> RunAsync(string? managed, string native, string adapter, string scratch)
    {
        scratch = Path.GetFullPath(scratch);
        if (Path.Exists(scratch)) throw new IOException("Checker benchmark requires a new evidence directory.");
        Directory.CreateDirectory(scratch);
        var input = Path.Combine(scratch, "cases.json");
        var cases = Enumerable.Range(0, Cases).Select(index => new { Operation = "sample", Args = new[] { index }, Expected = index });
        await File.WriteAllTextAsync(input, JsonSerializer.Serialize(cases));
        adapter = Path.GetFullPath(adapter);
        var platform = OperatingSystem.IsWindows() ? "windows" : "linux";
        var controller = ResidentBenchmark.ControllerPath();
        var complete = true;
        (string, string)[] engines = managed is null ? [("native", native)] : [("legacy", managed), ("native", native)];
        foreach (var (engine, executable) in engines)
        {
            var binary = Path.GetFullPath(executable);
            var root = Directory.CreateDirectory(Path.Combine(scratch, engine)).FullName;
            var receipt = Path.Combine(scratch, $"{platform}-{engine}-checker-receipt.json");
            var work = new Work(binary, Digest(binary), adapter, Digest(adapter), input, Digest(input), root, receipt);
            var request = Path.Combine(scratch, $"{platform}-{engine}-checker-spec.json");
            Program.Write(request, work);
            var spec = new SampleSpec(1, platform, engine, "checker", binary, work.BinaryDigest, [], root, 18_000, 40, null, null);
            var sample = await Sampler.CaptureAsync(spec, workload: ReadRuns,
                controller: new SampleController(controller, Digest(controller), ["--measure-checker", request]));
            if (!sample.Samples.Any(item => item.Processes >= 3) || sample.Samples.Any(item => item.ControllerResidentBytes is null))
                sample = sample with { ScopeComplete = false, ScopeOmissions = [.. sample.ScopeOmissions, "checker_or_controller_not_observed"] };
            Program.Write(Path.Combine(scratch, $"{platform}-{engine}-checker.json"), sample);
            complete &= sample.ScopeComplete;
            var peak = sample.Samples.Max(item => item.PrivateWorkingSetBytes ?? item.PssBytes);
            var appPeak = sample.Samples.Max(item => (item.PrivateWorkingSetBytes ?? item.PssBytes) - item.ControllerResidentBytes);
            Console.WriteLine($"{engine}/checker scopeComplete={sample.ScopeComplete} checkerPeakBytes={appPeak} scopePeakResidentBytes={peak}");

            async Task<IReadOnlyList<double>> ReadRuns()
            {
                using var deadline = new CancellationTokenSource(TimeSpan.FromSeconds(17));
                try
                {
                    while (!File.Exists(receipt))
                    {
                        if (File.Exists(receipt + ".failed"))
                            throw new IOException("Measured checker failed. " + await File.ReadAllTextAsync(receipt + ".failed", deadline.Token));
                        await Task.Delay(25, deadline.Token);
                    }
                    var latencies = Program.Read<double[]>(receipt);
                    if (latencies.Length != Runs || latencies.Any(value => !double.IsFinite(value) || value < 0))
                        throw new IOException("Checker did not record its complete workload.");
                    return latencies;
                }
                catch (OperationCanceledException error) { throw new IOException("Measured checker did not finish.", error); }
            }
        }
        return complete ? 0 : 1;
    }

    internal static async Task<int> RunWorkAsync(Work request)
    {
        Sampler.VerifyExecutable(request.Binary, request.BinaryDigest);
        Sampler.VerifyExecutable(request.Adapter, request.AdapterDigest);
        if (Digest(request.Input) != request.InputDigest || !Directory.Exists(request.Workspace))
            throw new IOException("Pinned checker inputs or workspace are unavailable.");
        var timings = new double[Runs];
        for (var index = 0; index < Runs; index++)
        {
            var elapsed = Stopwatch.StartNew();
            var args = new[] { "check", request.Adapter, request.Input, request.Workspace, "probe-batch" };
            using var result = JsonDocument.Parse(await ResidentBenchmark.RunProcess(request.Binary, args));
            elapsed.Stop();
            var resultNode = result.RootElement.GetProperty("Result");
            if (resultNode.GetProperty("Unsupported").GetInt32() != 0 || resultNode.GetProperty("Findings").GetArrayLength() != 0)
                throw new IOException("Matched checker did not return the expected complete result.");
            timings[index] = elapsed.Elapsed.TotalMilliseconds;
        }
        var staging = request.Receipt + ".pending";
        Program.Write(staging, timings);
        File.Move(staging, request.Receipt);
        await Task.Delay(Timeout.InfiniteTimeSpan);
        return 0;
    }

    private static string Digest(string path)
    {
        using var stream = File.OpenRead(path);
        return Convert.ToHexString(SHA256.HashData(stream));
    }
}
