using System.Diagnostics;
using System.Security.Cryptography;
using System.Text.Json;
using SlopHawk.Daemon;

namespace SlopHawk.RustMigrationQualification;

internal static class ResidentBenchmark
{
    private sealed record Report(string Epoch, long Revision, long CommittedRevision, string? Branch, string? Commit, int FileCount);
    internal sealed record HandoffSpec(string Engine, string Workspace, string Legacy, string LegacyDigest,
        string Native, string NativeDigest, string Profile, string Commit, int SourceCount, string Receipt);
    private sealed record HandoffReceipt(double ElapsedMilliseconds, string BeforeEpoch, string AfterEpoch,
        long BeforeCommittedRevision, long AfterCommittedRevision);

    internal static async Task<int> RunAsync(string legacy, string native, string scratch, bool includeSoak)
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
        var managedDigest = Digest(legacy);
        var nativeDigest = Digest(native);
        var profile = Convert.ToHexString(RandomNumberGenerator.GetBytes(32));
        var captures = new List<SampleCapture>();
        // Seed the equivalent existing inventory before timing a cold daemon launch.
        var seed = new SampleSpec(1, platform, "legacy", "seed", legacy, managedDigest,
            ["serve", "--root", root], root, 1500, 60, null, null);
        var seeded = await Sampler.CaptureAsync(seed, ready: async () => _ = await WaitForReport(report, item => Ready(item, null, "baseline")));
        if (!seeded.ScopeComplete) throw new IOException("Could not establish the baseline inventory in an authenticated scope.");

        await Measure("legacy", legacy, managedDigest, ["serve", "--root", root], []);
        await MeasureHandoff("legacy", legacy, managedDigest);
        MigrationStateBridge.TransferToNative(workspace, native, nativeDigest, profile, "resident-benchmark");
        var nativeAuthorityReturned = false;
        try
        {
            var state = MigrationStateBridge.ReadNativeStateDirectory(workspace, nativeDigest, profile);
            await Measure("native", native, nativeDigest, ["--native-resident-v1", root, state, profile],
                ["--native-workspace-v1", root, state, profile]);
            await MeasureHandoff("native", native, nativeDigest);
            MigrationStateBridge.RequireLegacyWriter(workspace);
            nativeAuthorityReturned = true;
        }
        finally
        {
            if (!nativeAuthorityReturned && HasNativeAuthority(workspace))
                MigrationStateBridge.RollbackToLegacy(workspace, native, nativeDigest, profile, "resident-benchmark");
        }

        Program.Write(Path.Combine(scratch, "resident-summary.json"), new
        {
            platform, managedDigest, nativeDigest, sourceCount = count, fixtureCommit = commit,
            captures = captures.Select(sample => new { sample.Engine, sample.Scenario, sample.ScopeComplete,
                sample.ScopeOmissions, sample.ElapsedMilliseconds, samples = sample.Samples.Length,
                medianResidentBytes = Quantile(sample.Samples.Select(point => point.PrivateWorkingSetBytes ?? point.PssBytes), .5),
                p95RecheckMilliseconds = Quantile(sample.Samples.Select(point => point.LatencyMilliseconds), .95) }),
        });
        return captures.All(sample => sample.ScopeComplete) ? 0 : 1;

        async Task MeasureHandoff(string engine, string executable, string digest)
        {
            var suffix = OperatingSystem.IsWindows() ? ".exe" : "";
            var controller = Path.Combine(AppContext.BaseDirectory, "SlopHawk.RustMigrationQualification" + suffix);
            var receipt = Path.Combine(scratch, $"{platform}-{engine}-handoff.json");
            var request = Path.Combine(scratch, $"{platform}-{engine}-handoff-spec.json");
            Program.Write(request, new HandoffSpec(engine, root, legacy, managedDigest, native, nativeDigest, profile, commit, count, receipt));
            var sample = new SampleSpec(1, platform, engine, "rollback", executable, digest, [], root, 12_000, 100, null, null);
            var capture = await Sampler.CaptureAsync(sample, workload: AwaitReceipt,
                controller: new SampleController(controller, Digest(controller), ["--measure-managed-handoff", request]));
            Program.Write(Path.Combine(scratch, $"{platform}-{engine}-rollback.json"), capture);
            captures.Add(capture);
            Console.WriteLine($"{engine}/rollback scopeComplete={capture.ScopeComplete} samples={capture.Samples.Length}");

            async Task<IReadOnlyList<double>> AwaitReceipt()
            {
                using var deadline = new CancellationTokenSource(TimeSpan.FromSeconds(11));
                try
                {
                    while (!File.Exists(receipt))
                    {
                        if (File.Exists(receipt + ".failed"))
                            throw new IOException("Measured handoff failed. " + await File.ReadAllTextAsync(receipt + ".failed", deadline.Token));
                        await Task.Delay(25, deadline.Token);
                    }
                    // The controller publishes only after the new writer serves the restored revision.
                    return [Program.Read<HandoffReceipt>(receipt).ElapsedMilliseconds];
                }
                catch (OperationCanceledException error)
                { throw new IOException("Measured handoff did not produce a receipt.", error); }
            }
        }

        async Task Measure(string engine, string executable, string digest, string[] daemon, string[] cliPrefix)
        {
            var scenarios = new List<(string Scenario, int Milliseconds, int Interval)>
            {
                ("cold", 5000, 100), ("edit", 8000, 100), ("burst-100", 8000, 100), ("branch-switch", 8000, 100),
            };
            if (includeSoak)
            {
                scenarios.Add(("idle-5m", 300_000, 500));
                scenarios.Add(("warm", 25_000, 100));
            }
            foreach (var (scenario, milliseconds, interval) in scenarios)
            {
                // Every launch starts with the same tracked input bytes and branch.
                await Git("switch", "--quiet", "baseline");
                await WriteSources(count, changed: false);
                var prior = ReadReport(report)?.Epoch;
                var spec = new SampleSpec(1, platform, engine, scenario, executable, digest, daemon, root,
                    milliseconds, interval, null, null);
                var launch = new Stopwatch();
                var capture = await Sampler.CaptureAsync(spec, scenario == "cold" ? null : WaitForLaunch,
                    scenario == "idle-5m" ? null : Exercise, launching: launch.Start);
                Program.Write(Path.Combine(scratch, $"{platform}-{engine}-{scenario}.json"), capture);
                captures.Add(capture);
                Console.WriteLine($"{engine}/{scenario} scopeComplete={capture.ScopeComplete} samples={capture.Samples.Length}");

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
                        var requested = await RunCommand(executable, RecheckArguments());
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
                    var requested = await RunCommand(executable, RecheckArguments());
                    await WaitForReport(report, item => item.Epoch == current.Epoch && item.Revision >= requested);
                    values.Add(clock.Elapsed.TotalMilliseconds);
                }
                return values;
            }

            string[] RecheckArguments() => cliPrefix.Length == 0 ? ["recheck", "--root", root] : cliPrefix.Append("recheck").ToArray();
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

    internal static async Task<int> RunHandoffAsync(HandoffSpec spec)
    {
        if (spec.Engine is not ("legacy" or "native") || spec.SourceCount < 1)
            throw new IOException("Invalid measured handoff request.");
        Sampler.VerifyExecutable(spec.Legacy, spec.LegacyDigest);
        Sampler.VerifyExecutable(spec.Native, spec.NativeDigest);
        var workspace = new Workspace(spec.Workspace);
        var path = Path.Combine(workspace.Root, ".slophawk", "results.json");
        var stale = ReadReport(path)?.Epoch;
        var managed = new[] { "serve", "--root", workspace.Root };
        var firstArgs = managed;
        if (spec.Engine == "native")
        {
            var state = MigrationStateBridge.ReadNativeStateDirectory(workspace, spec.NativeDigest, spec.Profile);
            firstArgs = ["--native-resident-v1", workspace.Root, state, spec.Profile];
        }
        using var current = StartDaemon(spec.Engine == "native" ? spec.Native : spec.Legacy, firstArgs);
        var before = await WaitForReport(path, item => Ready(item, stale));
        var elapsed = Stopwatch.StartNew();
        if (spec.Engine == "native")
            MigrationStateBridge.RollbackToLegacy(workspace, spec.Native, spec.NativeDigest, spec.Profile, "resident-benchmark");
        else
            await RunProcess(spec.Legacy, ["stop", "--root", workspace.Root]);

        using var drained = new CancellationTokenSource(TimeSpan.FromSeconds(5));
        await current.WaitForExitAsync(drained.Token);
        if (current.ExitCode != 0) throw new IOException("The outgoing writer did not exit normally.");
        MigrationStateBridge.RequireLegacyWriter(workspace);
        using var restored = StartDaemon(spec.Legacy, managed);
        var after = await WaitForReport(path, item => Ready(item, before.Epoch) && item.CommittedRevision >= before.CommittedRevision);
        var staging = spec.Receipt + ".pending";
        Program.Write(staging, new HandoffReceipt(elapsed.Elapsed.TotalMilliseconds, before.Epoch, after.Epoch,
            before.CommittedRevision, after.CommittedRevision));
        File.Move(staging, spec.Receipt);
        // Keep the restored daemon inside the measured scope until the sampler proves full drainage.
        await restored.WaitForExitAsync();
        return restored.ExitCode;

        bool Ready(Report item, string? previous) => item.Epoch != previous && item.FileCount == spec.SourceCount &&
            item.Branch == "refs/heads/baseline" && item.Commit == spec.Commit;
    }

    private static bool HasNativeAuthority(Workspace workspace)
    {
        try { MigrationStateBridge.RequireLegacyWriter(workspace); return false; }
        catch (IOException) { return true; }
    }

    private static Process StartDaemon(string executable, string[] arguments)
    {
        var start = new ProcessStartInfo(executable) { UseShellExecute = false, CreateNoWindow = true };
        foreach (var argument in arguments) start.ArgumentList.Add(argument);
        return Process.Start(start) ?? throw new IOException("Failed to launch the measured writer.");
    }

    private static async Task<long> RunCommand(string executable, string[] args)
    {
        using var response = JsonDocument.Parse(await RunProcess(executable, args));
        return response.RootElement.GetProperty("revision").GetInt64();
    }

    private static async Task<string> RunProcess(string executable, string[] args)
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
