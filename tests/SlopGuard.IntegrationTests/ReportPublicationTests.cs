using System.Text.Json;
using SlopGuard.Core;
using SlopGuard.Daemon;
using Xunit;

namespace SlopGuard.IntegrationTests;

public sealed partial class WorkspaceTests
{
    [Theory]
    [InlineData(false)]
    [InlineData(true)]
    public async Task StagedReportCannotPublishAfterSameCommitBranchTransition(bool returnToOriginal)
    {
        await using var fixture = new Fixture();
        await fixture.GitAsync("init", "-b", "main");
        await File.WriteAllTextAsync(Path.Combine(fixture.Root, "Source.cs"), "class Source {}");
        await fixture.GitAsync("add", "Source.cs");
        await fixture.GitAsync("commit", "-m", "Initial fixture");
        await fixture.GitAsync("branch", "other");

        var firstStaged = NewSignal();
        var secondStaged = NewSignal();
        var releaseFirst = NewSignal();
        var releaseSecond = NewSignal();
        var stages = 0;
        async Task AfterStaging(CancellationToken token)
        {
            switch (Interlocked.Increment(ref stages))
            {
                case 1:
                    firstStaged.TrySetResult(true);
                    await releaseFirst.Task.WaitAsync(token);
                    break;
                case 2:
                    secondStaged.TrySetResult(true);
                    await releaseSecond.Task.WaitAsync(token);
                    break;
            }
        }

        using var monitor = new WorkspaceMonitor(fixture.Workspace,
            await GitContext.DiscoverAsync(fixture.Root, CancellationToken.None), null, AfterStaging);
        using var stop = new CancellationTokenSource();
        monitor.Start();
        var running = monitor.RunAsync(stop.Token);
        try
        {
            await firstStaged.Task.WaitAsync(TimeSpan.FromSeconds(12));
            var firstEpoch = JsonSerializer.SerializeToElement(monitor.Status(), ReportFile.Json)
                .GetProperty("epoch").GetGuid();
            var context = await GitContext.DiscoverAsync(fixture.Root, CancellationToken.None);
            var startingCommit = (await context.ReadLiveAsync(CancellationToken.None)).Commit;

            await fixture.GitAsync("switch", "other");
            monitor.Invalidate(transition: true);
            if (returnToOriginal)
            {
                await fixture.GitAsync("switch", "main");
                monitor.Invalidate(transition: true);
            }
            releaseFirst.TrySetResult(true);

            // The next publication is held independently so the late first
            // writer cannot be confused with a successful later reconciliation.
            await secondStaged.Task.WaitAsync(TimeSpan.FromSeconds(12));
            Assert.False(File.Exists(fixture.ReportPath));
            Assert.Null(await monitor.ReadCurrentAsync(CancellationToken.None));
            releaseSecond.TrySetResult(true);

            var current = await WaitForMonitorReportAsync(monitor, report =>
                report.Branch == (returnToOriginal ? "refs/heads/main" : "refs/heads/other"));
            Assert.Equal(startingCommit, current.Commit);
            Assert.NotEqual(firstEpoch, current.Epoch);
        }
        finally
        {
            releaseFirst.TrySetResult(true);
            releaseSecond.TrySetResult(true);
            await StopMonitorAsync(stop, running);
        }
    }

    [Theory]
    [InlineData(false)]
    [InlineData(true)]
    public async Task LateLiveOrSequenceFenceRejectsStagedReport(bool rejectLive)
    {
        await using var fixture = new Fixture();
        var staging = Path.Combine(fixture.Root, ".slopguard.pending.json");
        var reachedStage = false;
        var writer = new ReportFile(fixture.Workspace, _ =>
        {
            reachedStage = File.Exists(staging);
            return Task.CompletedTask;
        });
        var published = await writer.PublishAsync(SampleReport(fixture.Workspace), RootIdentity.Read(fixture.Root),
            _ => Task.FromResult(!rejectLive), _ => false, CancellationToken.None);

        Assert.True(reachedStage);
        Assert.False(published);
        Assert.False(File.Exists(staging));
        Assert.False(File.Exists(fixture.ReportPath));
    }

    [Theory]
    [InlineData(false)]
    [InlineData(true)]
    public async Task LateForeignStagingOrDestinationIsPreserved(bool replaceStaging)
    {
        await using var fixture = new Fixture();
        var staging = Path.Combine(fixture.Root, ".slopguard.pending.json");
        var altered = replaceStaging ? staging : fixture.ReportPath;
        const string foreign = "{\"producer\":\"foreign\",\"root\":\"elsewhere\"}";
        var writer = new ReportFile(fixture.Workspace, _ =>
        {
            File.WriteAllText(altered, foreign);
            return Task.CompletedTask;
        });

        await Assert.ThrowsAsync<IOException>(() => writer.PublishAsync(
            SampleReport(fixture.Workspace), RootIdentity.Read(fixture.Root), _ => Task.FromResult(true), Commit, CancellationToken.None));
        Assert.Equal(foreign, File.ReadAllText(altered));
        if (replaceStaging)
            Assert.False(File.Exists(fixture.ReportPath));
        if (!replaceStaging)
            Assert.False(File.Exists(staging));
    }

    [Theory]
    [InlineData(false)]
    [InlineData(true)]
    public async Task LateLinkedStagingOrDestinationCannotRedirectPublication(bool linkStaging)
    {
        // Windows symbolic-link creation may require privileges not granted by
        // the integration fixture. Linux uses real filesystem symlinks here.
        if (!OperatingSystem.IsLinux())
            return;
        await using var fixture = new Fixture();
        var staging = Path.Combine(fixture.Root, ".slopguard.pending.json");
        var link = linkStaging ? staging : fixture.ReportPath;
        var outside = Path.Combine(fixture.Container, "foreign.json");
        const string foreign = "untouched outside file";
        File.WriteAllText(outside, foreign);
        var writer = new ReportFile(fixture.Workspace, _ =>
        {
            if (linkStaging)
                File.Delete(staging);
            File.CreateSymbolicLink(link, outside);
            return Task.CompletedTask;
        });

        await Assert.ThrowsAsync<IOException>(() => writer.PublishAsync(
            SampleReport(fixture.Workspace), RootIdentity.Read(fixture.Root), _ => Task.FromResult(true), Commit, CancellationToken.None));
        Assert.Equal(foreign, File.ReadAllText(outside));
        Assert.True((File.GetAttributes(link) & FileAttributes.ReparsePoint) != 0);
        if (!linkStaging)
            Assert.False(File.Exists(staging));
    }

    [Fact]
    public async Task ReplacingPhysicalRootAfterStagingDoesNotPublishIntoTheNewRoot()
    {
        await using var fixture = new Fixture();
        var oldRoot = Path.Combine(fixture.Container, "old-workspace");
        var writer = new ReportFile(fixture.Workspace, _ =>
        {
            Directory.Move(fixture.Root, oldRoot);
            Directory.CreateDirectory(fixture.Root);
            return Task.CompletedTask;
        });

        await Assert.ThrowsAsync<IOException>(() => writer.PublishAsync(
            SampleReport(fixture.Workspace), RootIdentity.Read(fixture.Root), _ => Task.FromResult(true), Commit, CancellationToken.None));
        Assert.False(File.Exists(fixture.ReportPath));
        Assert.True(File.Exists(Path.Combine(oldRoot, ".slopguard.pending.json")));
    }

    [Fact]
    public async Task ReplacingReportDirectoryAfterStagingIsRejected()
    {
        await using var fixture = new Fixture();
        var reportDirectory = Path.Combine(fixture.Root, ".slopguard");
        var oldDirectory = Path.Combine(fixture.Root, "old-output");
        var writer = new ReportFile(fixture.Workspace, _ =>
        {
            Directory.Move(reportDirectory, oldDirectory);
            Directory.CreateDirectory(reportDirectory);
            return Task.CompletedTask;
        });

        await Assert.ThrowsAsync<IOException>(() => writer.PublishAsync(
            SampleReport(fixture.Workspace), RootIdentity.Read(fixture.Root), _ => Task.FromResult(true), Commit, CancellationToken.None));
        Assert.False(File.Exists(fixture.ReportPath));
        Assert.False(File.Exists(Path.Combine(fixture.Root, ".slopguard.pending.json")));
    }

    [Fact]
    public async Task ValidatedReaderRejectsChangedGitContextEvenIfRawReportRemains()
    {
        await using var fixture = new Fixture();
        await fixture.GitAsync("init", "-b", "main");
        await File.WriteAllTextAsync(Path.Combine(fixture.Root, "Source.cs"), "class Source {}");
        await fixture.GitAsync("add", "Source.cs");
        await fixture.GitAsync("commit", "-m", "Initial fixture");
        await fixture.GitAsync("branch", "other");
        using var monitor = new WorkspaceMonitor(fixture.Workspace,
            await GitContext.DiscoverAsync(fixture.Root, CancellationToken.None), null);
        using var stop = new CancellationTokenSource();
        monitor.Start();
        var running = monitor.RunAsync(stop.Token);
        try
        {
            await WaitForMonitorReportAsync(monitor, report => report.Branch == "refs/heads/main");
            Assert.True(File.Exists(fixture.ReportPath));
            // Stop just the exporter, leaving its raw projection on disk until
            // monitor disposal. This reproduces an abandoned raw file.
            await StopMonitorAsync(stop, running);
            await fixture.GitAsync("switch", "other");
            Assert.True(File.Exists(fixture.ReportPath));
            Assert.Null(await monitor.ReadCurrentAsync(CancellationToken.None));
        }
        finally
        {
            if (!stop.IsCancellationRequested)
                await StopMonitorAsync(stop, running);
        }
    }

    [Fact]
    public async Task LinkedWorktreeGitDirectoryRebindingIsNotAValidSnapshot()
    {
        await using var fixture = new Fixture();
        await fixture.GitAsync("init", "-b", "main");
        await File.WriteAllTextAsync(Path.Combine(fixture.Root, "Source.cs"), "class Source {}");
        await fixture.GitAsync("add", "Source.cs");
        await fixture.GitAsync("commit", "-m", "Initial fixture");
        var linked = Path.Combine(fixture.Container, "linked");
        await fixture.GitAsync("worktree", "add", "--detach", linked, "HEAD");
        var oldContext = await GitContext.DiscoverAsync(linked, CancellationToken.None);
        var oldCommit = (await oldContext.ReadLiveAsync(CancellationToken.None)).Commit;
        var gitFile = Path.Combine(linked, ".git");
        if (OperatingSystem.IsWindows())
            File.SetAttributes(gitFile, File.GetAttributes(gitFile) & ~FileAttributes.Hidden);
        File.WriteAllText(gitFile, $"gitdir: {Path.Combine(fixture.Root, ".git")}\n");

        await Assert.ThrowsAsync<IOException>(() => oldContext.ReadLiveAsync(CancellationToken.None));
        var mainContext = await GitContext.DiscoverAsync(fixture.Root, CancellationToken.None);
        Assert.Equal(oldCommit, (await mainContext.ReadLiveAsync(CancellationToken.None)).Commit);
    }

    private static TaskCompletionSource<bool> NewSignal() =>
        new(TaskCreationOptions.RunContinuationsAsynchronously);

    private static bool Commit(Action move)
    {
        move();
        return true;
    }

    private static WorkspaceReport SampleReport(Workspace workspace) =>
        new(ReportFile.SchemaVersion, ReportFile.Producer, workspace.Root,
            Guid.NewGuid(), 1, null, null, 1, false, 1);

    private static async Task<WorkspaceReport> WaitForMonitorReportAsync(
        WorkspaceMonitor monitor, Func<WorkspaceReport, bool> accept)
    {
        using var timeout = new CancellationTokenSource(TimeSpan.FromSeconds(12));
        while (true)
        {
            var report = await monitor.ReadCurrentAsync(timeout.Token);
            if (report is not null && accept(report))
                return report;
            await Task.Delay(50, timeout.Token);
        }
    }

    private static async Task StopMonitorAsync(CancellationTokenSource stop, Task running)
    {
        stop.Cancel();
        try
        {
            await running.WaitAsync(TimeSpan.FromSeconds(5));
        }
        catch (OperationCanceledException) when (stop.IsCancellationRequested)
        {
            // Expected: the run loop was waiting for a watcher wake-up.
        }
    }
}
