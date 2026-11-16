using System.Globalization;
using System.Security.Cryptography;
using System.Xml;
using System.Xml.Linq;

namespace SlopGuard.Daemon;

// This is an evidence reader, not a runner. The caller must obtain facts from its
// own supervised attempt and read artifacts only from that attempt's empty output root.
internal enum VstestArtifactHealth { Complete, Missing, Oversized, Malformed, Inconsistent, Stale }
internal enum VstestRunnerHealth { ZeroExit, NonzeroExit, Incomplete, TimedOut, Cancelled, StartupOrBuildFailure }
internal enum VstestExecution { Unknown, Passed, Failed, ZeroDiscovered, AllSkipped }
internal enum VstestDisposition
{
    Passed, Failed, ZeroDiscovered, AllSkipped, Timeout, Cancelled,
    StartupOrBuildFailure, ArtifactFailure, RunnerFailure, Incomplete
}

internal sealed record VstestTrxCounters(int Total, int Executed, int Passed, int Failed,
    int Skipped, int Other);

// Names and messages are untrusted, display-only excerpts: TRX does not guarantee
// distinct adapter identities for repeated names or data-driven rows.
internal sealed record VstestFailureSample(string DisplayName, string? Message);

internal sealed record VstestTrxArtifact(VstestArtifactHealth Health, long ByteLength,
    string? Sha256, Guid? RunId, string? SummaryOutcome, DateTimeOffset? StartedUtc,
    DateTimeOffset? FinishedUtc, VstestTrxCounters? Counters, bool HasRunErrors,
    IReadOnlyList<VstestFailureSample> Failures, string Reason);

internal sealed record VstestProcessFacts(long AttemptId, DateTimeOffset StartedUtc,
    DateTimeOffset? FinishedUtc, bool OwnedResultsDirectory, DateTimeOffset? ArtifactLastWriteUtc,
    bool Exited, int? ExitCode, bool TimedOut = false, bool Cancelled = false,
    bool StartupOrBuildFailed = false);

internal sealed record VstestClassification(VstestDisposition Disposition,
    VstestRunnerHealth Runner, VstestTrxArtifact Artifact, VstestExecution Execution,
    string Reason)
{
    // Other dispositions must be explicitly resolved by a future adapter/policy.
    // In particular neither zero discovered nor all skipped satisfies a test intent.
    internal TestOutcome? SchedulerOutcome => Disposition switch
    {
        VstestDisposition.Passed => TestOutcome.Passed,
        VstestDisposition.Failed => TestOutcome.Failed,
        VstestDisposition.Cancelled => TestOutcome.Cancelled,
        _ => null
    };
}

internal static class VstestTrxClassifier
{
    internal const int MaxArtifactBytes = 2 * 1024 * 1024;
    internal const int MaxTests = 100_000;
    internal const int MaxXmlElements = 50_000;
    internal const int MaxXmlDepth = 32;
    internal const int MaxFailureSamples = 8;
    private const int MaxAttributes = 32;
    private const int MaxAttributeCharacters = 4096;
    private const int MaxNameCharacters = 160;
    private const int MaxMessageCharacters = 256;
    private static readonly XNamespace Trx = "http://microsoft.com/schemas/VisualStudio/TeamTest/2010";
    private static readonly string[] AdditionalCounters =
    [
        "error", "timeout", "aborted", "inconclusive", "passedButRunAborted", "notRunnable",
        "notExecuted", "disconnected", "warning", "completed", "inProgress", "pending"
    ];

    internal static VstestTrxArtifact Parse(byte[]? content)
    {
        if (content is null)
            return Invalid(VstestArtifactHealth.Missing, 0, null, "TRX artifact is missing.");
        if (content.Length == 0)
            return Invalid(VstestArtifactHealth.Malformed, 0,
                Convert.ToHexString(SHA256.HashData(content)), "TRX artifact is empty.");

        // The digest is of the exact artifact bytes, never of reserialized XML.
        if (content.Length > MaxArtifactBytes)
            return Invalid(VstestArtifactHealth.Oversized, content.LongLength, null, "TRX exceeds the byte limit.");

        var digest = Convert.ToHexString(SHA256.HashData(content));
        try
        {
            var settings = new XmlReaderSettings
            {
                DtdProcessing = DtdProcessing.Prohibit,
                XmlResolver = null,
                MaxCharactersInDocument = MaxArtifactBytes,
                MaxCharactersFromEntities = MaxArtifactBytes,
                IgnoreWhitespace = true,
                IgnoreComments = true,
                CheckCharacters = true
            };
            // Scan before materializing: cap structure and attribute amplification.
            using (var stream = new MemoryStream(content, writable: false))
            using (var reader = XmlReader.Create(stream, settings))
            {
                var elements = 0;
                while (reader.Read())
                {
                    if (reader.NodeType != XmlNodeType.Element)
                        continue;
                    if (++elements > MaxXmlElements || reader.Depth > MaxXmlDepth ||
                        reader.AttributeCount > MaxAttributes)
                        return Invalid(VstestArtifactHealth.Oversized, content.LongLength, digest,
                            "TRX exceeds the XML structure limit.");
                    if (!reader.HasAttributes)
                        continue;
                    while (reader.MoveToNextAttribute())
                    {
                        if (reader.Value.Length > MaxAttributeCharacters)
                            return Invalid(VstestArtifactHealth.Oversized, content.LongLength, digest,
                                "TRX exceeds the XML attribute limit.");
                    }
                    reader.MoveToElement();
                }
            }

            using var xmlStream = new MemoryStream(content, writable: false);
            using var xmlReader = XmlReader.Create(xmlStream, settings);
            var root = XDocument.Load(xmlReader, LoadOptions.None).Root;
            if (root?.Name != Trx + "TestRun" || !Guid.TryParse((string?)root.Attribute("id"), out var runId) ||
                runId == Guid.Empty)
                return Invalid(VstestArtifactHealth.Malformed, content.LongLength, digest,
                    "TRX root namespace or run identity is missing or invalid.");

            var summaries = root.Elements(Trx + "ResultSummary").ToArray();
            var times = root.Elements(Trx + "Times").ToArray();
            var results = root.Elements(Trx + "Results").ToArray();
            if (summaries.Length != 1 || times.Length != 1 || results.Length > 1 ||
                !TryTimestamp(times[0], "start", out var started) ||
                !TryTimestamp(times[0], "finish", out var finished) || finished < started)
                return Invalid(VstestArtifactHealth.Inconsistent, content.LongLength, digest,
                    "TRX requires a single summary and ordered start/finish times.");

            var summary = summaries[0];
            var summaryOutcome = (string?)summary.Attribute("outcome");
            if (summaryOutcome is not ("Completed" or "Failed" or "Error" or "Aborted" or
                "Timeout" or "Inconclusive" or "Passed"))
                return Invalid(VstestArtifactHealth.Inconsistent, content.LongLength, digest,
                    "TRX has an unsupported or non-final summary outcome.");

            var countersNodes = summary.Elements(Trx + "Counters").ToArray();
            if (countersNodes.Length != 1 ||
                !TryCount(countersNodes[0], "total", required: true, out var total) ||
                !TryCount(countersNodes[0], "executed", required: true, out var executed) ||
                !TryCount(countersNodes[0], "passed", required: true, out var passed) ||
                !TryCount(countersNodes[0], "failed", required: true, out var failed))
                return Invalid(VstestArtifactHealth.Inconsistent, content.LongLength, digest,
                    "TRX is missing a valid required counter.");

            // VSTest omits the Results element when its test filter matches no tests.
            if (results.Length == 0 && total != 0)
                return Invalid(VstestArtifactHealth.Inconsistent, content.LongLength, digest,
                    "A nonempty TRX run requires a Results section.");

            long other = 0;
            long activeOther = 0;
            int explicitlyUnexecuted = 0;
            foreach (var name in AdditionalCounters)
            {
                if (!TryCount(countersNodes[0], name, required: false, out var value))
                    return Invalid(VstestArtifactHealth.Inconsistent, content.LongLength, digest,
                        $"TRX has an invalid {name} counter.");
                other += value;
                if (name == "notExecuted")
                    explicitlyUnexecuted = value;
                else
                    activeOther += value;
            }
            if (activeOther != 0 || explicitlyUnexecuted > total - executed || executed > total || (long)passed + failed > executed ||
                (long)passed + failed + other > total ||
                (summaryOutcome is "Completed" or "Passed") && failed != 0)
                return Invalid(VstestArtifactHealth.Inconsistent, content.LongLength, digest,
                    "TRX counters contradict each other or the run summary.");

            // VSTest's current TRX logger often writes notExecuted=0 even for skips;
            // total - executed is the unexecuted aggregate, not per-case evidence.
            // Older logger fixtures can omit skipped rows from total entirely, so
            // an explicit NotExecuted result is not a contradiction of these counts.
            var counts = new VstestTrxCounters(total, executed, passed, failed, total - executed,
                checked((int)other));
            var hasRunErrors = summary.Descendants(Trx + "RunInfo").Any(info =>
                (string?)info.Attribute("outcome") == "Error");
            var failures = new List<VstestFailureSample>();
            foreach (var element in results.SelectMany(result => result.Descendants(Trx + "UnitTestResult")))
            {
                var caseOutcome = (string?)element.Attribute("outcome");
                if (caseOutcome is not ("Passed" or "Failed" or "NotExecuted") ||
                    total == 0 || (caseOutcome == "Passed" && passed == 0))
                    return Invalid(VstestArtifactHealth.Inconsistent, content.LongLength, digest,
                        "Individual result contradicts the summary counters.");
                if (caseOutcome != "Failed")
                    continue;
                if (failed == 0)
                    return Invalid(VstestArtifactHealth.Inconsistent, content.LongLength, digest,
                        "Failed result entry contradicts the aggregate counters.");
                if (failures.Count < MaxFailureSamples)
                {
                    var name = Excerpt((string?)element.Attribute("testName"), MaxNameCharacters);
                    var message = Excerpt((string?)element.Element(Trx + "Output")?
                        .Element(Trx + "ErrorInfo")?.Element(Trx + "Message"), MaxMessageCharacters);
                    failures.Add(new VstestFailureSample(name ?? "(unnamed)", message));
                }
            }
            return new VstestTrxArtifact(VstestArtifactHealth.Complete, content.LongLength, digest,
                runId, summaryOutcome, started, finished, counts, hasRunErrors, failures.ToArray(),
                "Complete bounded TRX receipt.");
        }
        catch (Exception ex) when (ex is XmlException or ArgumentException or FormatException or OverflowException)
        {
            return Invalid(VstestArtifactHealth.Malformed, content.LongLength, digest, "TRX XML cannot be safely parsed.");
        }
    }

    internal static VstestClassification Classify(VstestProcessFacts process, VstestTrxArtifact artifact)
    {
        var runner = process.Cancelled ? VstestRunnerHealth.Cancelled :
            process.TimedOut ? VstestRunnerHealth.TimedOut :
            process.StartupOrBuildFailed ? VstestRunnerHealth.StartupOrBuildFailure :
            !process.Exited || process.ExitCode is null ? VstestRunnerHealth.Incomplete :
            process.ExitCode != 0 ? VstestRunnerHealth.NonzeroExit : VstestRunnerHealth.ZeroExit;

        VstestClassification Result(VstestDisposition disposition, VstestExecution execution, string reason,
            VstestTrxArtifact? replacement = null) => new(disposition, runner, replacement ?? artifact, execution, reason);

        if (!process.Exited && process.ExitCode is not null &&
            !process.Cancelled && !process.TimedOut && !process.StartupOrBuildFailed)
            return Result(VstestDisposition.ArtifactFailure, VstestExecution.Unknown,
                "Exit code exists without a confirmed process exit.", artifact with
                { Health = VstestArtifactHealth.Inconsistent, Reason = "Conflicting process facts." });
        if (process.Cancelled && process.TimedOut)
            return Result(VstestDisposition.ArtifactFailure, VstestExecution.Unknown,
                "Attempt has contradictory timeout and cancellation facts.", artifact with
                { Health = VstestArtifactHealth.Inconsistent, Reason = "Conflicting termination facts." });
        if (runner == VstestRunnerHealth.Cancelled)
            return Result(VstestDisposition.Cancelled, VstestExecution.Unknown, "Attempt was cancelled.");
        if (runner == VstestRunnerHealth.TimedOut)
            return Result(VstestDisposition.Timeout, VstestExecution.Unknown, "Attempt timed out.");
        if (runner == VstestRunnerHealth.StartupOrBuildFailure)
            return Result(VstestDisposition.StartupOrBuildFailure, VstestExecution.Unknown,
                "The trusted runner reported a startup or build failure.");
        if (artifact.Health != VstestArtifactHealth.Complete)
            return Result(VstestDisposition.ArtifactFailure, VstestExecution.Unknown, artifact.Reason);

        // A digest and TRX run GUID are not proof of ownership. Both the empty
        // per-attempt directory and timestamp checks are required independently.
        if (!process.OwnedResultsDirectory || process.ArtifactLastWriteUtc is not { } modified ||
            modified < process.StartedUtc ||
            artifact.FinishedUtc < process.StartedUtc ||
            artifact.StartedUtc < process.StartedUtc - TimeSpan.FromSeconds(2) ||
            (process.FinishedUtc is { } end &&
                (artifact.FinishedUtc > end + TimeSpan.FromSeconds(2) || modified > end + TimeSpan.FromSeconds(2))))
            return Result(VstestDisposition.ArtifactFailure, VstestExecution.Unknown,
                "TRX cannot be attributed to this owned attempt.", artifact with
                { Health = VstestArtifactHealth.Stale, Reason = "Missing ownership or incompatible timestamps." });
        if (artifact.FinishedUtc < artifact.StartedUtc ||
            (process.FinishedUtc is { } finish && finish < process.StartedUtc))
            return Result(VstestDisposition.ArtifactFailure, VstestExecution.Unknown,
                "TRX completion facts contradict the process timeline.", artifact with
                { Health = VstestArtifactHealth.Inconsistent, Reason = "Contradictory completion timeline." });
        if (runner == VstestRunnerHealth.Incomplete)
            return Result(VstestDisposition.Incomplete, VstestExecution.Unknown,
                "No confirmed runner exit; TRX alone cannot resolve the attempt.");

        var counts = artifact.Counters!;
        var outcome = artifact.SummaryOutcome;
        if (counts.Failed > 0 && outcome is ("Failed" or "Error"))
            return Result(VstestDisposition.Failed, VstestExecution.Failed,
                "A complete matching receipt contains failed tests, regardless of exit code.");
        if (counts.Total == 0 && outcome is ("Completed" or "Passed" or "Failed") &&
            counts.Executed == 0 && !artifact.HasRunErrors)
            return Result(VstestDisposition.ZeroDiscovered, VstestExecution.ZeroDiscovered,
                "The completed runner receipt records zero discovered tests.");
        if (counts.Total > 0 && counts.Executed == 0 &&
            outcome is ("Completed" or "Passed") && !artifact.HasRunErrors)
            return Result(VstestDisposition.AllSkipped, VstestExecution.AllSkipped,
                "All selected tests were unexecuted; scope remains incomplete.");
        if (counts.Total > 0 && counts.Executed == counts.Passed && counts.Passed > 0 &&
            outcome is ("Completed" or "Passed") && !artifact.HasRunErrors &&
            runner == VstestRunnerHealth.ZeroExit)
            return Result(VstestDisposition.Passed, VstestExecution.Passed, "All executed tests passed.");
        if (runner == VstestRunnerHealth.NonzeroExit || artifact.HasRunErrors ||
            outcome is ("Failed" or "Error" or "Aborted" or "Timeout" or "Inconclusive"))
            return Result(VstestDisposition.RunnerFailure, VstestExecution.Unknown,
                "Runner exit or summary is not sufficient to establish a clean test run.");
        return Result(VstestDisposition.ArtifactFailure, VstestExecution.Unknown,
            "TRX counts do not establish a supported terminal execution disposition.", artifact with
            { Health = VstestArtifactHealth.Inconsistent, Reason = "Incomplete or ambiguous result counters." });
    }

    private static VstestTrxArtifact Invalid(VstestArtifactHealth health, long length, string? digest, string reason) =>
        new(health, length, digest, null, null, null, null, null, false, [], reason);

    private static bool TryTimestamp(XElement times, string name, out DateTimeOffset result) =>
        DateTimeOffset.TryParse((string?)times.Attribute(name), CultureInfo.InvariantCulture,
            DateTimeStyles.RoundtripKind, out result);

    private static bool TryCount(XElement counters, string name, bool required, out int value)
    {
        value = 0;
        var raw = (string?)counters.Attribute(name);
        return raw is null ? !required :
            raw.Length is > 0 and <= 10 && int.TryParse(raw, NumberStyles.None,
                CultureInfo.InvariantCulture, out value) && value <= MaxTests;
    }

    private static string? Excerpt(string? text, int limit)
    {
        if (string.IsNullOrWhiteSpace(text))
            return null;
        var length = Math.Min(text.Length, limit);
        return new string(text.AsSpan(0, length).ToArray()
            .Select(ch => char.IsControl(ch) ? ' ' : ch).ToArray());
    }
}
