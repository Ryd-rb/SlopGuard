using System.Security.Cryptography;
using System.Text;
using SlopHawk.Daemon;
using Xunit;

namespace SlopHawk.IntegrationTests;

public sealed class CoberturaCoverageImporterTests
{
    private static readonly Guid Epoch = Guid.Parse("da8a8249-76b2-401e-a156-bce5db02cd9c");
    private static readonly DateTimeOffset Now = new(2026, 11, 16, 9, 0, 0, TimeSpan.Zero);
    private static readonly string SourceDigest = new('A', 64);

    [Fact]
    public void FailedRunPreservesExactZeroCounterAndAggregateAttribution()
    {
        var bytes = Xml(methodLines: "<line number=\"12\" hits=\"99\" />");
        var (receipt, expected) = Bind(bytes, TestOutcome.Failed);

        var result = CoberturaCoverageImporter.Import(bytes, receipt, expected);

        Assert.Equal(CoberturaImportState.Imported, result.State);
        Assert.Equal("attributed", result.Reason);
        var line = Assert.Single(result.Lines);
        Assert.Equal(12, line.Number);
        Assert.Equal(0, line.Hits); // Class-level counter, not the method's duplicate.
        Assert.Equal(SourceDigest, line.SourceSha256);
        Assert.Equal(CoverageLineState.Uncovered, result.LineState("unit", "src/A.cs", 12));
        Assert.Equal(CoverageLineState.Unknown, result.LineState("unit", "src/A.cs", 13));
        Assert.Equal(CoverageLineState.Unknown, result.LineState("other", "src/A.cs", 12));
        var totals = Assert.IsType<CoberturaRunTotals>(result.Totals);
        Assert.Equal(TestOutcome.Failed, totals.Outcome);
        Assert.Equal(Epoch, totals.Epoch);
        Assert.Equal("collector-v1", totals.CollectorVersion);
        Assert.Equal(1, totals.LinesValid);
        Assert.Empty(result.BranchAggregates);
    }

    [Fact]
    public void ModifiedBytesDoNotMatchTheReceiptDigest()
    {
        var bytes = Xml();
        var (receipt, expected) = Bind(bytes);
        var altered = Encoding.UTF8.GetBytes(Encoding.UTF8.GetString(bytes).Replace("hits=\"0\"", "hits=\"1\""));

        var result = CoberturaCoverageImporter.Import(altered, receipt, expected);

        Assert.Equal(CoberturaImportState.Mismatched, result.State);
        Assert.Equal("artifact_digest_mismatch", result.Reason);
        Assert.Empty(result.Lines);
        Assert.Null(result.Totals);
    }

    [Theory]
    [InlineData("epoch")]
    [InlineData("attempt")]
    [InlineData("target")]
    [InlineData("build")]
    [InlineData("binary")]
    [InlineData("collector")]
    [InlineData("collector_version")]
    [InlineData("sources")]
    public void EveryReceiptIdentityIsRequired(string alteredField)
    {
        var bytes = Xml();
        var (receipt, expected) = Bind(bytes);
        receipt = alteredField switch
        {
            "epoch" => receipt with { Epoch = Guid.NewGuid() },
            "attempt" => receipt with { AttemptId = 88 },
            "target" => receipt with { Target = "other" },
            "build" => receipt with { BuildIdentity = "other" },
            "binary" => receipt with { BinaryIdentity = "other" },
            "collector" => receipt with { Collector = "other" },
            "collector_version" => receipt with { CollectorVersion = "other" },
            "sources" => receipt with { SourceIdentity = "other" },
            _ => throw new InvalidOperationException()
        };

        Assert.Equal(CoberturaImportState.Mismatched,
            CoberturaCoverageImporter.Import(bytes, receipt, expected).State);
    }

    [Fact]
    public void StaleOrFutureReceiptsAreExpired()
    {
        var bytes = Xml();
        var (receipt, expected) = Bind(bytes);
        Assert.Equal(CoberturaImportState.Expired,
            CoberturaCoverageImporter.Import(bytes,
                receipt with { CollectedAt = expected.StartedAt.AddSeconds(-1) }, expected).State);
        Assert.Equal(CoberturaImportState.Expired,
            CoberturaCoverageImporter.Import(bytes,
                receipt with { CollectedAt = expected.Now.AddMinutes(3) }, expected).State);
        Assert.Equal(CoberturaImportState.Expired,
            CoberturaCoverageImporter.Import(bytes, receipt,
                expected with { StartedAt = Now.AddDays(-3), Now = Now.AddDays(2) }).State);
    }

    [Fact]
    public void MissingReceiptOrArtifactNeverYieldsCoverage()
    {
        var bytes = Xml();
        var (receipt, expected) = Bind(bytes);
        Assert.Equal(CoberturaImportState.Missing,
            CoberturaCoverageImporter.Import(null, receipt, expected).State);
        Assert.Equal(CoberturaImportState.Missing,
            CoberturaCoverageImporter.Import(bytes, null, expected).State);
    }

    [Theory]
    [InlineData("")]
    [InlineData("%")]
    public void MalformedBranchCountersDoNotEscapeTheImporter(string condition)
    {
        var bytes = Xml(classLines: $"<line number='12' hits='0' branch='true' condition-coverage='{condition}' />");
        var (receipt, expected) = Bind(bytes);
        Assert.Equal(CoberturaImportState.Malformed,
            CoberturaCoverageImporter.Import(bytes, receipt, expected).State);
    }

    [Theory]
    [InlineData("")]
    [InlineData("<coverage")]
    [InlineData("<coverage />")]
    [InlineData("<!DOCTYPE coverage [<!ENTITY x SYSTEM 'file:///etc/passwd'>]><coverage />")]
    [InlineData("<coverage lines-covered='-1' lines-valid='0' branches-covered='0' branches-valid='0'><packages /></coverage>")]
    [InlineData("<coverage lines-covered='3' lines-valid='2' branches-covered='0' branches-valid='0'><packages /></coverage>")]
    [InlineData("<coverage lines-covered='0' lines-valid='1' branches-covered='0' branches-valid='0'><packages><package name='x'><classes><class name='A' filename='src/A.cs'><lines><line number='12' hits='-1'/></lines></class></classes></package></packages></coverage>")]
    public void EmptyCorruptDtdAndInvalidCountersAreMalformed(string content)
    {
        var bytes = Encoding.UTF8.GetBytes(content);
        var (receipt, expected) = Bind(bytes);
        Assert.Equal(CoberturaImportState.Malformed,
            CoberturaCoverageImporter.Import(bytes, receipt, expected).State);
    }

    [Fact]
    public void SizeAndRecordBudgetsAreEnforced()
    {
        var oversized = new byte[CoberturaCoverageImporter.MaxArtifactBytes + 1];
        var (oversizedReceipt, expected) = Bind(oversized);
        Assert.Equal(CoberturaImportState.Malformed,
            CoberturaCoverageImporter.Import(oversized, oversizedReceipt, expected).State);

        var bytes = Xml(classLines: "<line number=\"1000001\" hits=\"0\" />");
        var (receipt, context) = Bind(bytes);
        Assert.Equal(CoberturaImportState.Malformed,
            CoberturaCoverageImporter.Import(bytes, receipt, context).State);

        bytes = Xml(classLines: string.Concat(Enumerable.Range(1, CoberturaCoverageImporter.MaxLines + 1)
            .Select(n => $"<line number=\"{n}\" hits=\"0\" />")));
        (receipt, context) = Bind(bytes);
        Assert.Equal(CoberturaImportState.Malformed,
            CoberturaCoverageImporter.Import(bytes, receipt, context).State);
    }

    [Fact]
    public void DuplicatedClassCountersAndEscapingPathsAreRejected()
    {
        var duplicated = Xml(classLines:
            "<line number=\"12\" hits=\"0\" /><line number=\"12\" hits=\"1\" />");
        var (receipt, expected) = Bind(duplicated);
        Assert.Equal(CoberturaImportState.Malformed,
            CoberturaCoverageImporter.Import(duplicated, receipt, expected).State);

        var escaping = Xml(file: "../outside.cs");
        (receipt, expected) = Bind(escaping);
        Assert.Equal(CoberturaImportState.Malformed,
            CoberturaCoverageImporter.Import(escaping, receipt, expected).State);
    }

    [Fact]
    public void DuplicateSourceLineThroughTwoTrustedAliasesIsMalformed()
    {
        var content = Encoding.UTF8.GetString(Xml()).Replace("</class></classes>",
            "</class><class name=\"B\" filename=\"alternate/A.cs\"><lines>" +
            "<line number=\"12\" hits=\"5\" /></lines></class></classes>");
        var bytes = Encoding.UTF8.GetBytes(content);
        var (receipt, expected) = Bind(bytes);
        CoverageDocumentAlias[] aliases =
        [
            new("src/A.cs", "unit", "src/A.cs", SourceDigest),
            new("alternate/A.cs", "unit", "src/A.cs", SourceDigest)
        ];

        Assert.Equal(CoberturaImportState.Malformed,
            CoberturaCoverageImporter.Import(bytes, receipt with { Aliases = aliases },
                expected with { Aliases = aliases }).State);
    }

    [Fact]
    public void SourceDigestOrAliasChangedSinceReceiptWasWrittenIsMismatch()
    {
        var bytes = Xml();
        var (receipt, expected) = Bind(bytes);
        receipt = receipt with { Aliases =
            [new CoverageDocumentAlias("src/A.cs", "unit", "src/A.cs", new string('B', 64))] };
        Assert.Equal(CoberturaImportState.Mismatched,
            CoberturaCoverageImporter.Import(bytes, receipt, expected).State);
    }

    [Fact]
    public void AliasAmbiguityDoesNotClaimAHitOrMiss()
    {
        var bytes = Xml();
        var (receipt, expected) = Bind(bytes);
        CoverageDocumentAlias[] aliases =
        [
            new("src/A.cs", "unit", "src/A.cs", SourceDigest),
            new("src/A.cs", "unit", "src/Elsewhere.cs", new string('B', 64))
        ];
        expected = expected with { Aliases = aliases };
        receipt = receipt with { Aliases = aliases };

        var result = CoberturaCoverageImporter.Import(bytes, receipt, expected);
        Assert.Equal(CoberturaImportState.UnmappedScope, result.State);
        Assert.Equal("ambiguous_alias", Assert.Single(result.UnknownScope).Reason);
        Assert.Equal(CoverageLineState.Unknown, result.LineState("unit", "src/A.cs", 12));
        Assert.Empty(result.Lines);
        Assert.NotNull(result.Totals);
    }

    [Fact]
    public void UnmappedAndLinkedOtherTargetPathsRemainUnknown()
    {
        var bytes = Xml(file: "src/unmapped.cs");
        var (receipt, expected) = Bind(bytes);
        var result = CoberturaCoverageImporter.Import(bytes, receipt, expected);
        Assert.Equal(CoberturaImportState.UnmappedScope, result.State);
        Assert.Equal("unmapped_alias", Assert.Single(result.UnknownScope).Reason);
        Assert.Empty(result.Lines);

        bytes = Xml();
        (receipt, expected) = Bind(bytes);
        CoverageDocumentAlias[] otherTarget = [new("src/A.cs", "other", "src/A.cs", SourceDigest)];
        result = CoberturaCoverageImporter.Import(bytes,
            receipt with { Aliases = otherTarget }, expected with { Aliases = otherTarget });
        Assert.Equal(CoberturaImportState.UnmappedScope, result.State);
        Assert.Equal("other_target", Assert.Single(result.UnknownScope).Reason);
        Assert.Equal(CoverageLineState.Unknown, result.LineState("unit", "src/A.cs", 12));
    }

    [Fact]
    public void BranchTotalsAreAggregatesAndNeverIndividualArmProof()
    {
        var bytes = Xml(classLines:
            "<line number=\"12\" hits=\"7\" branch=\"true\" condition-coverage=\"50% (1/2)\">" +
            "<conditions><condition number=\"0\" type=\"jump\" coverage=\"50%\" /></conditions></line>",
            rootBranchCovered: 1, rootBranchValid: 2, rootLineCovered: 1);
        var (receipt, expected) = Bind(bytes, TestOutcome.Failed);

        var result = CoberturaCoverageImporter.Import(bytes, receipt, expected);

        Assert.Equal(CoberturaImportState.Imported, result.State);
        Assert.Equal(CoverageLineState.Observed, result.LineState("unit", "src/A.cs", 12));
        var aggregate = Assert.Single(result.BranchAggregates);
        Assert.Equal((1, 2), (aggregate.Covered, aggregate.Valid));
        var totals = Assert.IsType<CoberturaRunTotals>(result.Totals);
        Assert.Equal(1, totals.BranchesCovered);
        Assert.Equal(2, totals.BranchesValid);
        Assert.Equal(TestOutcome.Failed, totals.Outcome);
    }

    [Fact]
    public void ALineExplicitlyWithoutBranchesCannotSupplyAnAggregate()
    {
        var bytes = Xml(classLines: "<line number='12' hits='1' branch='false' condition-coverage='100% (2/2)' />");
        var (receipt, expected) = Bind(bytes);
        Assert.Equal(CoberturaImportState.Malformed,
            CoberturaCoverageImporter.Import(bytes, receipt, expected).State);
    }

    [Fact]
    public void AbsentInstrumentationNeverBecomesZeroHits()
    {
        var bytes = Xml(classLines: "");
        var (receipt, expected) = Bind(bytes);
        var result = CoberturaCoverageImporter.Import(bytes, receipt, expected);
        Assert.Equal(CoberturaImportState.Imported, result.State);
        Assert.Empty(result.Lines);
        Assert.Equal(CoverageLineState.Unknown, result.LineState("unit", "src/A.cs", 12));
    }

    private static (CoberturaReceipt Receipt, CoverageExpectedRun Expected) Bind(byte[] bytes,
        TestOutcome outcome = TestOutcome.Passed)
    {
        CoverageDocumentAlias[] aliases = [new("src/A.cs", "unit", "src/A.cs", SourceDigest)];
        var expected = new CoverageExpectedRun(Epoch, 42, "unit", "build-v1", "binary-v1", "coverlet",
            "collector-v1", "sources-v1", Now.AddMinutes(-5), Now, outcome, aliases);
        var receipt = new CoberturaReceipt(Epoch, 42, "unit", "build-v1", "binary-v1", "coverlet",
            "collector-v1", "sources-v1", Now.AddMinutes(-1),
            Convert.ToHexString(SHA256.HashData(bytes)), aliases);
        return (receipt, expected);
    }

    private static byte[] Xml(string file = "src/A.cs", string? classLines = null,
        string methodLines = "", int rootLineCovered = 0, int rootBranchCovered = 0,
        int rootBranchValid = 0)
    {
        classLines ??= "<line number=\"12\" hits=\"0\" />";
        return Encoding.UTF8.GetBytes($"""
            <coverage lines-covered="{rootLineCovered}" lines-valid="1"
                      branches-covered="{rootBranchCovered}" branches-valid="{rootBranchValid}"
                      line-rate="0" branch-rate="0" timestamp="0" version="1.9">
              <sources><source>/untrusted/ignored</source></sources>
              <packages><package name="unit" line-rate="0" branch-rate="0" complexity="0">
                <classes><class name="A" filename="{file}" line-rate="0" branch-rate="0" complexity="0">
                  <methods><method name="M" signature="()" line-rate="0" branch-rate="0" complexity="0">
                    <lines>{methodLines}</lines>
                  </method></methods>
                  <lines>{classLines}</lines>
                </class></classes>
              </package></packages>
            </coverage>
            """);
    }
}
