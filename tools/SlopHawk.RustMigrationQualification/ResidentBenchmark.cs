using System.Diagnostics;
using System.Security.Cryptography;
using System.Text.Json;
using SlopHawk.Daemon;

namespace SlopHawk.RustMigrationQualification;

internal static class ResidentBenchmark
{
    internal static async Task<int> RunAsync(string legacy, string native, string scratch)
    {
        if (!OperatingSystem.IsWindows() && !OperatingSystem.IsLinux())
            throw new IOException("Resident process-tree measurement requires Windows or Linux.");
        legacy = Path.GetFullPath(legacy);
        native = Path.GetFullPath(native);
        scratch = Path.GetFullPath(scratch);
        if (Directory.Exists(scratch) || File.Exists(scratch))
            throw new IOException("Resident benchmark requires a new evidence directory.");
        var root = Directory.CreateDirectory(Path.Combine(scratch, "workspace")).FullName;
        var workspace = new Workspace(root);
        const int count = 100;
        for (var index = 0; index < count; index++)
            await File.WriteAllTextAsync(Path.Combine(root, $"Type{index:D3}.cs"),
                $"namespace Qualification; public sealed class Type{index:D3} {{ public int Value => {index}; }}\n");
        var report = Path.Combine(root, ".slophawk", "results.json");
        var platform = OperatingSystem.IsWindows() ? "windows" : "linux";
        var managedDigest = Digest(legacy);
        var nativeDigest = Digest(native);
        var profile = Convert.ToHexString(RandomNumberGenerator.GetBytes(32));
        var captures = new List<SampleCapture>();

        await Measure("legacy", legacy, managedDigest, ["serve", "--root", root], []);
        MigrationStateBridge.TransferToNative(workspace, native, nativeDigest, profile, "resident-benchmark");
        try
        {
            var state = MigrationStateBridge.ReadNativeStateDirectory(workspace, nativeDigest, profile);
            await Measure("native", native, nativeDigest, ["--native-resident-v1", root, state, profile],
                ["--native-workspace-v1", root, state, profile]);
        }
        finally
        {
            MigrationStateBridge.RollbackToLegacy(workspace, native, nativeDigest, profile, "resident-benchmark");
        }

        Program.Write(Path.Combine(scratch, "resident-summary.json"), new
        {
            platform, managedDigest, nativeDigest, sourceCount = count,
            captures = captures.Select(sample => new { sample.Engine, sample.Scenario, sample.ScopeComplete,
                sample.ScopeOmissions, sample.ElapsedMilliseconds, samples = sample.Samples.Length,
                medianResidentBytes = Quantile(sample.Samples.Select(point => point.PrivateWorkingSetBytes ?? point.PssBytes), .5),
                p95RecheckMilliseconds = Quantile(sample.Samples.Select(point => point.LatencyMilliseconds), .95) }),
        });
        return captures.All(sample => sample.ScopeComplete) ? 0 : 1;

        async Task Measure(string engine, string executable, string digest, string[] daemon, string[] cliPrefix)
        {
            foreach (var (scenario, milliseconds, interval) in new[] { ("idle-5m", 300_000, 500), ("warm", 25_000, 100) })
            {
                var prior = ReadReport(report).Epoch;
                var spec = new SampleSpec(1, platform, engine, scenario, executable, digest, daemon, root,
                    milliseconds, interval, null, null);
                var capture = await Sampler.CaptureAsync(spec,
                    ready: async () => _ = await WaitForReport(report, prior, null),
                    workload: scenario == "warm" ? Rechecks : null);
                Program.Write(Path.Combine(scratch, $"{platform}-{engine}-{scenario}.json"), capture);
                captures.Add(capture);
                Console.WriteLine($"{engine}/{scenario} scopeComplete={capture.ScopeComplete} samples={capture.Samples.Length}");
            }

            async Task<IReadOnlyList<double>> Rechecks()
            {
                var values = new List<double>(24);
                for (var index = 0; index < 24; index++)
                {
                    var current = ReadReport(report);
                    var clock = Stopwatch.StartNew();
                    var args = cliPrefix.Length == 0 ? ["recheck", "--root", root] : cliPrefix.Append("recheck").ToArray();
                    var requested = await RunCommand(executable, args);
                    await WaitForReport(report, current.Epoch, requested);
                    values.Add(clock.Elapsed.TotalMilliseconds);
                }
                return values;
            }
        }
    }

    private static async Task<long> RunCommand(string executable, string[] args)
    {
        var info = new ProcessStartInfo(executable) { UseShellExecute = false, CreateNoWindow = true,
            RedirectStandardOutput = true, RedirectStandardError = true };
        foreach (var arg in args) info.ArgumentList.Add(arg);
        using var child = Process.Start(info) ?? throw new IOException("Could not request benchmark reconciliation.");
        using var deadline = new CancellationTokenSource(TimeSpan.FromSeconds(5));
        var output = child.StandardOutput.ReadToEndAsync(deadline.Token);
        var error = child.StandardError.ReadToEndAsync(deadline.Token);
        await child.WaitForExitAsync(deadline.Token);
        if (child.ExitCode != 0) throw new IOException(await error);
        using var response = JsonDocument.Parse(await output);
        return response.RootElement.GetProperty("revision").GetInt64();
    }

    private static async Task<(string? Epoch, long Revision)> WaitForReport(string path, string? epoch, long? revision)
    {
        using var deadline = new CancellationTokenSource(TimeSpan.FromSeconds(8));
        while (true)
        {
            var report = ReadReport(path);
            if (report.Epoch is not null && (revision is null ? report.Epoch != epoch :
                report.Epoch == epoch && report.Revision >= revision)) return report;
            await Task.Delay(20, deadline.Token);
        }
    }

    private static (string? Epoch, long Revision) ReadReport(string path)
    {
        try
        {
            using var stream = new FileStream(path, FileMode.Open, FileAccess.Read, FileShare.ReadWrite | FileShare.Delete);
            using var report = JsonDocument.Parse(stream);
            if (report.RootElement.GetProperty("schemaVersion").GetInt32() == 2 &&
                report.RootElement.GetProperty("producer").GetString() == "SlopHawk")
                return (report.RootElement.GetProperty("epoch").GetString(), report.RootElement.GetProperty("revision").GetInt64());
        }
        catch (IOException) { }
        catch (JsonException) { }
        return default;
    }

    private static double? Quantile(IEnumerable<double?> values, double fraction)
    {
        var ordered = values.OfType<double>().Order().ToArray();
        return ordered.Length == 0 ? null : ordered[(int)Math.Ceiling(ordered.Length * fraction) - 1];
    }

    private static string Digest(string path)
    {
        using var file = File.OpenRead(path);
        return Convert.ToHexString(SHA256.HashData(file));
    }
}
