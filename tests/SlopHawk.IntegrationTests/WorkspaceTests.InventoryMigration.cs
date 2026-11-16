using System.Text;
using Microsoft.Data.Sqlite;
using SlopHawk.Daemon;
using Xunit;

namespace SlopHawk.IntegrationTests;

public sealed partial class WorkspaceTests
{
    [Theory]
    [InlineData(1)]
    [InlineData(2)]
    [InlineData(3)]
    [InlineData(4)]
    public async Task HistoricalInventoryMigrationRetainsAllRepresentedDurableWork(int version)
    {
        await using var fixture = new Fixture();
        await fixture.WriteBaselineAsync();
        SeedHistoricalInventory(fixture.Workspace, version);

        var state = new SourceInventory(fixture.Workspace).Load();
        Assert.Equal(7L, state.Revision);
        Assert.Equal("old-fingerprint", state.Files["Source.cs"].Digest);
        Assert.True(state.Files["Source.cs"].Verified);
        Assert.Equal("unverified-fingerprint", state.Files["Uncertain.cs"].Digest);
        Assert.False(state.Files["Uncertain.cs"].Verified);
        Assert.Contains("partial", state.UncertainScopes);
        Assert.Contains("unobserved", state.UnobservedScopes);
        Assert.Equal(1, state.Pending.DetailedCount);
        Assert.Equal(new IntentFallback(3, 7), state.Pending.Fallback);
        Assert.Equal(6L, state.Pending.OldestRevision);
        Assert.Equal(7L, state.Pending.LatestRevision);

        // This is deliberately a new database handle, not the inventory's read transaction.
        using var verify = OpenMigrationFixture(fixture.Workspace, SqliteOpenMode.ReadWrite);
        Assert.Equal(4L, Number(verify, "PRAGMA user_version"));
        Assert.Equal("old-fingerprint", TextValue(verify, "SELECT old_digest FROM test_intent"));
        Assert.Equal("changed-fingerprint", TextValue(verify, "SELECT new_digest FROM test_intent"));
        Assert.Equal(version >= 3 ? "graph:5" : "graph_unavailable",
            TextValue(verify, "SELECT predecessor FROM test_intent"));
        Assert.Equal(6L, Number(verify, "SELECT revision FROM test_intent"));
        Assert.Equal(3L, Number(verify, "SELECT first_revision FROM intent_fallback"));
        Assert.Equal(7L, Number(verify, "SELECT last_revision FROM intent_fallback"));
        Assert.Equal(version >= 2 ? 1L : 0L, Number(verify, "SELECT count(*) FROM test_attempt"));
        Assert.Equal(version >= 2 ? 1L : 0L, Number(verify, "SELECT count(*) FROM test_evidence"));
        Assert.Equal(version >= 3 ? 1L : 0L, Number(verify, "SELECT count(*) FROM project_graph"));
        if (version >= 2)
        {
            Assert.Equal("interrupted", TextValue(verify, "SELECT state FROM test_attempt"));
            Assert.Equal("[\"cause-6\"]", TextValue(verify, "SELECT causes FROM test_attempt"));
            Assert.Equal(version == 4 ? "lease-11" : null,
                TextValue(verify, "SELECT lease_id FROM test_attempt"));
            Assert.Equal(version == 4 ? "profile-11" : null,
                TextValue(verify, "SELECT profile_revision FROM test_attempt"));
        }
        if (version >= 3)
            Assert.Equal("7B7D", TextValue(verify, "SELECT hex(payload) FROM project_graph"));

        // A second load is idempotent and cannot advance the inventory revision.
        Assert.Equal(7, new SourceInventory(fixture.Workspace).Load().Revision);
    }

    [Fact]
    public async Task HistoricalPendingWorkSurvivesDaemonRestartAfterMigration()
    {
        await using var fixture = new Fixture();
        await fixture.WriteBaselineAsync();
        SeedHistoricalInventory(fixture.Workspace, 1);

        fixture.Start();
        var first = await fixture.WaitForReportAsync(report => FileCount(report) == 1);
        Assert.True(CommittedRevision(first) > 7);
        Assert.Equal("old-fingerprint", Assert.Single(fixture.ReadIntents()).OldDigest);
        await fixture.StopAsync();

        fixture.Start();
        await fixture.WaitForReportAsync(report => FileCount(report) == 1);
        Assert.Equal("old-fingerprint", Assert.Single(fixture.ReadIntents()).OldDigest);
        Assert.Equal(3L, ReadNumber(fixture.Workspace, "SELECT first_revision FROM intent_fallback"));
        Assert.Equal(7L, ReadNumber(fixture.Workspace, "SELECT last_revision FROM intent_fallback"));
        await fixture.StopAsync();
    }

    [Fact]
    public async Task FailedLateMigrationStatementRollsBackEarlierAlterAndCanBeRetried()
    {
        await using var fixture = new Fixture();
        await fixture.WriteBaselineAsync();
        SeedHistoricalInventory(fixture.Workspace, 3);
        using (var connection = OpenMigrationFixture(fixture.Workspace, SqliteOpenMode.ReadWrite))
        {
            // A torn schema-4 attempt: the second ALTER must now fail, after the first succeeds.
            ExecuteMigrationSql(connection, "ALTER TABLE test_attempt ADD COLUMN profile_revision TEXT;");
        }
        var before = Snapshot(fixture.Workspace, includeVersion: true);
        var error = Assert.Throws<IOException>(() => new SourceInventory(fixture.Workspace).Load());
        Assert.Contains("migration", error.Message, StringComparison.OrdinalIgnoreCase);
        Assert.Contains("Pending work was preserved", error.Message);
        Assert.Equal(before, Snapshot(fixture.Workspace, includeVersion: true));
        using (var verify = OpenMigrationFixture(fixture.Workspace, SqliteOpenMode.ReadWrite))
        {
            Assert.Equal(3L, Number(verify, "PRAGMA user_version"));
            Assert.DoesNotContain("lease_id", ReadColumns(verify, "test_attempt"));
            Assert.Contains("profile_revision", ReadColumns(verify, "test_attempt"));
            // Repair the synthetic corruption without deleting a source, intent, or attempt.
            ExecuteMigrationSql(verify, "ALTER TABLE test_attempt DROP COLUMN profile_revision;");
        }

        Assert.Equal(7, new SourceInventory(fixture.Workspace).Load().Revision);
        await using var clean = new Fixture();
        await clean.WriteBaselineAsync();
        SeedHistoricalInventory(clean.Workspace, 3);
        new SourceInventory(clean.Workspace).Load();
        Assert.Equal(Snapshot(clean.Workspace, includeVersion: true),
            Snapshot(fixture.Workspace, includeVersion: true));
    }

    [Fact]
    public async Task MalformedLayoutBlocksWithoutLosingPendingCausesAndCanBeRepaired()
    {
        await using var fixture = new Fixture();
        await fixture.WriteBaselineAsync();
        SeedHistoricalInventory(fixture.Workspace, 2);
        using (var connection = OpenMigrationFixture(fixture.Workspace, SqliteOpenMode.ReadWrite))
            ExecuteMigrationSql(connection, "ALTER TABLE test_intent RENAME COLUMN predecessor TO invalid_predecessor;");

        var before = Snapshot(fixture.Workspace, includeVersion: true, malformed: true);
        var error = Assert.Throws<IOException>(() => new SourceInventory(fixture.Workspace).Load());
        Assert.Contains("test_intent", error.Message);
        Assert.Contains("Pending work was preserved", error.Message);
        Assert.Equal(before, Snapshot(fixture.Workspace, includeVersion: true, malformed: true));
        Assert.Equal(2L, ReadNumber(fixture.Workspace, "PRAGMA user_version"));

        using (var connection = OpenMigrationFixture(fixture.Workspace, SqliteOpenMode.ReadWrite))
            ExecuteMigrationSql(connection,
                "ALTER TABLE test_intent RENAME COLUMN invalid_predecessor TO predecessor;");
        Assert.Equal(7, new SourceInventory(fixture.Workspace).Load().Revision);
        Assert.Equal(1L, ReadNumber(fixture.Workspace, "SELECT count(*) FROM test_intent"));
    }

    [Fact]
    public async Task UnsupportedSchemaVersionBlocksWithoutModifyingAuthoritativeRows()
    {
        await using var fixture = new Fixture();
        await fixture.WriteBaselineAsync();
        SeedHistoricalInventory(fixture.Workspace, 4);
        using (var connection = OpenMigrationFixture(fixture.Workspace, SqliteOpenMode.ReadWrite))
            ExecuteMigrationSql(connection, "PRAGMA user_version = 5;");

        var before = Snapshot(fixture.Workspace, includeVersion: true);
        var error = Assert.Throws<IOException>(() => new SourceInventory(fixture.Workspace).Load());
        Assert.Contains("5", error.Message);
        Assert.Contains("Pending work was preserved", error.Message);
        Assert.Equal(before, Snapshot(fixture.Workspace, includeVersion: true));
    }

    [Fact]
    public async Task IncompatiblePageSizeCannotAdvanceTheHistoricalSchema()
    {
        await using var fixture = new Fixture();
        await fixture.WriteBaselineAsync();
        SeedHistoricalInventory(fixture.Workspace, 3, pageSize: 8192);
        var before = Snapshot(fixture.Workspace, includeVersion: true);

        var error = Assert.Throws<IOException>(() => new SourceInventory(fixture.Workspace).Load());

        Assert.Contains("unsupported page size", error.Message);
        Assert.Equal(before, Snapshot(fixture.Workspace, includeVersion: true));
        using var verify = OpenMigrationFixture(fixture.Workspace, SqliteOpenMode.ReadWrite);
        Assert.Equal(3L, Number(verify, "PRAGMA user_version"));
        Assert.DoesNotContain("lease_id", ReadColumns(verify, "test_attempt"));
    }

    private static void SeedHistoricalInventory(Workspace workspace, int version, int pageSize = 4096)
    {
        Workspace.CreatePrivateDirectory(workspace.StateDirectory);
        using var connection = OpenMigrationFixture(workspace, SqliteOpenMode.ReadWriteCreate);
        ExecuteMigrationSql(connection, $"PRAGMA page_size = {pageSize};");
        ExecuteMigrationSql(connection, """
            CREATE TABLE workspace (
                id INTEGER PRIMARY KEY CHECK (id = 1), revision INTEGER NOT NULL,
                root_identity TEXT, policy TEXT, branch TEXT, commit_id TEXT,
                retired_revision INTEGER, retired_intents INTEGER
            );
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
            """);
        using (var workspaceRow = connection.CreateCommand())
        {
            workspaceRow.CommandText = """
                INSERT INTO workspace (id, revision, root_identity, policy, branch, commit_id)
                VALUES (1, 7, $root, 'csharp.syntax:inventory-v1', 'feature/fixture', 'commit-seven');
                """;
            workspaceRow.Parameters.AddWithValue("$root", RootIdentity.Read(workspace.Root));
            workspaceRow.ExecuteNonQuery();
        }
        var sourceKey = OperatingSystem.IsWindows() ? "SOURCE.CS" : "Source.cs";
        var uncertainKey = OperatingSystem.IsWindows() ? "UNCERTAIN.CS" : "Uncertain.cs";
        ExecuteMigrationSql(connection, $"""
            INSERT INTO source VALUES ('{sourceKey}', 'Source.cs', 'old-fingerprint', 1);
            INSERT INTO source VALUES ('{uncertainKey}', 'Uncertain.cs', 'unverified-fingerprint', 0);
            INSERT INTO scope VALUES (0, 'partial');
            INSERT INTO scope VALUES (1, 'unobserved');
            INSERT INTO test_intent VALUES (6, '{sourceKey}', 'Source.cs', 'old-fingerprint',
                'Source.cs', 'changed-fingerprint', 'graph_unavailable');
            INSERT INTO intent_fallback VALUES (1, 3, 7);
            """);
        if (version >= 2)
        {
            ExecuteMigrationSql(connection, """
                CREATE TABLE test_attempt (
                    id INTEGER PRIMARY KEY AUTOINCREMENT, number INTEGER NOT NULL, revision INTEGER NOT NULL,
                    epoch TEXT NOT NULL, target TEXT NOT NULL, fingerprint TEXT NOT NULL, causes TEXT NOT NULL,
                    plan_key TEXT NOT NULL, state TEXT NOT NULL, outcome TEXT, retry_at INTEGER
                );
                CREATE INDEX test_attempt_plan ON test_attempt (plan_key, id);
                CREATE TABLE test_evidence (
                    cause TEXT NOT NULL, target TEXT NOT NULL, fingerprint TEXT NOT NULL, PRIMARY KEY (cause, target)
                );
                INSERT INTO test_attempt (id, number, revision, epoch, target, fingerprint, causes, plan_key, state)
                VALUES (11, 2, 6, '11111111-1111-1111-1111-111111111111',
                    'unit', 'fingerprint-11', '["cause-6"]', 'plan-11', 'interrupted');
                INSERT INTO test_evidence VALUES ('cause-6', 'unit', 'fingerprint-11');
                """);
        }
        if (version >= 3)
        {
            ExecuteMigrationSql(connection, """
                CREATE TABLE project_graph (
                    revision INTEGER PRIMARY KEY, root_identity TEXT NOT NULL, complete INTEGER NOT NULL, payload BLOB NOT NULL
                );
                UPDATE test_intent SET predecessor = 'graph:5';
                """);
            using var insert = connection.CreateCommand();
            insert.CommandText = "INSERT INTO project_graph VALUES (5, $root, 1, x'7B7D');";
            insert.Parameters.AddWithValue("$root", RootIdentity.Read(workspace.Root));
            insert.ExecuteNonQuery();
        }
        if (version >= 4)
            ExecuteMigrationSql(connection, """
                ALTER TABLE test_attempt ADD COLUMN lease_id TEXT;
                ALTER TABLE test_attempt ADD COLUMN profile_revision TEXT;
                UPDATE test_attempt SET lease_id = 'lease-11', profile_revision = 'profile-11';
                """);
        ExecuteMigrationSql(connection, $"PRAGMA user_version = {version};");
    }

    private static SqliteConnection OpenMigrationFixture(Workspace workspace, SqliteOpenMode mode)
    {
        var connection = new SqliteConnection(new SqliteConnectionStringBuilder
        {
            DataSource = workspace.InventoryPath,
            Mode = mode,
            Pooling = false,
        }.ToString());
        connection.Open();
        return connection;
    }

    private static void ExecuteMigrationSql(SqliteConnection connection, string sql)
    {
        using var command = connection.CreateCommand();
        command.CommandText = sql;
        command.ExecuteNonQuery();
    }

    private static long Number(SqliteConnection connection, string sql) =>
        (long)Value(connection, sql)!;

    private static long ReadNumber(Workspace workspace, string sql)
    {
        using var connection = OpenMigrationFixture(workspace, SqliteOpenMode.ReadWrite);
        return Number(connection, sql);
    }

    private static string? TextValue(SqliteConnection connection, string sql) =>
        Value(connection, sql) as string;

    private static object? Value(SqliteConnection connection, string sql)
    {
        using var command = connection.CreateCommand();
        command.CommandText = sql;
        return command.ExecuteScalar();
    }

    private static string[] ReadColumns(SqliteConnection connection, string table)
    {
        using var command = connection.CreateCommand();
        command.CommandText = "SELECT name FROM pragma_table_info($table) ORDER BY cid";
        command.Parameters.AddWithValue("$table", table);
        using var rows = command.ExecuteReader();
        var columns = new List<string>();
        while (rows.Read())
            columns.Add(rows.GetString(0));
        return columns.ToArray();
    }

    private static string Snapshot(Workspace workspace, bool includeVersion, bool malformed = false)
    {
        using var connection = OpenMigrationFixture(workspace, SqliteOpenMode.ReadWrite);
        var builder = new StringBuilder();
        var rootIdentity = RootIdentity.Read(workspace.Root);
        if (includeVersion)
            builder.Append("version=").Append(Number(connection, "PRAGMA user_version")).Append('\n');
        var statements = new[]
        {
            "SELECT * FROM workspace ORDER BY id",
            "SELECT * FROM source ORDER BY path_key",
            "SELECT * FROM scope ORDER BY kind, path",
            "SELECT * FROM test_intent ORDER BY revision, path_key",
            "SELECT * FROM intent_fallback ORDER BY id",
            "SELECT * FROM test_attempt ORDER BY id",
            "SELECT * FROM test_evidence ORDER BY cause, target",
            "SELECT * FROM project_graph ORDER BY revision"
        };
        foreach (var sql in statements)
        {
            var table = sql.Split(' ')[3];
            using var exists = connection.CreateCommand();
            exists.CommandText = "SELECT count(*) FROM sqlite_schema WHERE type = 'table' AND name = $name";
            exists.Parameters.AddWithValue("$name", table);
            if ((long)exists.ExecuteScalar()! == 0)
                continue;
            builder.Append(table).Append(':');
            using var command = connection.CreateCommand();
            command.CommandText = sql;
            using var rows = command.ExecuteReader();
            while (rows.Read())
            {
                for (var i = 0; i < rows.FieldCount; i++)
                {
                    var cell = rows.GetValue(i);
                    var value = cell switch
                    {
                        DBNull => "<null>",
                        byte[] bytes => Convert.ToHexString(bytes),
                        _ => Convert.ToString(cell, System.Globalization.CultureInfo.InvariantCulture) ?? ""
                    };
                    if (value == rootIdentity)
                        value = "<root>";
                    builder.Append(value.Length).Append(':').Append(value).Append(';');
                }
                builder.Append('|');
            }
            builder.Append('\n');
        }
        if (malformed)
            builder.Append("columns=").Append(string.Join(',', ReadColumns(connection, "test_intent"))).Append('\n');
        return builder.ToString();
    }
}
