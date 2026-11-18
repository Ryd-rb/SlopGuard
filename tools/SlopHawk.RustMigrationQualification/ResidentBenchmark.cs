using System.Diagnostics;
using System.Security.Cryptography;
using System.Text.Json;

namespace SlopHawk.RustMigrationQualification;

internal static class ResidentBenchmark
{
    private sealed record Report(string Epoch, long Revision, long CommittedRevision, string? Branch, string? Commit, int FileCount);

    internal static async Task<int> RunAsync(string native, string scratch)
    {
        if (!OperatingSystem.IsWindows() && !OperatingSystem.IsLinux())
            throw new IOException("Resident process-tree measurement requires Windows or Linux.");
        native = Path.GetFullPath(native);
        scratch = Path.GetFullPath(scratch);
        if (Directory.Exists(scratch) || File.Exists(scratch))
            throw new IOException("Resident benchmark requires a new evidence directory.");
        var root = Directory.CreateDirectory(Path.Combine(scratch, "workspace")).FullName;
        const int count = 100;
        await WriteSources(count, changed: false);
        var hooks = Directory.CreateDirectory(Path.Combine(scratch, "empty-git-hooks")).FullName;
        await Git("init", "--quiet", "--initial-branch=baseline");
        await Git("add", "--", "*.cs");
        await Git("-c", "user.name=SlopHawk Qualification", "-c", "user.email=qualification@example.invalid",
            "commit", "--quiet", "--no-gpg-sign", "-m", "Fixed scope fixture");
        await Git("branch", "alternate");
        var commit = (await Git("rev-parse", "HEAD")).Trim();
        var report = Path.Combine(root, ".slophawk", "results.json");
        var platform = OperatingSystem.IsWindows() ? "windows" : "linux";
        var nativeDigest = Digest(native);
        var captures = new List<SampleCapture>();
        await Measure();

        Program.Write(Path.Combine(scratch, "resident-summary.json"), new
        {
            platform, nativeDigest, sourceCount = count, fixtureCommit = commit,
            captures = captures.Select(sample => new { sample.Engine, sample.Scenario, sample.ScopeComplete,
                sample.ScopeOmissions, sample.ElapsedMilliseconds, samples = sample.Samples.Length,
                medianResidentBytes = Quantile(sample.Samples.Select(point => point.PrivateWorkingSetBytes ?? point.PssBytes), .5),
                p95RecheckMilliseconds = Quantile(sample.Samples.Select(point => point.LatencyMilliseconds), .95) }),
        });
        return captures.All(sample => sample.ScopeComplete) ? 0 : 1;

        async Task Measure()
        {
            (string Scenario, int Milliseconds, int Interval)[] scenarios =
            {
                ("cold", 5000, 100), ("edit", 8000, 100), ("burst-100", 8000, 100), ("branch-switch", 8000, 100),
                ("idle-5m", 300_000, 500), ("warm", 25_000, 100),
            };
            foreach (var (scenario, milliseconds, interval) in scenarios)
            {
                // Every launch starts with the same tracked input bytes and branch.
                await Git("switch", "--quiet", "baseline");
                await WriteSources(count, changed: false);
                var prior = ReadReport(report)?.Epoch;
                var spec = new SampleSpec(1, platform, "native", scenario, native, nativeDigest, ["serve", "--root", root], root,
                    milliseconds, interval, null, null);
                var launch = new Stopwatch();
                var capture = await Sampler.CaptureAsync(spec, scenario == "cold" ? null : WaitForLaunch,
                    scenario == "idle-5m" ? null : Exercise, launching: launch.Start);
                Program.Write(Path.Combine(scratch, $"{platform}-native-{scenario}.json"), capture);
                captures.Add(capture);
                Console.WriteLine($"native/{scenario} scopeComplete={capture.ScopeComplete} samples={capture.Samples.Length}");

                async Task WaitForLaunch() => _ = await WaitForReport(report, item => Ready(item, prior, "baseline"));

                async Task<IReadOnlyList<double>> Exercise()
                {
                    if (scenario == "cold")
                    {
                        await WaitForLaunch();
                        return [launch.Elapsed.TotalMilliseconds];
                    }
                    if (scenario == "warm") return await Rechecks();
                    var current = ReadReport(report) ?? throw new IOException("Starting report disappeared.");
                    var clock = Stopwatch.StartNew();
                    if (scenario == "branch-switch")
                    {
                        await Git("switch", "--quiet", "alternate");
                        await WaitForReport(report, item => Ready(item, current.Epoch, "alternate") &&
                            item.CommittedRevision > current.CommittedRevision);
                    }
                    else
                    {
                        await WriteSources(scenario == "edit" ? 1 : count, changed: true);
                        // Correlate publication to the request made after all file writes.
                        var requested = await RunCommand(native, ["recheck", "--root", root]);
                        await WaitForReport(report, item => item.Epoch == current.Epoch &&
                            item.Revision >= requested && item.CommittedRevision > current.CommittedRevision);
                    }
                    return [clock.Elapsed.TotalMilliseconds];
                }
            }
            await Git("switch", "--quiet", "baseline");
            await WriteSources(count, changed: false);

            async Task<IReadOnlyList<double>> Rechecks()
            {
                var values = new List<double>(24);
                for (var index = 0; index < 24; index++)
                {
                    var current = ReadReport(report) ?? throw new IOException("A warm report disappeared.");
                    var clock = Stopwatch.StartNew();
                    var requested = await RunCommand(native, ["recheck", "--root", root]);
                    await WaitForReport(report, item => item.Epoch == current.Epoch && item.Revision >= requested);
                    values.Add(clock.Elapsed.TotalMilliseconds);
                }
                return values;
            }
        }

        bool Ready(Report item, string? previousEpoch, string branch) =>
            item.Epoch != previousEpoch && item.FileCount == count && item.Commit == commit && item.Branch == "refs/heads/" + branch;

        async Task WriteSources(int files, bool changed)
        {
            for (var index = 0; index < files; index++)
                await File.WriteAllTextAsync(Path.Combine(root, $"Type{index:D3}.cs"),
                    $"namespace Qualification; public sealed class Type{index:D3} {{ public int Value => {index + (changed ? 100 : 0)}; }}\n");
        }

        Task<string> Git(params string[] args) => RunProcess("git", ["-C", root, "-c", "core.hooksPath=" + hooks, .. args]);
    }

    private static async Task<long> RunCommand(string executable, string[] args)
    {
        using var response = JsonDocument.Parse(await RunProcess(executable, args));
        return response.RootElement.GetProperty("revision").GetInt64();
    }

    internal static string ControllerPath() => Path.Combine(AppContext.BaseDirectory,
        OperatingSystem.IsWindows() ? "SlopHawk.RustMigrationQualification.exe" : "SlopHawk.RustMigrationQualification");

    internal static async Task<string> RunProcess(string executable, string[] args)
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
        return await output;
    }

    private static async Task<Report> WaitForReport(string path, Func<Report, bool> matches)
    {
        using var deadline = new CancellationTokenSource(TimeSpan.FromSeconds(8));
        Report? last = null;
        try
        {
            while (true)
            {
                last = ReadReport(path) ?? last;
                if (last is not null && matches(last)) return last;
                await Task.Delay(20, deadline.Token);
            }
        }
        catch (OperationCanceledException error) when (deadline.IsCancellationRequested)
        { throw new IOException($"Expected benchmark report was not published. Last observed report: {last}", error); }
    }

    private static Report? ReadReport(string path)
    {
        try
        {
            using var stream = new FileStream(path, FileMode.Open, FileAccess.Read, FileShare.ReadWrite | FileShare.Delete);
            using var report = JsonDocument.Parse(stream);
            if (report.RootElement.GetProperty("schemaVersion").GetInt32() == 2 &&
                report.RootElement.GetProperty("producer").GetString() == "SlopHawk" &&
                !report.RootElement.GetProperty("partial").GetBoolean())
                return new Report(report.RootElement.GetProperty("epoch").GetString()!, report.RootElement.GetProperty("revision").GetInt64(),
                    report.RootElement.GetProperty("committedRevision").GetInt64(), report.RootElement.GetProperty("branch").GetString(),
                    report.RootElement.GetProperty("commit").GetString(), report.RootElement.GetProperty("fileCount").GetInt32());
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
