using System.Globalization;
using System.Text.Json;
using Microsoft.Data.Sqlite;

namespace SlopHawk.Daemon;

// Semantic v5 interchange. No SQLite files, locks, WALs or derived cache pages are copied.
// Column order and SQLite storage classes are part of the versioned format.
internal sealed record MigrationValue(string Type, string? Value);
internal sealed record MigrationTable(string Name, string[] Columns, MigrationValue[][] Rows);
internal sealed record MigrationSnapshot(int Format, int InventorySchema, string RootIdentity,
    long Revision, string? Policy, MigrationTable[] Tables);

internal sealed partial class SourceInventory
{
    internal const int MigrationFormat = 1;
    internal const int MaximumMigrationBytes = 16 * 1024 * 1024;

    private sealed record TableShape(string Name, string Columns, int MaximumRows)
    {
        internal string[] Names => Columns.Split(',');
    }

    // Exact column lists from the pinned schema (v5); additions need a new interchange version.
    private static readonly TableShape[] MigrationTables =
    [
        new("workspace", "id,revision,root_identity,policy,branch,commit_id,retired_revision,retired_intents", 1),
        new("source", "path_key,path,digest,verified", 100_000),
        new("scope", "kind,path", 100_000),
        new("test_intent", "revision,path_key,old_path,old_digest,new_path,new_digest,predecessor", MaxDetailedIntents),
        new("intent_fallback", "id,first_revision,last_revision", 1),
        new("test_attempt", "id,number,revision,epoch,target,fingerprint,causes,plan_key,state,outcome,retry_at,lease_id,profile_revision", 64),
        new("test_evidence", "cause,target,fingerprint", 32_784),
        new("project_graph", "revision,root_identity,complete,payload", 128),
        new("test_run_receipt", "attempt_id,format,revision,epoch,fence_sequence,git_fingerprint,root_identity,target,fingerprint,profile_revision,lease_id,build_domain,build_sha256,runner_sha256,artifact_sha256,artifact_bytes,run_id,platform,runner,artifact,execution,total,executed,passed,failed,skipped,other,reason_code", 64)
    ];

    internal MigrationSnapshot ExportMigration(string rootIdentity)
    {
        lock (writer)
        {
            EnsureMigrationRoot(rootIdentity);
            using var connection = Open(initialize: false); // rejects unknown schema before mutation
            using var transaction = connection.BeginTransaction(deferred: true);
            ValidateLayout(connection, SchemaVersion, transaction);
            var tables = new List<MigrationTable>(MigrationTables.Length);
            foreach (var shape in MigrationTables)
            {
                var rows = new List<MigrationValue[]>();
                using var select = Command(connection, transaction,
                    $"SELECT {shape.Columns} FROM {shape.Name} ORDER BY " + MigrationOrder(shape.Name));
                using var reader = select.ExecuteReader();
                while (reader.Read())
                {
                    if (rows.Count >= shape.MaximumRows)
                        throw new IOException($"The {shape.Name} migration scope exceeds its bounded format.");
                    var row = new MigrationValue[reader.FieldCount];
                    for (var index = 0; index < row.Length; index++)
                        row[index] = EncodeValue(reader.GetValue(index));
                    if (shape.Name == "test_attempt" && row[8].Value == "running")
                        row[8] = new MigrationValue("text", "interrupted");
                    rows.Add(row);
                }
                tables.Add(new MigrationTable(shape.Name, shape.Names, rows.ToArray()));
            }
            var workspaceRow = tables[0].Rows.SingleOrDefault()
                ?? throw new IOException("The inventory is missing its committed workspace record.");
            var revision = DecodeInteger(workspaceRow[1]);
            if (workspaceRow[2].Value != rootIdentity || revision < 0)
                throw new IOException("The durable inventory is not bound to this physical checkout.");
            var snapshot = new MigrationSnapshot(MigrationFormat, SchemaVersion, rootIdentity, revision,
                workspaceRow[3].Value, tables.ToArray());
            CheckMigrationBudget(snapshot);
            EnsureMigrationRoot(rootIdentity);
            transaction.Commit();
            return snapshot;
        }
    }

    private static string MigrationOrder(string table) => table switch
    {
        "workspace" => "id", "source" => "path_key", "scope" => "kind,path",
        "test_intent" => "revision,path_key", "intent_fallback" => "id",
        "test_attempt" => "id", "test_evidence" => "cause,target",
        "project_graph" => "revision", "test_run_receipt" => "attempt_id",
        _ => throw new IOException("Unknown durable-state table.")
    };

    private static MigrationValue EncodeValue(object value) => value switch
    {
        DBNull => new("null", null),
        byte[] bytes => new("blob", Convert.ToBase64String(bytes)),
        long number => new("integer", number.ToString(CultureInfo.InvariantCulture)),
        int number => new("integer", number.ToString(CultureInfo.InvariantCulture)),
        string text => new("text", text),
        _ => throw new IOException("The durable-state interchange contains an unsupported SQLite value.")
    };

    private static long DecodeInteger(MigrationValue value) => value.Type == "integer" &&
        long.TryParse(value.Value, NumberStyles.Integer, CultureInfo.InvariantCulture, out var result)
        ? result : throw new IOException("The durable-state interchange contains an invalid integer.");

    private static void CheckMigrationBudget(MigrationSnapshot snapshot)
    {
        if (JsonSerializer.SerializeToUtf8Bytes(snapshot).Length > MaximumMigrationBytes)
            throw new IOException("The migration snapshot exceeds the 16 MiB bounded format.");
    }

    private void EnsureMigrationRoot(string expected)
    {
        if (string.IsNullOrWhiteSpace(expected) || RootIdentity.Read(workspace.Root) != expected)
            throw new IOException("The checkout identity changed during durable-state transition.");
    }
}
