using System.Threading.Channels;
using Microsoft.Data.Sqlite;
using SlopGuard.Core;

namespace SlopGuard.Daemon;

internal sealed class WorkspaceMonitor : IDisposable
{
    private const int MaxPendingPaths = 512;
    private readonly Channel<bool> wakeup = Channel.CreateBounded<bool>(1);
    private readonly object gate = new();
    private Guid epoch = Guid.NewGuid();
    private readonly HashSet<string> pending = new(Workspace.PathComparer);
    private readonly List<FileSystemWatcher> watchers = [];
    private readonly Workspace workspace;
    private readonly SourceScanner scanner;
    private readonly SourceInventory inventory;
    private readonly ReportFile reportFile;
    private readonly DurableTestScheduler? scheduler;
    private GitContext gitContext;
    private FileSystemWatcher? parentWatcher;
    private WorkspaceReport? report;
    private InventoryState? committed;
    private SourceScan? reconciled;
    private ScheduleFence? schedule;
    private TestSchedulerStatus scheduleStatus;
    private long revision;
    private bool scanAll;
    private bool restartWatchers;
    private string? lastError;
    private string? recordingError;

    internal WorkspaceMonitor(Workspace workspace, GitContext git, ITestAdapter? adapter)
    {
        this.workspace = workspace;
        gitContext = git;
        scanner = new SourceScanner(workspace);
        inventory = new SourceInventory(workspace);
        reportFile = new ReportFile(workspace);
        scheduler = adapter is null ? null : new DurableTestScheduler(inventory, this, adapter);
        scheduleStatus = new TestSchedulerStatus(adapter is null ? "awaiting_adapter" : "pending_reconciliation");
    }

    internal Task? RunSchedulerAsync(CancellationToken cancellationToken) => scheduler?.RunAsync(cancellationToken);

    internal void Start()
    {
        if (Directory.GetParent(workspace.Root) is { } parent)
        {
            parentWatcher = Watch(parent.FullName, recursive: false, NotifyFilters.DirectoryName, (path, _) =>
            {
                if (Workspace.PathComparer.Equals(path, workspace.Root))
                    Invalidate(restart: true);
            });
        }
        StartWorkspaceWatchers();
        Invalidate();
    }

    private void StartWorkspaceWatchers()
    {
        foreach (var existing in watchers)
            existing.Dispose();
        watchers.Clear();
        Workspace.CheckDirectory(workspace.Root);
        watchers.Add(Watch(workspace.Root, recursive: true,
            NotifyFilters.FileName | NotifyFilters.LastWrite | NotifyFilters.Size, (path, structural) =>
            {
                if (!workspace.IsIncluded(path))
                {
                    if (Workspace.PathComparer.Equals(path, Path.Combine(workspace.Root, ".git")))
                        Invalidate(restart: true);
                    return;
                }
                if (SourceScanner.Matches(path))
                    Invalidate(structural ? null : path);
                else if (IsProjectInput(path))
                    Invalidate();
            }));
        watchers.Add(Watch(workspace.Root, recursive: true, NotifyFilters.DirectoryName, (path, _) =>
        {
            if (Workspace.PathComparer.Equals(path, Path.Combine(workspace.Root, ".git")))
                Invalidate(restart: true);
            else if (workspace.IsIncluded(path))
                Invalidate();
        }));
        foreach (var directory in gitContext.MetadataDirectories)
        {
            watchers.Add(Watch(directory, recursive: true,
                NotifyFilters.FileName | NotifyFilters.DirectoryName | NotifyFilters.LastWrite | NotifyFilters.Size,
                (path, _) =>
                {
                    if (GitContext.IsContextChange(directory, path))
                        Invalidate(transition: true);
                }));
        }
    }

    internal long Invalidate(string? path = null, bool restart = false, bool transition = false)
    {
        long current;
        lock (gate)
        {
            current = ++revision;
            if (restart || transition)
                epoch = Guid.NewGuid();
            restartWatchers |= restart;
            if (path is null || pending.Count == MaxPendingPaths)
            {
                scanAll = true;
                pending.Clear();
            }
            if (!scanAll && path is not null)
                pending.Add(path);
            wakeup.Writer.TryWrite(true);
        }
        scheduler?.Signal();
        return current;
    }

    internal object Status()
    {
        lock (gate)
        {
            var snapshot = report?.Revision == revision ? report : null;
            return new
            {
                workspace.Root, epoch, revision, committedRevision = committed?.Revision,
                reportSchemaVersion = ReportFile.SchemaVersion,
                state = snapshot is not null ? "ready" : lastError is null ? "pending" : "degraded",
                lastError, fileCount = snapshot?.FileCount, partial = snapshot?.Partial,
                recordingError, testScheduler = scheduleStatus,
                pendingTestIntents = committed?.Pending, retiredTestIntents = committed?.Retirement,
            };
        }
    }

    internal async Task<WorkspaceReport?> ReadCurrentAsync(CancellationToken cancellationToken)
    {
        WorkspaceReport? snapshot;
        string? rootIdentity;
        lock (gate)
        {
            snapshot = report?.Revision == revision ? report : null;
            rootIdentity = committed?.RootIdentity;
        }
        if (snapshot is null)
            return null;

        try
        {
            if (await gitContext.ReadAsync(cancellationToken) == new GitRevision(snapshot.Branch, snapshot.Commit)
                && RootIdentity.Read(workspace.Root) == rootIdentity)
                return IsCurrent(snapshot.Revision) ? snapshot : null;
        }
        catch (Exception ex) when (ex is IOException or UnauthorizedAccessException)
        {
            RecordError(ex.Message);
        }
        Invalidate(restart: true);
        return null;
    }

    internal async Task RunAsync(CancellationToken cancellationToken)
    {
        var retryDelay = 250;
        while (await wakeup.Reader.WaitToReadAsync(cancellationToken))
        {
            wakeup.Reader.TryRead(out _);
            var reconnect = false;
            var recording = false;
            try
            {
                Exception? withdrawalError = null;
                try
                {
                    reportFile.Withdraw();
                }
                catch (Exception ex) when (ex is IOException or UnauthorizedAccessException)
                {
                    withdrawalError = ex;
                }
                await Task.Delay(160, cancellationToken);
                var batch = TakeBatch();
                reconnect = batch.Restart;
                if (reconnect)
                {
                    gitContext = await GitContext.DiscoverAsync(workspace.Root, cancellationToken);
                    StartWorkspaceWatchers();
                    reconnect = false;
                }
                var startingGit = await gitContext.ReadAsync(cancellationToken);
                var rootIdentity = RootIdentity.Read(workspace.Root);
                recording = true;
                var basis = committed ?? LoadInventory();
                recording = false;
                var startup = reconciled is null;
                var fullScan = startup || batch.FullScan || reconciled!.Partial || basis.RootIdentity != rootIdentity;
                var scan = await scanner.ScanAsync(fullScan ? null : batch.Paths, reconciled, cancellationToken);
                if (await gitContext.ReadAsync(cancellationToken) != startingGit ||
                    RootIdentity.Read(workspace.Root) != rootIdentity)
                {
                    Invalidate(restart: true);
                    continue;
                }
                if (!IsCurrent(batch.Revision))
                {
                    // A newer event does not cover the paths taken by the discarded scan.
                    Invalidate();
                    continue;
                }

                cancellationToken.ThrowIfCancellationRequested();
                recording = true;
                var next = inventory.Reconcile(basis, scan, rootIdentity, startingGit, startup,
                    () => !cancellationToken.IsCancellationRequested && IsCurrent(batch.Revision));
                recording = false;
                if (next is null)
                {
                    Invalidate();
                    continue;
                }
                reconciled = scan;
                lock (gate)
                {
                    committed = next;
                    recordingError = null;
                    if (revision == batch.Revision && !scan.Partial)
                        schedule = new ScheduleFence(batch.Epoch, batch.Revision, next.Revision, rootIdentity, startingGit);
                }
                scheduler?.Signal();

                if (withdrawalError is not null)
                    throw withdrawalError;
                if (!IsCurrent(batch.Revision))
                    continue;
                var snapshot = new WorkspaceReport(ReportFile.SchemaVersion, ReportFile.Producer, workspace.Root,
                    batch.Epoch, batch.Revision, startingGit.Branch, startingGit.Commit, scan.Files.Count,
                    scan.Partial, next.Revision);
                if (await gitContext.ReadAsync(cancellationToken) != startingGit ||
                    RootIdentity.Read(workspace.Root) != rootIdentity)
                {
                    Invalidate(restart: true);
                    continue;
                }
                if (!reportFile.Publish(snapshot, () => IsCurrent(batch.Revision)))
                    continue;

                lock (gate)
                {
                    report = snapshot;
                    lastError = null;
                }
                if (!IsCurrent(batch.Revision))
                    reportFile.Withdraw();
                retryDelay = 250;
            }
            catch (Exception ex) when (ex is IOException or UnauthorizedAccessException or SqliteException
                                        or System.ComponentModel.Win32Exception || reconnect && ex is ArgumentException)
            {
                RecordError(ex.Message, recording);
                Invalidate(restart: reconnect);
                await Task.Delay(retryDelay, cancellationToken);
                retryDelay = Math.Min(retryDelay * 2, 8_000);
            }
        }
    }

    public void Dispose()
    {
        parentWatcher?.Dispose();
        foreach (var watcher in watchers)
            watcher.Dispose();
        if (Directory.Exists(workspace.Root))
            reportFile.Withdraw();
    }

    private (long Revision, Guid Epoch, string[] Paths, bool FullScan, bool Restart) TakeBatch()
    {
        lock (gate)
        {
            // An event included in this batch may have signalled during the quiet interval.
            wakeup.Reader.TryRead(out _);
            var batch = (revision, epoch, pending.ToArray(), scanAll, restartWatchers);
            pending.Clear();
            scanAll = false;
            restartWatchers = false;
            return batch;
        }
    }

    private bool IsCurrent(long candidate)
    {
        lock (gate)
            return candidate == revision;
    }

    internal ScheduleFence? CurrentSchedule()
    {
        lock (gate)
            return schedule?.Sequence == revision ? schedule : null;
    }

    internal bool ScheduleIsCurrent(ScheduleFence fence) => CurrentSchedule() == fence;

    internal async Task<bool> VerifyScheduleAsync(ScheduleFence fence, CancellationToken cancellationToken)
    {
        if (!ScheduleIsCurrent(fence))
            return false;
        try
        {
            if (await gitContext.ReadAsync(cancellationToken) == fence.Git &&
                RootIdentity.Read(workspace.Root) == fence.RootIdentity)
                return ScheduleIsCurrent(fence);
        }
        catch (Exception ex) when (ex is IOException or UnauthorizedAccessException)
        {
            RecordError(ex.Message);
        }
        Invalidate(restart: true);
        return false;
    }

    internal void UpdateScheduler(TestSchedulerStatus status, ScheduleFence? fence = null, PendingIntents? pending = null)
    {
        lock (gate)
        {
            scheduleStatus = status;
            if (pending is not null && committed is not null && committed.Revision == fence?.Revision)
                committed = committed with { Pending = pending };
        }
    }

    private InventoryState LoadInventory()
    {
        var loaded = inventory.Load();
        lock (gate)
            committed = loaded;
        return loaded;
    }

    private void RecordError(string message, bool recording = false)
    {
        lock (gate)
        {
            lastError = message;
            if (recording)
                recordingError = message;
        }
    }

    private static bool IsProjectInput(string path) =>
        Path.GetExtension(path).ToUpperInvariant() is ".CSPROJ" or ".SLN" or ".SLNX" or ".PROPS" or ".TARGETS" or ".EDITORCONFIG"
        || Path.GetFileName(path).ToUpperInvariant() is "GLOBAL.JSON" or "PACKAGES.LOCK.JSON" or "NUGET.CONFIG";

    private FileSystemWatcher Watch(string root, bool recursive, NotifyFilters filters, Action<string, bool> onPath)
    {
        var watcher = new FileSystemWatcher(root)
        {
            IncludeSubdirectories = recursive,
            NotifyFilter = filters,
        };
        watcher.Changed += (_, args) => onPath(args.FullPath, false);
        watcher.Created += (_, args) => onPath(args.FullPath, true);
        watcher.Deleted += (_, args) => onPath(args.FullPath, true);
        watcher.Renamed += (_, args) =>
        {
            onPath(args.OldFullPath, true);
            onPath(args.FullPath, true);
        };
        watcher.Error += (_, _) => Invalidate(restart: true);
        try
        {
            watcher.EnableRaisingEvents = true;
            return watcher;
        }
        catch
        {
            watcher.Dispose();
            throw;
        }
    }
}
