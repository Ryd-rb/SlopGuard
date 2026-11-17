// To be linked into the existing SlopHawk.IntegrationTests project by the owner
// using LinkParity.targets. These are source-only differential reference cases
// against the pinned v1 C# import and receipt implementation.
using System;
using System.IO;
using System.Linq;
using System.Security.Cryptography;
using System.Text;
using SlopHawk.Daemon;
using Xunit;

namespace SlopHawk.IntegrationTests;

public sealed class SGMig13ParityTests
{
    private static readonly Guid Epoch = Guid.Parse("da8a8249-76b2-401e-a156-bce5db02cd9c");
    private static readonly DateTimeOffset Now = new(2026, 11, 16, 9, 0, 0, TimeSpan.Zero);
    private static readonly string A = new('A', 64);
    private static readonly string B = new('B', 64);
    private static readonly string C = new('C', 64);

    private static BuildIdentityInputs Build() => new(
        new ProjectTarget("tests/Unit.csproj|net10.0", "tests/Unit.csproj", "net10.0",
            "Debug", "AnyCPU", "", "10.0.201", true,
            [new CompileMember("tests/Alpha.cs", null),
                new CompileMember("tests/Beta.cs", "Shared/Beta.cs")],
            ["lib/Lib.csproj|net10.0"], A),
        true, "trusted-physical-map", true, BuildCollectionMode.ImmutableSnapshot,
        A, "vstest-1", "adapter/123",
        [new BuildCompileInput("tests/Alpha.cs", null, B),
            new BuildCompileInput("tests/Beta.cs", "Shared/Beta.cs", C)],
        [new BuildIdentityFile("ref/Lib.dll", B)],
        [new BuildIdentityFile("assets/config.json", C)],
        [new BuildDeclaredOutput("out/Unit.dll", A, BuildOutputObservation.ByteVerified)]);

    [Fact]
    public void BuildSectionsAndMismatchShareTheRustVector()
    {
        var input = Build();
        var original = BuildOutputIdentity.Capture(input);
        Assert.True(original.Complete);
        Assert.Equal(11, original.Sections.Count);
        Assert.Equal(64, original.Sha256.Length);
        Assert.True(BuildOutputIdentity.Compare(original, original).Reusable);
        var changed = input with
        {
            RuntimeAssets = [new BuildIdentityFile("assets/config.json", B)]
        };
        Assert.Contains(BuildOutputIdentity.Compare(original, BuildOutputIdentity.Capture(changed)).Problems,
            p => p.Kind == BuildIdentityProblemKind.Mismatch && p.Field == "runtime-assets");
        var invalid = input with { RuntimeAssets = null };
        Assert.False(BuildOutputIdentity.Compare(BuildOutputIdentity.Capture(invalid),
            BuildOutputIdentity.Capture(invalid)).Reusable);
        var altered = original with { Sections = original.Sections.Select((s,i) =>
            i == 0 ? s with { Sha256 = B } : s).ToArray() };
        Assert.False(BuildOutputIdentity.Compare(original, altered).Reusable);
    }

    private static byte[] Xml(string line = "<line number='12' hits='0' branch='true' condition-coverage='50% (1/2)'/>") =>
        Encoding.UTF8.GetBytes("<coverage lines-covered='0' lines-valid='1' branches-covered='1' branches-valid='2'>" +
            "<packages><package name='unit'><classes><class name='A' filename='src/A.cs'>" +
            "<methods><method name='M' signature='()'><lines><line number='12' hits='99'/></lines></method></methods>" +
            "<lines>" + line + "</lines></class></classes></package></packages></coverage>");

    private static (CoberturaReceipt Receipt, CoverageExpectedRun Expected) Bind(byte[] bytes,
        TestOutcome outcome = TestOutcome.Failed)
    {
        var aliases = new[] { new CoverageDocumentAlias("src/A.cs", "unit", "src/A.cs", A) };
        var receipt = new CoberturaReceipt(Epoch, 42, "unit", "build-v1", "binary-v1",
            "coverlet", "collector-v1", "sources-1", Now,
            Convert.ToHexString(SHA256.HashData(bytes)), aliases);
        var expected = new CoverageExpectedRun(Epoch, 42, "unit", "build-v1", "binary-v1",
            "coverlet", "collector-v1", "sources-1", Now.AddSeconds(-1),
            Now.AddSeconds(2), outcome, aliases);
        return (receipt, expected);
    }

    [Fact]
    public void FailedRunHasExactLineAndOnlyAggregateBranch()
    {
        var xml = Xml();
        var (receipt, expected) = Bind(xml);
        var result = CoberturaCoverageImporter.Import(xml, receipt, expected);
        Assert.Equal(CoberturaImportState.Imported, result.State);
        Assert.Equal(TestOutcome.Failed, Assert.IsType<CoberturaRunTotals>(result.Totals).Outcome);
        Assert.Equal(CoverageLineState.Uncovered, result.LineState("unit", "src/A.cs", 12, A));
        Assert.Equal(CoverageLineState.Unknown, result.LineState("unit", "src/A.cs", 12, B));
        Assert.Equal(0, Assert.Single(result.Lines).Hits); // method duplicate is not a separate sample
        var aggregate = Assert.Single(result.BranchAggregates);
        Assert.Equal(1, aggregate.Covered);
        Assert.Equal(2, aggregate.Valid);
    }

    [Theory]
    [InlineData("invalid")]
    [InlineData("duplicate")]
    [InlineData("dtd")]
    public void MalformedCoberturaCannotMintCounters(string kind)
    {
        var xml = kind switch
        {
            "invalid" => Xml("<line number='12' hits='0' branch='true' condition-coverage='100% (2/1)'/>"),
            "duplicate" => Xml("<line number='12' hits='0'/><line number='12' hits='1'/>"),
            _ => Encoding.UTF8.GetBytes("<!DOCTYPE coverage [<!ENTITY x SYSTEM 'file:///etc/passwd'>]><coverage/>")
        };
        var (receipt, expected) = Bind(xml);
        var result = CoberturaCoverageImporter.Import(xml, receipt, expected);
        Assert.Equal(CoberturaImportState.Malformed, result.State);
        Assert.Null(result.Totals);
        Assert.Empty(result.Lines);
    }

    [Fact]
    public void DigestBuildAndSourceMapChangesRejectSameXml()
    {
        var xml = Xml();
        var (receipt, expected) = Bind(xml);
        var alteredBytes = Xml("<line number='12' hits='1' branch='true' condition-coverage='50% (1/2)'/>");
        Assert.Equal(CoberturaImportState.Mismatched,
            CoberturaCoverageImporter.Import(alteredBytes, receipt, expected).State);
        Assert.Equal(CoberturaImportState.Mismatched,
            CoberturaCoverageImporter.Import(xml, receipt with { BuildIdentity = "stale" }, expected).State);
        Assert.Equal(CoberturaImportState.Mismatched,
            CoberturaCoverageImporter.Import(xml, receipt,
                expected with { Aliases = [new CoverageDocumentAlias("src/A.cs", "unit", "src/A.cs", B)] }).State);
        Assert.Equal(CoberturaImportState.Expired,
            CoberturaCoverageImporter.Import(xml, receipt with { CollectedAt = Now.AddSeconds(-2) }, expected).State);
    }

    private static TestRunReceiptDetails TestDetails() => new(
        42, 5, Epoch, 17, A, "root-1", "unit", "inputs", "profile", "lease",
        1, A, B, C, 100, Guid.Parse("00000000-0000-0000-0000-000000000001"),
        VstestDisposition.Failed, VstestRunnerHealth.NonzeroExit,
        VstestArtifactHealth.Complete, VstestExecution.Failed,
        new VstestTrxCounters(2, 2, 1, 1, 0, 0), "failed_tests");

    [Fact]
    public void ReceiptPayloadDispositionsAndHashesRejectContradictions()
    {
        TestDetails().ValidatePayload();
        Assert.Throws<IOException>(() => (TestDetails() with { BuildSha256 = "wrong" }).ValidatePayload());
        Assert.Throws<IOException>(() => (TestDetails() with { Counters =
            new VstestTrxCounters(2, 2, 1, 3, 0, 0) }).ValidatePayload());
        (TestDetails() with
        {
            Platform = VstestDisposition.ZeroDiscovered,
            Execution = VstestExecution.ZeroDiscovered,
            Counters = new VstestTrxCounters(0, 0, 0, 0, 0, 0)
        }).ValidatePayload();
        (TestDetails() with
        {
            Platform = VstestDisposition.AllSkipped,
            Execution = VstestExecution.AllSkipped,
            Counters = new VstestTrxCounters(2, 0, 0, 0, 2, 0)
        }).ValidatePayload();
        (TestDetails() with
        {
            Platform = VstestDisposition.Incomplete,
            Execution = VstestExecution.Unknown,
            Runner = VstestRunnerHealth.Incomplete,
            Artifact = VstestArtifactHealth.Missing,
            Counters = null
        }).ValidatePayload();
    }
}
