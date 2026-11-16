using System.Security.Cryptography;
using System.Runtime.InteropServices;
using System.Text;
using SlopHawk.Daemon;
using Xunit;

namespace SlopHawk.IntegrationTests;

public sealed class CoverageReceiptCaptureTests
{
    private const string ArtifactPath = "run-guid/coverage.cobertura.xml";
    private static readonly Guid Epoch = Guid.Parse("5b9eb5db-cbc4-4dcb-ae1f-4f17f40238aa");

    [Fact]
    public void FreshLeasedArtifactIsBoundToAttemptAndImportedFromTheSameBytes()
    {
        using var fixture = new Fixture();
        var output = fixture.Capture();

        Assert.Equal(CoberturaImportState.Imported, output.Result.State);
        var receipt = Assert.IsType<CoberturaReceipt>(output.Receipt);
        Assert.Equal(Epoch, receipt.Epoch);
        Assert.Equal(23, receipt.AttemptId);
        Assert.Equal("build-1", receipt.BuildIdentity);
        Assert.Equal("binary-1", receipt.BinaryIdentity);
        Assert.Equal("collector-1", receipt.CollectorVersion);
        Assert.Equal("unit", receipt.Target);
        Assert.Equal(Convert.ToHexString(SHA256.HashData(fixture.Bytes)), receipt.ArtifactSha256);
        Assert.Equal("src/A.cs", Assert.Single(receipt.Aliases).SourcePath);
        Assert.Equal(CoverageLineState.Observed, output.Result.LineState("unit", "src/A.cs", 12));
        Assert.Equal(CoverageLineState.Unknown, output.Result.LineState("unit", "src/A.cs", 15));
        Assert.Equal(1, Assert.IsType<CoberturaRunTotals>(output.Result.Totals).LinesCovered);
    }

    [Fact]
    public void CompletedFailedRunStillSuppliesGenuineHitsWithoutInventingBranchArmProof()
    {
        using var fixture = new Fixture(branch: true);
        var output = fixture.Capture(fixture.Attempt with { Outcome = TestOutcome.Failed });

        Assert.Equal(CoberturaImportState.Imported, output.Result.State);
        var totals = Assert.IsType<CoberturaRunTotals>(output.Result.Totals);
        Assert.Equal(TestOutcome.Failed, totals.Outcome);
        Assert.Equal(CoverageLineState.Observed, output.Result.LineState("unit", "src/A.cs", 12));
        var aggregate = Assert.Single(output.Result.BranchAggregates);
        Assert.Equal((1, 2), (aggregate.Covered, aggregate.Valid));
        Assert.Equal(1, totals.BranchesCovered);
        Assert.Equal(2, totals.BranchesValid);
    }

    [Fact]
    public void StaleCopiedBytesAreNotAcceptedEvenWhenTheirXmlParses()
    {
        using var fixture = new Fixture();
        File.SetLastWriteTimeUtc(fixture.Artifact, fixture.Attempt.StartedAt.AddMinutes(-5).UtcDateTime);

        var output = fixture.Capture();

        Assert.Equal(CoberturaImportState.Expired, output.Result.State);
        Assert.Equal("coverage_artifact_not_fresh", output.Result.Reason);
        Assert.Null(output.Receipt);
    }

    [Fact]
    public void ReplacementOfAnOpenedPathCannotRedirectItsReceipt()
    {
        using var fixture = new Fixture();
        var output = fixture.Capture(afterRead: () =>
        {
            File.Move(fixture.Artifact, fixture.Artifact + ".previous");
            File.WriteAllBytes(fixture.Artifact, fixture.Bytes);
        });

        Assert.Equal(CoberturaImportState.Mismatched, output.Result.State);
        Assert.Equal("coverage_artifact_replaced_during_read", output.Result.Reason);
        Assert.Null(output.Receipt);
    }

    [Fact]
    public void InPlaceMutationDuringReadCannotKeepTheFirstDigest()
    {
        using var fixture = new Fixture();
        var output = fixture.Capture(afterRead: () =>
        {
            var altered = Encoding.UTF8.GetString(fixture.Bytes).Replace("hits=\"7\"", "hits=\"8\"");
            File.WriteAllText(fixture.Artifact, altered);
        });
        Assert.Equal(CoberturaImportState.Mismatched, output.Result.State);
        Assert.Null(output.Receipt);
    }

    [Fact]
    public void ReusedLeaseAndExpiredAttemptCannotProduceFreshEvidence()
    {
        using var fixture = new Fixture();
        Assert.Equal(CoberturaImportState.Mismatched, fixture.Capture(lease:
            fixture.Lease with { CreatedAt = fixture.Attempt.StartedAt.AddDays(-3) }).Result.State);
        Assert.Equal(CoberturaImportState.Malformed, fixture.Capture(
            fixture.Attempt with { Now = fixture.Attempt.StartedAt.AddDays(3) }).Result.State);
    }

    [Fact]
    public void OversizedPathAndAliasBudgetsAreRejectedBeforeReading()
    {
        using var fixture = new Fixture();
        Assert.Equal(CoberturaImportState.Malformed, fixture.Capture(
            declaration: new CoverageArtifactDeclaration(new string('a', 1025))).Result.State);
        var entries = Enumerable.Range(0, CoberturaCoverageImporter.MaxAliases + 1)
            .Select(index => new CoverageDocumentAlias("source/" + index, "unit", "src/A.cs",
                new string('A', 64))).ToArray();
        Assert.Equal(CoberturaImportState.Malformed, fixture.Capture(
            fixture.Attempt with { VerifiedAliases = entries }).Result.State);
    }

    [Theory]
    [InlineData("epoch")]
    [InlineData("attempt")]
    [InlineData("target")]
    [InlineData("build")]
    [InlineData("binary")]
    [InlineData("collector")]
    [InlineData("version")]
    [InlineData("sources")]
    public void EachIndependentLeaseIdentityMustMatch(string field)
    {
        using var fixture = new Fixture();
        var lease = field switch
        {
            "epoch" => fixture.Lease with { Epoch = Guid.NewGuid() },
            "attempt" => fixture.Lease with { AttemptId = 24 },
            "target" => fixture.Lease with { Target = "other" },
            "build" => fixture.Lease with { BuildIdentity = "other" },
            "binary" => fixture.Lease with { BinaryIdentity = "other" },
            "collector" => fixture.Lease with { Collector = "other" },
            "version" => fixture.Lease with { CollectorVersion = "other" },
            "sources" => fixture.Lease with { SourceIdentity = "other" },
            _ => throw new InvalidOperationException()
        };

        var output = fixture.Capture(lease: lease);
        Assert.Equal(CoberturaImportState.Mismatched, output.Result.State);
        Assert.Null(output.Receipt);
    }

    [Fact]
    public void CollectorAbsenceAndMissingOrUndeclaredReportAreIncompleteScope()
    {
        using var fixture = new Fixture();
        Assert.Equal("collector_unavailable", fixture.Capture(
            fixture.Attempt with { CollectorAvailable = false }).Result.Reason);
        Assert.Equal(CoberturaImportState.Missing, fixture.Capture(
            declaration: new CoverageArtifactDeclaration("run-guid/missing.xml")).Result.State);
        Assert.Equal(CoberturaImportState.Missing, CoverageReceiptCapture.Capture(
            fixture.Attempt, fixture.Lease, null).Result.State);
        Assert.Equal("results_lease_not_drained", fixture.Capture(lease:
            fixture.Lease with { OwnedAndDrained = false }).Result.Reason);
        Assert.Equal("test_run_not_completed", fixture.Capture(
            fixture.Attempt with { Completed = false }).Result.Reason);
        Assert.Equal("test_run_not_completed", fixture.Capture(
            fixture.Attempt with { Outcome = TestOutcome.Cancelled }).Result.Reason);
    }

    [Fact]
    public void MappingMustBeValidatedOutsideTheCollectorAndBelongToRunTarget()
    {
        using var fixture = new Fixture();
        var wrongTarget = fixture.Attempt with { VerifiedAliases =
            [new CoverageDocumentAlias("src/A.cs", "other", "src/A.cs", new string('A', 64))] };
        Assert.Equal(CoberturaImportState.Malformed, fixture.Capture(wrongTarget).Result.State);
        var badDigest = fixture.Attempt with { VerifiedAliases =
            [new CoverageDocumentAlias("src/A.cs", "unit", "src/A.cs", "bad-digest")] };
        Assert.Equal(CoberturaImportState.Malformed, fixture.Capture(badDigest).Result.State);
        var escapingSource = fixture.Attempt with { VerifiedAliases =
            [new CoverageDocumentAlias("src/A.cs", "unit", "../outside.cs", new string('A', 64))] };
        Assert.Equal(CoberturaImportState.Malformed, fixture.Capture(escapingSource).Result.State);
        var duplicateCollector = fixture.Attempt with { VerifiedAliases =
        [
            new("src/A.cs", "unit", "src/A.cs", new string('A', 64)),
            new("src/A.cs", "unit", "src/B.cs", new string('A', 64))
        ] };
        Assert.Equal(CoberturaImportState.Malformed, fixture.Capture(duplicateCollector).Result.State);
    }

    [Theory]
    [InlineData("../coverage.cobertura.xml")]
    [InlineData("run-guid/../../outside.xml")]
    [InlineData("/tmp/outside.xml")]
    [InlineData("run-guid/./coverage.cobertura.xml")]
    [InlineData("run-guid//coverage.cobertura.xml")]
    [InlineData("run-guid\\coverage.cobertura.xml")]
    [InlineData("C:/outside.xml")]
    public void PathEscapesAndNoncanonicalSegmentsAreRejected(string path)
    {
        using var fixture = new Fixture();
        Assert.Equal(CoberturaImportState.Malformed, fixture.Capture(
            declaration: new CoverageArtifactDeclaration(path)).Result.State);
    }

    [Fact]
    public void ALinkIsNotACollectorArtifact()
    {
        using var fixture = new Fixture();
        var link = Path.Combine(Path.GetDirectoryName(fixture.Artifact)!, "linked.xml");
        if (OperatingSystem.IsWindows())
            Assert.True(CreateHardLink(link, fixture.Artifact, IntPtr.Zero), $"CreateHardLinkW error {Marshal.GetLastWin32Error()}");
        else
            File.CreateSymbolicLink(link, fixture.Artifact);

        var output = fixture.Capture(declaration: new CoverageArtifactDeclaration("run-guid/linked.xml"));

        Assert.Equal(CoberturaImportState.Mismatched, output.Result.State);
        Assert.Null(output.Receipt);
    }

    [Fact]
    public void OverBudgetFileCannotEnterParserOrReceipt()
    {
        using var fixture = new Fixture();
        File.WriteAllBytes(fixture.Artifact, new byte[CoberturaCoverageImporter.MaxArtifactBytes + 1]);
        var output = fixture.Capture();
        Assert.Equal(CoberturaImportState.Malformed, output.Result.State);
        Assert.Null(output.Receipt);
    }

    [Fact]
    public void MalformedXmlCannotMintAReceipt()
    {
        using var fixture = new Fixture();
        File.WriteAllText(fixture.Artifact, "<coverage>");
        var output = fixture.Capture();
        Assert.Equal(CoberturaImportState.Malformed, output.Result.State);
        Assert.Null(output.Receipt);
    }

    [DllImport("kernel32.dll", EntryPoint = "CreateHardLinkW", ExactSpelling = true,
        CharSet = CharSet.Unicode, SetLastError = true)]
    [return: MarshalAs(UnmanagedType.Bool)]
    private static extern bool CreateHardLink(string path, string existingPath, IntPtr reserved);

    [Fact]
    public void XmlSourcePathsCannotAuthorizeAnotherReadOrCreateAnAlias()
    {
        using var fixture = new Fixture(xmlSource: "/etc/shadow", xmlFile: "untrusted/elsewhere.cs");
        var output = fixture.Capture();
        Assert.Equal(CoberturaImportState.UnmappedScope, output.Result.State);
        Assert.Equal("unmapped_alias", Assert.Single(output.Result.UnknownScope).Reason);
        Assert.Empty(output.Result.Lines);
        Assert.NotNull(output.Receipt);
    }

    private sealed class Fixture : IDisposable
    {
        public Fixture(bool branch = false, string xmlSource = "/nonexistent/ignored", string xmlFile = "src/A.cs")
        {
            var now = DateTimeOffset.UtcNow;
            Root = Path.Combine(Path.GetTempPath(), "sg18-coverage-" + Guid.NewGuid().ToString("N"));
            Directory.CreateDirectory(Path.Combine(Root, "run-guid"));
            Artifact = Path.Combine(Root, "run-guid", "coverage.cobertura.xml");
            Bytes = Encoding.UTF8.GetBytes($"""
                <coverage lines-covered="1" lines-valid="1" branches-covered="{(branch ? 1 : 0)}"
                    branches-valid="{(branch ? 2 : 0)}" version="1.9">
                  <sources><source>{xmlSource}</source></sources>
                  <packages><package name="unit"><classes><class name="A" filename="{xmlFile}">
                    <lines><line number="12" hits="7"{(branch ? " branch=\"true\" condition-coverage=\"50% (1/2)\"" : "")} /></lines>
                  </class></classes></package></packages>
                </coverage>
                """);
            File.WriteAllBytes(Artifact, Bytes);
            CoverageDocumentAlias[] mappings = [new("src/A.cs", "unit", "src/A.cs", new string('A', 64))];
            Attempt = new CoverageSupervisedAttempt(Epoch, 23, "unit", "build-1", "binary-1",
                "coverlet", "collector-1", "sources-1", now.AddMinutes(-1),
                now.AddSeconds(1), now.AddSeconds(2), TestOutcome.Passed,
                Completed: true, CollectorAvailable: true, mappings);
            Lease = new CoverageResultsLease(Epoch, 23, "unit", "build-1", "binary-1",
                "coverlet", "collector-1", "sources-1", Root,
                now.AddSeconds(-30), OwnedAndDrained: true);
        }

        public string Root { get; }
        public string Artifact { get; }
        public byte[] Bytes { get; }
        public CoverageSupervisedAttempt Attempt { get; }
        public CoverageResultsLease Lease { get; }
        public CoverageCapture Capture(CoverageSupervisedAttempt? attempt = null,
            CoverageResultsLease? lease = null, CoverageArtifactDeclaration? declaration = null,
            Action? afterRead = null) => CoverageReceiptCapture.Capture(attempt ?? Attempt,
                lease ?? Lease, declaration ?? new CoverageArtifactDeclaration(ArtifactPath), afterRead);

        public void Dispose() => Directory.Delete(Root, recursive: true);
    }
}
