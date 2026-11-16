using System.Text.Json;
using Microsoft.Data.Sqlite;

namespace SlopHawk.Daemon;

internal sealed partial class SourceInventory
{
    private const int MaxAttemptsPerPlan = 3;

    internal TestWork? ReadTestWork(ScheduleFence fence, bool recover, Func<bool> isCurrent, bool includeProjects = false)
    {
        lock (writer)
        {
            using var connection = Open(initialize: false);
            using var transaction = connection.BeginTransaction(deferred: !recover);
            if (!MatchesFence(connection, transaction, fence) || !isCurrent())
                return null;

            if (recover)
                Execute(connection, transaction, """
                    UPDATE test_attempt SET state = 'interrupted' WHERE state = 'running'
                    """);
            var pending = ReadPending(connection, transaction);
            var causes = new List<TestCause>();
            using (var command = Command(connection, transaction, """
                SELECT revision, path_key, old_path, old_digest, new_path, new_digest, predecessor
                FROM test_intent ORDER BY revision, path_key
                """))
            using (var rows = command.ExecuteReader())
            {
                while (rows.Read())
                    causes.Add(new TestCause(rows.GetInt64(0), rows.GetString(1), Text(rows, 2), Text(rows, 3),
                        Text(rows, 4), Text(rows, 5), rows.GetString(6)));
            }
            if (pending.Fallback is { } fallback)
                causes.Add(new TestCause(fallback.LastRevision, "", null, null, null, null,
                    "graph_unavailable", fallback.FirstRevision));
            var evidence = new HashSet<TestEvidence>();
            using (var command = Command(connection, transaction, "SELECT cause, target, fingerprint FROM test_evidence"))
            using (var rows = command.ExecuteReader())
            {
                while (rows.Read())
                    evidence.Add(new TestEvidence(rows.GetString(0), rows.GetString(1), rows.GetString(2)));
            }
            var latest = ReadLatest(connection, transaction);
            var projects = includeProjects ? ReadProjects(connection, transaction, fence, causes) : null;
            transaction.Commit();
            return new TestWork(new TestSelectionInput(fence, causes.AsReadOnly(), projects), evidence, pending, latest);
        }
    }

    internal TestAdmission Admit(ScheduleFence fence, SelectedTarget target, WorkerLease worker, DateTimeOffset now,
        Func<bool> isCurrent)
    {
        var scope = JsonSerializer.Serialize(target.CauseIds);
        var plan = TestCause.Hash(JsonSerializer.Serialize(new[] { target.Id, target.InputFingerprint, scope }));
        lock (writer)
        {
            using var connection = Open(initialize: false);
            using var transaction = connection.BeginTransaction();
            if (!MatchesFence(connection, transaction, fence) || !isCurrent())
                return new TestAdmission(null, null);

            var number = 1;
            using (var previous = Command(connection, transaction, """
                SELECT state, number, retry_at FROM test_attempt
                WHERE plan_key = $plan ORDER BY id DESC LIMIT 1
                """, ("$plan", plan)))
            using (var row = previous.ExecuteReader())
            {
                if (row.Read())
                {
                    var state = row.GetString(0);
                    var priorNumber = row.GetInt32(1);
                    if (state == "superseded")
                        number = priorNumber;
                    else
                    {
                        if (state is not ("retryable_failure" or "interrupted") || priorNumber >= MaxAttemptsPerPlan)
                            return new TestAdmission(null, null);
                        if (ReadRetryAt(row, 2) is { } retryAt && retryAt > now)
                            return new TestAdmission(null, retryAt);
                        number = priorNumber + 1;
                    }
                }
            }
            using var insert = Command(connection, transaction, """
                INSERT INTO test_attempt(number, revision, epoch, target, fingerprint, causes, plan_key, state,
                    lease_id, profile_revision)
                VALUES ($number, $revision, $epoch, $target, $fingerprint, $scope, $plan, 'running', $lease, $profile)
                RETURNING id
                """, ("$number", number), ("$revision", fence.Revision), ("$epoch", fence.Epoch.ToString()),
                ("$target", target.Id), ("$fingerprint", target.InputFingerprint), ("$scope", scope), ("$plan", plan),
                ("$lease", worker.Id.ToString("N")), ("$profile", worker.ProfileRevision));
            var id = (long)insert.ExecuteScalar()!;
            Execute(connection, transaction, """
                DELETE FROM test_attempt WHERE id <= (SELECT max(id) - 64 FROM test_attempt) AND state <> 'running'
                """);
            Execute(connection, transaction, """
                DELETE FROM test_run_receipt WHERE attempt_id NOT IN (SELECT id FROM test_attempt)
                """);
            transaction.Commit();
            return new TestAdmission(new TestAttempt(id, number, fence, target, worker.ProfileRevision), null);
        }
    }

    internal (PendingIntents Pending, TestAttemptReceipt Receipt) Finish(TestAttempt attempt, TestOutcome outcome,
        TestWork work, ValidatedSelection selection, DateTimeOffset now, Func<bool> isCurrent,
        TestRunReceiptDetails? details = null)
    {
        lock (writer)
        {
            using var connection = Open(initialize: false);
            using var transaction = connection.BeginTransaction();
            var current = MatchesFence(connection, transaction, attempt.Fence) && isCurrent();
            var result = outcome switch
            {
                TestOutcome.Passed => "passed",
                TestOutcome.Failed => "failed",
                TestOutcome.RetryableFailure => "retryable_failure",
                TestOutcome.Cancelled => "cancelled",
                _ => "blocked"
            };
            var state = current ? result : "superseded";
            DateTimeOffset? retryAt = current && outcome == TestOutcome.RetryableFailure &&
                attempt.Number < MaxAttemptsPerPlan ? now.AddMilliseconds(250 << (attempt.Number - 1)) : null;
            if (details is not null)
                PersistReceipt(connection, transaction, attempt, outcome, details);
            using (var update = Command(connection, transaction, """
                UPDATE test_attempt SET state = $state, outcome = $outcome, retry_at = $retry
                WHERE id = $id AND state = 'running'
                """, ("$state", state), ("$outcome", result), ("$retry", retryAt?.ToUnixTimeMilliseconds()), ("$id", attempt.Id)))
            {
                if (update.ExecuteNonQuery() != 1)
                    throw new IOException("The test attempt is no longer running.");
            }

            if (current && outcome is TestOutcome.Passed or TestOutcome.Failed)
            {
                var evidence = new HashSet<TestEvidence>(work.Evidence);
                foreach (var id in attempt.Target.CauseIds)
                {
                    Execute(connection, transaction, """
                        INSERT INTO test_evidence(cause, target, fingerprint) VALUES ($cause, $target, $fingerprint)
                        ON CONFLICT(cause, target) DO UPDATE SET fingerprint = excluded.fingerprint
                        """, ("$cause", id), ("$target", attempt.Target.Id), ("$fingerprint", attempt.Target.InputFingerprint));
                    evidence.Add(new TestEvidence(id, attempt.Target.Id, attempt.Target.InputFingerprint));
                }
                Resolve(connection, transaction, work, selection, evidence);
                Execute(connection, transaction, """
                    DELETE FROM test_evidence WHERE rowid IN
                    (SELECT rowid FROM test_evidence ORDER BY rowid DESC LIMIT -1 OFFSET 32784)
                    """);
            }
            var pending = ReadPending(connection, transaction);
            transaction.Commit();
            return (pending, new TestAttemptReceipt(attempt.Id, attempt.Number, attempt.Fence.Revision,
                attempt.Fence.Epoch, attempt.Target.Id, state, result, retryAt));
        }
    }

    internal PendingIntents? ResolveExisting(TestWork work, ValidatedSelection selection, Func<bool> isCurrent)
    {
        lock (writer)
        {
            using var connection = Open(initialize: false);
            using var transaction = connection.BeginTransaction();
            if (!MatchesFence(connection, transaction, work.Input.Fence) || !isCurrent())
                return null;
            Resolve(connection, transaction, work, selection, work.Evidence);
            var pending = ReadPending(connection, transaction);
            transaction.Commit();
            return pending;
        }
    }

    private static void Resolve(SqliteConnection connection, SqliteTransaction transaction, TestWork work,
        ValidatedSelection selection, HashSet<TestEvidence> evidence)
    {
        if (!selection.Complete)
            return;
        foreach (var cause in work.Input.Causes)
        {
            if (!selection.Satisfied(cause.Id, evidence))
                continue;
            if (cause.FallbackFirstRevision is not null)
                Execute(connection, transaction, "DELETE FROM intent_fallback WHERE last_revision = $revision",
                    ("$revision", cause.Revision));
            else
                Execute(connection, transaction, "DELETE FROM test_intent WHERE revision = $revision AND path_key = $key",
                    ("$revision", cause.Revision), ("$key", cause.PathKey));
            Execute(connection, transaction, "DELETE FROM test_evidence WHERE cause = $cause", ("$cause", cause.Id));
        }
    }

    private static bool MatchesFence(SqliteConnection connection, SqliteTransaction transaction, ScheduleFence fence)
    {
        using var command = Command(connection, transaction, """
            SELECT 1 FROM workspace WHERE id = 1 AND revision = $revision AND root_identity = $root
            """, ("$revision", fence.Revision), ("$root", fence.RootIdentity));
        return command.ExecuteScalar() is not null;
    }

    private static TestAttemptReceipt? ReadLatest(SqliteConnection connection, SqliteTransaction transaction)
    {
        using var command = Command(connection, transaction, """
            SELECT id, number, revision, epoch, target, state, outcome, retry_at FROM test_attempt
            ORDER BY id DESC LIMIT 1
            """);
        using var row = command.ExecuteReader();
        return row.Read() ? new TestAttemptReceipt(row.GetInt64(0), row.GetInt32(1), row.GetInt64(2),
            Guid.Parse(row.GetString(3)), row.GetString(4), row.GetString(5), Text(row, 6), ReadRetryAt(row, 7)) : null;
    }

    private static string? Text(SqliteDataReader row, int column) => row.IsDBNull(column) ? null : row.GetString(column);

    private static DateTimeOffset? ReadRetryAt(SqliteDataReader row, int column) =>
        row.IsDBNull(column) ? null : DateTimeOffset.FromUnixTimeMilliseconds(row.GetInt64(column));

    private const string SchedulerSchema = """
        CREATE TABLE test_attempt (
            id INTEGER PRIMARY KEY AUTOINCREMENT, number INTEGER NOT NULL, revision INTEGER NOT NULL,
            epoch TEXT NOT NULL, target TEXT NOT NULL, fingerprint TEXT NOT NULL, causes TEXT NOT NULL,
            plan_key TEXT NOT NULL, state TEXT NOT NULL, outcome TEXT, retry_at INTEGER
        );
        CREATE INDEX test_attempt_plan ON test_attempt (plan_key, id);
        CREATE TABLE test_evidence (
            cause TEXT NOT NULL, target TEXT NOT NULL, fingerprint TEXT NOT NULL, PRIMARY KEY (cause, target)
        );
        PRAGMA user_version = 2;
        """;

    private const string WorkerSchema = """
        ALTER TABLE test_attempt ADD COLUMN lease_id TEXT;
        ALTER TABLE test_attempt ADD COLUMN profile_revision TEXT;
        PRAGMA user_version = 4;
        """;
}
