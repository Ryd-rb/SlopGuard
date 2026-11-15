using System.Diagnostics;
using System.Text.Json;
using SlopGuard.Daemon;
using SlopGuard.WorkerFixture;
using Xunit;

namespace SlopGuard.IntegrationTests;

public sealed partial class WorkspaceTests
{
    [Fact]
    public async Task MissingProfileLeavesWorkPendingUntilAnExplicitGrant()
    {
        await using var fixture = new Fixture();
        var source = await fixture.WriteBaselineAsync();
        fixture.Start(new FakeAdapter((_, _) => Task.FromResult(TestOutcome.Passed), "unit"), grant: false);
        await fixture.WaitForReportAsync(report => FileCount(report) == 1);
        await File.WriteAllTextAsync(source, "class Changed {}");
        await fixture.WaitForSchedulerAsync("blocked_policy");
        Assert.Single(fixture.ReadIntents());
        Assert.Empty(fixture.ReadAttempts());

        fixture.WriteProfile(["test"], []);
        await fixture.WaitForCompletedPlanAsync(0);
        Assert.Empty(fixture.ReadIntents());
    }

    [Fact]
    public async Task ProfileIsBoundToThePhysicalRootAndGrantsOperationsIndependently()
    {
        await using var fixture = new Fixture();
        var output = Path.Combine(fixture.Container, "approved");
        fixture.WriteProfile(["evaluate", "restore"], [output]);
        var root = RootIdentity.Read(fixture.Root);
        var grant = ExecutionProfile.Read(fixture.Workspace, root);
        grant.Require(new WorkerRequirements(ExecutionOperation.Evaluate | ExecutionOperation.Restore, false,
            [Path.Combine(output, "nested")]));
        Assert.Throws<UnauthorizedAccessException>(() =>
            grant.Require(new WorkerRequirements(ExecutionOperation.Network, false, [])));
        Assert.Throws<UnauthorizedAccessException>(() =>
            grant.Require(new WorkerRequirements(ExecutionOperation.Evaluate, false, [fixture.Container])));
        Directory.Move(fixture.Root, Path.Combine(fixture.Container, "previous"));
        Directory.CreateDirectory(fixture.Root);
        Assert.Throws<UnauthorizedAccessException>(() => ExecutionProfile.Read(fixture.Workspace, RootIdentity.Read(fixture.Root)));
    }

    [Theory]
    [InlineData(true)]
    [InlineData(false)]
    public async Task CompetingWorktreesWaitBeforeRecordingAnAttempt(bool overlap)
    {
        await using var first = new Fixture();
        await using var second = new Fixture(first.Workspace.UserStateDirectory);
        var firstRoot = Path.Combine(first.Container, "shared-output");
        var secondRoot = overlap ? Path.Combine(firstRoot, "nested") : Path.Combine(second.Container, "other-output");
        first.WriteProfile(["test"], [firstRoot]);
        second.WriteProfile(["test"], [secondRoot]);
        var firstSource = await first.WriteBaselineAsync();
        var secondSource = await second.WriteBaselineAsync();
        var release = new TaskCompletionSource(TaskCreationOptions.RunContinuationsAsynchronously);
        first.Start(new LeaseFixtureAdapter(firstRoot, async (_, token) =>
        {
            await release.Task.WaitAsync(token);
            return TestOutcome.Passed;
        }));
        second.Start(new LeaseFixtureAdapter(secondRoot, (_, _) => Task.FromResult(TestOutcome.Passed)));
        await first.WaitForReportAsync(report => FileCount(report) == 1);
        await second.WaitForReportAsync(report => FileCount(report) == 1);
        await File.WriteAllTextAsync(firstSource, "class FirstEdit {}");
        await first.WaitForSchedulerAsync("running");
        try
        {
            await File.WriteAllTextAsync(secondSource, "class SecondEdit {}");
            await second.WaitForSchedulerAsync(overlap ? "waiting_output" : "waiting_worker");
            Assert.Empty(second.ReadAttempts());
            Assert.Single(second.ReadIntents());
        }
        finally
        {
            release.TrySetResult();
        }
        await first.WaitForCompletedPlanAsync(0);
        await second.WaitForCompletedPlanAsync(0);
        Assert.Equal("passed", Assert.Single(first.ReadAttempts()).State);
        Assert.Equal("passed", Assert.Single(second.ReadAttempts()).State);
    }

    [Fact]
    public async Task RevokingTheProfileCancelsTheEntireProcessTree()
    {
        if (!OperatingSystem.IsWindowsVersionAtLeast(10))
            return;
        await using var fixture = new Fixture();
        var output = Directory.CreateDirectory(Path.Combine(fixture.Container, "output with spaces")).FullName;
        fixture.WriteProfile(["test"], [output]);
        var source = await fixture.WriteBaselineAsync();
        fixture.Start(new ProcessFixtureAdapter(output));
        await fixture.WaitForReportAsync(report => FileCount(report) == 1);
        await File.WriteAllTextAsync(source, "class Changed {}");
        var processes = await ReadTreeAsync(fixture, output);
        Assert.All(processes, process => Assert.True(process.IsAlive()));
        var running = Assert.Single(fixture.ReadAttempts());
        var lease = fixture.ReadLease(running.Id);

        fixture.WriteProfile([], [output]);
        await fixture.WaitForSchedulerAsync("blocked_policy");
        Assert.All(processes, process => Assert.False(process.IsAlive()));
        Assert.True(WindowsJob.IsDrained(lease));
        Assert.True(new WorkerAdmission(fixture.Workspace).CanRecover());
        Assert.Equal("superseded", Assert.Single(fixture.ReadAttempts()).State);
        Assert.Single(fixture.ReadIntents());
    }

    [Fact]
    public async Task RestartHoldsTheOutputLeaseUntilThePreviousProcessScopeIsEmpty()
    {
        if (!OperatingSystem.IsWindowsVersionAtLeast(10))
            return;
        await using var fixture = new Fixture();
        var output = Directory.CreateDirectory(Path.Combine(fixture.Container, "fixture-output")).FullName;
        fixture.WriteProfile(["test"], [output]);
        var source = await fixture.WriteBaselineAsync();
        using var owner = Process.Start(Commands.Create("owner", fixture.Root, fixture.Workspace.UserStateDirectory, output))
            ?? throw new InvalidOperationException("Could not start the fixture daemon.");
        try
        {
            await fixture.WaitForReportAsync(report => FileCount(report) == 1);
            await File.WriteAllTextAsync(source, "class Changed {}");
            var original = await ReadTreeAsync(fixture, output);
            var running = Assert.Single(fixture.ReadAttempts());
            var lease = fixture.ReadLease(running.Id);
            using (var observer = WindowsJob.OpenForQuery(lease))
            {
                Assert.False(observer.IsInvalid);
                owner.Kill(); // Close the actual daemon's handles without a graceful shutdown.
                await owner.WaitForExitAsync().WaitAsync(TimeSpan.FromSeconds(8));
                fixture.Start(new LeaseFixtureAdapter(output, (attempt, _) =>
                {
                    Assert.Equal(2, attempt.Number);
                    Assert.All(original, process => Assert.False(process.IsAlive()));
                    return Task.FromResult(TestOutcome.Passed);
                }));
                await fixture.WaitForSchedulerAsync("waiting_recovery");
                Assert.All(original, process => Assert.True(process.IsAlive()));
                Assert.Equal("running", Assert.Single(fixture.ReadAttempts()).State);
                Assert.Single(fixture.ReadIntents());
            }
            // Closing the last Job Object handle terminates the old tree. The
            // restarted daemon can now reclaim the persisted slot and output lease.
            await fixture.WaitForCompletedPlanAsync(running.Id);
            Assert.Equal(new[] { "interrupted", "passed" }, fixture.ReadAttempts().Select(row => row.State));
            Assert.Empty(fixture.ReadIntents());
        }
        finally
        {
            if (!owner.HasExited)
            {
                owner.Kill();
                await owner.WaitForExitAsync().WaitAsync(TimeSpan.FromSeconds(8));
            }
        }
    }

    private static async Task<FixtureProcess[]> ReadTreeAsync(Fixture fixture, string output)
    {
        var paths = new[] { Path.Combine(output, "parent.json"), Path.Combine(output, "child.json") };
        await fixture.WaitUntilAsync(() => paths.All(File.Exists));
        return paths.Select(path => JsonSerializer.Deserialize<FixtureProcess>(File.ReadAllText(path))!).ToArray();
    }

    private sealed class LeaseFixtureAdapter(string output, Func<TestAttempt, CancellationToken, Task<TestOutcome>> run)
        : ITestAdapter
    {
        public Task<TestSelection> SelectAsync(TestSelectionInput input, CancellationToken cancellationToken) =>
            Task.FromResult(new TestSelection(true,
                [new SelectedTarget("supervised-fixture", "fixture", input.Causes.Select(cause => cause.Id).ToArray())]));

        public WorkerRequirements Requirements(SelectedTarget target) => new(ExecutionOperation.Test, false, [output]);
        public Task<TestOutcome> RunAsync(TestAttempt attempt, WorkerLease worker, CancellationToken cancellationToken) =>
            run(attempt, cancellationToken);
    }
}
