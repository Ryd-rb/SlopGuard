using Microsoft.Data.Sqlite;
using SlopHawk.Daemon;
using System.Diagnostics;
using System.Security.AccessControl;
using System.Security.Cryptography;
using System.Security.Principal;
using System.Text.Json;
using Xunit;

namespace SlopHawk.IntegrationTests;

[Collection(NativeCliCollection.Name)]
public sealed partial class WorkspaceTests
{
    [NativeCliFact]
    public async Task RustImportRetainsPendingIntentsAndReceiptsWithoutChangingSqlite()
    {
        await using var fixture = new Fixture(useDefaultState: true);
        if (OperatingSystem.IsWindows())
        {
            // Elevated Windows runners can default the directory owner to Administrators.
            // This fixture represents a user-owned inventory.
            using var user = WindowsIdentity.GetCurrent();
            var acl = new DirectorySecurity();
            acl.SetOwner(user.User!);
            acl.SetAccessRuleProtection(isProtected: true, preserveInheritance: false);
            acl.AddAccessRule(new FileSystemAccessRule(user.User!, FileSystemRights.FullControl,
                InheritanceFlags.ContainerInherit | InheritanceFlags.ObjectInherit,
                PropagationFlags.None, AccessControlType.Allow));
            new DirectoryInfo(fixture.Workspace.StateDirectory).Create(acl);
        }
        var (store, attempt, work, selection) = PrepareReceiptAttempt(fixture);
        store.Finish(attempt, TestOutcome.Passed, work, selection, DateTimeOffset.UtcNow,
            () => false, Receipt(attempt));
        var previous = Snapshot(fixture.Workspace, includeVersion: true);
        var unavailable = await Native("start");
        Assert.NotEqual(0, unavailable.Code);
        Assert.Contains("offline state importer", unavailable.Error);

        var executable = Environment.GetEnvironmentVariable("SLOPHAWK_NATIVE_MIGRATION_BINARY")!;
        var digest = Convert.ToHexString(SHA256.HashData(await File.ReadAllBytesAsync(executable)));
        MigrationStateBridge.ImportLegacyInventory(fixture.Workspace, executable, digest);
        Assert.Equal(previous, Snapshot(fixture.Workspace, includeVersion: true));
        var markerPath = Path.Combine(fixture.Workspace.StateDirectory, "migration-engine.json");
        using var marker = JsonDocument.Parse(await File.ReadAllBytesAsync(markerPath));
        Assert.Equal(2, marker.RootElement.GetProperty("version").GetInt32());
        var epoch = marker.RootElement.GetProperty("epoch").GetString();
        var backupPath = Path.Combine(fixture.Workspace.StateDirectory, $"migration-backup-{epoch}.json");
        using var backup = JsonDocument.Parse(await File.ReadAllBytesAsync(backupPath));
        var tables = backup.RootElement.GetProperty("tables").EnumerateArray()
            .ToDictionary(table => table.GetProperty("name").GetString()!, table => table.GetProperty("rows"));
        Assert.Equal("6", Assert.Single(tables["test_intent"].EnumerateArray())[0].GetProperty("value").GetString());
        Assert.Equal("11", Assert.Single(tables["test_run_receipt"].EnumerateArray())[0].GetProperty("value").GetString());
        Assert.Throws<IOException>(() => MigrationStateBridge.ImportLegacyInventory(fixture.Workspace, executable, digest));

        try
        {
            var started = await Native("start");
            Assert.True(started.Code == 0, started.Error);
            using var published = JsonDocument.Parse(started.Output);
            Assert.True(published.RootElement.GetProperty("committedRevision").GetInt64() >= 7);
            Assert.Equal(1, published.RootElement.GetProperty("pendingTestIntents").GetProperty("detailedCount").GetInt32());
        }
        finally
        {
            await Native("stop");
            using var deadline = new CancellationTokenSource(TimeSpan.FromSeconds(5));
            while (File.Exists(Path.Combine(fixture.Workspace.StateDirectory, "native-daemon.json")))
                await Task.Delay(30, deadline.Token);
        }
        Assert.Equal(previous, Snapshot(fixture.Workspace, includeVersion: true));

        async Task<(int Code, string Output, string Error)> Native(string verb)
        {
            var command = NativeCliFactAttribute.Command(verb, "--root", fixture.Root);
            command.RedirectStandardOutput = true;
            command.RedirectStandardError = true;
            using var process = Process.Start(command)!;
            var output = process.StandardOutput.ReadToEndAsync();
            var error = process.StandardError.ReadToEndAsync();
            await process.WaitForExitAsync().WaitAsync(TimeSpan.FromSeconds(15));
            return (process.ExitCode, await output, await error);
        }
    }

    [Fact]
    public async Task DetailedAttemptCommitsWithResolvedIntentAndSurvivesReopen()
    {
        await using var fixture = new Fixture();
        var (store, attempt, work, selection) = PrepareReceiptAttempt(fixture);
        var detail = Receipt(attempt);

        var (pending, result) = store.Finish(attempt, TestOutcome.Passed, work, selection,
            DateTimeOffset.UtcNow, () => true, detail);

        Assert.Equal("passed", result.State);
        Assert.Equal("none", pending.State);
        Assert.Equal(detail, store.ReadTestRunReceipt(attempt.Id));
        Assert.Equal(detail, new SourceInventory(fixture.Workspace).ReadTestRunReceipt(attempt.Id));
        using var db = OpenMigrationFixture(fixture.Workspace, SqliteOpenMode.ReadWrite);
        Assert.Equal(0L, Number(db, "SELECT count(*) FROM test_intent"));
        Assert.Equal(1L, Number(db, "SELECT count(*) FROM test_run_receipt"));
        Assert.Equal("passed", TextValue(db, "SELECT outcome FROM test_attempt"));
    }

    [Fact]
    public async Task LegacyCallersLeaveDetailsAbsentAndKeepAggregateBehavior()
    {
        await using var fixture = new Fixture();
        var (store, attempt, work, selection) = PrepareReceiptAttempt(fixture);
        var (pending, result) = store.Finish(attempt, TestOutcome.Failed, work, selection,
            DateTimeOffset.UtcNow, () => true);
        Assert.Equal("failed", result.Outcome);
        Assert.Equal("none", pending.State);
        Assert.Null(new SourceInventory(fixture.Workspace).ReadTestRunReceipt(attempt.Id));
    }

    [Fact]
    public async Task StaleFencePreservesHistoricalDetailWithoutCurrentEvidence()
    {
        await using var fixture = new Fixture();
        var (store, attempt, work, selection) = PrepareReceiptAttempt(fixture);
        var (pending, result) = store.Finish(attempt, TestOutcome.Passed, work, selection,
            DateTimeOffset.UtcNow, () => false, Receipt(attempt));
        Assert.Equal("superseded", result.State);
        Assert.Equal("passed", result.Outcome);
        Assert.Equal("awaiting_selection", pending.State);
        Assert.NotNull(new SourceInventory(fixture.Workspace).ReadTestRunReceipt(attempt.Id));
        Assert.Equal(1, new SourceInventory(fixture.Workspace).Load().Pending.DetailedCount);
        using var db = OpenMigrationFixture(fixture.Workspace, SqliteOpenMode.ReadWrite);
        Assert.Equal(1L, Number(db, "SELECT count(*) FROM test_intent"));
        Assert.Equal(0L, Number(db, "SELECT count(*) FROM test_evidence"));
    }

    [Theory]
    [InlineData("receipt")]
    [InlineData("outcome")]
    public async Task FailedReceiptOrOutcomeWriteRollsBackEverythingAndCanReplay(string stage)
    {
        await using var fixture = new Fixture();
        var (store, attempt, work, selection) = PrepareReceiptAttempt(fixture);
        var trigger = stage == "receipt"
            ? "CREATE TRIGGER fail_write BEFORE INSERT ON test_run_receipt BEGIN SELECT RAISE(ABORT, 'injected'); END;"
            : "CREATE TRIGGER fail_write BEFORE UPDATE ON test_attempt BEGIN SELECT RAISE(ABORT, 'injected'); END;";
        using (var db = OpenMigrationFixture(fixture.Workspace, SqliteOpenMode.ReadWrite))
            ExecuteMigrationSql(db, trigger);

        Assert.Throws<SqliteException>(() => store.Finish(attempt, TestOutcome.Passed, work, selection,
            DateTimeOffset.UtcNow, () => true, Receipt(attempt)));
        using (var db = OpenMigrationFixture(fixture.Workspace, SqliteOpenMode.ReadWrite))
        {
            Assert.Equal("running", TextValue(db, "SELECT state FROM test_attempt"));
            Assert.Equal(1L, Number(db, "SELECT count(*) FROM test_intent"));
            Assert.Equal(0L, Number(db, "SELECT count(*) FROM test_run_receipt"));
            Assert.Equal(0L, Number(db, "SELECT count(*) FROM test_evidence"));
            ExecuteMigrationSql(db, "DROP TRIGGER fail_write");
        }
        store.Finish(attempt, TestOutcome.Passed, work, selection, DateTimeOffset.UtcNow,
            () => true, Receipt(attempt));
        Assert.NotNull(new SourceInventory(fixture.Workspace).ReadTestRunReceipt(attempt.Id));
        Assert.Throws<IOException>(() => store.Finish(attempt, TestOutcome.Passed, work, selection,
            DateTimeOffset.UtcNow, () => true, Receipt(attempt)));
    }

    [Fact]
    public async Task ContradictoryReceiptsCannotFinalizeAttempt()
    {
        await using var fixture = new Fixture();
        var (store, attempt, work, selection) = PrepareReceiptAttempt(fixture);
        var valid = Receipt(attempt);
        var wrongTarget = valid with { Target = "not-the-admitted-target" };
        Assert.Throws<IOException>(() => store.Finish(attempt, TestOutcome.Passed, work, selection,
            DateTimeOffset.UtcNow, () => true, wrongTarget));
        var zeroDiscovery = valid with
        {
            Platform = VstestDisposition.ZeroDiscovered,
            Execution = VstestExecution.ZeroDiscovered,
            Counters = new VstestTrxCounters(0, 0, 0, 0, 0, 0)
        };
        Assert.Throws<IOException>(() => store.Finish(attempt, TestOutcome.Passed, work, selection,
            DateTimeOffset.UtcNow, () => true, zeroDiscovery));
        Assert.Throws<IOException>(() => store.Finish(attempt, TestOutcome.Passed, work, selection,
            DateTimeOffset.UtcNow, () => true, valid with
            {
                Counters = new VstestTrxCounters(2, 0, 0, 0, 2, 0)
            }));
        Assert.Null(store.ReadTestRunReceipt(attempt.Id));
        using var db = OpenMigrationFixture(fixture.Workspace, SqliteOpenMode.ReadWrite);
        Assert.Equal("running", TextValue(db, "SELECT state FROM test_attempt"));
        Assert.Equal(1L, Number(db, "SELECT count(*) FROM test_intent"));
    }

    [Theory]
    [InlineData("ZeroDiscovered", "ZeroDiscovered")]
    [InlineData("AllSkipped", "AllSkipped")]
    [InlineData("Timeout", "Unknown")]
    [InlineData("StartupOrBuildFailure", "Unknown")]
    public async Task NonPassingPlatformDispositionIsNotCurrentPassingEvidence(
        string platform, string executionState)
    {
        var disposition = Enum.Parse<VstestDisposition>(platform);
        var execution = Enum.Parse<VstestExecution>(executionState);
        await using var fixture = new Fixture();
        var (store, attempt, work, selection) = PrepareReceiptAttempt(fixture);
        var baseReceipt = Receipt(attempt);
        var counts = disposition switch
        {
            VstestDisposition.ZeroDiscovered => new VstestTrxCounters(0, 0, 0, 0, 0, 0),
            VstestDisposition.AllSkipped => new VstestTrxCounters(2, 0, 0, 0, 2, 0),
            _ => null
        };
        var detail = baseReceipt with
        {
            Platform = disposition, Execution = execution,
            Runner = disposition switch
            {
                VstestDisposition.Timeout => VstestRunnerHealth.TimedOut,
                VstestDisposition.StartupOrBuildFailure => VstestRunnerHealth.StartupOrBuildFailure,
                _ => VstestRunnerHealth.ZeroExit
            },
            Artifact = counts is null ? VstestArtifactHealth.Missing : VstestArtifactHealth.Complete,
            ArtifactSha256 = counts is null ? null : baseReceipt.ArtifactSha256,
            ArtifactBytes = counts is null ? null : baseReceipt.ArtifactBytes,
            RunId = counts is null ? null : baseReceipt.RunId,
            Counters = counts
        };
        var (pending, result) = store.Finish(attempt, TestOutcome.RetryableFailure,
            work, selection, DateTimeOffset.UtcNow, () => true, detail);
        Assert.Equal("retryable_failure", result.Outcome);
        Assert.Equal("awaiting_selection", pending.State);
        Assert.Equal(detail, new SourceInventory(fixture.Workspace).ReadTestRunReceipt(attempt.Id));
    }

    [Theory]
    [InlineData("Failed", "Failed")]
    [InlineData("Cancelled", "Cancelled")]
    [InlineData("Blocked", "StartupOrBuildFailure")]
    public async Task SchedulerAndPlatformStatesRemainIndependentAfterRestart(string scheduler, string platform)
    {
        await using var fixture = new Fixture();
        var (store, attempt, work, selection) = PrepareReceiptAttempt(fixture);
        var outcome = Enum.Parse<TestOutcome>(scheduler);
        var disposition = Enum.Parse<VstestDisposition>(platform);
        var original = Receipt(attempt);
        var detail = disposition switch
        {
            VstestDisposition.Failed => original with
            {
                Platform = disposition, Execution = VstestExecution.Failed,
                Runner = VstestRunnerHealth.NonzeroExit,
                Counters = new VstestTrxCounters(2, 2, 1, 1, 0, 0), ReasonCode = "failed"
            },
            VstestDisposition.Cancelled => original with
            {
                Platform = disposition, Runner = VstestRunnerHealth.Cancelled,
                Artifact = VstestArtifactHealth.Missing, Execution = VstestExecution.Unknown,
                ArtifactSha256 = null, ArtifactBytes = null, RunId = null,
                Counters = null, ReasonCode = "cancelled"
            },
            _ => original with
            {
                Platform = disposition, Runner = VstestRunnerHealth.StartupOrBuildFailure,
                Artifact = VstestArtifactHealth.Missing, Execution = VstestExecution.Unknown,
                ArtifactSha256 = null, ArtifactBytes = null, RunId = null,
                Counters = null, ReasonCode = "build_blocked"
            }
        };
        var (pending, state) = store.Finish(attempt, outcome, work, selection,
            DateTimeOffset.UtcNow, () => true, detail);
        Assert.Equal(scheduler.ToLowerInvariant(), state.Outcome);
        Assert.Equal(outcome == TestOutcome.Failed ? "none" : "awaiting_selection", pending.State);
        Assert.Equal(detail, new SourceInventory(fixture.Workspace).ReadTestRunReceipt(attempt.Id));
    }

    [Fact]
    public async Task BoundedMetadataRejectsUntrustedMessagesBeforeDatabaseWrite()
    {
        await using var fixture = new Fixture();
        var (store, attempt, work, selection) = PrepareReceiptAttempt(fixture);
        Assert.Throws<IOException>(() => store.Finish(attempt, TestOutcome.Passed, work, selection,
            DateTimeOffset.UtcNow, () => true, Receipt(attempt) with { ReasonCode = "C:\\secret\\password" }));
        Assert.Throws<IOException>(() => store.Finish(attempt, TestOutcome.Passed, work, selection,
            DateTimeOffset.UtcNow, () => true, Receipt(attempt) with { BuildSha256 = "not-a-sha256" }));
        Assert.Null(store.ReadTestRunReceipt(attempt.Id));
    }

    [Fact]
    public async Task ReceiptRetentionFollowsTheAttemptHistory()
    {
        await using var fixture = new Fixture();
        var (store, attempt, work, selection) = PrepareReceiptAttempt(fixture);
        store.Finish(attempt, TestOutcome.Passed, work, selection, DateTimeOffset.UtcNow,
            () => true, Receipt(attempt));
        using (var db = OpenMigrationFixture(fixture.Workspace, SqliteOpenMode.ReadWrite))
        {
            ExecuteMigrationSql(db, """
                WITH RECURSIVE ids(value) AS (VALUES(12) UNION ALL SELECT value + 1 FROM ids WHERE value < 74)
                INSERT INTO test_attempt(id, number, revision, epoch, target, fingerprint, causes, plan_key, state)
                SELECT value, 1, revision, epoch, target, fingerprint, causes, 'history-' || value, 'superseded'
                FROM ids CROSS JOIN test_attempt WHERE id = 11;
                """);
        }
        var grant = new ExecutionGrant("receipt-retention", attempt.Fence.RootIdentity,
            ExecutionOperation.Test, [], FromProfile: false);
        var (lease, _) = new WorkerAdmission(fixture.Workspace)
            .TryAcquire(grant, new WorkerRequirements(ExecutionOperation.Test, false, []));
        Assert.NotNull(lease);
        await using var worker = lease!;
        Assert.NotNull(store.Admit(attempt.Fence, attempt.Target, worker, DateTimeOffset.UtcNow, () => true).Attempt);
        using var verify = OpenMigrationFixture(fixture.Workspace, SqliteOpenMode.ReadWrite);
        Assert.Equal(64L, Number(verify, "SELECT count(*) FROM test_attempt"));
        Assert.Null(new SourceInventory(fixture.Workspace).ReadTestRunReceipt(attempt.Id));
    }

    private static (SourceInventory Store, TestAttempt Attempt, TestWork Work, ValidatedSelection Selection)
        PrepareReceiptAttempt(Fixture fixture)
    {
        var store = new SourceInventory(fixture.Workspace);
        store.Load();
        var epoch = Guid.NewGuid();
        var root = RootIdentity.Read(fixture.Root);
        var fence = new ScheduleFence(epoch, 1, 7, root, new GitRevision("feature/receipts", "commit-seven"));
        var cause = new TestCause(6, "Source.cs", "Source.cs", "old", "Source.cs", "new", "graph_unavailable");
        var target = new SelectedTarget("unit", new string('A', 64), [cause.Id]);
        var attempt = new TestAttempt(11, 1, fence, target, "profile-one");
        var work = new TestWork(new TestSelectionInput(fence, [cause]), [],
            new PendingIntents(1, 6, 6, null), null);
        var selection = new ValidatedSelection(work, new TestSelection(true, [target]));
        using var db = OpenMigrationFixture(fixture.Workspace, SqliteOpenMode.ReadWrite);
        using var command = db.CreateCommand();
        command.CommandText = "UPDATE workspace SET revision = 7, root_identity = $root;";
        command.Parameters.AddWithValue("$root", root);
        command.ExecuteNonQuery();
        using var insert = db.CreateCommand();
        insert.CommandText = """
            INSERT INTO test_intent VALUES (6, 'Source.cs', 'Source.cs', 'old', 'Source.cs', 'new', 'graph_unavailable');
            INSERT INTO test_attempt(id, number, revision, epoch, target, fingerprint, causes,
                plan_key, state, lease_id, profile_revision)
            VALUES (11, 1, 7, $epoch, 'unit', $fp, $causes, 'plan-one', 'running', 'lease-one', 'profile-one');
            """;
        insert.Parameters.AddWithValue("$epoch", epoch.ToString());
        insert.Parameters.AddWithValue("$fp", target.InputFingerprint);
        insert.Parameters.AddWithValue("$causes", System.Text.Json.JsonSerializer.Serialize(target.CauseIds));
        insert.ExecuteNonQuery();
        return (store, attempt, work, selection);
    }

    private static TestRunReceiptDetails Receipt(TestAttempt attempt) => new(
        attempt.Id, attempt.Fence.Revision, attempt.Fence.Epoch, attempt.Fence.Sequence,
        TestRunReceiptDetails.GitIdentity(attempt.Fence.Git), attempt.Fence.RootIdentity,
        attempt.Target.Id, attempt.Target.InputFingerprint, attempt.ProfileRevision, "lease-one",
        BuildOutputIdentity.DomainVersion, new string('A', 64), new string('B', 64),
        new string('C', 64), 214, Guid.Parse("cf0f3a54-1de8-42fa-865f-dad0c8b13f3d"),
        VstestDisposition.Passed, VstestRunnerHealth.ZeroExit, VstestArtifactHealth.Complete,
        VstestExecution.Passed, new VstestTrxCounters(2, 2, 2, 0, 0, 0), "passed");
}
