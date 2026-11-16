using System.Globalization;
using System.Text.Json;
using Microsoft.Data.Sqlite;

namespace SlopHawk.Daemon;

internal sealed partial class SourceInventory
{
    internal ProjectGraph? RecordProjects(ScheduleFence fence, ProjectGraph graph, Func<bool> isCurrent)
    {
        var bytes = JsonSerializer.SerializeToUtf8Bytes(graph);
        if (bytes.Length > 2 * 1024 * 1024)
        {
            graph = new ProjectGraph([], [new ProjectEvaluationIssue(".", "graph_storage_budget")]);
            bytes = JsonSerializer.SerializeToUtf8Bytes(graph);
        }
        lock (writer)
        {
            using var connection = Open(initialize: false);
            using var transaction = connection.BeginTransaction();
            if (!MatchesFence(connection, transaction, fence) || !isCurrent())
                return null;
            Execute(connection, transaction, """
                INSERT INTO project_graph(revision, root_identity, complete, payload)
                VALUES ($revision, $root, $complete, $payload)
                ON CONFLICT(revision) DO UPDATE SET complete = excluded.complete, payload = excluded.payload
                """, ("$revision", fence.Revision), ("$root", fence.RootIdentity),
                ("$complete", graph.Complete), ("$payload", bytes));
            Execute(connection, transaction, """
                DELETE FROM project_graph WHERE revision NOT IN (
                    SELECT revision FROM project_graph
                    ORDER BY revision = $revision DESC,
                        EXISTS(SELECT 1 FROM test_intent WHERE predecessor = 'graph:' || project_graph.revision) DESC,
                        revision DESC
                    LIMIT 8
                )
                """, ("$revision", fence.Revision));
            transaction.Commit();
            return graph;
        }
    }

    private static EvaluatedSelectionInputs ReadProjects(SqliteConnection connection, SqliteTransaction transaction,
        ScheduleFence fence, List<TestCause> causes)
    {
        var graph = new ProjectGraph([], [new ProjectEvaluationIssue(".", "evaluation_unavailable")]);
        var predecessors = new Dictionary<string, ProjectGraph>(StringComparer.Ordinal);
        var keys = causes.Select(cause => cause.Predecessor).ToHashSet(StringComparer.Ordinal);
        using (var command = Command(connection, transaction, """
            SELECT revision, payload FROM project_graph WHERE root_identity = $root
            """, ("$root", fence.RootIdentity)))
        using (var rows = command.ExecuteReader())
        {
            while (rows.Read())
            {
                var revision = rows.GetInt64(0);
                var key = "graph:" + revision.ToString(CultureInfo.InvariantCulture);
                if (revision != fence.Revision && !keys.Contains(key))
                    continue;
                ProjectGraph value;
                try
                {
                    value = JsonSerializer.Deserialize<ProjectGraph>(rows.GetFieldValue<byte[]>(1))
                        ?? throw new IOException("An evaluated project graph is missing.");
                }
                catch (JsonException ex)
                {
                    throw new IOException("An evaluated project graph is invalid.", ex);
                }
                if (revision == fence.Revision)
                    graph = value;
                else
                    predecessors.Add(key, value);
            }
        }
        var digests = new Dictionary<string, string>(Workspace.PathComparer);
        using (var command = Command(connection, transaction, "SELECT path, digest FROM source WHERE verified = 1"))
        using (var rows = command.ExecuteReader())
        {
            while (rows.Read())
                digests.Add(rows.GetString(0), rows.GetString(1));
        }
        return new EvaluatedSelectionInputs(graph, predecessors, digests);
    }

    private const string ProjectSchema = """
        CREATE TABLE project_graph (
            revision INTEGER PRIMARY KEY, root_identity TEXT NOT NULL, complete INTEGER NOT NULL, payload BLOB NOT NULL
        );
        PRAGMA user_version = 3;
        """;
}
