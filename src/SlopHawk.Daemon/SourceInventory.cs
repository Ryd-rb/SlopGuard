using Microsoft.Data.Sqlite;

namespace SlopHawk.Daemon;

internal sealed partial class SourceInventory(Workspace workspace)
{
    internal const int MaxDetailedIntents = 2_048;
    private const int SchemaVersion = 5;
    private readonly object writer = new();

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
            if (!files.TryAdd(path, new SourceFingerprint(path, sourceRows.GetString(1), sourceRows.GetBoolean(2))))
                throw new IOException("The source inventory contains duplicate logical paths. Pending work was preserved.");
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
        if (initialize)
            Workspace.CreatePrivateDirectory(workspace.StateDirectory);
        else
            Workspace.CheckDirectory(workspace.StateDirectory);
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
            if (version != 0 || !initialize)
                RequirePageSize(command);
            if (version is >= 0 and < SchemaVersion && initialize)
            {
                if (version == 0)
                {
                    command.CommandText = "SELECT count(*) FROM sqlite_schema";
                    if ((long)command.ExecuteScalar()! != 0)
                        throw new IOException("The SQLite inventory has an unrecognized schema. Pending work was preserved.");
                    command.CommandText = "PRAGMA page_size = 4096;";
                    command.ExecuteNonQuery();
                }
                try
                {
                    // DDL and user_version updates share the same transaction. A failure
                    // in a later schema step must not publish an intermediate version.
                    using var transaction = connection.BeginTransaction();
                    if (version > 0)
                        ValidateLayout(connection, (int)version, transaction);
                    if (version == 0)
                        Execute(connection, transaction, Schema);
                    if (version <= 1)
                        Execute(connection, transaction, SchedulerSchema);
                    if (version <= 2)
                        Execute(connection, transaction, ProjectSchema);
                    if (version <= 3)
                        Execute(connection, transaction, WorkerSchema);
                    if (version <= 4)
                        Execute(connection, transaction, ReceiptSchema);
                    command.Transaction = transaction;
                    RequirePageSize(command);
                    ValidateLayout(connection, SchemaVersion, transaction);
                    transaction.Commit();
                }
                catch (SqliteException exception)
                {
                    throw new IOException($"The SQLite inventory migration from schema {version} to {SchemaVersion} " +
                        "failed. Pending work was preserved; repair the database or restore a backup before retrying.",
                        exception);
                }
            }
            else if (version != SchemaVersion)
            {
                throw new IOException($"The SQLite inventory schema version {version} is incompatible with " +
                    $"schema {SchemaVersion}. Pending work was preserved; open it with a compatible SlopHawk version.");
            }
            else if (initialize)
                ValidateLayout(connection, SchemaVersion);
            return connection;
        }
        catch
        {
            connection.Dispose();
            throw;
        }
    }

    // Check only columns required by this version. Additive columns in an older
    // schema are deliberately left for the transactional upgrader to accept or reject.
    private static void RequirePageSize(SqliteCommand command)
    {
        // The rollback journal and database must fit inside the workspace disk budget.
        command.CommandText = "PRAGMA page_size";
        if ((long)command.ExecuteScalar()! != 4096)
            throw new IOException("The SQLite inventory has an unsupported page size. Pending work was preserved.");
    }

    private static void ValidateLayout(SqliteConnection connection, int version, SqliteTransaction? transaction = null)
    {
        RequireColumns(connection, transaction, "workspace", "id", "revision", "root_identity", "policy", "branch",
            "commit_id", "retired_revision", "retired_intents");
        RequireColumns(connection, transaction, "source", "path_key", "path", "digest", "verified");
        RequireColumns(connection, transaction, "scope", "kind", "path");
        RequireColumns(connection, transaction, "test_intent", "revision", "path_key", "old_path", "old_digest",
            "new_path", "new_digest", "predecessor");
        RequireColumns(connection, transaction, "intent_fallback", "id", "first_revision", "last_revision");
        if (version >= 2)
        {
            RequireColumns(connection, transaction, "test_attempt", "id", "number", "revision", "epoch", "target",
                "fingerprint", "causes", "plan_key", "state", "outcome", "retry_at");
            RequireColumns(connection, transaction, "test_evidence", "cause", "target", "fingerprint");
        }
        if (version >= 3)
            RequireColumns(connection, transaction, "project_graph", "revision", "root_identity", "complete", "payload");
        if (version >= 4)
            RequireColumns(connection, transaction, "test_attempt", "lease_id", "profile_revision");
        if (version >= 5)
            RequireColumns(connection, transaction, "test_run_receipt", "attempt_id", "format", "revision", "epoch",
                "fence_sequence", "git_fingerprint", "root_identity", "target", "fingerprint",
                "profile_revision", "lease_id", "build_domain",
                "build_sha256", "runner_sha256", "artifact_sha256", "artifact_bytes", "run_id", "platform",
                "runner", "artifact", "execution", "total", "executed", "passed", "failed", "skipped",
                "other", "reason_code");
    }

    private static void RequireColumns(
        SqliteConnection connection, SqliteTransaction? transaction, string table, params string[] required)
    {
        using var kind = connection.CreateCommand();
        kind.Transaction = transaction;
        kind.CommandText = "SELECT type FROM sqlite_schema WHERE name = $name";
        kind.Parameters.AddWithValue("$name", table);
        if (kind.ExecuteScalar() as string != "table")
            throw new IOException($"The SQLite inventory has an invalid {table} table. " +
                "Pending work was preserved; repair the layout or restore a compatible backup.");

        using var query = connection.CreateCommand();
        query.Transaction = transaction;
        query.CommandText = "SELECT name FROM pragma_table_info($name)";
        query.Parameters.AddWithValue("$name", table);
        var columns = new HashSet<string>(StringComparer.OrdinalIgnoreCase);
        using var rows = query.ExecuteReader();
        while (rows.Read())
            columns.Add(rows.GetString(0));
        var missing = required.Where(column => !columns.Contains(column)).ToArray();
        if (missing.Length != 0)
            throw new IOException($"The SQLite inventory {table} table is missing column(s): " +
                $"{string.Join(", ", missing)}. Pending work was preserved; repair the layout or restore a backup.");
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

    private const string ProjectSchema = """
        CREATE TABLE project_graph (
            revision INTEGER PRIMARY KEY, root_identity TEXT NOT NULL, complete INTEGER NOT NULL, payload BLOB NOT NULL
        );
        PRAGMA user_version = 3;
        """;

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
        """;
}

internal sealed record InventoryState(long Revision, string? RootIdentity, string? Policy,
    Dictionary<string, SourceFingerprint> Files, HashSet<string> UncertainScopes,
    HashSet<string> UnobservedScopes, PendingIntents Pending,
    RetiredIntents? Retirement);

internal sealed record SourceFingerprint(string Path, string Digest, bool Verified = true);

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
