using System.Diagnostics;
using System.Text.Json;
using SlopGuard.Daemon;
using SlopGuard.Providers.CSharp;
using Xunit;

namespace SlopGuard.IntegrationTests;

public sealed class WorkspaceTests
{
    [Fact]
    public async Task WatchesEditsAndPopulatedDirectoryRenames()
    {
        await using var fixture = new Fixture();
        var original = Directory.CreateDirectory(Path.Combine(fixture.Root, "First")).FullName;
        var source = Path.Combine(original, "Source.cs");
        await File.WriteAllTextAsync(source, "class Source {}");
        fixture.Start();
        await fixture.WaitForReportAsync(report => FileCount(report) == 1);

        await File.WriteAllTextAsync(source, "class Source {");
        var invalid = await fixture.WaitForReportAsync(report => FindingPaths(report).Contains("First/Source.cs"));
        var findings = invalid.GetProperty("findings");
        var finding = Assert.Single(findings.EnumerateArray());
        Assert.Equal("CS1513", finding.GetProperty("rule").GetString());
        Assert.Equal(64, finding.GetProperty("sourceDigest").GetString()!.Length);

        var renamed = Path.Combine(fixture.Root, "Second");
        Directory.Move(original, renamed);
        var moved = await fixture.WaitForReportAsync(report => FindingPaths(report).Contains("Second/Source.cs"));
        Assert.Equal(new[] { "Second/Source.cs" }, FindingPaths(moved));

        File.Delete(Path.Combine(renamed, "Source.cs"));
        var empty = await fixture.WaitForReportAsync(report => FileCount(report) == 0);
        Assert.Empty(FindingPaths(empty));

        await File.WriteAllTextAsync(Path.Combine(fixture.Root, "New.cs"), "class New {");
        await fixture.WaitForReportAsync(report => FindingPaths(report).Contains("New.cs"));
    }

    [Fact]
    public async Task BranchChangesInvalidateEvenWhenHeadCommitIsIdentical()
    {
        await using var fixture = new Fixture();
        await fixture.GitAsync("init", "-b", "main");
        await File.WriteAllTextAsync(Path.Combine(fixture.Root, "Source.cs"), "class Source {}");
        await fixture.GitAsync("add", "Source.cs");
        await fixture.GitAsync("commit", "-m", "First fixture");
        await fixture.GitAsync("branch", "same-commit");
        fixture.Start();
        var before = await fixture.WaitForReportAsync(report => report.GetProperty("branch").GetString() == "refs/heads/main");

        await fixture.GitAsync("switch", "same-commit");
        var immediate = await fixture.Connection.SendAsync(DaemonCommand.Results);
        if (immediate is { Ok: true, Payload: { } current })
            Assert.Equal("refs/heads/same-commit", current.GetProperty("branch").GetString());
        var after = await fixture.WaitForReportAsync(report =>
            report.GetProperty("branch").GetString() == "refs/heads/same-commit");
        Assert.Equal(before.GetProperty("commit").GetString(), after.GetProperty("commit").GetString());
        Assert.True(after.GetProperty("revision").GetInt64() > before.GetProperty("revision").GetInt64());

        var indexLock = Path.Combine(fixture.Root, ".git", "index.lock");
        await File.WriteAllTextAsync(indexLock, "fixture");
        var locked = await fixture.Connection.SendAsync(DaemonCommand.Results);
        Assert.False(locked?.Ok);
        await fixture.WaitUntilAsync(() => !File.Exists(fixture.ReportPath));
        File.Delete(indexLock);
        await fixture.WaitForReportAsync(report => report.GetProperty("branch").GetString() == "refs/heads/same-commit");
    }

    [Fact]
    public async Task ReattachesWhenWorkspaceDirectoryIsReplaced()
    {
        await using var fixture = new Fixture();
        await File.WriteAllTextAsync(Path.Combine(fixture.Root, "Original.cs"), "class Original {");
        fixture.Start();
        await fixture.WaitForReportAsync(report => FindingPaths(report).Contains("Original.cs"));

        Directory.Move(fixture.Root, Path.Combine(fixture.Container, "previous"));
        Directory.CreateDirectory(fixture.Root);
        var fresh = Path.Combine(fixture.Root, "Fresh.cs");
        await File.WriteAllTextAsync(fresh, "class Fresh {");
        var replacement = await fixture.WaitForReportAsync(report => FindingPaths(report).Contains("Fresh.cs"));
        Assert.Equal(new[] { "Fresh.cs" }, FindingPaths(replacement));

        await File.WriteAllTextAsync(fresh, "class Fresh {}");
        await fixture.WaitForReportAsync(report => FileCount(report) == 1 && FindingPaths(report).Length == 0);
    }

    [Fact]
    public async Task LockedReportFailsClosedAndRecovers()
    {
        if (!OperatingSystem.IsWindows())
            return;

        await using var fixture = new Fixture();
        var source = Path.Combine(fixture.Root, "Source.cs");
        await File.WriteAllTextAsync(source, "class Source {}");
        fixture.Start();
        await fixture.WaitForReportAsync(report => FileCount(report) == 1);
        using (File.Open(fixture.ReportPath, FileMode.Open, FileAccess.Read, FileShare.Read))
        {
            await File.WriteAllTextAsync(source, "class Source {");
            using var timeout = new CancellationTokenSource(TimeSpan.FromSeconds(10));
            while (!timeout.IsCancellationRequested)
            {
                var reply = await fixture.Connection.SendAsync(DaemonCommand.Status, timeout.Token);
                if (reply?.Payload?.GetProperty("state").GetString() == "degraded")
                    break;
                await Task.Delay(50, timeout.Token);
            }
            Assert.False(timeout.IsCancellationRequested);
            Assert.False((await fixture.Connection.SendAsync(DaemonCommand.Results))?.Ok);
        }
        await fixture.WaitForReportAsync(report => FindingPaths(report).Contains("Source.cs"));
        await fixture.StopAsync();
        Assert.False(File.Exists(fixture.ReportPath));
    }

    private static int FileCount(JsonElement report) => report.GetProperty("fileCount").GetInt32();

    private static string?[] FindingPaths(JsonElement report) =>
        report.GetProperty("findings").EnumerateArray().Select(finding => finding.GetProperty("path").GetString()).ToArray();

    private sealed class Fixture : IAsyncDisposable
    {
        private readonly CancellationTokenSource cancellation = new();
        private Task? daemon;

        internal Fixture()
        {
            Container = Directory.CreateTempSubdirectory("slopguard-").FullName;
            Root = Directory.CreateDirectory(Path.Combine(Container, "workspace")).FullName;
        }

        internal string Container { get; }
        internal string Root { get; }
        internal string ReportPath => Path.Combine(Root, ".slopguard", "results.json");
        internal DaemonConnection Connection => new(new Workspace(Root));

        internal void Start()
        {
            daemon = Task.Factory.StartNew(() => DaemonHost.Run(new Workspace(Root), new SyntaxChecker(), cancellation.Token),
                CancellationToken.None, TaskCreationOptions.LongRunning, TaskScheduler.Default);
        }

        internal async Task<JsonElement> WaitForReportAsync(Func<JsonElement, bool> matches)
        {
            using var timeout = new CancellationTokenSource(TimeSpan.FromSeconds(12));
            while (true)
            {
                if (daemon?.IsCompleted == true)
                    await daemon;
                var result = await Connection.SendAsync(DaemonCommand.Results, timeout.Token);
                if (result is { Ok: true, Payload: { } payload } && matches(payload))
                    return payload;
                await Task.Delay(50, timeout.Token);
            }
        }

        internal async Task WaitUntilAsync(Func<bool> predicate)
        {
            using var timeout = new CancellationTokenSource(TimeSpan.FromSeconds(12));
            while (!predicate())
                await Task.Delay(50, timeout.Token);
        }

        internal async Task GitAsync(params string[] arguments)
        {
            var options = new ProcessStartInfo("git")
            {
                WorkingDirectory = Root,
                UseShellExecute = false,
                RedirectStandardError = true,
                RedirectStandardOutput = true,
            };
            foreach (var argument in new[]
                     {
                         "-c", "user.name=SlopGuard", "-c", "user.email=slopguard@example.invalid",
                         "-c", "commit.gpgsign=false", "-c", "core.hooksPath=" + Path.Combine(Root, "no-hooks")
                     }.Concat(arguments))
                options.ArgumentList.Add(argument);
            using var process = Process.Start(options) ?? throw new InvalidOperationException("Git did not start.");
            var stderr = process.StandardError.ReadToEndAsync();
            var stdout = process.StandardOutput.ReadToEndAsync();
            await process.WaitForExitAsync().WaitAsync(TimeSpan.FromSeconds(8));
            await stdout;
            Assert.True(process.ExitCode == 0, await stderr);
        }

        internal async Task StopAsync()
        {
            Assert.True((await Connection.SendAsync(DaemonCommand.Stop))?.Ok);
            await daemon!.WaitAsync(TimeSpan.FromSeconds(6));
        }

        public async ValueTask DisposeAsync()
        {
            try
            {
                await cancellation.CancelAsync();
                if (daemon is not null)
                    await daemon.WaitAsync(TimeSpan.FromSeconds(6));
            }
            finally
            {
                cancellation.Dispose();
                var objects = Path.Combine(Root, ".git", "objects");
                if (OperatingSystem.IsWindows() && Directory.Exists(objects))
                {
                    // Git marks loose objects read-only on Windows.
                    foreach (var path in Directory.EnumerateFiles(objects, "*", SearchOption.AllDirectories))
                        File.SetAttributes(path, File.GetAttributes(path) & ~FileAttributes.ReadOnly);
                }
                Directory.Delete(Container, recursive: true);
            }
        }
    }
}
