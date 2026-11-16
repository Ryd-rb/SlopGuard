using System.Security.Cryptography;
using System.Text;
using SlopGuard.Daemon;
using Xunit;

namespace SlopGuard.IntegrationTests;

public sealed class VstestTrxClassifierTests
{
    private const string RunId = "01234567-89ab-cdef-0123-456789abcdef";
    private static readonly DateTimeOffset Started = DateTimeOffset.Parse("2026-01-02T12:00:00Z");

    [Theory]
    [InlineData("Completed", 3, 3, 3, 0, 0, (int)VstestDisposition.Passed, (int)VstestExecution.Passed)]
    [InlineData("Completed", 3, 2, 2, 0, 0, (int)VstestDisposition.Passed, (int)VstestExecution.Passed)]
    [InlineData("Failed", 3, 3, 2, 1, 1, (int)VstestDisposition.Failed, (int)VstestExecution.Failed)]
    [InlineData("Completed", 0, 0, 0, 0, 0, (int)VstestDisposition.ZeroDiscovered, (int)VstestExecution.ZeroDiscovered)]
    [InlineData("Failed", 0, 0, 0, 0, 1, (int)VstestDisposition.ZeroDiscovered, (int)VstestExecution.ZeroDiscovered)]
    [InlineData("Completed", 2, 0, 0, 0, 0, (int)VstestDisposition.AllSkipped, (int)VstestExecution.AllSkipped)]
    public void AggregatesAreIndependentFromExitCode(string outcome, int total, int executed,
        int passed, int failed, int exit, int disposition, int execution)
    {
        var artifact = VstestTrxClassifier.Parse(Trx(outcome, total, executed, passed, failed));
        var result = VstestTrxClassifier.Classify(Facts(exitCode: exit), artifact);
        Assert.Equal(VstestArtifactHealth.Complete, result.Artifact.Health);
        Assert.Equal((VstestDisposition)disposition, result.Disposition);
        Assert.Equal((VstestExecution)execution, result.Execution);
        Assert.Equal(total - executed, artifact.Counters!.Skipped);
        Assert.Equal(RunId, artifact.RunId!.Value.ToString());
        Assert.Equal(Convert.ToHexString(SHA256.HashData(Trx(outcome, total, executed, passed, failed))),
            artifact.Sha256);
    }

    [Fact]
    public void ZeroDiscoveryReceiptMayOmitTheResultsElement()
    {
        var xml = Encoding.UTF8.GetString(Trx("Completed", 0, 0, 0, 0));
        var start = xml.IndexOf("<Results", StringComparison.Ordinal);
        var end = xml.IndexOf("</Results>", StringComparison.Ordinal);
        Assert.True(start >= 0 && end > start);
        var receipt = Classify(Encoding.UTF8.GetBytes(xml.Remove(start, end - start + "</Results>".Length)), Facts());
        Assert.Equal(VstestDisposition.ZeroDiscovered, receipt.Disposition);
        Assert.Null(receipt.SchedulerOutcome);
    }

    [Theory]
    [InlineData("Timeout")]
    [InlineData("Error")]
    [InlineData("")]
    public void InconsistentCaseOutcomeCannotBeReportedAsPassed(string outcome)
    {
        var result = Classify(Trx("Completed", 1, 1, 1, 0,
            results: $"<UnitTestResult testName='Case' outcome='{outcome}'/>"), Facts());
        Assert.Equal(VstestDisposition.ArtifactFailure, result.Disposition);
        Assert.Null(result.SchedulerOutcome);
    }

    [Fact]
    public void FinishedRunFromBeforeTheAttemptIsStaleDespiteACurrentFileTimestamp()
    {
        var result = Classify(Trx("Completed", 1, 1, 1, 0,
            times: "<Times start='2026-01-02T11:59:58Z' finish='2026-01-02T11:59:59Z'/>"), Facts());
        Assert.Equal(VstestArtifactHealth.Stale, result.Artifact.Health);
        Assert.Null(result.SchedulerOutcome);
    }

    [Fact]
    public void FailedTestWithCompleteReceiptIsDefinitiveDespiteSuccessfulProcessExit()
    {
        var artifact = VstestTrxClassifier.Parse(Trx("Failed", 1, 1, 0, 1,
            results: "<UnitTestResult testName=\"Case A\" outcome=\"Failed\"><Output><ErrorInfo><Message>assert failed</Message></ErrorInfo></Output></UnitTestResult>"));
        var result = VstestTrxClassifier.Classify(Facts(), artifact);
        Assert.Equal(VstestDisposition.Failed, result.Disposition);
        Assert.Equal((TestOutcome?)TestOutcome.Failed, result.SchedulerOutcome);
        var sample = Assert.Single(artifact.Failures);
        Assert.Equal("Case A", sample.DisplayName);
        Assert.Equal("assert failed", sample.Message);
    }

    [Fact]
    public void AggregateOnlyReceiptDoesNotInventTestIdentities()
    {
        var receipt = VstestTrxClassifier.Parse(Trx("Failed", 2, 2, 1, 1));
        Assert.Equal(1, receipt.Counters!.Failed);
        Assert.Empty(receipt.Failures);
        Assert.Equal(VstestDisposition.Failed, VstestTrxClassifier.Classify(Facts(exitCode: 1), receipt).Disposition);
    }

    [Fact]
    public void NonzeroExitCannotOverrideAnOtherwiseSuccessfulReceipt()
    {
        var classified = Classify(Trx("Completed", 1, 1, 1, 0), Facts(exitCode: 1));
        Assert.Equal(VstestDisposition.RunnerFailure, classified.Disposition);
        Assert.Equal(VstestRunnerHealth.NonzeroExit, classified.Runner);
        Assert.Null(classified.SchedulerOutcome);
    }

    [Theory]
    [InlineData(true, false, (int)VstestDisposition.Timeout, (int)VstestRunnerHealth.TimedOut)]
    [InlineData(false, true, (int)VstestDisposition.Cancelled, (int)VstestRunnerHealth.Cancelled)]
    public void TerminationFactsAreDistinctFromMissingArtifacts(bool timedOut, bool cancelled,
        int expected, int health)
    {
        var process = Facts(exited: false, exitCode: null) with { TimedOut = timedOut, Cancelled = cancelled };
        var result = VstestTrxClassifier.Classify(process, VstestTrxClassifier.Parse(null));
        Assert.Equal((VstestDisposition)expected, result.Disposition);
        Assert.Equal((VstestRunnerHealth)health, result.Runner);
        Assert.Equal(VstestArtifactHealth.Missing, result.Artifact.Health);
        Assert.Equal(cancelled ? (TestOutcome?)TestOutcome.Cancelled : null, result.SchedulerOutcome);
    }

    [Fact]
    public void StartupOrBuildFailureDoesNotRequireAReceipt()
    {
        var result = VstestTrxClassifier.Classify(Facts(exited: false, exitCode: null) with
        { StartupOrBuildFailed = true }, VstestTrxClassifier.Parse(null));
        Assert.Equal(VstestDisposition.StartupOrBuildFailure, result.Disposition);
        Assert.Null(result.SchedulerOutcome);
    }

    [Fact]
    public void AbsentReceiptIsAnArtifactFailureEvenWithZeroExitCode()
    {
        var result = VstestTrxClassifier.Classify(Facts(), VstestTrxClassifier.Parse(null));
        Assert.Equal(VstestDisposition.ArtifactFailure, result.Disposition);
        Assert.Equal(VstestArtifactHealth.Missing, result.Artifact.Health);
    }

    [Theory]
    [InlineData("<TestRun")]
    [InlineData("<!DOCTYPE TestRun [<!ENTITY x SYSTEM 'file:///etc/passwd'>]><TestRun>&x;</TestRun>")]
    [InlineData("<TestRun xmlns='urn:wrong' id='01234567-89ab-cdef-0123-456789abcdef' />")]
    [InlineData("<TestRun xmlns='http://microsoft.com/schemas/VisualStudio/TeamTest/2010' id='not-a-guid' />")]
    public void InvalidXmlNeverResolvesTheAttempt(string xml)
    {
        var result = Classify(Encoding.UTF8.GetBytes(xml), Facts());
        Assert.NotEqual(VstestArtifactHealth.Complete, result.Artifact.Health);
        Assert.Equal(VstestDisposition.ArtifactFailure, result.Disposition);
        Assert.Null(result.SchedulerOutcome);
    }

    [Fact]
    public void EmptyFileIsMalformedRatherThanAbsent()
    {
        var parsed = VstestTrxClassifier.Parse([]);
        Assert.Equal(VstestArtifactHealth.Malformed, parsed.Health);
        Assert.Equal(Convert.ToHexString(SHA256.HashData(Array.Empty<byte>())), parsed.Sha256);
    }

    [Fact]
    public void ExcessiveXmlBytesAreRejectedBeforeParsing()
    {
        var input = new byte[VstestTrxClassifier.MaxArtifactBytes + 1];
        var result = VstestTrxClassifier.Parse(input);
        Assert.Equal(VstestArtifactHealth.Oversized, result.Health);
        Assert.Equal((long)input.Length, result.ByteLength);
        Assert.Null(result.Sha256);
    }

    [Theory]
    [InlineData("2147483648", "1", "1", "0")]
    [InlineData("100001", "1", "1", "0")]
    [InlineData("2", "3", "1", "0")]
    [InlineData("2", "1", "2", "0")]
    [InlineData("2", "2", "1", "2")]
    [InlineData("-1", "0", "0", "0")]
    [InlineData("xyz", "0", "0", "0")]
    public void ImpossibleAndOverflowedCountersAreRejected(string total, string executed, string passed, string failed)
    {
        var result = Classify(TrxWithCounters("Completed",
            $"total=\"{total}\" executed=\"{executed}\" passed=\"{passed}\" failed=\"{failed}\""), Facts());
        Assert.Equal(VstestArtifactHealth.Inconsistent, result.Artifact.Health);
        Assert.Equal(VstestDisposition.ArtifactFailure, result.Disposition);
    }

    [Fact]
    public void UnknownNonzeroCounterCannotBeSilentlyTreatedAsPassing()
    {
        var result = Classify(TrxWithCounters("Completed",
            "total=\"2\" executed=\"1\" passed=\"1\" failed=\"0\" timeout=\"1\""), Facts());
        Assert.NotEqual(VstestDisposition.Passed, result.Disposition);
    }

    [Fact]
    public void ExplicitPassedEntryCannotContradictZeroPassedCounter()
    {
        var result = Classify(Trx("Completed", 1, 0, 0, 0,
            results: "<UnitTestResult testName=\"Contradiction\" outcome=\"Passed\" />"), Facts());
        Assert.Equal(VstestArtifactHealth.Inconsistent, result.Artifact.Health);
    }

    [Fact]
    public void ACompletedSummaryCannotClaimFailedTests()
    {
        var result = Classify(Trx("Completed", 1, 1, 0, 1), Facts());
        Assert.Equal(VstestArtifactHealth.Inconsistent, result.Artifact.Health);
    }

    [Fact]
    public void RunLevelErrorIsNotConfusedWithTreatNoTestsAsError()
    {
        var result = Classify(Trx("Failed", 0, 0, 0, 0,
            runInfo: "<RunInfos><RunInfo outcome=\"Error\"><Text>bad adapter</Text></RunInfo></RunInfos>"),
            Facts(exitCode: 1));
        Assert.Equal(VstestDisposition.RunnerFailure, result.Disposition);
    }

    [Theory]
    [InlineData("Failed", 1, 1, 1, 0)]
    [InlineData("Error", 1, 1, 1, 0)]
    [InlineData("Aborted", 1, 1, 1, 0)]
    public void UnsupportedRunOutcomesCannotResolveCleanPass(string outcome, int total,
        int executed, int passed, int failed)
    {
        var result = Classify(Trx(outcome, total, executed, passed, failed), Facts());
        Assert.Equal(VstestDisposition.RunnerFailure, result.Disposition);
        Assert.Null(result.SchedulerOutcome);
    }

    [Fact]
    public void AnIncompleteProcessCannotBeResolvedFromTrxAlone()
    {
        var result = Classify(Trx("Completed", 1, 1, 1, 0), Facts(exited: false, exitCode: null));
        Assert.Equal(VstestDisposition.Incomplete, result.Disposition);
    }

    [Fact]
    public void ContradictoryTerminationSignalsAreRejected()
    {
        var result = VstestTrxClassifier.Classify(Facts() with { TimedOut = true, Cancelled = true },
            VstestTrxClassifier.Parse(null));
        Assert.Equal(VstestArtifactHealth.Inconsistent, result.Artifact.Health);
    }

    [Fact]
    public void OldMtimeAndUnownedDirectoryAreRejectedAsStale()
    {
        var artifact = Trx("Completed", 1, 1, 1, 0);
        foreach (var process in new[]
        {
            Facts() with { ArtifactLastWriteUtc = Started.AddSeconds(-1) },
            Facts() with { OwnedResultsDirectory = false },
            Facts() with { ArtifactLastWriteUtc = null }
        })
        {
            var result = Classify(artifact, process);
            Assert.Equal(VstestArtifactHealth.Stale, result.Artifact.Health);
            Assert.Equal(VstestDisposition.ArtifactFailure, result.Disposition);
        }
    }

    [Fact]
    public void TrxFromAnEarlierAttemptIsRejectedDespiteFreshFileTimestamp()
    {
        var previous = Trx("Completed", 1, 1, 1, 0, times:
            "<Times start=\"2026-01-01T12:00:00Z\" finish=\"2026-01-01T12:00:01Z\" />");
        var result = Classify(previous, Facts());
        Assert.Equal(VstestArtifactHealth.Stale, result.Artifact.Health);
    }

    [Fact]
    public void FutureTrxCompletionAndReversedXmlTimesAreRejected()
    {
        var result = Classify(Trx("Completed", 1, 1, 1, 0, times:
            "<Times start=\"2026-01-02T12:00:02Z\" finish=\"2026-01-02T12:00:20Z\" />"), Facts());
        Assert.Equal(VstestArtifactHealth.Stale, result.Artifact.Health);
        result = Classify(Trx("Completed", 1, 1, 1, 0, times:
            "<Times start=\"2026-01-02T12:00:04Z\" finish=\"2026-01-02T12:00:01Z\" />"), Facts());
        Assert.Equal(VstestArtifactHealth.Inconsistent, result.Artifact.Health);
    }

    [Fact]
    public void MultipleResultSummariesAreRejected()
    {
        var xml = Encoding.UTF8.GetString(Trx("Completed", 0, 0, 0, 0));
        var duplicate = "<ResultSummary outcome=\"Completed\"><Counters total=\"0\" executed=\"0\" passed=\"0\" failed=\"0\" /></ResultSummary>";
        var result = Classify(Encoding.UTF8.GetBytes(xml.Replace("</TestRun>", duplicate + "</TestRun>", StringComparison.Ordinal)), Facts());
        Assert.Equal(VstestArtifactHealth.Inconsistent, result.Artifact.Health);
    }

    [Fact]
    public void PerCaseExcerptsAreBoundedAndNotTreatedAsIds()
    {
        var cases = string.Concat(Enumerable.Repeat(
            $"<UnitTestResult testName=\"{new string('N', 300)}\" outcome=\"Failed\"><Output><ErrorInfo><Message>{new string('E', 600)}</Message></ErrorInfo></Output></UnitTestResult>", 12));
        var parsed = VstestTrxClassifier.Parse(Trx("Failed", 12, 12, 0, 12, results: cases));
        Assert.Equal(VstestArtifactHealth.Complete, parsed.Health);
        Assert.Equal(VstestTrxClassifier.MaxFailureSamples, parsed.Failures.Count);
        Assert.All(parsed.Failures, item =>
        {
            Assert.Equal(160, item.DisplayName.Length);
            Assert.Equal(256, item.Message!.Length);
        });
    }

    [Fact]
    public void HistoricalReceiptCanIncludeSkippedResultOutsideExecutedTotal()
    {
        // A public 2012 TRX sample from allure2 contains total=4/executed=4,
        // three Passed results, one Failed, plus a NotExecuted result.
        // Treat the aggregate failed test as definitive; do not equate the
        // number of result elements with the logger's summary counters.
        var entries = string.Concat(
            "<UnitTestResult testName=\"AddingSeveralNumbers_40\" outcome=\"Passed\" />",
            "<UnitTestResult testName=\"AddingSeveralNumbers_60\" outcome=\"Passed\" />",
            "<UnitTestResult testName=\"AddTwoNumbers\" outcome=\"Passed\" />",
            "<UnitTestResult testName=\"SkippedTest\" outcome=\"NotExecuted\" />",
            "<UnitTestResult testName=\"FailToAddTwoNumbers\" outcome=\"Failed\" />");
        var result = Classify(Trx("Failed", 4, 4, 3, 1, results: entries), Facts(exitCode: 1));
        Assert.Equal(VstestDisposition.Failed, result.Disposition);
        Assert.Equal("FailToAddTwoNumbers", Assert.Single(result.Artifact.Failures).DisplayName);
    }

    [Fact]
    public void ValidSkippedResultDoesNotNeedNotExecutedSummaryCounter()
    {
        var result = Classify(Trx("Completed", 2, 0, 0, 0,
            results: "<UnitTestResult testName=\"Skip\" outcome=\"NotExecuted\" />"), Facts());
        Assert.Equal(VstestDisposition.AllSkipped, result.Disposition);
    }

    private static VstestClassification Classify(byte[] bytes, VstestProcessFacts process) =>
        VstestTrxClassifier.Classify(process, VstestTrxClassifier.Parse(bytes));

    private static VstestProcessFacts Facts(int? exitCode = 0, bool exited = true) =>
        new(42, Started, Started.AddSeconds(6), true, Started.AddSeconds(3), exited, exitCode);

    private static byte[] Trx(string outcome, int total, int executed, int passed, int failed,
        string? results = null, string? runInfo = null, string? times = null) =>
        TrxWithCounters(outcome, $"total=\"{total}\" executed=\"{executed}\" passed=\"{passed}\" failed=\"{failed}\"",
            results, runInfo, times);

    private static byte[] TrxWithCounters(string outcome, string counters, string? results = null,
        string? runInfo = null, string? times = null) => Encoding.UTF8.GetBytes($"""
        <?xml version="1.0" encoding="utf-8"?>
        <TestRun id="{RunId}" xmlns="http://microsoft.com/schemas/VisualStudio/TeamTest/2010">
          {times ?? "<Times start=\"2026-01-02T12:00:01Z\" finish=\"2026-01-02T12:00:02Z\" />"}
          <Results>{results}</Results>
          <ResultSummary outcome="{outcome}">
            <Counters {counters} />
            {runInfo}
          </ResultSummary>
        </TestRun>
        """);
}
