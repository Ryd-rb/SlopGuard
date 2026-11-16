using System.Diagnostics;
using System.Collections.Concurrent;
using System.Text.Json;
using Microsoft.Data.Sqlite;
using SlopHawk.Daemon;
using Xunit;

namespace SlopHawk.IntegrationTests;

public sealed partial class WorkspaceTests
{
    [Fact]
    public async Task WatchesEditsAndPopulatedDirectoryRenames()
    {
        await using var fixture = new Fixture();
        var original = Directory.CreateDirectory(Path.Combine(fixture.Root, "First")).FullName;
        var source = Path.Combine(original, "Source.cs");
        await File.WriteAllTextAsync(source, "class Source {}");
        fixture.Start();
        var baseline = await fixture.WaitForReportAsync(report => FileCount(report) == 1);

        await File.WriteAllTextAsync(source, "class Source {");
        var edited = await fixture.WaitForNextReportAsync(baseline);
        Assert.False(edited.TryGetProperty("findings", out _));
        var change = Assert.Single(fixture.ReadIntents());
        Assert.Equal("First/Source.cs", change.NewPath);
        Assert.Equal(64, change.NewDigest!.Length);

        var renamed = Path.Combine(fixture.Root, "Second");
        Directory.Move(original, renamed);
        var moved = await fixture.WaitForNextReportAsync(edited);
        Assert.Equal(1, FileCount(moved));
        Assert.Collection(fixture.ReadIntents().Where(intent => intent.Revision == CommittedRevision(moved)),
            removed => Assert.Equal(("First/Source.cs", null), (removed.OldPath, removed.NewPath)),
            added => Assert.Equal((null, "Second/Source.cs"), (added.OldPath, added.NewPath)));

        File.Delete(Path.Combine(renamed, "Source.cs"));
        await fixture.WaitForReportAsync(report => FileCount(report) == 0);

        await File.WriteAllTextAsync(Path.Combine(fixture.Root, "New.cs"), "class New {");
        await fixture.WaitForReportAsync(report => FileCount(report) == 1);
        Assert.Contains(fixture.ReadIntents(), intent => intent.OldPath is null && intent.NewPath == "New.cs");
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
        Assert.NotEqual(before.GetProperty("epoch").GetGuid(), after.GetProperty("epoch").GetGuid());
        Assert.True(after.GetProperty("revision").GetInt64() > before.GetProperty("revision").GetInt64());
        Assert.Empty(fixture.ReadIntents());

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
        var initial = await fixture.WaitForReportAsync(report => FileCount(report) == 1);

        Directory.Move(fixture.Root, Path.Combine(fixture.Container, "previous"));
        Directory.CreateDirectory(fixture.Root);
        var fresh = Path.Combine(fixture.Root, "Fresh.cs");
        await File.WriteAllTextAsync(fresh, "class Fresh {");
        var replacement = await fixture.WaitForReportAsync(report => FileCount(report) == 1 &&
            report.GetProperty("epoch").GetGuid() != initial.GetProperty("epoch").GetGuid());
        var freshDigest = fixture.ReadDigest("Fresh.cs");

        await File.WriteAllTextAsync(fresh, "class Fresh {}");
        await fixture.WaitForNextReportAsync(replacement);
        Assert.NotEqual(freshDigest, fixture.ReadDigest("Fresh.cs"));
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
        var initial = await fixture.WaitForReportAsync(report => FileCount(report) == 1);
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
        await fixture.WaitForNextReportAsync(initial);
        await fixture.StopAsync();
        Assert.False(File.Exists(fixture.ReportPath));
    }

    [Fact]
    public async Task RestartRetainsCommittedIntentsWithoutTriggeringOnOfflineOrUnchangedSaves()
    {
        await using var fixture = new Fixture();
        var source = Path.Combine(fixture.Root, "Source.cs");
        await File.WriteAllTextAsync(source, "class Original {}");
        fixture.Start();
        var baseline = await fixture.WaitForReportAsync(report => FileCount(report) == 1);
        Assert.Empty(fixture.ReadIntents());

        await File.WriteAllTextAsync(source, "class Online {");
        var online = await fixture.WaitForNextReportAsync(baseline);
        var saved = Assert.Single(fixture.ReadIntents());
        Assert.Equal(CommittedRevision(online), saved.Revision);
        Assert.True(saved.Revision > CommittedRevision(baseline));
        Assert.Equal("Source.cs", saved.OldPath);
        Assert.Equal("Source.cs", saved.NewPath);
        Assert.NotEqual(saved.OldDigest, saved.NewDigest);
        Assert.Equal("graph_unavailable", saved.Predecessor);
        await fixture.StopAsync();

        await File.WriteAllTextAsync(source, "class Offline {}");
        fixture.Start();
        var restarted = await fixture.WaitForReportAsync(report => FileCount(report) == 1);
        Assert.True(CommittedRevision(restarted) > saved.Revision);
        var status = await fixture.ReadStatusAsync();
        Assert.Equal(1, status.GetProperty("pendingTestIntents").GetProperty("detailedCount").GetInt32());
        Assert.Equal(saved, Assert.Single(fixture.ReadIntents()));

        var replacement = Path.Combine(fixture.Root, "save.tmp");
        await File.WriteAllTextAsync(replacement, "class Offline {}");
        File.Move(replacement, source, overwrite: true);
        await File.WriteAllTextAsync(Path.Combine(fixture.Root, "Directory.Build.props"), "<Project />");
        var unchanged = await fixture.WaitForNextReportAsync(restarted);
        Assert.Equal(saved, Assert.Single(fixture.ReadIntents()));

        await File.WriteAllTextAsync(source, "class AfterRestart {");
        await fixture.WaitForNextReportAsync(unchanged);
        var intents = fixture.ReadIntents();
        Assert.Equal(2, intents.Length);
        Assert.Equal(saved, intents[0]);
        Assert.NotEqual(saved.NewDigest, intents[1].OldDigest);
        Assert.Equal(saved.NewPath, intents[1].OldPath);
    }

    [Fact]
    public async Task FailedIntentInsertRollsBackTheSourceAndRecoversOnRetry()
    {
        await using var fixture = new Fixture();
        var source = Path.Combine(fixture.Root, "Source.cs");
        await File.WriteAllTextAsync(source, "class Original {}");
        fixture.Start();
        var before = await fixture.WaitForReportAsync(report => FileCount(report) == 1);
        var originalDigest = fixture.ReadDigest("Source.cs");
        fixture.ExecuteSql("""
            CREATE TRIGGER fail_intent BEFORE INSERT ON test_intent
            BEGIN SELECT RAISE(ABORT, 'injected recording failure'); END;
            """);

        await File.WriteAllTextAsync(source, "class Changed {");
        var degraded = await fixture.WaitForStatusAsync(status =>
            status.GetProperty("recordingError").ValueKind == JsonValueKind.String);
        Assert.Equal(CommittedRevision(before), CommittedRevision(degraded));
        Assert.Equal("degraded", degraded.GetProperty("state").GetString());
        Assert.Equal(originalDigest, fixture.ReadDigest("Source.cs"));
        Assert.Empty(fixture.ReadIntents());

        fixture.ExecuteSql("DROP TRIGGER fail_intent");
        await fixture.WaitForNextReportAsync(before);
        var intent = Assert.Single(fixture.ReadIntents());
        Assert.Equal(originalDigest, intent.OldDigest);
        Assert.Equal(fixture.ReadDigest("Source.cs"), intent.NewDigest);
    }

    [Fact]
    public async Task PartialObservationRetainsThePreviousDigestUntilSourceIsReadable()
    {
        await using var fixture = new Fixture();
        var source = Path.Combine(fixture.Root, "Source.cs");
        var other = Path.Combine(fixture.Root, "Other.cs");
        await File.WriteAllTextAsync(source, "class Original {");
        await File.WriteAllTextAsync(other, "class Other {}");
        fixture.Start();
        await fixture.WaitForReportAsync(report => FileCount(report) == 2);
        var digest = fixture.ReadDigest("Source.cs");

        using (var stream = File.OpenWrite(source))
            stream.SetLength(2 * 1024 * 1024);
        await File.WriteAllTextAsync(other, "class Other {");
        var partial = await fixture.WaitForReportAsync(report =>
            report.GetProperty("partial").GetBoolean() && FileCount(report) == 1 &&
            fixture.ReadIntents().Any(intent => intent.NewPath == "Other.cs"));
        Assert.Equal(1, FileCount(partial));
        Assert.Equal(digest, fixture.ReadDigest("Source.cs"));
        Assert.Equal("Other.cs", Assert.Single(fixture.ReadIntents()).NewPath);

        await File.WriteAllTextAsync(source, "class Recovered {}");
        await fixture.WaitForReportAsync(report => FileCount(report) == 2 && !report.GetProperty("partial").GetBoolean());
        var recovered = Assert.Single(fixture.ReadIntents(), intent => intent.NewPath == "Source.cs");
        Assert.Equal(digest, recovered.OldDigest);
        Assert.NotEqual(digest, recovered.NewDigest);
    }

    [Fact]
    public async Task RestartAfterRootReplacementRetiresOldIntents()
    {
        await using var fixture = new Fixture();
        var source = Path.Combine(fixture.Root, "Source.cs");
        await File.WriteAllTextAsync(source, "class Original {}");
        fixture.Start();
        var initial = await fixture.WaitForReportAsync(report => FileCount(report) == 1);
        await File.WriteAllTextAsync(source, "class Changed {");
        await fixture.WaitForNextReportAsync(initial);
        Assert.Single(fixture.ReadIntents());
        await fixture.StopAsync();

        Directory.Move(fixture.Root, Path.Combine(fixture.Container, "previous"));
        Directory.CreateDirectory(fixture.Root);
        await File.WriteAllTextAsync(source, "class Replacement {}");
        fixture.Start();
        var replacement = await fixture.WaitForReportAsync(report => FileCount(report) == 1);
        Assert.Empty(fixture.ReadIntents());
        var retirement = (await fixture.ReadStatusAsync()).GetProperty("retiredTestIntents");
        Assert.Equal("root_replaced", retirement.GetProperty("reason").GetString());
        Assert.Equal(1, retirement.GetProperty("count").GetInt32());
        Assert.Equal(CommittedRevision(replacement), retirement.GetProperty("revision").GetInt64());
    }

    [Fact]
    public async Task CaseOnlyRenameRetainsBothEndpointSpellings()
    {
        if (!OperatingSystem.IsWindows())
            return;
        await using var fixture = new Fixture();
        var original = Path.Combine(fixture.Root, "source.cs");
        await File.WriteAllTextAsync(original, "class Source {");
        fixture.Start();
        var initial = await fixture.WaitForReportAsync(report => FileCount(report) == 1);
        File.Move(original, Path.Combine(fixture.Root, "Source.cs"));

        await fixture.WaitForNextReportAsync(initial);
        var change = Assert.Single(fixture.ReadIntents());
        Assert.Equal("source.cs", change.OldPath);
        Assert.Equal("Source.cs", change.NewPath);
        Assert.Equal(change.OldDigest, change.NewDigest);
    }

    [Fact]
    public async Task AcceptedObservationSurvivesAnInvalidationAndIsTheNextTransactionBase()
    {
        await using var fixture = new Fixture();
        var store = new SourceInventory(fixture.Workspace);
        var root = RootIdentity.Read(fixture.Root);
        var git = new GitRevision(null, null);
        var baseline = store.Reconcile(store.Load(), Scan("first"), root, git, startup: true, () => true)!;
        var sequence = 1;
        var inFlight = store.Reconcile(baseline, Scan("second"), root, git, startup: false, () =>
        {
            // The writer accepts this revision just as the next event arrives.
            Assert.Equal(1, sequence);
            sequence++;
            return true;
        });
        Assert.NotNull(inFlight);

        var recoveredStore = new SourceInventory(fixture.Workspace);
        var recovered = recoveredStore.Load();
        Assert.Equal(inFlight.Revision, recovered.Revision);
        Assert.Equal("second", recovered.Files["Source.cs"].Digest);
        Assert.Null(recoveredStore.Reconcile(recovered, Scan("stale"), root, git, startup: false, () => sequence == 1));
        Assert.Equal(recovered.Revision, recoveredStore.Load().Revision);
        var latest = recoveredStore.Reconcile(recovered, Scan("third"), root, git, startup: false, () => sequence == 2)!;
        Assert.Equal(2, latest.Pending.DetailedCount);
        Assert.Collection(fixture.ReadIntents(),
            first => Assert.Equal(("first", "second"), (first.OldDigest, first.NewDigest)),
            next => Assert.Equal(("second", "third"), (next.OldDigest, next.NewDigest)));

        static SourceScan Scan(string digest) =>
            new(new Dictionary<string, string>(Workspace.PathComparer) { ["Source.cs"] = digest },
                new HashSet<string>(Workspace.PathComparer));
    }

    [Fact]
    public async Task TestIntentLimitUsesOneDurableWorkspaceFallback()
    {
        await using var fixture = new Fixture();
        var store = new SourceInventory(fixture.Workspace);
        var root = RootIdentity.Read(fixture.Root);
        var git = new GitRevision(null, null);
        var files = new Dictionary<string, string>(Workspace.PathComparer);
        var scopes = new HashSet<string>(Workspace.PathComparer);
        var baseline = store.Reconcile(store.Load(), new SourceScan(files, scopes), root, git, startup: true, () => true)!;
        for (var i = 0; i <= SourceInventory.MaxDetailedIntents; i++)
            files.Add($"Source{i}.cs", "first");
        var overflow = store.Reconcile(baseline, new SourceScan(files, scopes), root, git, startup: false, () => true)!;
        Assert.Equal(new IntentFallback(overflow.Revision, overflow.Revision), overflow.Pending.Fallback);
        Assert.Empty(fixture.ReadIntents());

        var recovered = store.Load();
        Assert.Equal(overflow.Pending, recovered.Pending);
        Assert.Equal(files.Count, recovered.Files.Count);
        files["Source0.cs"] = "second";
        var next = store.Reconcile(recovered, new SourceScan(files, scopes), root, git, startup: false, () => true)!;
        Assert.Equal(new IntentFallback(overflow.Revision, next.Revision), store.Load().Pending.Fallback);
    }

    [Fact]
    public async Task RestartRecoversAnInterruptedRetryAndDoesNotRepeatTheCompletedAttempt()
    {
        await using var fixture = new Fixture();
        var source = await fixture.WriteBaselineAsync();
        var retryStarted = new TaskCompletionSource<TestAttempt>(TaskCreationOptions.RunContinuationsAsynchronously);
        fixture.Start(new FakeAdapter(async (attempt, token) =>
        {
            if (attempt.Number == 1)
                return TestOutcome.RetryableFailure;
            retryStarted.TrySetResult(attempt);
            await Task.Delay(Timeout.InfiniteTimeSpan, token);
            return TestOutcome.Passed;
        }, "unit"));
        await fixture.WaitForReportAsync(report => FileCount(report) == 1);
        await File.WriteAllTextAsync(source, "class Edited { }");

        var retry = await retryStarted.Task.WaitAsync(TimeSpan.FromSeconds(10));
        Assert.Equal(2, retry.Number);
        Assert.Single(fixture.ReadIntents());
        await fixture.StopAsync();
        Assert.Collection(fixture.ReadAttempts(),
            first => { Assert.Equal("retryable_failure", first.State); Assert.NotNull(first.RetryAt); },
            second => Assert.Equal("running", second.State));

        fixture.Start(new FakeAdapter((attempt, _) =>
        {
            Assert.Equal(3, attempt.Number);
            Assert.Equal(retry.Target.CauseIds, attempt.Target.CauseIds);
            return Task.FromResult(TestOutcome.Passed);
        }, "unit"));
        await fixture.WaitForSchedulerAsync("idle");
        Assert.Empty(fixture.ReadIntents());
        Assert.Equal(new[] { "retryable_failure", "interrupted", "passed" },
            fixture.ReadAttempts().Select(receipt => receipt.State));

        await fixture.StopAsync();
        fixture.Start(new FakeAdapter((_, _) => throw new InvalidOperationException("Completed work ran again."), "unit"));
        await fixture.WaitForSchedulerAsync("idle");
        Assert.Equal(3, fixture.ReadAttempts().Length);
    }

    [Fact]
    public async Task CompletedTargetDoesNotConsumeOtherTargetsOrRepeatAfterRestart()
    {
        await using var fixture = new Fixture();
        var source = await fixture.WriteBaselineAsync();
        var secondStarted = new TaskCompletionSource(TaskCreationOptions.RunContinuationsAsynchronously);
        fixture.Start(new FakeAdapter(async (attempt, token) =>
        {
            if (attempt.Target.Id == "first")
                return TestOutcome.Passed;
            secondStarted.TrySetResult();
            await Task.Delay(Timeout.InfiniteTimeSpan, token);
            return TestOutcome.Passed;
        }, "first", "second"));
        await fixture.WaitForReportAsync(report => FileCount(report) == 1);
        await File.WriteAllTextAsync(source, "class Edited {}");
        await secondStarted.Task.WaitAsync(TimeSpan.FromSeconds(10));
        Assert.Single(fixture.ReadIntents());
        await fixture.StopAsync();

        fixture.Start(new FakeAdapter((attempt, _) =>
        {
            Assert.Equal("second", attempt.Target.Id);
            Assert.Equal(2, attempt.Number);
            return Task.FromResult(TestOutcome.Failed);
        }, "first", "second"));
        await fixture.WaitForSchedulerAsync("idle");
        Assert.Empty(fixture.ReadIntents());
        Assert.Collection(fixture.ReadAttempts(),
            first => Assert.Equal(("first", "passed"), (first.Target, first.State)),
            interrupted => Assert.Equal(("second", "interrupted"), (interrupted.Target, interrupted.State)),
            failed => Assert.Equal(("second", "failed"), (failed.Target, failed.State)));
    }

    [Fact]
    public async Task ObsoleteAttemptKeepsBothSourceChangesPendingUntilNewAttemptCompletes()
    {
        await using var fixture = new Fixture();
        var first = await fixture.WriteBaselineAsync();
        var second = Path.Combine(fixture.Root, "Other.cs");
        await File.WriteAllTextAsync(second, "class Other {}");
        var started = new TaskCompletionSource<TestAttempt>(TaskCreationOptions.RunContinuationsAsynchronously);
        var release = new TaskCompletionSource(TaskCreationOptions.RunContinuationsAsynchronously);
        var nextStarted = new TaskCompletionSource<TestAttempt>(TaskCreationOptions.RunContinuationsAsynchronously);
        var finishSecond = new TaskCompletionSource(TaskCreationOptions.RunContinuationsAsynchronously);
        fixture.Start(new FakeAdapter(async (attempt, token) =>
        {
            if (started.TrySetResult(attempt))
            {
                // Simulate an in-process adapter returning a late success after cancellation.
                await release.Task.WaitAsync(TimeSpan.FromSeconds(8));
            }
            else
            {
                nextStarted.TrySetResult(attempt);
                await finishSecond.Task.WaitAsync(token);
            }
            return TestOutcome.Passed;
        }, "unit"));
        await fixture.WaitForReportAsync(report => FileCount(report) == 2);
        await File.WriteAllTextAsync(first, "class Edited {}");
        var active = await started.Task.WaitAsync(TimeSpan.FromSeconds(10));
        try
        {
            await File.WriteAllTextAsync(second, "class OtherEdited {}");
            await fixture.WaitForReportAsync(report => CommittedRevision(report) > active.Fence.Revision);
            Assert.Equal(2, fixture.ReadIntents().Length);
            release.TrySetResult();

            var latest = await nextStarted.Task.WaitAsync(TimeSpan.FromSeconds(10));
            Assert.Equal(2, latest.Target.CauseIds.Count);
            Assert.Contains(Assert.Single(active.Target.CauseIds), latest.Target.CauseIds);
            Assert.Equal(2, fixture.ReadIntents().Length);
            Assert.Equal("superseded", fixture.ReadAttempts()[0].State);
            Assert.Equal("passed", fixture.ReadAttempts()[0].Outcome);
        }
        finally
        {
            release.TrySetResult();
            finishSecond.TrySetResult();
        }
        await fixture.WaitForSchedulerAsync("idle");
        Assert.Empty(fixture.ReadIntents());
    }

    [Fact]
    public async Task ExhaustedRetryBudgetRemainsBlockedAcrossRestart()
    {
        await using var fixture = new Fixture();
        var source = await fixture.WriteBaselineAsync();
        fixture.Start(new FakeAdapter((_, _) => Task.FromResult(TestOutcome.RetryableFailure), "unit"));
        await fixture.WaitForReportAsync(report => FileCount(report) == 1);
        await File.WriteAllTextAsync(source, "class Edited {}");
        await fixture.WaitForSchedulerAsync("blocked");
        Assert.Equal(new[] { 1, 2, 3 }, fixture.ReadAttempts().Select(receipt => receipt.Number));
        Assert.Single(fixture.ReadIntents());
        await fixture.StopAsync();

        fixture.Start(new FakeAdapter((_, _) => throw new InvalidOperationException("Exhausted plan ran again."), "unit"));
        await fixture.WaitForSchedulerAsync("blocked");
        Assert.Equal(3, fixture.ReadAttempts().Length);
        Assert.Single(fixture.ReadIntents());
    }

    [Fact]
    public async Task PartialSelectionKeepsTheIntentAndReusesMatchingEvidenceAfterRestart()
    {
        await using var fixture = new Fixture();
        var source = await fixture.WriteBaselineAsync();
        fixture.Start(new FakeAdapter((_, _) => Task.FromResult(TestOutcome.Passed), "unit") { Complete = false });
        await fixture.WaitForReportAsync(report => FileCount(report) == 1);
        await File.WriteAllTextAsync(source, "class Edited {}");
        await fixture.WaitForSchedulerAsync("awaiting_selection");
        var receipt = Assert.Single(fixture.ReadAttempts());
        Assert.Equal("passed", receipt.State);
        Assert.Single(fixture.ReadIntents());
        await fixture.StopAsync();

        fixture.Start(new FakeAdapter((_, _) => throw new InvalidOperationException("Matching evidence was ignored."), "unit"));
        await fixture.WaitForSchedulerAsync("idle");
        Assert.Equal(receipt, Assert.Single(fixture.ReadAttempts()));
        Assert.Empty(fixture.ReadIntents());
    }

    [Fact]
    public async Task OutcomeWriteFailureRetriesRecordingWithoutRerunningTests()
    {
        await using var fixture = new Fixture();
        var source = await fixture.WriteBaselineAsync();
        var finish = new TaskCompletionSource(TaskCreationOptions.RunContinuationsAsynchronously);
        fixture.Start(new FakeAdapter(async (_, token) =>
        {
            await finish.Task.WaitAsync(token);
            return TestOutcome.Passed;
        }, "unit"));
        await fixture.WaitForReportAsync(report => FileCount(report) == 1);
        await File.WriteAllTextAsync(source, "class Edited {}");
        await fixture.WaitForSchedulerAsync("running");
        fixture.ExecuteSql("""
            CREATE TRIGGER fail_receipt BEFORE UPDATE ON test_attempt WHEN NEW.state = 'passed'
            BEGIN SELECT RAISE(ABORT, 'injected receipt write failure'); END;
            """);
        finish.TrySetResult();
        await fixture.WaitForSchedulerAsync("degraded");
        var running = Assert.Single(fixture.ReadAttempts());
        Assert.Equal("running", running.State);
        Assert.Single(fixture.ReadIntents());

        fixture.ExecuteSql("DROP TRIGGER fail_receipt");
        await fixture.WaitForSchedulerAsync("idle");
        var completed = Assert.Single(fixture.ReadAttempts());
        Assert.Equal(running.Id, completed.Id);
        Assert.Equal("passed", completed.State);
        Assert.Empty(fixture.ReadIntents());
    }

    [Fact]
    public async Task MigratingTheOldSchemaPreservesItsPendingSourceChanges()
    {
        await using var fixture = new Fixture();
        var source = await fixture.WriteBaselineAsync();
        fixture.Start();
        var baseline = await fixture.WaitForReportAsync(report => FileCount(report) == 1);
        await File.WriteAllTextAsync(source, "class Edited { }");
        await fixture.WaitForNextReportAsync(baseline);
        var oldIntent = Assert.Single(fixture.ReadIntents());
        await fixture.StopAsync();
        fixture.ExecuteSql("DROP TABLE project_graph; DROP TABLE test_evidence; DROP TABLE test_attempt; PRAGMA user_version = 1;");

        fixture.Start();
        var migrated = await fixture.WaitForReportAsync(report => FileCount(report) == 1);
        Assert.True(CommittedRevision(migrated) > oldIntent.Revision);
        Assert.Equal(oldIntent, Assert.Single(fixture.ReadIntents()));
        Assert.Empty(fixture.ReadAttempts());
    }

    [Fact]
    public async Task EvaluatedConditionalAndLinkedSourcesSelectOnlyTheirMappedTargets()
    {
        if (!OperatingSystem.IsWindowsVersionAtLeast(10))
            return;
        await using var fixture = new Fixture();
        await fixture.WriteInputAsync("Library/Library.csproj", """
            <Project Sdk="Microsoft.NET.Sdk" InitialTargets="EvaluationMustSkipTargets">
              <PropertyGroup><TargetFrameworks>net8.0;net10.0</TargetFrameworks></PropertyGroup>
              <ItemGroup>
                <Compile Remove="Modern.cs" Condition="'$(TargetFramework)' != 'net10.0'" />
                <Compile Include="../Shared/Linked.cs" Link="LibraryLink.cs"
                         Condition="'$(TargetFramework)' == 'net8.0'" />
              </ItemGroup>
              <Target Name="EvaluationMustSkipTargets">
                <WriteLinesToFile File="../target-ran.txt" Lines="ran" />
              </Target>
            </Project>
            """);
        await fixture.WriteInputAsync("Tests/Tests.csproj", """
            <Project Sdk="Microsoft.NET.Sdk">
              <PropertyGroup>
                <TargetFrameworks>net9.0;net10.0</TargetFrameworks>
                <IsTestProject>true</IsTestProject>
              </PropertyGroup>
              <ItemGroup><ProjectReference Include="../Library/Library.csproj" /></ItemGroup>
            </Project>
            """);
        await fixture.WriteInputAsync("Pinned/Pinned.csproj", """
            <Project Sdk="Microsoft.NET.Sdk">
              <PropertyGroup>
                <TargetFramework>net10.0</TargetFramework><IsTestProject>true</IsTestProject>
              </PropertyGroup>
              <ItemGroup>
                <ProjectReference Include="../Library/Library.csproj" SetTargetFramework="TargetFramework=net8.0" />
                <Compile Include="../Shared/Linked.cs" Link="Fixture.cs" />
              </ItemGroup>
            </Project>
            """);
        var modern = await fixture.WriteInputAsync("Library/Modern.cs", "class Modern {}");
        var linked = await fixture.WriteInputAsync("Shared/Linked.cs", "class Linked {}");
        fixture.Start(new EvaluatedTestAdapter(new ProjectEvaluator(fixture.Workspace),
            ["Tests/Tests.csproj", "Pinned/Pinned.csproj"], (_, _) => Task.FromResult(TestOutcome.Passed)));

        var baseline = await fixture.WaitForSchedulerAsync("idle");
        var graph = fixture.ReadGraph(CommittedRevision(baseline));
        Assert.True(graph.Complete, JsonSerializer.Serialize(graph.Issues));
        Assert.Equal(5, graph.Targets.Count);
        var library8 = Assert.Single(graph.Targets, target => target.Id == "Library/Library.csproj|net8.0");
        var library10 = Assert.Single(graph.Targets, target => target.Id == "Library/Library.csproj|net10.0");
        var tests9 = Assert.Single(graph.Targets, target => target.Id == "Tests/Tests.csproj|net9.0");
        var tests10 = Assert.Single(graph.Targets, target => target.Id == "Tests/Tests.csproj|net10.0");
        var pinned = Assert.Single(graph.Targets, target => target.Id == "Pinned/Pinned.csproj|net10.0");
        Assert.Equal(new CompileMember("Shared/Linked.cs", "LibraryLink.cs"), Assert.Single(library8.Compile));
        Assert.Equal("Library/Modern.cs", Assert.Single(library10.Compile).Path);
        Assert.Equal(library8.Id, Assert.Single(tests9.References));
        Assert.Equal(library10.Id, Assert.Single(tests10.References));
        Assert.Equal(library8.Id, Assert.Single(pinned.References));
        Assert.Equal(new CompileMember("Shared/Linked.cs", "Fixture.cs"), Assert.Single(pinned.Compile));
        Assert.False(File.Exists(Path.Combine(fixture.Root, "target-ran.txt")));

        await File.WriteAllTextAsync(modern, "class ModernChanged {}");
        var first = await fixture.WaitForCompletedPlanAsync(0);
        Assert.Equal(tests10.Id, Assert.Single(fixture.ReadAttempts()).Target);

        await File.WriteAllTextAsync(linked, "class LinkedChanged {}");
        await fixture.WaitForCompletedPlanAsync(first);
        Assert.Equal(new[] { tests10.Id, pinned.Id, tests9.Id }, fixture.ReadAttempts().Select(attempt => attempt.Target));
        Assert.Empty(fixture.ReadIntents());
        Assert.False(File.Exists(Path.Combine(fixture.Root, "target-ran.txt")));
    }

    [Fact]
    public async Task ChangingProjectMetadataReplansTheSameIntentAgainstBothMemberships()
    {
        if (!OperatingSystem.IsWindowsVersionAtLeast(10))
            return;
        await using var fixture = new Fixture();
        var props = await fixture.WriteInputAsync("Directory.Build.props",
            "<Project><PropertyGroup><Route>old</Route></PropertyGroup></Project>");
        await fixture.WriteInputAsync("Library/Library.csproj", """
            <Project Sdk="Microsoft.NET.Sdk">
              <PropertyGroup>
                <TargetFrameworks>net8.0;net10.0</TargetFrameworks>
                <EnableDefaultCompileItems>false</EnableDefaultCompileItems>
              </PropertyGroup>
              <ItemGroup>
                <Compile Include="Routed.cs" Condition="('$(Route)' == 'old' And '$(TargetFramework)' == 'net10.0')
                    Or ('$(Route)' == 'new' And '$(TargetFramework)' == 'net8.0')" />
              </ItemGroup>
            </Project>
            """);
        await fixture.WriteInputAsync("Tests/Tests.csproj", """
            <Project Sdk="Microsoft.NET.Sdk">
              <PropertyGroup>
                <TargetFrameworks>net8.0;net10.0</TargetFrameworks>
                <IsTestProject>true</IsTestProject>
              </PropertyGroup>
              <ItemGroup><ProjectReference Include="../Library/Library.csproj" /></ItemGroup>
            </Project>
            """);
        var source = await fixture.WriteInputAsync("Library/Routed.cs", "class Routed {}");
        var first = new TaskCompletionSource<TestAttempt>(TaskCreationOptions.RunContinuationsAsynchronously);
        var release = new TaskCompletionSource(TaskCreationOptions.RunContinuationsAsynchronously);
        var runs = new ConcurrentQueue<TestAttempt>();
        fixture.Start(new EvaluatedTestAdapter(new ProjectEvaluator(fixture.Workspace), ["Tests/Tests.csproj"],
            async (attempt, _) =>
            {
                runs.Enqueue(attempt);
                if (first.TrySetResult(attempt))
                    await release.Task.WaitAsync(TimeSpan.FromSeconds(12));
                return TestOutcome.Passed;
            }));
        await fixture.WaitForSchedulerAsync("idle");
        await File.WriteAllTextAsync(source, "class RoutedChanged {}");
        var initial = await first.Task.WaitAsync(TimeSpan.FromSeconds(15));
        try
        {
            Assert.Equal("Tests/Tests.csproj|net10.0", initial.Target.Id);
            var intent = Assert.Single(fixture.ReadIntents());
            Assert.StartsWith("graph:", intent.Predecessor);
            await File.WriteAllTextAsync(props, "<Project><PropertyGroup><Route>new</Route></PropertyGroup></Project>");
            await fixture.WaitForReportAsync(report => CommittedRevision(report) > initial.Fence.Revision);
            Assert.Equal(intent, Assert.Single(fixture.ReadIntents()));
        }
        finally
        {
            release.TrySetResult();
        }
        await fixture.WaitForCompletedPlanAsync(initial.Id);
        Assert.Equal(new[] { "superseded", "passed", "passed" }, fixture.ReadAttempts().Select(attempt => attempt.State));
        var executed = runs.ToArray();
        Assert.Equal(new[] { "Tests/Tests.csproj|net10.0", "Tests/Tests.csproj|net10.0", "Tests/Tests.csproj|net8.0" },
            executed.Select(attempt => attempt.Target.Id));
        Assert.NotEqual(initial.Target.InputFingerprint, executed[1].Target.InputFingerprint);
        Assert.All(executed, attempt => Assert.Equal(initial.Target.CauseIds, attempt.Target.CauseIds));
        Assert.Empty(fixture.ReadIntents());
    }

    [Fact]
    public async Task DeletedSourceUsesItsPersistedPredecessorAfterRestart()
    {
        if (!OperatingSystem.IsWindowsVersionAtLeast(10))
            return;
        await using var fixture = new Fixture();
        await fixture.WriteInputAsync("Library/Library.csproj", """
            <Project Sdk="Microsoft.NET.Sdk">
              <PropertyGroup><TargetFramework>net10.0</TargetFramework></PropertyGroup>
            </Project>
            """);
        await fixture.WriteInputAsync("Related/Tests.csproj", """
            <Project Sdk="Microsoft.NET.Sdk">
              <PropertyGroup><TargetFramework>net10.0</TargetFramework><IsTestProject>true</IsTestProject></PropertyGroup>
              <ItemGroup><ProjectReference Include="../Library/Library.csproj" /></ItemGroup>
            </Project>
            """);
        await fixture.WriteInputAsync("Unrelated/Tests.csproj", """
            <Project Sdk="Microsoft.NET.Sdk">
              <PropertyGroup><TargetFramework>net10.0</TargetFramework><IsTestProject>true</IsTestProject></PropertyGroup>
            </Project>
            """);
        var source = await fixture.WriteInputAsync("Library/Delete.cs", "class Deleted {}");
        var evaluator = new ProjectEvaluator(fixture.Workspace);
        string[] approved = ["Related/Tests.csproj", "Unrelated/Tests.csproj"];
        var started = new TaskCompletionSource<TestAttempt>(TaskCreationOptions.RunContinuationsAsynchronously);
        fixture.Start(new EvaluatedTestAdapter(evaluator, approved, async (attempt, token) =>
        {
            started.TrySetResult(attempt);
            await Task.Delay(Timeout.InfiniteTimeSpan, token);
            return TestOutcome.Passed;
        }));
        await fixture.WaitForSchedulerAsync("idle");
        File.Delete(source);
        var original = await started.Task.WaitAsync(TimeSpan.FromSeconds(15));
        Assert.Equal("Related/Tests.csproj|net10.0", original.Target.Id);
        Assert.StartsWith("graph:", Assert.Single(fixture.ReadIntents()).Predecessor);
        await fixture.StopAsync();

        fixture.Start(new EvaluatedTestAdapter(evaluator, approved, (attempt, _) =>
        {
            Assert.Equal(original.Target.Id, attempt.Target.Id);
            Assert.Equal(original.Target.CauseIds, attempt.Target.CauseIds);
            return Task.FromResult(TestOutcome.Passed);
        }));
        await fixture.WaitForCompletedPlanAsync(original.Id);
        Assert.Equal(new[] { "interrupted", "passed" }, fixture.ReadAttempts().Select(attempt => attempt.State));
        Assert.Empty(fixture.ReadIntents());
    }

    [Fact]
    public async Task UnresolvedReferenceLeavesAChangePending()
    {
        if (!OperatingSystem.IsWindowsVersionAtLeast(10))
            return;
        await using var fixture = new Fixture();
        await fixture.WriteInputAsync("Library/Library.csproj", """
            <Project Sdk="Microsoft.NET.Sdk">
              <PropertyGroup><TargetFramework>net10.0</TargetFramework></PropertyGroup>
            </Project>
            """);
        await fixture.WriteInputAsync("Tests/Tests.csproj", """
            <Project Sdk="Microsoft.NET.Sdk">
              <PropertyGroup><TargetFramework>net10.0</TargetFramework><IsTestProject>true</IsTestProject></PropertyGroup>
              <ItemGroup>
                <ProjectReference Include="../Library/Library.csproj" AdditionalProperties="Configuration=Release" />
              </ItemGroup>
            </Project>
            """);
        fixture.Start(new EvaluatedTestAdapter(new ProjectEvaluator(fixture.Workspace), ["Tests/Tests.csproj"],
            (_, _) => throw new InvalidOperationException("An unresolved reference was used as a confirmed edge.")));
        await fixture.WaitForSchedulerAsync("idle");
        await fixture.WriteInputAsync("Library/Added.cs", "class Added {}");
        var status = await fixture.WaitForSchedulerAsync("awaiting_selection");
        var graph = fixture.ReadGraph(CommittedRevision(status));
        Assert.False(graph.Complete);
        Assert.Contains(new ProjectEvaluationIssue("Tests/Tests.csproj", "unresolved_reference_context"), graph.Issues);
        Assert.Empty(Assert.Single(graph.Targets, target => target.IsTestProject).References);
        Assert.Single(fixture.ReadIntents());
        Assert.Empty(fixture.ReadAttempts());
    }

    private static int FileCount(JsonElement report) => report.GetProperty("fileCount").GetInt32();
    private static long CommittedRevision(JsonElement report) => report.GetProperty("committedRevision").GetInt64();

    private sealed record Intent(long Revision, string? OldPath, string? OldDigest,
        string? NewPath, string? NewDigest, string Predecessor);

    private sealed record AttemptRow(long Id, int Number, string Target, string State, string? Outcome, long? RetryAt);

    private sealed class FakeAdapter(Func<TestAttempt, CancellationToken, Task<TestOutcome>> run, params string[] targets)
        : ITestAdapter
    {
        internal bool Complete { get; init; } = true;

        public Task<TestSelection> SelectAsync(TestSelectionInput input, CancellationToken cancellationToken)
        {
            var ids = input.Causes.Select(cause => cause.Id).ToArray();
            return Task.FromResult(new TestSelection(Complete,
                targets.Select(target => new SelectedTarget(target, target + "-fixture", ids)).ToArray()));
        }

        public Task<TestOutcome> RunAsync(TestAttempt attempt, WorkerLease worker, CancellationToken cancellationToken) =>
            run(attempt, cancellationToken);
    }

    private sealed class Fixture : IAsyncDisposable
    {
        private readonly CancellationTokenSource cancellation = new();
        private readonly string userState;
        private Task? daemon;

        internal Fixture(string? sharedUserState = null)
        {
            Container = Directory.CreateTempSubdirectory("slophawk-").FullName;
            Root = Directory.CreateDirectory(Path.Combine(Container, "workspace")).FullName;
            userState = sharedUserState ?? Path.Combine(Container, "state");
        }

        internal string Container { get; }
        internal string Root { get; }
        internal string ReportPath => Path.Combine(Root, ".slophawk", "results.json");
        internal Workspace Workspace => new(Root, userState);
        internal DaemonConnection Connection => new(Workspace);

        internal void Start(ITestAdapter? adapter = null, bool grant = true)
        {
            if (adapter is not null && grant && !File.Exists(ExecutionProfile.PathFor(Workspace)))
                WriteProfile(["evaluate", "test"], []);
            daemon = Task.Factory.StartNew(() => DaemonHost.Run(Workspace, adapter, cancellation.Token),
                CancellationToken.None, TaskCreationOptions.LongRunning, TaskScheduler.Default);
        }

        internal void WriteProfile(string[] operations, string[] outputRoots)
        {
            Directory.CreateDirectory(Workspace.StateDirectory);
            var profile = ExecutionProfile.PathFor(Workspace);
            File.WriteAllText(profile + ".tmp", JsonSerializer.Serialize(new
            {
                schemaVersion = 1,
                rootIdentity = RootIdentity.Read(Root),
                operations,
                outputRoots
            }));
            File.Move(profile + ".tmp", profile, overwrite: true);
        }

        internal Guid ReadLease(long attempt)
        {
            using var connection = OpenInventory();
            using var query = connection.CreateCommand();
            query.CommandText = "SELECT lease_id FROM test_attempt WHERE id = $attempt";
            query.Parameters.AddWithValue("$attempt", attempt);
            return Guid.ParseExact(Assert.IsType<string>(query.ExecuteScalar()), "N");
        }

        internal async Task<string> WriteBaselineAsync()
        {
            var source = Path.Combine(Root, "Source.cs");
            await File.WriteAllTextAsync(source, "class Source {}");
            return source;
        }

        internal Task<JsonElement> WaitForSchedulerAsync(string state) => WaitForStatusAsync(status =>
            status.GetProperty("testScheduler").GetProperty("state").GetString() == state);

        internal async Task<long> WaitForCompletedPlanAsync(long previousAttempt)
        {
            var status = await WaitForStatusAsync(value =>
            {
                var scheduler = value.GetProperty("testScheduler");
                return scheduler.GetProperty("state").GetString() == "idle" &&
                    scheduler.GetProperty("latest") is { ValueKind: JsonValueKind.Object } latest &&
                    latest.GetProperty("id").GetInt64() > previousAttempt;
            });
            return status.GetProperty("testScheduler").GetProperty("latest").GetProperty("id").GetInt64();
        }

        internal async Task<string> WriteInputAsync(string relative, string contents)
        {
            var path = Path.Combine(Root, relative);
            Directory.CreateDirectory(Path.GetDirectoryName(path)!);
            await File.WriteAllTextAsync(path, contents);
            return path;
        }

        internal ProjectGraph ReadGraph(long revision)
        {
            using var connection = OpenInventory();
            using var command = connection.CreateCommand();
            command.CommandText = "SELECT payload FROM project_graph WHERE revision = $revision";
            command.Parameters.AddWithValue("$revision", revision);
            return JsonSerializer.Deserialize<ProjectGraph>(Assert.IsType<byte[]>(command.ExecuteScalar()))!;
        }

        internal async Task<JsonElement> ReadStatusAsync()
        {
            var reply = await Connection.SendAsync(DaemonCommand.Status);
            Assert.True(reply?.Ok);
            return reply!.Payload!.Value;
        }

        internal async Task<JsonElement> WaitForStatusAsync(Func<JsonElement, bool> matches)
        {
            using var timeout = new CancellationTokenSource(TimeSpan.FromSeconds(12));
            while (true)
            {
                if (daemon?.IsCompleted == true)
                    await daemon;
                var reply = await Connection.SendAsync(DaemonCommand.Status, timeout.Token);
                if (reply is { Ok: true, Payload: { } status } && matches(status))
                    return status;
                await Task.Delay(50, timeout.Token);
            }
        }

        internal Intent[] ReadIntents()
        {
            using var connection = OpenInventory();
            using var command = connection.CreateCommand();
            command.CommandText = """
                SELECT revision, old_path, old_digest, new_path, new_digest, predecessor
                FROM test_intent ORDER BY revision, path_key
                """;
            using var reader = command.ExecuteReader();
            var intents = new List<Intent>();
            while (reader.Read())
                intents.Add(new Intent(reader.GetInt64(0), ReadText(1), ReadText(2),
                    ReadText(3), ReadText(4), reader.GetString(5)));
            return intents.ToArray();

            string? ReadText(int column) => reader.IsDBNull(column) ? null : reader.GetString(column);
        }

        internal AttemptRow[] ReadAttempts()
        {
            using var connection = OpenInventory();
            using var command = connection.CreateCommand();
            command.CommandText = "SELECT id, number, target, state, outcome, retry_at FROM test_attempt ORDER BY id";
            using var reader = command.ExecuteReader();
            var attempts = new List<AttemptRow>();
            while (reader.Read())
            {
                attempts.Add(new AttemptRow(reader.GetInt64(0), reader.GetInt32(1), reader.GetString(2), reader.GetString(3),
                    reader.IsDBNull(4) ? null : reader.GetString(4), reader.IsDBNull(5) ? null : reader.GetInt64(5)));
            }
            return attempts.ToArray();
        }

        internal string ReadDigest(string path)
        {
            using var connection = OpenInventory();
            using var command = connection.CreateCommand();
            command.CommandText = "SELECT digest FROM source WHERE path = $path";
            command.Parameters.AddWithValue("$path", path);
            return Assert.IsType<string>(command.ExecuteScalar());
        }

        internal void ExecuteSql(string sql)
        {
            using var connection = OpenInventory();
            using var command = connection.CreateCommand();
            command.CommandText = sql;
            command.ExecuteNonQuery();
        }

        private SqliteConnection OpenInventory()
        {
            var connection = new SqliteConnection(new SqliteConnectionStringBuilder
            {
                DataSource = Workspace.InventoryPath,
                Mode = SqliteOpenMode.ReadWrite,
                Pooling = false,
                DefaultTimeout = 2
            }.ToString());
            connection.Open();
            return connection;
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

        internal Task<JsonElement> WaitForNextReportAsync(JsonElement previous) =>
            WaitForReportAsync(report => CommittedRevision(report) > CommittedRevision(previous));

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
                         "-c", "user.name=SlopHawk", "-c", "user.email=slophawk@example.invalid",
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
            catch (OperationCanceledException) when (cancellation.IsCancellationRequested)
            {
                // The fixture can exit while a restarted daemon is still initializing.
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
