using System.Threading.Channels;
using SlopGuard.Core;

namespace SlopGuard.Daemon;

internal sealed class WorkspaceMonitor(Workspace workspace, GitContext git, ISourceChecker checker) : IDisposable
{
    private const int MaxPendingPaths = 512;
    private readonly Channel<bool> wakeup = Channel.CreateBounded<bool>(1);
    private readonly object gate = new();
    private readonly Guid epoch = Guid.NewGuid();
    private readonly HashSet<string> pending = new(Workspace.PathComparer);
    private readonly List<FileSystemWatcher> watchers = [];
    private readonly SourceIndex index = new(workspace, checker);
    private readonly ReportFile reportFile = new(workspace);
    private GitContext gitContext = git;
    private FileSystemWatcher? parentWatcher;
    private WorkspaceReport? report;
    private long revision;
    private bool scanAll;
    private bool restartWatchers;
    private string? lastError;

    internal void Start()
    {
        if (Directory.GetParent(workspace.Root) is { } parent)
        {
            parentWatcher = Watch(parent.FullName, recursive: false, NotifyFilters.DirectoryName, path =>
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
        watchers.Add(Watch(workspace.Root, recursive: true,
            NotifyFilters.FileName | NotifyFilters.LastWrite | NotifyFilters.Size, path =>
            {
                if (!workspace.IsIncluded(path))
                {
                    if (Workspace.PathComparer.Equals(path, Path.Combine(workspace.Root, ".git")))
                        Invalidate(restart: true);
                    return;
                }
                if (checker.Matches(path))
                    Invalidate(path);
            }));
        watchers.Add(Watch(workspace.Root, recursive: true, NotifyFilters.DirectoryName, path =>
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
                path =>
                {
                    if (GitContext.IsContextChange(directory, path))
                        Invalidate();
                }));
        }
    }

    internal long Invalidate(string? path = null, bool restart = false)
    {
        long current;
        lock (gate)
        {
            current = ++revision;
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
        return current;
    }

    internal object Status()
    {
        lock (gate)
        {
            var snapshot = report?.Revision == revision ? report : null;
            return new
            {
                workspace.Root, epoch, revision, checker = checker.Id,
                state = snapshot is not null ? "ready" : lastError is null ? "pending" : "degraded",
                lastError, fileCount = snapshot?.FileCount, partial = snapshot?.Partial,
                findingsTruncated = snapshot?.FindingsTruncated
            };
        }
    }

    internal async Task<WorkspaceReport?> ReadCurrentAsync(CancellationToken cancellationToken)
    {
        WorkspaceReport? snapshot;
        lock (gate)
            snapshot = report?.Revision == revision ? report : null;
        if (snapshot is null)
            return null;

        try
        {
            if (await gitContext.ReadAsync(cancellationToken) == new GitRevision(snapshot.Branch, snapshot.Commit)
                && IsCurrent(snapshot.Revision))
                return snapshot;
        }
        catch (Exception ex) when (ex is IOException or UnauthorizedAccessException)
        {
            RecordError(ex.Message);
        }
        Invalidate();
        return null;
    }

    internal async Task RunAsync(CancellationToken cancellationToken)
    {
        var retryDelay = 250;
        while (await wakeup.Reader.WaitToReadAsync(cancellationToken))
        {
            wakeup.Reader.TryRead(out _);
            var reconnect = false;
            try
            {
                reportFile.Withdraw();
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
                await index.RefreshAsync(batch.Paths, batch.FullScan, cancellationToken);
                if (await gitContext.ReadAsync(cancellationToken) != startingGit)
                {
                    Invalidate();
                    continue;
                }
                if (!IsCurrent(batch.Revision))
                    continue;

                var snapshot = index.CreateReport(epoch, batch.Revision, startingGit);
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
            catch (Exception ex) when (ex is IOException or UnauthorizedAccessException
                                        or System.ComponentModel.Win32Exception || reconnect && ex is ArgumentException)
            {
                RecordError(ex.Message);
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

    private (long Revision, string[] Paths, bool FullScan, bool Restart) TakeBatch()
    {
        lock (gate)
        {
            // An event included in this batch may have signalled during the quiet interval.
            wakeup.Reader.TryRead(out _);
            var batch = (revision, pending.ToArray(), scanAll, restartWatchers);
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

    private void RecordError(string message)
    {
        lock (gate)
            lastError = message;
    }

    private FileSystemWatcher Watch(string root, bool recursive, NotifyFilters filters, Action<string> onPath)
    {
        var watcher = new FileSystemWatcher(root)
        {
            IncludeSubdirectories = recursive,
            NotifyFilter = filters,
        };
        watcher.Changed += (_, args) => onPath(args.FullPath);
        watcher.Created += (_, args) => onPath(args.FullPath);
        watcher.Deleted += (_, args) => onPath(args.FullPath);
        watcher.Renamed += (_, args) =>
        {
            onPath(args.OldFullPath);
            onPath(args.FullPath);
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
