using System.Text;
using System.Security.Cryptography;
using Microsoft.Data.Sqlite;

namespace SlopHawk.Daemon;

// This is a projection of caller-observed facts, not a TRX file or a test result
// invented by the store. The later production adapter supplies it at Finish.
// A null detail is the intentional legacy/fake-adapter representation.
internal sealed record TestRunReceiptDetails(
    long AttemptId, long Revision, Guid Epoch, long FenceSequence, string GitFingerprint,
    string RootIdentity, string Target, string InputFingerprint, string ProfileRevision, string? LeaseId,
    int? BuildDomainVersion, string? BuildSha256, string? RunnerSha256,
    string? ArtifactSha256, long? ArtifactBytes, Guid? RunId,
    VstestDisposition Platform, VstestRunnerHealth Runner,
    VstestArtifactHealth Artifact, VstestExecution Execution,
    VstestTrxCounters? Counters, string ReasonCode)
{
    internal const int FormatVersion = 1;
    internal const int MaxReasonLength = 64;
    internal const int MaxRecordBytes = 2_048;

    // Bind a branch/commit tuple without retaining a branch name. Prefix lengths
    // prevent collisions between different two-field concatenations.
    internal static string GitIdentity(GitRevision git)
    {
        var branch = git.Branch ?? "";
        var commit = git.Commit ?? "";
        return Convert.ToHexString(SHA256.HashData(Encoding.UTF8.GetBytes(
            $"{branch.Length}:{branch}:{commit.Length}:{commit}")));
    }

    // No untrusted free-form messages, filesystem locations, test names, XML,
    // binaries, or environmental values are accepted in a durable receipt.
    private static bool IsCode(string text) => text.Length is > 0 and <= MaxReasonLength &&
        text.All(c => c is >= 'a' and <= 'z' or >= 'A' and <= 'Z' or >= '0' and <= '9' or '_' or '-' or '.');

    private static bool IsDigest(string? value) => value is { Length: 64 } &&
        value.All(c => c is >= '0' and <= '9' or >= 'A' and <= 'F');

    private static bool OptionalDigest(string? value) => value is null || IsDigest(value);

    internal void Validate(TestAttempt attempt, TestOutcome outcome, string? storedLeaseId)
    {
        if (AttemptId != attempt.Id || Revision != attempt.Fence.Revision || Epoch != attempt.Fence.Epoch ||
            FenceSequence != attempt.Fence.Sequence || GitFingerprint != GitIdentity(attempt.Fence.Git) ||
            RootIdentity != attempt.Fence.RootIdentity || Target != attempt.Target.Id ||
            InputFingerprint != attempt.Target.InputFingerprint || ProfileRevision != attempt.ProfileRevision ||
            LeaseId != storedLeaseId)
            throw new IOException("Receipt provenance disagrees with the admitted attempt.");
        ValidatePayload(outcome);
    }

    internal void ValidatePayload(TestOutcome? schedulerOutcome = null)
    {
        if (AttemptId <= 0 || Revision < 0 || Epoch == Guid.Empty || FenceSequence < 0 ||
            !IsDigest(GitFingerprint) || RootIdentity.Length is < 1 or > 160 || Target.Length is < 1 or > 128 ||
            InputFingerprint.Length is < 1 or > 128 || ProfileRevision.Length is < 1 or > 128 ||
            (LeaseId is { Length: > 64 }) || !OptionalDigest(BuildSha256) ||
            !OptionalDigest(RunnerSha256) || !OptionalDigest(ArtifactSha256) ||
            (BuildDomainVersion is not null && BuildDomainVersion != BuildOutputIdentity.DomainVersion) ||
            (BuildDomainVersion is null) != (BuildSha256 is null) ||
            (ArtifactBytes is < 0 or > VstestTrxClassifier.MaxArtifactBytes) ||
            (ArtifactSha256 is null) != (ArtifactBytes is null) ||
            RunId == Guid.Empty || !IsCode(ReasonCode) ||
            !Enum.IsDefined(Platform) || !Enum.IsDefined(Runner) ||
            !Enum.IsDefined(Artifact) || !Enum.IsDefined(Execution))
            throw new IOException("The test run receipt has invalid or unbounded metadata.");

        if (Counters is { } counts)
        {
            if (counts.Total is < 0 or > VstestTrxClassifier.MaxTests ||
                counts.Executed < 0 || counts.Passed < 0 || counts.Failed < 0 ||
                counts.Skipped < 0 || counts.Other < 0 ||
                counts.Executed > counts.Total || counts.Passed > counts.Executed ||
                counts.Failed > counts.Executed || counts.Passed + (long)counts.Failed > counts.Executed ||
                counts.Skipped + (long)counts.Other > counts.Total)
                throw new IOException("The test run receipt counters are inconsistent.");
        }

        if (Artifact == VstestArtifactHealth.Complete &&
            (ArtifactSha256 is null || ArtifactBytes is not > 0 || RunId is null || Counters is null))
            throw new IOException("A complete TRX receipt requires a digest, run id and counters.");
        if (Artifact != VstestArtifactHealth.Complete && Counters is not null)
            throw new IOException("An incomplete TRX cannot assert completed counters.");

        if (Platform switch
        {
            VstestDisposition.Passed => Execution != VstestExecution.Passed ||
                Runner != VstestRunnerHealth.ZeroExit || Artifact != VstestArtifactHealth.Complete ||
                Counters is not { Executed: > 0, Failed: 0 } || Counters.Passed != Counters.Executed,
            VstestDisposition.Failed => Execution != VstestExecution.Failed ||
                Runner is not (VstestRunnerHealth.ZeroExit or VstestRunnerHealth.NonzeroExit) ||
                Artifact != VstestArtifactHealth.Complete || Counters is not { Failed: > 0 },
            VstestDisposition.ZeroDiscovered => Execution != VstestExecution.ZeroDiscovered ||
                Runner is not (VstestRunnerHealth.ZeroExit or VstestRunnerHealth.NonzeroExit) ||
                Artifact != VstestArtifactHealth.Complete || Counters is not { Total: 0 },
            VstestDisposition.AllSkipped => Execution != VstestExecution.AllSkipped ||
                Runner is not (VstestRunnerHealth.ZeroExit or VstestRunnerHealth.NonzeroExit) ||
                Artifact != VstestArtifactHealth.Complete || Counters is not { Total: > 0, Executed: 0 },
            VstestDisposition.Cancelled => Runner != VstestRunnerHealth.Cancelled,
            VstestDisposition.Timeout => Runner != VstestRunnerHealth.TimedOut,
            VstestDisposition.ArtifactFailure => Artifact == VstestArtifactHealth.Complete,
            VstestDisposition.RunnerFailure => Execution != VstestExecution.Unknown ||
                Runner is not (VstestRunnerHealth.ZeroExit or VstestRunnerHealth.NonzeroExit) ||
                Artifact != VstestArtifactHealth.Complete,
            VstestDisposition.StartupOrBuildFailure => Runner != VstestRunnerHealth.StartupOrBuildFailure,
            VstestDisposition.Incomplete => Runner != VstestRunnerHealth.Incomplete || Execution != VstestExecution.Unknown,
            _ => true
        })
            throw new IOException("The test platform receipt contradicts its observed facts.");

        if (schedulerOutcome is { } outcome && (outcome switch
        {
            TestOutcome.Passed => Platform != VstestDisposition.Passed,
            TestOutcome.Failed => Platform != VstestDisposition.Failed,
            TestOutcome.Cancelled => Platform != VstestDisposition.Cancelled,
            _ => Platform is VstestDisposition.Passed or VstestDisposition.Failed or VstestDisposition.Cancelled
        }))
            throw new IOException("The platform receipt contradicts the scheduler outcome.");

        // A conservative upper bound before entering the SQLite transaction.
        var bytes = Encoding.UTF8.GetByteCount(RootIdentity) + Encoding.UTF8.GetByteCount(Target) +
            Encoding.UTF8.GetByteCount(InputFingerprint) + Encoding.UTF8.GetByteCount(ProfileRevision) +
            Encoding.UTF8.GetByteCount(LeaseId ?? "") + Encoding.UTF8.GetByteCount(ReasonCode) +
            (BuildSha256?.Length ?? 0) + (RunnerSha256?.Length ?? 0) +
            (ArtifactSha256?.Length ?? 0) + GitFingerprint.Length + 512;
        if (bytes > MaxRecordBytes)
            throw new IOException("The test run receipt exceeds its storage budget.");
    }
}

internal sealed partial class SourceInventory
{
    // A single row per retained attempt: the existing 64-attempt history is the
    // receipt retention envelope. No secondary unbounded evidence history.
    private const string ReceiptSchema = """
        CREATE TABLE test_run_receipt (
            attempt_id INTEGER PRIMARY KEY, format INTEGER NOT NULL CHECK (format = 1),
            revision INTEGER NOT NULL, epoch TEXT NOT NULL, fence_sequence INTEGER NOT NULL,
            git_fingerprint TEXT NOT NULL, root_identity TEXT NOT NULL,
            target TEXT NOT NULL, fingerprint TEXT NOT NULL, profile_revision TEXT NOT NULL, lease_id TEXT,
            build_domain INTEGER, build_sha256 TEXT, runner_sha256 TEXT, artifact_sha256 TEXT,
            artifact_bytes INTEGER, run_id TEXT, platform TEXT NOT NULL, runner TEXT NOT NULL,
            artifact TEXT NOT NULL, execution TEXT NOT NULL, total INTEGER, executed INTEGER,
            passed INTEGER, failed INTEGER, skipped INTEGER, other INTEGER, reason_code TEXT NOT NULL
        );
        PRAGMA user_version = 5;
        """;

    private static void PersistReceipt(SqliteConnection connection, SqliteTransaction transaction,
        TestAttempt attempt, TestOutcome outcome, TestRunReceiptDetails details)
    {
        using (var query = Command(connection, transaction, """
            SELECT revision, epoch, target, fingerprint, profile_revision, lease_id
            FROM test_attempt WHERE id = $id AND state = 'running'
            """, ("$id", attempt.Id)))
        using (var row = query.ExecuteReader())
        {
            if (!row.Read() || row.GetInt64(0) != details.Revision || row.GetString(1) != details.Epoch.ToString() ||
                row.GetString(2) != details.Target || row.GetString(3) != details.InputFingerprint ||
                (row.IsDBNull(4) ? "" : row.GetString(4)) != details.ProfileRevision)
                throw new IOException("The receipt does not identify the running attempt.");
            var lease = row.IsDBNull(5) ? null : row.GetString(5);
            details.Validate(attempt, outcome, lease);
        }

        var counts = details.Counters;
        Execute(connection, transaction, """
            INSERT INTO test_run_receipt (
                attempt_id, format, revision, epoch, fence_sequence, git_fingerprint,
                root_identity, target, fingerprint,
                profile_revision, lease_id, build_domain, build_sha256, runner_sha256,
                artifact_sha256, artifact_bytes, run_id, platform, runner, artifact, execution,
                total, executed, passed, failed, skipped, other, reason_code)
            VALUES ($id, $format, $revision, $epoch, $sequence, $git, $root, $target, $fingerprint,
                $profile, $lease, $domain, $build, $runnerDigest, $artifactDigest, $bytes,
                $run, $platform, $runner, $artifact, $execution, $total, $executed,
                $passed, $failed, $skipped, $other, $reason)
            """, ("$id", attempt.Id), ("$format", TestRunReceiptDetails.FormatVersion),
            ("$revision", details.Revision), ("$epoch", details.Epoch.ToString()),
            ("$sequence", details.FenceSequence), ("$git", details.GitFingerprint),
            ("$root", details.RootIdentity), ("$target", details.Target),
            ("$fingerprint", details.InputFingerprint), ("$profile", details.ProfileRevision),
            ("$lease", details.LeaseId), ("$domain", details.BuildDomainVersion),
            ("$build", details.BuildSha256), ("$runnerDigest", details.RunnerSha256),
            ("$artifactDigest", details.ArtifactSha256), ("$bytes", details.ArtifactBytes),
            ("$run", details.RunId?.ToString()), ("$platform", details.Platform.ToString()),
            ("$runner", details.Runner.ToString()), ("$artifact", details.Artifact.ToString()),
            ("$execution", details.Execution.ToString()), ("$total", counts?.Total),
            ("$executed", counts?.Executed), ("$passed", counts?.Passed),
            ("$failed", counts?.Failed), ("$skipped", counts?.Skipped),
            ("$other", counts?.Other), ("$reason", details.ReasonCode));
    }

    internal TestRunReceiptDetails? ReadTestRunReceipt(long attemptId)
    {
        if (attemptId <= 0)
            throw new ArgumentOutOfRangeException(nameof(attemptId));
        lock (writer)
        {
            using var connection = Open(initialize: false);
            using var command = connection.CreateCommand();
            command.CommandText = """
                SELECT format, revision, epoch, fence_sequence, git_fingerprint, root_identity,
                    target, fingerprint, profile_revision, lease_id, build_domain, build_sha256,
                    runner_sha256, artifact_sha256, artifact_bytes, run_id, platform, runner,
                    artifact, execution, total, executed, passed, failed, skipped, other,
                    reason_code FROM test_run_receipt WHERE attempt_id = $id
                """;
            command.Parameters.AddWithValue("$id", attemptId);
            using var row = command.ExecuteReader();
            if (!row.Read())
                return null;
            if (row.GetInt32(0) != TestRunReceiptDetails.FormatVersion)
                throw new IOException("The test receipt format is newer than this reader. Historical evidence was preserved.");
            static T Parse<T>(string text) where T : struct, Enum =>
                Enum.TryParse<T>(text, out var result) && Enum.IsDefined(result)
                    ? result : throw new IOException("The stored receipt has an unsupported disposition.");
            static string? Text(SqliteDataReader reader, int column) =>
                reader.IsDBNull(column) ? null : reader.GetString(column);
            static int? Count(SqliteDataReader reader, int column) =>
                reader.IsDBNull(column) ? null : reader.GetInt32(column);
            var total = Count(row, 20);
            var numbers = new[] { Count(row, 21), Count(row, 22), Count(row, 23), Count(row, 24), Count(row, 25) };
            if ((total is null && numbers.Any(n => n is not null)) ||
                (total is not null && numbers.Any(n => n is null)))
                throw new IOException("The stored receipt counters are incomplete.");
            var counts = total is null ? null : new VstestTrxCounters(total.Value, numbers[0]!.Value,
                numbers[1]!.Value, numbers[2]!.Value, numbers[3]!.Value, numbers[4]!.Value);
            var details = new TestRunReceiptDetails(attemptId, row.GetInt64(1), Guid.Parse(row.GetString(2)),
                row.GetInt64(3), row.GetString(4), row.GetString(5), row.GetString(6),
                row.GetString(7), row.GetString(8), Text(row, 9), Count(row, 10),
                Text(row, 11), Text(row, 12), Text(row, 13),
                row.IsDBNull(14) ? null : row.GetInt64(14),
                Text(row, 15) is { } run ? Guid.Parse(run) : null,
                Parse<VstestDisposition>(row.GetString(16)), Parse<VstestRunnerHealth>(row.GetString(17)),
                Parse<VstestArtifactHealth>(row.GetString(18)), Parse<VstestExecution>(row.GetString(19)),
                counts, row.GetString(26));
            details.ValidatePayload();
            return details;
        }
    }
}
