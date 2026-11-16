using System.Security.Cryptography;

namespace SlopHawk.Daemon;

// Only the supervisor can assert these facts. The collector's XML, including its <sources>,
// is untrusted; source aliases must come from an independently verified target/build mapping.
internal sealed record CoverageSupervisedAttempt(
    Guid Epoch, long AttemptId, string Target, string BuildIdentity, string BinaryIdentity,
    string Collector, string CollectorVersion, string SourceIdentity,
    DateTimeOffset StartedAt, DateTimeOffset FinishedAt, DateTimeOffset Now,
    TestOutcome Outcome, bool Completed, bool CollectorAvailable,
    IReadOnlyList<CoverageDocumentAlias> VerifiedAliases);

// A fresh, dedicated output lease is established before starting the supervised attempt.
// No caller may reuse it for a second attempt, even when the collector bytes are identical.
internal sealed record CoverageResultsLease(
    Guid Epoch, long AttemptId, string Target, string BuildIdentity, string BinaryIdentity,
    string Collector, string CollectorVersion, string SourceIdentity,
    string ResultsDirectory, DateTimeOffset CreatedAt, bool OwnedAndDrained);

// This path is declared by the supervisor, not found by searching the XML or the filesystem.
internal sealed record CoverageArtifactDeclaration(string RelativePath);
internal sealed record CoverageCapture(CoberturaImportResult Result, CoberturaReceipt? Receipt);

internal static class CoverageReceiptCapture
{
    private static readonly TimeSpan ClockSkew = TimeSpan.FromMinutes(2);
    private static readonly TimeSpan MaxDuration = TimeSpan.FromDays(1);
    private const int MaxPathLength = 1024;

    // afterReadForTest is a deterministic seam for exercising path replacement while the
    // original file handle is held. Production callers leave it null.
    internal static CoverageCapture Capture(CoverageSupervisedAttempt attempt,
        CoverageResultsLease lease, CoverageArtifactDeclaration? declaration,
        Action? afterReadForTest = null)
    {
        ArgumentNullException.ThrowIfNull(attempt);
        ArgumentNullException.ThrowIfNull(lease);
        // Never re-enumerate a caller-owned mutable alias list after the physical read.
        // Freeze the exact mapping that is validated and placed into both records.
        try
        {
            if (attempt.VerifiedAliases is null || attempt.VerifiedAliases.Count > CoberturaCoverageImporter.MaxAliases)
                return Reject(CoberturaImportState.Malformed, "supervised_aliases_invalid");
            attempt = attempt with
            {
                VerifiedAliases = attempt.VerifiedAliases.Take(CoberturaCoverageImporter.MaxAliases + 1).ToArray()
            };
        }
        catch (InvalidOperationException)
        {
            return Reject(CoberturaImportState.Malformed, "supervised_aliases_changed");
        }
        if (!ValidAttempt(attempt))
            return Reject(CoberturaImportState.Malformed, "supervised_attempt_invalid");
        if (!MatchLease(attempt, lease))
            return Reject(CoberturaImportState.Mismatched, "attempt_results_lease_mismatch");
        if (!lease.OwnedAndDrained)
            return Reject(CoberturaImportState.Missing, "results_lease_not_drained");
        if (!attempt.Completed || attempt.Outcome is not (TestOutcome.Passed or TestOutcome.Failed))
            return Reject(CoberturaImportState.Missing, "test_run_not_completed");
        if (!attempt.CollectorAvailable)
            return Reject(CoberturaImportState.Missing, "collector_unavailable");
        if (declaration is null)
            return Reject(CoberturaImportState.Missing, "coverage_report_not_declared");
        if (!ValidRelativePath(declaration.RelativePath) ||
            !Path.IsPathFullyQualified(lease.ResultsDirectory))
            return Reject(CoberturaImportState.Malformed, "coverage_artifact_path_invalid");

        try
        {
            var root = Path.TrimEndingDirectorySeparator(Path.GetFullPath(lease.ResultsDirectory));
            var candidate = Path.GetFullPath(Path.Combine(root,
                declaration.RelativePath.Replace('/', Path.DirectorySeparatorChar)));
            if (!ExecutionProfile.Contains(root, candidate) || candidate == root)
                return Reject(CoberturaImportState.Malformed, "coverage_artifact_path_escape");

            // Paths may be swapped independently of the opened handle. Record the physical
            // identities of every path component, then check them again after the read.
            var directoryPaths = Ancestors(root).Concat(ParentsInside(root, candidate)).ToArray();
            var directories = new PhysicalArtifactFile.FileIdentity[directoryPaths.Length];
            for (var i = 0; i < directoryPaths.Length; i++)
            {
                if (!PhysicalArtifactFile.TryPathIdentity(directoryPaths[i], expectDirectory: true, out directories[i]))
                    return Reject(CoberturaImportState.Mismatched, "results_directory_link_or_replacement");
            }
            var rootCreated = Directory.GetCreationTimeUtc(root);
            if (!Directory.Exists(root) || rootCreated <
                    (attempt.StartedAt - ClockSkew).UtcDateTime ||
                rootCreated < (lease.CreatedAt - ClockSkew).UtcDateTime ||
                rootCreated > (attempt.FinishedAt + ClockSkew).UtcDateTime)
                return Reject(CoberturaImportState.Expired, "results_directory_not_fresh");

            if (!PhysicalArtifactFile.TryPathIdentity(candidate, expectDirectory: false, out var before))
                return Reject(File.Exists(candidate) ? CoberturaImportState.Mismatched :
                    CoberturaImportState.Missing, "coverage_report_missing_or_linked");

            using var handle = PhysicalArtifactFile.OpenRead(candidate);
            if (!PhysicalArtifactFile.TryHandleIdentity(handle, expectDirectory: false, out var opened) || opened != before)
                return Reject(CoberturaImportState.Mismatched, "coverage_artifact_replaced_before_open");

            var length = RandomAccess.GetLength(handle);
            if (length is <= 0 or > CoberturaCoverageImporter.MaxArtifactBytes)
                return Reject(CoberturaImportState.Malformed, "coverage_artifact_size_invalid");
            var writeTime = File.GetLastWriteTimeUtc(handle);
            if (writeTime < attempt.StartedAt.UtcDateTime ||
                writeTime > (attempt.FinishedAt + ClockSkew).UtcDateTime)
                return Reject(CoberturaImportState.Expired, "coverage_artifact_not_fresh");

            var bytes = new byte[(int)length];
            var offset = 0;
            while (offset < bytes.Length)
            {
                var count = RandomAccess.Read(handle, bytes.AsSpan(offset), offset);
                if (count == 0)
                    return Reject(CoberturaImportState.Mismatched, "coverage_artifact_truncated_during_read");
                offset += count;
            }
            afterReadForTest?.Invoke();
            // A second read detects in-place writes that race the first snapshot.
            var check = new byte[bytes.Length];
            offset = 0;
            while (offset < check.Length)
            {
                var count = RandomAccess.Read(handle, check.AsSpan(offset), offset);
                if (count == 0)
                    return Reject(CoberturaImportState.Mismatched, "coverage_artifact_changed_during_read");
                offset += count;
            }
            if (!CryptographicOperations.FixedTimeEquals(bytes, check) ||
                RandomAccess.GetLength(handle) != length ||
                File.GetLastWriteTimeUtc(handle) != writeTime ||
                !PhysicalArtifactFile.TryHandleIdentity(handle, expectDirectory: false, out var afterHandle) ||
                opened != afterHandle ||
                !PhysicalArtifactFile.TryPathIdentity(candidate, expectDirectory: false, out var afterPath) ||
                opened != afterPath)
                return Reject(CoberturaImportState.Mismatched, "coverage_artifact_replaced_during_read");
            for (var i = 0; i < directoryPaths.Length; i++)
            {
                if (!PhysicalArtifactFile.TryPathIdentity(directoryPaths[i], expectDirectory: true, out var current) ||
                    current != directories[i])
                    return Reject(CoberturaImportState.Mismatched, "results_directory_replaced_during_read");
            }

            // The one bounded snapshot supplies both the receipt digest and the importer.
            // Neither the collector XML nor a standalone digest can mint a trusted alias.
            var aliases = attempt.VerifiedAliases;
            var receipt = new CoberturaReceipt(attempt.Epoch, attempt.AttemptId, attempt.Target,
                attempt.BuildIdentity, attempt.BinaryIdentity, attempt.Collector,
                attempt.CollectorVersion, attempt.SourceIdentity, attempt.FinishedAt,
                Convert.ToHexString(SHA256.HashData(bytes)), aliases);
            var expected = new CoverageExpectedRun(attempt.Epoch, attempt.AttemptId, attempt.Target,
                attempt.BuildIdentity, attempt.BinaryIdentity, attempt.Collector,
                attempt.CollectorVersion, attempt.SourceIdentity, attempt.StartedAt,
                attempt.Now, attempt.Outcome, aliases);
            var result = CoberturaCoverageImporter.Import(bytes, receipt, expected);
            return new CoverageCapture(result,
                result.State is CoberturaImportState.Imported or CoberturaImportState.UnmappedScope ? receipt : null);
        }
        catch (FileNotFoundException)
        {
            return Reject(CoberturaImportState.Missing, "coverage_report_missing");
        }
        catch (DirectoryNotFoundException)
        {
            return Reject(CoberturaImportState.Missing, "results_directory_missing");
        }
        catch (Exception ex) when (ex is IOException or UnauthorizedAccessException or
            ArgumentException or NotSupportedException or OverflowException or
            DllNotFoundException or EntryPointNotFoundException)
        {
            return Reject(CoberturaImportState.Mismatched, "coverage_artifact_unverifiable");
        }
    }

    private static bool ValidAttempt(CoverageSupervisedAttempt a)
    {
        if (a.Epoch == Guid.Empty || a.AttemptId <= 0 || !Field(a.Target) ||
            !Field(a.BuildIdentity) || !Field(a.BinaryIdentity) ||
            !Field(a.Collector) || !Field(a.CollectorVersion) || !Field(a.SourceIdentity) ||
            a.StartedAt > a.FinishedAt || a.FinishedAt > a.Now + ClockSkew ||
            a.Now - a.StartedAt > MaxDuration || a.Now < a.StartedAt ||
            a.VerifiedAliases is null ||
            a.VerifiedAliases.Count > CoberturaCoverageImporter.MaxAliases)
            return false;
        var aliases = new HashSet<string>(StringComparer.Ordinal);
        var sourceDigests = new Dictionary<string, string>(StringComparer.Ordinal);
        foreach (var item in a.VerifiedAliases)
        {
            if (item is null || item.Target != a.Target || !Field(item.CollectorPath, 1024) ||
                item.CollectorPath.Contains('\\') || item.CollectorPath.Split('/').Any(
                    segment => segment is "." or "..") ||
                !ValidRelativePath(item.SourcePath) ||
                !Sha256(item.SourceSha256) || !aliases.Add(item.CollectorPath) ||
                (sourceDigests.TryGetValue(item.SourcePath, out var prior) &&
                 prior != item.SourceSha256))
                return false;
            sourceDigests[item.SourcePath] = item.SourceSha256;
        }
        return true;
    }

    private static bool MatchLease(CoverageSupervisedAttempt a, CoverageResultsLease l) =>
        l.Epoch == a.Epoch && l.AttemptId == a.AttemptId && l.Target == a.Target &&
        l.BuildIdentity == a.BuildIdentity && l.BinaryIdentity == a.BinaryIdentity &&
        l.Collector == a.Collector && l.CollectorVersion == a.CollectorVersion &&
        l.SourceIdentity == a.SourceIdentity &&
        l.CreatedAt >= a.StartedAt - ClockSkew && l.CreatedAt <= a.FinishedAt &&
        !string.IsNullOrWhiteSpace(l.ResultsDirectory);

    private static bool Field(string? value, int limit = 256) =>
        value is { Length: > 0 } && value.Length <= limit &&
        !string.IsNullOrWhiteSpace(value) && !value.Any(char.IsControl);

    private static bool Sha256(string? value) => value is { Length: 64 } &&
        value.All(character => character is >= '0' and <= '9' or >= 'A' and <= 'F' or
            >= 'a' and <= 'f');

    private static bool ValidRelativePath(string? value) =>
        Field(value, MaxPathLength) && !Path.IsPathRooted(value!) &&
        !value!.StartsWith('/') && !value.Contains('\\') && !value.Contains(':') &&
        value.Split('/').All(segment => segment.Length > 0 && segment is not ("." or ".."));

    private static IEnumerable<string> Ancestors(string root)
    {
        var path = root;
        var parents = new Stack<string>();
        while (path is not null)
        {
            parents.Push(path);
            path = Path.GetDirectoryName(path);
        }
        return parents;
    }

    private static IEnumerable<string> ParentsInside(string root, string file)
    {
        var parent = Path.GetDirectoryName(file)!;
        var segments = new Stack<string>();
        while (parent != root)
        {
            segments.Push(parent);
            parent = Path.GetDirectoryName(parent)!;
        }
        return segments;
    }

    private static CoverageCapture Reject(CoberturaImportState state, string reason) =>
        new(new CoberturaImportResult(state, reason, null, [], [], []), null);
}
