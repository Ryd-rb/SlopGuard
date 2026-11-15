using System.Globalization;
using System.Security.Cryptography;
using System.Text;

namespace SlopGuard.Daemon;

// The adapter supplies the evaluated input identity and runs in-process in this stage.
// A process-backed adapter will require execution profiles and supervised output leases.
internal interface ITestAdapter
{
    Task<TestSelection> SelectAsync(TestSelectionInput input, CancellationToken cancellationToken);
    Task<TestOutcome> RunAsync(TestAttempt attempt, CancellationToken cancellationToken);
}

internal sealed record ScheduleFence(Guid Epoch, long Sequence, long Revision, string RootIdentity, GitRevision Git);
internal sealed record TestSelectionInput(ScheduleFence Fence, IReadOnlyList<TestCause> Causes);

internal sealed record TestCause(long Revision, string PathKey, string? OldPath, string? OldDigest,
    string? NewPath, string? NewDigest, string? Predecessor, long? FallbackFirstRevision = null)
{
    internal string Id => Identifier(Revision, PathKey);

    internal static string Identifier(long revision, string pathKey) =>
        Hash(revision.ToString(CultureInfo.InvariantCulture) + ":" + pathKey);

    internal static string Hash(string value) =>
        Convert.ToHexString(SHA256.HashData(Encoding.UTF8.GetBytes(value)));
}

internal sealed record TestSelection(bool Complete, IReadOnlyList<SelectedTarget> Targets);
internal sealed record SelectedTarget(string Id, string InputFingerprint, IReadOnlyList<string> CauseIds);
internal sealed record TestEvidence(string CauseId, string Target, string InputFingerprint);
internal sealed record TestWork(TestSelectionInput Input, HashSet<TestEvidence> Evidence,
    PendingIntents Pending, TestAttemptReceipt? Latest);

internal enum TestOutcome { Passed, Failed, RetryableFailure, Blocked, Cancelled }

internal sealed record TestAttempt(long Id, int Number, ScheduleFence Fence, SelectedTarget Target);
internal sealed record TestAttemptReceipt(long Id, int Number, long Revision, Guid Epoch, string Target,
    string State, string? Outcome, DateTimeOffset? RetryAt);
internal sealed record TestAdmission(TestAttempt? Attempt, DateTimeOffset? RetryAt);
internal sealed record TestSchedulerStatus(string State, TestAttemptReceipt? Latest = null,
    DateTimeOffset? RetryAt = null, string? Error = null);

internal sealed class ValidatedSelection
{
    internal const int MaxTargets = 16;
    private readonly Dictionary<string, List<TestEvidence>> required;

    internal ValidatedSelection(TestWork work, TestSelection selection)
    {
        required = work.Input.Causes.ToDictionary(cause => cause.Id, _ => new List<TestEvidence>(), StringComparer.Ordinal);
        if (selection.Targets.Count > MaxTargets)
            throw new IOException("Test selection exceeds the scheduler target budget.");
        var selected = new List<SelectedTarget>();
        var targets = new HashSet<string>(StringComparer.Ordinal);
        foreach (var target in selection.Targets)
        {
            if (string.IsNullOrWhiteSpace(target.Id) || target.Id.Length > 128 || !targets.Add(target.Id) ||
                string.IsNullOrWhiteSpace(target.InputFingerprint) || target.InputFingerprint.Length > 128 ||
                target.CauseIds.Count == 0 || target.CauseIds.Count > required.Count)
                throw new IOException("The test selector returned an invalid target.");

            foreach (var id in target.CauseIds.Distinct(StringComparer.Ordinal))
            {
                if (!required.TryGetValue(id, out var evidence))
                    throw new IOException("The test selector returned a cause outside the current inventory.");
                evidence.Add(new TestEvidence(id, target.Id, target.InputFingerprint));
            }
            selected.Add(target with
            {
                CauseIds = target.CauseIds.Distinct(StringComparer.Ordinal).Order(StringComparer.Ordinal).ToArray()
            });
        }
        Targets = selected;
        Complete = selection.Complete;
    }

    internal IReadOnlyList<SelectedTarget> Targets { get; }
    internal bool Complete { get; }
    internal bool Satisfied(string cause, HashSet<TestEvidence> evidence) =>
        Complete && required[cause].All(evidence.Contains);
}
