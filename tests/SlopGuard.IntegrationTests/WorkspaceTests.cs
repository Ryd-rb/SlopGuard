using System.Diagnostics;
using System.Text.Json;
using Microsoft.Data.Sqlite;
using SlopGuard.Core;
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
        Assert.Collection(fixture.ReadIntents().Where(intent => intent.Revision == CommittedRevision(moved)),
            removed => Assert.Equal(("First/Source.cs", null), (removed.OldPath, removed.NewPath)),
            added => Assert.Equal((null, "Second/Source.cs"), (added.OldPath, added.NewPath)));

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
        var online = await fixture.WaitForReportAsync(report => FindingPaths(report).Contains("Source.cs"));
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
        await fixture.WaitForReportAsync(report => CommittedRevision(report) > CommittedRevision(restarted));
        Assert.Equal(saved, Assert.Single(fixture.ReadIntents()));

        await File.WriteAllTextAsync(source, "class AfterRestart {");
        await fixture.WaitForReportAsync(report => FindingPaths(report).Contains("Source.cs"));
        var intents = fixture.ReadIntents();
        Assert.Equal(2, intents.Length);
        Assert.Equal(saved, intents[0]);
        Assert.NotEqual(saved.NewDigest, intents[1].OldDigest);
        Assert.Equal(saved.NewPath, intents[1].OldPath);
    }

    [Fact]
    public async Task EditDuringScanReconcilesBothPathsAgainstTheCommittedBaseline()
    {
        await using var fixture = new Fixture();
        using var checker = new PausingChecker();
        var first = Path.Combine(fixture.Root, "First.cs");
        var second = Path.Combine(fixture.Root, "Second.cs");
        await File.WriteAllTextAsync(first, "class First {}");
        await File.WriteAllTextAsync(second, "class Second {}");
        fixture.Start(checker);
        var baseline = await fixture.WaitForReportAsync(report => FileCount(report) == 2);
        await File.WriteAllTextAsync(first, "class Discarded {");
        await checker.Entered.Task.WaitAsync(TimeSpan.FromSeconds(10));
        try
        {
            var scanning = await fixture.ReadStatusAsync();
            await File.WriteAllTextAsync(second, "class Changed {");
            await fixture.WaitForStatusAsync(status => status.GetProperty("revision").GetInt64() >
                scanning.GetProperty("revision").GetInt64());
            Assert.False((await fixture.Connection.SendAsync(DaemonCommand.Results))?.Ok);
            Assert.Empty(fixture.ReadIntents());
        }
        finally
        {
            checker.Release.Set();
        }

        var report = await fixture.WaitForReportAsync(value => FindingPaths(value).Length == 2);
        Assert.True(CommittedRevision(report) > CommittedRevision(baseline));
        var intents = fixture.ReadIntents();
        Assert.Equal(new[] { "First.cs", "Second.cs" }, intents.Select(intent => intent.NewPath));
        Assert.All(intents, intent => Assert.Equal(CommittedRevision(report), intent.Revision));
        Assert.Equal(report.GetProperty("findings")[0].GetProperty("sourceDigest").GetString(), intents[0].NewDigest);
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
        await fixture.WaitForReportAsync(report => FindingPaths(report).Contains("Source.cs"));
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
            report.GetProperty("partial").GetBoolean() && FindingPaths(report).Contains("Other.cs"));
        Assert.Equal(new[] { "Other.cs" }, FindingPaths(partial));
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
        await fixture.WaitForReportAsync(report => FileCount(report) == 1);
        await File.WriteAllTextAsync(source, "class Changed {");
        await fixture.WaitForReportAsync(report => FindingPaths(report).Contains("Source.cs"));
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
        await fixture.WaitForReportAsync(report => FindingPaths(report).Contains("source.cs"));
        File.Move(original, Path.Combine(fixture.Root, "Source.cs"));

        await fixture.WaitForReportAsync(report => FindingPaths(report).Contains("Source.cs"));
        var change = Assert.Single(fixture.ReadIntents());
        Assert.Equal("source.cs", change.OldPath);
        Assert.Equal("Source.cs", change.NewPath);
        Assert.Equal(change.OldDigest, change.NewDigest);
    }

    [Fact]
    public async Task AcceptedObservationSurvivesAnInvalidationAndIsTheNextTransactionBase()
    {
        await using var fixture = new Fixture();
        var store = new SourceInventory(fixture.Workspace, "fixture", trackTestIntents: true);
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

        var recoveredStore = new SourceInventory(fixture.Workspace, "fixture", trackTestIntents: true);
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
            new(new Dictionary<string, SyntaxEntry>(Workspace.PathComparer) { ["Source.cs"] = new(digest, [], false) },
                new HashSet<string>(Workspace.PathComparer));
    }

    [Fact]
    public async Task TestIntentLimitUsesOneDurableWorkspaceFallback()
    {
        await using var fixture = new Fixture();
        var store = new SourceInventory(fixture.Workspace, "fixture", trackTestIntents: true);
        var root = RootIdentity.Read(fixture.Root);
        var git = new GitRevision(null, null);
        var files = new Dictionary<string, SyntaxEntry>(Workspace.PathComparer);
        var scopes = new HashSet<string>(Workspace.PathComparer);
        var baseline = store.Reconcile(store.Load(), new SourceScan(files, scopes), root, git, startup: true, () => true)!;
        for (var i = 0; i <= SourceInventory.MaxDetailedIntents; i++)
            files.Add($"Source{i}.cs", new SyntaxEntry("first", [], false));
        var overflow = store.Reconcile(baseline, new SourceScan(files, scopes), root, git, startup: false, () => true)!;
        Assert.Equal(new IntentFallback(overflow.Revision, overflow.Revision), overflow.Pending.Fallback);
        Assert.Empty(fixture.ReadIntents());

        var recovered = store.Load();
        Assert.Equal(overflow.Pending, recovered.Pending);
        Assert.Equal(files.Count, recovered.Files.Count);
        files["Source0.cs"] = new SyntaxEntry("second", [], false);
        var next = store.Reconcile(recovered, new SourceScan(files, scopes), root, git, startup: false, () => true)!;
        Assert.Equal(new IntentFallback(overflow.Revision, next.Revision), store.Load().Pending.Fallback);
    }

    private static int FileCount(JsonElement report) => report.GetProperty("fileCount").GetInt32();
    private static long CommittedRevision(JsonElement report) => report.GetProperty("committedRevision").GetInt64();

    private static string?[] FindingPaths(JsonElement report) =>
        report.GetProperty("findings").EnumerateArray().Select(finding => finding.GetProperty("path").GetString()).ToArray();

    private sealed record Intent(long Revision, string? OldPath, string? OldDigest,
        string? NewPath, string? NewDigest, string Predecessor);

    private sealed class PausingChecker : ISourceChecker, IDisposable
    {
        private readonly SyntaxChecker checker = new();
        internal TaskCompletionSource Entered { get; } = new(TaskCreationOptions.RunContinuationsAsynchronously);
        internal ManualResetEventSlim Release { get; } = new();
        public string Id => checker.Id;
        public string Language => checker.Language;
        public bool Matches(string path) => checker.Matches(path);

        public SourceAnalysis Analyze(string source, CancellationToken cancellationToken)
        {
            if (source == "class Discarded {")
            {
                Entered.TrySetResult();
                if (!Release.Wait(TimeSpan.FromSeconds(10), cancellationToken))
                    throw new TimeoutException("The overlap fixture did not release the paused scan.");
            }
            return checker.Analyze(source, cancellationToken);
        }

        public void Dispose() => Release.Dispose();
    }

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
        internal Workspace Workspace => new(Root, Path.Combine(Container, "state"));
        internal DaemonConnection Connection => new(Workspace);

        internal void Start(ISourceChecker? checker = null)
        {
            daemon = Task.Factory.StartNew(() => DaemonHost.Run(Workspace, checker ?? new SyntaxChecker(), cancellation.Token),
                CancellationToken.None, TaskCreationOptions.LongRunning, TaskScheduler.Default);
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
