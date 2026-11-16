using System.ComponentModel;
using System.Diagnostics;
using System.Text.Json;
using Microsoft.Data.Sqlite;

namespace SlopHawk.Daemon;

internal sealed class WorkerAdmission(Workspace workspace)
{
    private static readonly long OwnerStarted = ReadOwnerStart();

    internal bool CanRecover()
    {
        using var connection = Open();
        using var transaction = connection.BeginTransaction();
        var slot = ReadSlot(connection, transaction);
        if (slot is not null && Reclaim(connection, transaction, slot))
            slot = null;
        transaction.Commit();
        return slot?.Workspace != workspace.InstanceName;
    }

    internal (WorkerLease? Lease, string? Waiting) TryAcquire(ExecutionGrant grant, WorkerRequirements request)
    {
        var requirements = request with { OutputRoots = ExecutionProfile.NormalizeRoots(workspace, request.OutputRoots) };
        grant.Require(requirements);
        if (requirements.UsesProcesses && !OperatingSystem.IsWindowsVersionAtLeast(10))
            throw new UnauthorizedAccessException("A supported process-tree supervisor is unavailable.");

        var id = Guid.NewGuid();
        using (var connection = Open())
        using (var transaction = connection.BeginTransaction())
        {
            var active = ReadSlot(connection, transaction);
            if (active is not null && Reclaim(connection, transaction, active))
                active = null;
            if (!QueueTurn(connection, transaction, enqueue: grant.FromProfile) || active is not null)
            {
                var overlap = active is not null && requirements.OutputRoots.Any(path => active.Outputs.Any(root =>
                    ExecutionProfile.Contains(root, path) || ExecutionProfile.Contains(path, root)));
                transaction.Commit();
                return (null, overlap ? "waiting_output" : "waiting_worker");
            }

            using var claim = Command(connection, transaction, """
                INSERT INTO worker_slot(id, lease, workspace, owner_pid, owner_started, uses_processes, outputs)
                VALUES (1, $lease, $workspace, $pid, $started, $processes, $outputs)
                """, ("$lease", id.ToString("N")), ("$workspace", workspace.InstanceName), ("$pid", Environment.ProcessId),
                ("$started", OwnerStarted), ("$processes", requirements.UsesProcesses),
                ("$outputs", JsonSerializer.Serialize(requirements.OutputRoots)));
            claim.ExecuteNonQuery();
            using var dequeue = Command(connection, transaction,
                "DELETE FROM worker_waiter WHERE workspace = $workspace", ("$workspace", workspace.InstanceName));
            dequeue.ExecuteNonQuery();
            transaction.Commit();
        }

        try
        {
            var job = requirements.UsesProcesses ? new WindowsJob(id) : null;
            return (new WorkerLease(this, workspace, id, grant, requirements, job), null);
        }
        catch
        {
            if (WindowsJob.IsDrained(id))
                Release(id);
            throw;
        }
    }

    internal void Release(Guid id)
    {
        using var connection = Open();
        using var transaction = connection.BeginTransaction();
        DeleteSlot(connection, transaction, id);
        transaction.Commit();
    }

    private bool QueueTurn(SqliteConnection connection, SqliteTransaction transaction, bool enqueue)
    {
        var now = DateTimeOffset.UtcNow.ToUnixTimeMilliseconds();
        using (var prune = Command(connection, transaction,
            "DELETE FROM worker_waiter WHERE expires <= $now OR expires > $latest",
            ("$now", now), ("$latest", now + 3_000)))
            prune.ExecuteNonQuery();
        if (enqueue)
        {
            using var queue = Command(connection, transaction, """
                INSERT INTO worker_waiter(workspace, expires)
                    SELECT $workspace, $expires WHERE
                        EXISTS(SELECT 1 FROM worker_waiter WHERE workspace = $workspace) OR
                        (SELECT count(*) FROM worker_waiter) < 64
                    ON CONFLICT(workspace) DO UPDATE SET expires = excluded.expires
                """, ("$workspace", workspace.InstanceName), ("$expires", now + 3_000));
            queue.ExecuteNonQuery();
        }
        using var first = Command(connection, transaction, "SELECT workspace FROM worker_waiter ORDER BY ticket LIMIT 1");
        var waiting = first.ExecuteScalar() as string;
        return enqueue ? waiting == workspace.InstanceName : waiting is null;
    }

    private static bool Reclaim(SqliteConnection connection, SqliteTransaction transaction, Slot slot)
    {
        if (!OwnerGone(slot) || slot.UsesProcesses && !WindowsJob.IsDrained(slot.Lease))
            return false;
        DeleteSlot(connection, transaction, slot.Lease);
        return true;
    }

    private static bool OwnerGone(Slot slot)
    {
        try
        {
            using var process = Process.GetProcessById(slot.OwnerPid);
            return process.HasExited || process.StartTime.ToUniversalTime().Ticks != slot.OwnerStarted;
        }
        catch (ArgumentException)
        {
            return true; // The system can no longer find the recorded process ID.
        }
        catch (Exception ex) when (ex is InvalidOperationException or Win32Exception)
        {
            return false;
        }
    }

    private static long ReadOwnerStart()
    {
        using var process = Process.GetCurrentProcess();
        return process.StartTime.ToUniversalTime().Ticks;
    }

    private static Slot? ReadSlot(SqliteConnection connection, SqliteTransaction transaction)
    {
        using var query = Command(connection, transaction, """
            SELECT lease, workspace, owner_pid, owner_started, uses_processes, outputs FROM worker_slot WHERE id = 1
            """);
        using var row = query.ExecuteReader();
        if (!row.Read())
            return null;
        if (!Guid.TryParseExact(row.GetString(0), "N", out var id))
            throw new IOException("The worker reservation identity could not be verified.");
        string[] outputs;
        try
        {
            outputs = JsonSerializer.Deserialize<string[]>(row.GetString(5))
                ?? throw new IOException("The output lease could not be verified.");
        }
        catch (JsonException ex)
        {
            throw new IOException("The output lease could not be verified.", ex);
        }
        var pid = row.GetInt32(2);
        var started = row.GetInt64(3);
        if (pid <= 0 || started <= 0 || outputs.Length > 32 || outputs.Any(path => !Path.IsPathFullyQualified(path)))
            throw new IOException("The worker reservation could not be verified.");
        return new Slot(id, row.GetString(1), pid, started, row.GetBoolean(4), outputs);
    }

    private static void DeleteSlot(SqliteConnection connection, SqliteTransaction transaction, Guid id)
    {
        using var command = Command(connection, transaction,
            "DELETE FROM worker_slot WHERE lease = $lease", ("$lease", id.ToString("N")));
        command.ExecuteNonQuery();
    }

    private SqliteConnection Open()
    {
        var directory = Path.Combine(workspace.UserStateDirectory, "workers");
        Workspace.CreatePrivateDirectory(directory);
        var path = Path.Combine(directory, "worker-admission.sqlite3");
        foreach (var entry in new[] { path, path + "-journal", path + "-wal", path + "-shm" })
        {
            try
            {
                if ((File.GetAttributes(entry) & (FileAttributes.Directory | FileAttributes.ReparsePoint)) != 0)
                    throw new IOException("An unexpected path overlaps the worker admission database.");
            }
            catch (FileNotFoundException)
            {
                // SQLite owns these files in the user state directory.
            }
        }
        var connection = new SqliteConnection(new SqliteConnectionStringBuilder
        {
            DataSource = path, Pooling = false, DefaultTimeout = 1
        }.ToString());
        try
        {
            connection.Open();
            using var settings = connection.CreateCommand();
            settings.CommandText = "PRAGMA synchronous=EXTRA;";
            settings.ExecuteNonQuery();
            using var transaction = connection.BeginTransaction();
            using var command = Command(connection, transaction, "PRAGMA user_version");
            var version = (long)command.ExecuteScalar()!;
            if (version == 0)
            {
                command.CommandText = "SELECT count(*) FROM sqlite_schema";
                if ((long)command.ExecuteScalar()! != 0)
                    throw new IOException("The worker admission database has an unrecognized schema.");
                command.CommandText = """
                    CREATE TABLE worker_slot (
                        id INTEGER PRIMARY KEY CHECK (id = 1), lease TEXT NOT NULL, workspace TEXT NOT NULL,
                        owner_pid INTEGER NOT NULL, owner_started INTEGER NOT NULL, uses_processes INTEGER NOT NULL,
                        outputs TEXT NOT NULL
                    );
                    """;
                command.ExecuteNonQuery();
            }
            if (version is 0 or 1)
            {
                command.CommandText = """
                    CREATE TABLE worker_waiter (
                        ticket INTEGER PRIMARY KEY AUTOINCREMENT, workspace TEXT UNIQUE NOT NULL, expires INTEGER NOT NULL
                    );
                    PRAGMA user_version=2;
                    """;
                command.ExecuteNonQuery();
            }
            else if (version != 2)
                throw new IOException("The worker admission database has an incompatible schema.");
            transaction.Commit();
            return connection;
        }
        catch
        {
            connection.Dispose();
            throw;
        }
    }

    private static SqliteCommand Command(SqliteConnection connection, SqliteTransaction transaction, string sql,
        params (string Name, object Value)[] parameters)
    {
        var command = connection.CreateCommand();
        command.Transaction = transaction;
        command.CommandText = sql;
        foreach (var (name, value) in parameters)
            command.Parameters.AddWithValue(name, value);
        return command;
    }

    private sealed record Slot(Guid Lease, string Workspace, int OwnerPid, long OwnerStarted, bool UsesProcesses,
        string[] Outputs);
}

internal sealed class WorkerLease(WorkerAdmission admission, Workspace workspace, Guid id, ExecutionGrant grant,
    WorkerRequirements requirements, WindowsJob? job) : IAsyncDisposable
{
    private bool disposed;
    internal Guid Id => id;
    internal string ProfileRevision => grant.Revision;

    internal void RequireTestOutputRoot(string path)
    {
        Revalidate();
        if (job is null || !requirements.Operations.HasFlag(ExecutionOperation.Test) ||
            !requirements.OutputRoots.Any(root => ExecutionProfile.Contains(root, path)))
            throw new UnauthorizedAccessException("The process lease does not cover this test output root.");
    }

    internal void Revalidate()
    {
        ObjectDisposedException.ThrowIf(disposed, this);
        if (!grant.FromProfile && RootIdentity.Read(workspace.Root) != grant.RootIdentity)
            throw new UnauthorizedAccessException("The workspace root changed during evaluation.");
        var current = grant.FromProfile ? ExecutionProfile.Read(workspace, grant.RootIdentity) : grant;
        if (current.Revision != grant.Revision)
            throw new UnauthorizedAccessException("The workspace execution profile changed.");
        current.Require(requirements with { OutputRoots = ExecutionProfile.NormalizeRoots(workspace, requirements.OutputRoots) });
    }

    internal Task<WorkerResult> RunAsync(ProcessStartInfo command, int maxOutput, CancellationToken cancellationToken)
    {
        Revalidate();
        if (job is null)
            throw new InvalidOperationException("This worker was admitted without a process scope.");
        return job.RunAsync(command, maxOutput, cancellationToken);
    }

    public async ValueTask DisposeAsync()
    {
        if (disposed)
            return;
        disposed = true;
        if (job is not null)
        {
            using (job)
                await job.DrainAsync();
        }
        // Keep the reservation if draining failed. Another owner can only reclaim
        // it after proving the previous owner and its process scope are gone.
        for (var attempt = 0; ; attempt++)
        {
            try
            {
                admission.Release(id);
                return;
            }
            catch (SqliteException ex) when (attempt < 2 && ex.SqliteErrorCode is 5 or 6)
            {
                await Task.Delay(100);
            }
        }
    }
}

internal sealed record WorkerResult(int ExitCode, string StandardOutput, string StandardError);
