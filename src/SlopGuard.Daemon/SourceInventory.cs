using Microsoft.Data.Sqlite;

namespace SlopGuard.Daemon;

internal sealed class SourceInventory(Workspace workspace, string policy, bool trackTestIntents)
{
    internal const int MaxDetailedIntents = 2_048;
    private const int MaxIntentPathCharacters = 2_048;
    private const int SchemaVersion = 1;

    internal InventoryState Load()
    {
        using var connection = Open(initialize: true);
        using var transaction = connection.BeginTransaction(deferred: true);
        using var metadata = Command(connection, transaction,
            "SELECT revision, root_identity, policy, retired_revision, retired_intents FROM workspace WHERE id = 1");
        using var row = metadata.ExecuteReader();
        if (!row.Read())
            throw new IOException("The source inventory is missing its workspace record.");

        var revision = row.GetInt64(0);
        var rootIdentity = row.IsDBNull(1) ? null : row.GetString(1);
        var priorPolicy = row.IsDBNull(2) ? null : row.GetString(2);
        var retirement = row.IsDBNull(3) ? null : new RetiredIntents(row.GetInt64(3), row.GetInt32(4));
        row.Close();

        var files = new Dictionary<string, SourceFingerprint>(Workspace.PathComparer);
        using var sources = Command(connection, transaction, "SELECT path, digest, verified FROM source");
        using var sourceRows = sources.ExecuteReader();
        while (sourceRows.Read())
        {
            var path = sourceRows.GetString(0);
            files.Add(path, new SourceFingerprint(path, sourceRows.GetString(1), sourceRows.GetBoolean(2)));
        }
        sourceRows.Close();

        var scopes = new HashSet<string>(Workspace.PathComparer);
        var unobserved = new HashSet<string>(Workspace.PathComparer);
        using var frontier = Command(connection, transaction, "SELECT kind, path FROM scope");
        using var frontierRows = frontier.ExecuteReader();
        while (frontierRows.Read())
            (frontierRows.GetInt32(0) == 0 ? scopes : unobserved).Add(frontierRows.GetString(1));
        frontierRows.Close();

        return new InventoryState(revision, rootIdentity, priorPolicy, files, scopes, unobserved,
            ReadPending(connection, transaction), retirement);
    }

    internal InventoryState? Reconcile(InventoryState basis, SourceScan scan, string rootIdentity, GitRevision git,
        bool startup, Func<bool> accept)
    {
        var rootReplaced = basis.RootIdentity is not null && basis.RootIdentity != rootIdentity;
        var resetInventory = rootReplaced || basis.Policy != policy;
        var createIntents = trackTestIntents && !startup && !resetInventory;
        var files = resetInventory
            ? new Dictionary<string, SourceFingerprint>(Workspace.PathComparer)
            : new Dictionary<string, SourceFingerprint>(basis.Files, Workspace.PathComparer);
        var changes = new List<SourceChange>();

        if (!resetInventory)
        {
            foreach (var prior in basis.Files.Values)
            {
                if (scan.Files.ContainsKey(prior.Path))
                    continue;
                if (SourceScan.IsUncertain(scan.UncertainScopes, prior.Path))
                {
                    if (prior.Verified)
                    {
                        var unverified = prior with { Verified = false };
                        files[prior.Path] = unverified;
                        changes.Add(new SourceChange(prior, unverified));
                    }
                    continue;
                }
                files.Remove(prior.Path);
                changes.Add(new SourceChange(prior, null));
            }
        }

        var pathCharacters = files.Values.Sum(file => file.Path.Length);
        var omitted = new List<string>();
        foreach (var (path, entry) in scan.Files)
        {
            var previous = files.GetValueOrDefault(path);
            var next = new SourceFingerprint(path, entry.Digest);
            if (previous == next)
                continue;

            var growth = path.Length - (previous?.Path.Length ?? 0);
            if ((previous is null && files.Count >= SourceIndex.MaxFiles) ||
                pathCharacters + growth > SourceIndex.MaxPathCharacters)
            {
                omitted.Add(path);
                continue;
            }

            files[path] = next;
            pathCharacters += growth;
            changes.Add(new SourceChange(previous, next));
        }

        if (omitted.Count > 0)
        {
            foreach (var path in omitted)
            {
                scan.Files.Remove(path);
                if (files.TryGetValue(path, out var prior) && prior.Verified)
                {
                    var unverified = prior with { Verified = false };
                    files[path] = unverified;
                    changes.Add(new SourceChange(prior, unverified));
                }
            }
            scan.UncertainScopes.Clear();
            scan.UncertainScopes.Add("");
        }

        var revision = checked(basis.Revision + 1);
        var intents = createIntents ? changes.Where(change =>
            (change.Old?.Path != change.New?.Path || change.Old?.Digest != change.New?.Digest) &&
            (change.Old is not null || !SourceScan.IsUncertain(basis.UnobservedScopes, change.New!.Path))).ToArray() : [];
        // Newly inaccessible scopes keep their prior absence proofs. Scopes left
        // unobserved by the baseline narrow as later scans verify them.
        var unobserved = startup || resetInventory
            ? scan.UncertainScopes
            : IntersectScopes(basis.UnobservedScopes, scan.UncertainScopes);
        var retirement = rootReplaced
            ? new RetiredIntents(revision, basis.Pending.DetailedCount + (basis.Pending.Fallback is null ? 0 : 1))
            : basis.Retirement;
        using var connection = Open(initialize: false);
        using var transaction = connection.BeginTransaction();
        using (var check = Command(connection, transaction, "SELECT revision FROM workspace WHERE id = 1"))
        {
            if ((long?)check.ExecuteScalar() != basis.Revision)
                throw new IOException("The source inventory changed outside its writer.");
        }

        // The watcher can invalidate immediately once this transaction accepts its observation.
        // Its changes still commit together. Only a current fence can publish the resulting report.
        if (!accept())
            return null;

        if (resetInventory)
            Execute(connection, transaction, "DELETE FROM source");
        if (rootReplaced)
            Execute(connection, transaction, "DELETE FROM test_intent; DELETE FROM intent_fallback;");

        using var delete = Command(connection, transaction, "DELETE FROM source WHERE path_key = $key", ("$key", ""));
        using var upsert = Command(connection, transaction, """
            INSERT INTO source(path_key, path, digest, verified) VALUES ($key, $path, $digest, $verified)
            ON CONFLICT(path_key) DO UPDATE SET path = excluded.path, digest = excluded.digest,
                verified = excluded.verified
            """, ("$key", ""), ("$path", ""), ("$digest", ""), ("$verified", true));
        foreach (var change in changes)
        {
            if (change.New is { } next)
            {
                upsert.Parameters["$key"].Value = Key(next.Path);
                upsert.Parameters["$path"].Value = next.Path;
                upsert.Parameters["$digest"].Value = next.Digest;
                upsert.Parameters["$verified"].Value = next.Verified;
                upsert.ExecuteNonQuery();
            }
            else
            {
                delete.Parameters["$key"].Value = Key(change.Old!.Path);
                delete.ExecuteNonQuery();
            }
        }

        UpdateScopes(connection, transaction, 0, basis.UncertainScopes, scan.UncertainScopes);
        UpdateScopes(connection, transaction, 1, basis.UnobservedScopes, unobserved);

        if (intents.Length > 0)
            RecordIntents(connection, transaction, basis.Pending, revision, intents);
        Execute(connection, transaction, """
            UPDATE workspace SET revision = $revision, root_identity = $root, policy = $policy,
                branch = $branch, commit_id = $commit, retired_revision = $retired, retired_intents = $count WHERE id = 1
            """, ("$revision", revision), ("$root", rootIdentity), ("$policy", policy),
            ("$branch", git.Branch), ("$commit", git.Commit), ("$retired", retirement?.Revision),
            ("$count", retirement?.Count));

        var pending = ReadPending(connection, transaction);
        transaction.Commit();
        return new InventoryState(revision, rootIdentity, policy, files, scan.UncertainScopes, unobserved, pending, retirement);
    }

    private static string Key(string path) => OperatingSystem.IsWindows() ? path.ToUpperInvariant() : path;

    private static HashSet<string> IntersectScopes(HashSet<string> first, HashSet<string> second)
    {
        var intersection = new HashSet<string>(Workspace.PathComparer);
        foreach (var scope in first.Where(path => SourceScan.IsUncertain(second, path)))
            intersection.Add(scope);
        foreach (var scope in second.Where(path => SourceScan.IsUncertain(first, path)))
            intersection.Add(scope);
        // Existing nested scopes do not need independent rows in the frontier.
        intersection.RemoveWhere(path => path.Length != 0 &&
            SourceScan.IsUncertain(intersection, path[..Math.Max(0, path.LastIndexOf('/'))]));
        return intersection.Count <= 512 ? intersection : second;
    }

    private static void UpdateScopes(SqliteConnection connection, SqliteTransaction transaction,
        int kind, HashSet<string> before, HashSet<string> after)
    {
        if (after.SetEquals(before))
            return;
        Execute(connection, transaction, "DELETE FROM scope WHERE kind = $kind", ("$kind", kind));
        using var insert = Command(connection, transaction,
            "INSERT INTO scope(kind, path) VALUES ($kind, $path)", ("$kind", kind), ("$path", ""));
        foreach (var path in after)
        {
            insert.Parameters["$path"].Value = path;
            insert.ExecuteNonQuery();
        }
    }

    private static void RecordIntents(SqliteConnection connection, SqliteTransaction transaction,
        PendingIntents pending, long revision, SourceChange[] intents)
    {
        if (pending.Fallback is not null || intents.Length > MaxDetailedIntents - pending.DetailedCount ||
            intents.Any(intent => (intent.Old?.Path.Length ?? 0) + (intent.New?.Path.Length ?? 0) > MaxIntentPathCharacters))
        {
            Execute(connection, transaction, """
                INSERT INTO intent_fallback(id, first_revision, last_revision) VALUES (1, $revision, $revision)
                ON CONFLICT(id) DO UPDATE SET last_revision = excluded.last_revision
                """, ("$revision", revision));
            return;
        }

        using var insert = Command(connection, transaction, """
            INSERT INTO test_intent(revision, path_key, old_path, old_digest, new_path, new_digest, predecessor)
            VALUES ($revision, $key, $oldPath, $oldDigest, $newPath, $newDigest, 'graph_unavailable')
            """, ("$revision", revision), ("$key", ""), ("$oldPath", null), ("$oldDigest", null),
            ("$newPath", null), ("$newDigest", null));
        foreach (var intent in intents)
        {
            insert.Parameters["$key"].Value = Key(intent.New?.Path ?? intent.Old!.Path);
            insert.Parameters["$oldPath"].Value = (object?)intent.Old?.Path ?? DBNull.Value;
            insert.Parameters["$oldDigest"].Value = (object?)intent.Old?.Digest ?? DBNull.Value;
            insert.Parameters["$newPath"].Value = (object?)intent.New?.Path ?? DBNull.Value;
            insert.Parameters["$newDigest"].Value = (object?)intent.New?.Digest ?? DBNull.Value;
            insert.ExecuteNonQuery();
        }
    }

    private static PendingIntents ReadPending(SqliteConnection connection, SqliteTransaction transaction)
    {
        using var detail = Command(connection, transaction, "SELECT count(*), min(revision), max(revision) FROM test_intent");
        using var row = detail.ExecuteReader();
        row.Read();
        var count = row.GetInt32(0);
        var first = row.IsDBNull(1) ? (long?)null : row.GetInt64(1);
        var last = row.IsDBNull(2) ? (long?)null : row.GetInt64(2);
        row.Close();
        using var fallback = Command(connection, transaction, "SELECT first_revision, last_revision FROM intent_fallback");
        using var fallbackRow = fallback.ExecuteReader();
        if (!fallbackRow.Read())
            return new PendingIntents(count, first, last, null);

        var marker = new IntentFallback(fallbackRow.GetInt64(0), fallbackRow.GetInt64(1));
        return new PendingIntents(count, first ?? marker.FirstRevision, marker.LastRevision, marker);
    }

    private SqliteConnection Open(bool initialize)
    {
        Workspace.CheckDirectory(workspace.StateDirectory, allowMissing: initialize);
        if (initialize)
        {
            if (OperatingSystem.IsWindows())
                Directory.CreateDirectory(workspace.StateDirectory);
            else
                Directory.CreateDirectory(workspace.StateDirectory,
                    UnixFileMode.UserRead | UnixFileMode.UserWrite | UnixFileMode.UserExecute);
        }
        var path = workspace.InventoryPath;
        foreach (var candidate in new[] { path, path + "-journal", path + "-wal", path + "-shm" })
        {
            try
            {
                if ((File.GetAttributes(candidate) & (FileAttributes.Directory | FileAttributes.ReparsePoint)) != 0)
                    throw new IOException("The SQLite inventory path contains an unexpected entry.");
            }
            catch (FileNotFoundException)
            {
                // SQLite creates its database and journal when needed.
            }
        }
        var connection = new SqliteConnection(new SqliteConnectionStringBuilder
        {
            DataSource = path,
            Mode = initialize ? SqliteOpenMode.ReadWriteCreate : SqliteOpenMode.ReadWrite,
            Pooling = false,
            DefaultTimeout = 1,
        }.ToString());
        try
        {
            connection.Open();
            using var command = connection.CreateCommand();
            command.CommandText = "PRAGMA synchronous = EXTRA; PRAGMA max_page_count = 16384;";
            command.ExecuteNonQuery();
            command.CommandText = "PRAGMA user_version";
            var version = (long)command.ExecuteScalar()!;
            if (version == 0 && initialize)
            {
                command.CommandText = "SELECT count(*) FROM sqlite_schema";
                if ((long)command.ExecuteScalar()! != 0)
                    throw new IOException("The SQLite inventory has an unrecognized schema.");
                command.CommandText = "PRAGMA page_size = 4096;";
                command.ExecuteNonQuery();
                using var transaction = connection.BeginTransaction();
                Execute(connection, transaction, Schema);
                transaction.Commit();
            }
            else if (version != SchemaVersion)
            {
                throw new IOException("The SQLite inventory schema is incompatible. Pending work was preserved.");
            }

            // One short-lived writer uses a rollback journal. Both files fit inside the workspace disk budget.
            command.CommandText = "PRAGMA page_size";
            if ((long)command.ExecuteScalar()! != 4096)
                throw new IOException("The SQLite inventory has an unsupported page size. Pending work was preserved.");
            return connection;
        }
        catch
        {
            connection.Dispose();
            throw;
        }
    }

    private static void Execute(SqliteConnection connection, SqliteTransaction transaction, string sql,
        params (string Name, object? Value)[] parameters)
    {
        using var command = Command(connection, transaction, sql, parameters);
        command.ExecuteNonQuery();
    }

    private static SqliteCommand Command(SqliteConnection connection, SqliteTransaction transaction, string sql,
        params (string Name, object? Value)[] parameters)
    {
        var command = connection.CreateCommand();
        command.Transaction = transaction;
        command.CommandText = sql;
        foreach (var (name, value) in parameters)
            command.Parameters.AddWithValue(name, value ?? DBNull.Value);
        return command;
    }

    private const string Schema = """
        CREATE TABLE workspace (
            id INTEGER PRIMARY KEY CHECK (id = 1), revision INTEGER NOT NULL,
            root_identity TEXT, policy TEXT, branch TEXT, commit_id TEXT,
            retired_revision INTEGER, retired_intents INTEGER
        );
        INSERT INTO workspace(id, revision) VALUES (1, 0);
        CREATE TABLE source (
            path_key TEXT PRIMARY KEY, path TEXT NOT NULL, digest TEXT NOT NULL, verified INTEGER NOT NULL
        ) WITHOUT ROWID;
        CREATE TABLE scope (kind INTEGER NOT NULL, path TEXT NOT NULL, PRIMARY KEY (kind, path)) WITHOUT ROWID;
        CREATE TABLE test_intent (
            revision INTEGER NOT NULL, path_key TEXT NOT NULL,
            old_path TEXT, old_digest TEXT, new_path TEXT, new_digest TEXT, predecessor TEXT NOT NULL,
            PRIMARY KEY (revision, path_key)
        ) WITHOUT ROWID;
        CREATE TABLE intent_fallback (
            id INTEGER PRIMARY KEY CHECK (id = 1), first_revision INTEGER NOT NULL, last_revision INTEGER NOT NULL
        );
        PRAGMA user_version = 1;
        """;
}

internal sealed record InventoryState(long Revision, string? RootIdentity, string? Policy,
    Dictionary<string, SourceFingerprint> Files, HashSet<string> UncertainScopes,
    HashSet<string> UnobservedScopes, PendingIntents Pending,
    RetiredIntents? Retirement);

internal sealed record SourceFingerprint(string Path, string Digest, bool Verified = true);
internal sealed record SourceChange(SourceFingerprint? Old, SourceFingerprint? New);

internal sealed record PendingIntents(int DetailedCount, long? OldestRevision, long? LatestRevision, IntentFallback? Fallback)
{
    public string State => DetailedCount > 0 || Fallback is not null ? "awaiting_selection" : "none";
}

internal sealed record IntentFallback(long FirstRevision, long LastRevision)
{
    public string Scope => "workspace";
}

internal sealed record RetiredIntents(long Revision, int Count)
{
    public string Reason => "root_replaced";
}
