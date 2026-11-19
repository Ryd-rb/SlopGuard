using System.Security.Cryptography;
using System.Text;
using System.Text.Json;
using SlopHawk.Daemon;
using Xunit;

namespace SlopHawk.IntegrationTests;

// These fixtures use inert bytes and the existing physical collector; they never
// launch MSBuild or treat a settings entry as evaluated AdditionalFiles membership.
public sealed class AdditionalFileEvidenceTests
{
    private static readonly JsonSerializerOptions Web = new(JsonSerializerDefaults.Web);
    private static bool PhysicalCaptureSupported => OperatingSystem.IsLinux() || OperatingSystem.IsWindows();

    [Fact]
    public void WitnessRetentionIsOptInAndDoesNotChangeV1IdentityOrReuse()
    {
        if (!PhysicalCaptureSupported) return;
        using var data = new Fixture();
        var ordinary = data.Capture(retain: false);
        var retained = data.Capture();

        Assert.True(ordinary.Stamp.Complete, string.Join(", ", ordinary.Stamp.Problems));
        Assert.True(retained.Stamp.Complete);
        Assert.Null(ordinary.IdentityInputs);
        Assert.Null(ordinary.SettingsFiles);
        Assert.NotNull(retained.IdentityInputs);
        Assert.NotNull(retained.SettingsFiles);
        Assert.Equal(ordinary.Stamp.Sha256, retained.Stamp.Sha256);
        Assert.Equal(ordinary.Stamp.Sections.ToArray(), retained.Stamp.Sections.ToArray());
        Assert.Equal(ordinary.Stamp.Problems.ToArray(), retained.Stamp.Problems.ToArray());
        Assert.True(BuildOutputIdentity.Compare(ordinary.Stamp, retained.Stamp).Reusable);
        Assert.True(BuildOutputIdentity.Compare(retained.Stamp,
            BuildOutputIdentity.Capture(retained.IdentityInputs!)).Reusable);

        var missing = AdditionalFileEvidence.Export(ordinary, data.Identity, "post-build");
        Assert.Equal("capture_witness_unavailable", missing.Status);
        Assert.Null(missing.Witness);
    }

    [Fact]
    public void RetainedInputsDoNotBorrowMutableTargetMembershipOrReferences()
    {
        if (!PhysicalCaptureSupported) return;
        using var data = new Fixture();
        var retained = data.Capture();
        var before = BuildOutputIdentity.Capture(retained.IdentityInputs!);
        data.Compile[0] = new CompileMember("src/Changed.cs", "alias.cs");
        data.References[0] = "Other/Other.csproj|net10.0";

        Assert.Equal("src/Input.cs", retained.IdentityInputs!.Target.Compile[0].Path);
        Assert.Equal("Library/Library.csproj|net10.0", retained.IdentityInputs.Target.References[0]);
        Assert.True(BuildOutputIdentity.Compare(before,
            BuildOutputIdentity.Capture(retained.IdentityInputs)).Reusable);
        Assert.Equal("captured_settings_only",
            AdditionalFileEvidence.Export(retained, data.Identity, "post-build").Status);
    }

    [Fact]
    public void ExportPreservesTheFullSettingsWitnessAndUsesRustIdentityJson()
    {
        if (!PhysicalCaptureSupported) return;
        using var data = new Fixture();
        var capture = data.Capture();
        var export = AdditionalFileEvidence.Export(capture, data.Identity, "post-build");
        Assert.Equal("captured_settings_only", export.Status);
        Assert.True(export.Witness.HasValue);
        var wire = export.Witness.Value;

        Assert.Equal(AdditionalFileEvidence.Schema, wire.GetProperty("schema").GetString());
        Assert.Equal(AdditionalFileEvidence.Provider, wire.GetProperty("provider").GetString());
        Assert.Equal("evaluated_additional_files_unavailable",
            wire.GetProperty("observation_status").GetString());
        var inputs = wire.GetProperty("identity_inputs");
        Assert.Equal("live_checkout", inputs.GetProperty("collection_mode").GetString());
        Assert.Equal("byte_verified", inputs.GetProperty("declared_outputs")[0]
            .GetProperty("observation").GetString());
        Assert.Equal(data.Manifest.Target.ProjectPath,
            inputs.GetProperty("target").GetProperty("project_path").GetString());
        Assert.False(inputs.GetProperty("target").TryGetProperty("projectPath", out _));
        Assert.Equal(1, wire.GetProperty("stamp").GetProperty("domain_version").GetInt32());
        Assert.Equal(capture.Stamp.Sha256,
            wire.GetProperty("binding").GetProperty("build_sha256").GetString());
        Assert.Equal("post-build", wire.GetProperty("binding").GetProperty("capture_id").GetString());

        var rows = wire.GetProperty("settings_files").EnumerateArray().ToArray();
        Assert.Equal(new[] { "settings/a.bin", "settings/z.bin" },
            rows.Select(row => row.GetProperty("path").GetString()).ToArray());
        Assert.Equal(capture.SettingsFiles!.Count, rows.Length);
        foreach (var row in rows)
        {
            var path = row.GetProperty("path").GetString();
            var original = Assert.Single(capture.SettingsFiles!, file => file.LogicalPath == path);
            Assert.Equal(original.Sha256, row.GetProperty("sha256").GetString());
            Assert.Equal(original.Length, row.GetProperty("length").GetInt64());
            Assert.False(row.TryGetProperty("physical_path", out _));
            Assert.False(row.TryGetProperty("last_write_utc", out _));
        }
        Assert.True(Encoding.UTF8.GetByteCount(wire.GetRawText()) <= AdditionalFileEvidence.MaxWireBytes);
        Assert.Equal(wire.GetRawText(), AdditionalFileEvidence.Export(capture, data.Identity,
            "post-build").Witness!.Value.GetRawText());
    }

    [Fact]
    public void BinaryBytesArePreservedAndExportDoesNotReopenCapturedPaths()
    {
        if (!PhysicalCaptureSupported) return;
        using var data = new Fixture();
        var capture = data.Capture();
        var binary = Assert.Single(capture.SettingsFiles!, file => file.LogicalPath == "settings/a.bin");
        Assert.Equal(Convert.ToHexString(SHA256.HashData(Fixture.BinaryInput)), binary.Sha256);
        Assert.Equal(Fixture.BinaryInput.LongLength, binary.Length);
        var first = AdditionalFileEvidence.Export(capture, data.Identity, "post-build");
        data.DeleteCapturedFiles();
        var afterDeletion = AdditionalFileEvidence.Export(capture, data.Identity, "post-build");

        Assert.Equal("captured_settings_only", afterDeletion.Status);
        Assert.Equal(first.Witness!.Value.GetRawText(), afterDeletion.Witness!.Value.GetRawText());
    }

    [Fact]
    public void SameNameByteChangeAffectsSettingsWhileAnIrrelevantFileDoesNot()
    {
        if (!PhysicalCaptureSupported) return;
        using var data = new Fixture();
        var before = data.Capture();
        File.WriteAllText(Path.Combine(data.Root, "irrelevant.txt"), "not in the captured closure");
        var irrelevant = data.Capture();
        Assert.True(BuildOutputIdentity.Compare(before.Stamp, irrelevant.Stamp).Reusable);
        Assert.DoesNotContain(irrelevant.SettingsFiles!, item => item.LogicalPath == "irrelevant.txt");

        var path = data.Physical("settings/a.bin");
        var timestamp = File.GetLastWriteTimeUtc(path);
        var bytes = File.ReadAllBytes(path);
        bytes[0] ^= 0x10;
        File.WriteAllBytes(path, bytes);
        File.SetLastWriteTimeUtc(path, timestamp);
        var after = data.Capture();
        Assert.True(after.Stamp.Complete);
        Assert.NotEqual(before.IdentityInputs!.SettingsSha256, after.IdentityInputs!.SettingsSha256);
        Assert.Contains(BuildOutputIdentity.Compare(before.Stamp, after.Stamp).Problems,
            problem => problem.Kind == BuildIdentityProblemKind.Mismatch && problem.Field == "settings");
        Assert.Equal("evaluated_additional_files_unavailable", AdditionalFileEvidence.Export(after,
            data.Identity, "post-build").Witness!.Value.GetProperty("observation_status").GetString());
    }

    [Fact]
    public void MissingSettingsAreUnavailableButKnownEmptySettingsRemainAValidWitness()
    {
        if (!PhysicalCaptureSupported) return;
        using var data = new Fixture();
        var missing = BuildArtifactCapture.Capture(data.Manifest with { SettingsFiles = null },
            data.Scope, retainIdentityWitness: true);
        Assert.False(missing.Stamp.Complete);
        Assert.Null(missing.SettingsFiles);
        var absent = AdditionalFileEvidence.Export(missing, data.Identity, "post-build");
        Assert.Equal("capture_witness_unavailable", absent.Status);
        Assert.Null(absent.Witness);

        var empty = BuildArtifactCapture.Capture(data.Manifest with { SettingsFiles = [] },
            data.Scope, retainIdentityWitness: true);
        Assert.True(empty.Stamp.Complete);
        Assert.Empty(empty.SettingsFiles!);
        var knownEmpty = AdditionalFileEvidence.Export(empty, data.Identity, "post-build");
        Assert.Equal("captured_settings_only", knownEmpty.Status);
        Assert.Equal(0, knownEmpty.Witness!.Value.GetProperty("settings_files").GetArrayLength());
    }

    [Fact]
    public void WrongTargetAndInvalidCaptureBindingCannotExportAWitness()
    {
        if (!PhysicalCaptureSupported) return;
        using var data = new Fixture();
        var capture = data.Capture();
        foreach (var identity in new[]
        {
            data.Identity with { Context = "Other/Other.csproj|net10.0" },
            data.Identity with { CommittedRevision = -1 },
            data.Identity with { Epoch = Guid.Empty }
        })
        {
            var wrong = AdditionalFileEvidence.Export(capture, identity, "post-build");
            Assert.Equal("binding_mismatch", wrong.Status);
            Assert.Null(wrong.Witness);
        }
        var invalid = AdditionalFileEvidence.Export(capture, data.Identity, "post\nbuild");
        Assert.Equal("binding_mismatch", invalid.Status);
        Assert.Null(invalid.Witness);
    }

    [Fact]
    public void TruncatedOrTamperedSettingsCannotClaimTheOriginalAggregate()
    {
        if (!PhysicalCaptureSupported) return;
        using var data = new Fixture();
        var capture = data.Capture();
        var files = capture.SettingsFiles!.ToArray();
        var changed = files.ToArray();
        changed[0] = changed[0] with { Sha256 = new string('F', 64) };
        foreach (var altered in new[] { files[..1], changed, new[] { files[0], files[0] } })
        {
            var export = AdditionalFileEvidence.Export(capture with { SettingsFiles = altered },
                data.Identity, "post-build");
            Assert.Equal("binding_mismatch", export.Status);
            Assert.Null(export.Witness);
        }
        var malformed = files.ToArray();
        malformed[0] = malformed[0] with { Length = -1 };
        var bad = AdditionalFileEvidence.Export(capture with { SettingsFiles = malformed },
            data.Identity, "post-build");
        Assert.Equal("capture_witness_unavailable", bad.Status);
        Assert.Null(bad.Witness);
    }

    [Fact]
    public void RetainedInputsAndStampMustStillDescribeTheSameCapture()
    {
        if (!PhysicalCaptureSupported) return;
        using var data = new Fixture();
        var capture = data.Capture();
        var altered = capture with
        {
            IdentityInputs = capture.IdentityInputs! with { SettingsSha256 = new string('F', 64) }
        };
        var export = AdditionalFileEvidence.Export(altered, data.Identity, "post-build");
        Assert.Equal("binding_mismatch", export.Status);
        Assert.Null(export.Witness);
        Assert.True(BuildOutputIdentity.Compare(capture.Stamp, data.Capture().Stamp).Reusable);
    }

    [Fact]
    public void SupplementalBudgetFailureDoesNotModifyTheExistingStamp()
    {
        if (!PhysicalCaptureSupported) return;
        using var data = new Fixture();
        var capture = data.Capture();
        var oversized = capture with
        {
            SettingsFiles = Enumerable.Repeat(capture.SettingsFiles![0], 4097).ToArray()
        };
        var export = AdditionalFileEvidence.Export(oversized, data.Identity, "post-build");
        Assert.Equal("budget_exceeded", export.Status);
        Assert.Null(export.Witness);
        Assert.Same(capture.Stamp, oversized.Stamp);
        Assert.True(BuildOutputIdentity.Compare(capture.Stamp, data.Capture().Stamp).Reusable);
    }

    [Fact]
    public void DeniedPhysicalScopeRemainsIncompleteWithOrWithoutRetention()
    {
        using var data = new Fixture();
        var denied = data.Scope with { MappingTrusted = false };
        var ordinary = BuildArtifactCapture.Capture(data.Manifest, denied);
        var retained = BuildArtifactCapture.Capture(data.Manifest, denied, retainIdentityWitness: true);
        Assert.False(ordinary.Stamp.Complete);
        Assert.False(retained.Stamp.Complete);
        Assert.Equal(ordinary.Stamp.Sha256, retained.Stamp.Sha256);
        Assert.False(BuildOutputIdentity.Compare(ordinary.Stamp, retained.Stamp).Reusable);
        var export = AdditionalFileEvidence.Export(retained, data.Identity, "post-build");
        Assert.Equal("identity_incomplete", export.Status);
        Assert.Null(export.Witness);
    }

    [Fact]
    public void OldWorkJsonLeavesEvidenceDisabledAndDefaultSerializationUnchanged()
    {
        var old = JsonSerializer.Deserialize<NativeExecutionWork>(
            "{\"root\":\"workspace\",\"dotnetPath\":\"dotnet\"}", Web)!;
        Assert.False(old.IncludeAdditionalFileEvidence);
        var ordinary = JsonSerializer.SerializeToElement(old, Web);
        Assert.False(ordinary.TryGetProperty("includeAdditionalFileEvidence", out _));
        var requested = JsonSerializer.SerializeToElement(old with { IncludeAdditionalFileEvidence = true }, Web);
        Assert.True(requested.GetProperty("includeAdditionalFileEvidence").GetBoolean());
    }

    [Fact]
    public void OptionalAttachmentPreservesBuildDataAndDoesNotCrowdTheWorkerFrame()
    {
        if (!PhysicalCaptureSupported) return;
        using var data = new Fixture();
        var capture = data.Capture();
        var original = JsonSerializer.SerializeToElement(new
        {
            selectedTargetId = capture.TargetId, stamp = capture.Stamp, outputs = capture.Outputs
        }, Web);
        var attached = AdditionalFileEvidence.Attach(original, capture, data.Identity, "post-build");
        Assert.Equal(original.GetProperty("stamp").GetRawText(), attached.GetProperty("stamp").GetRawText());
        Assert.Equal(original.GetProperty("outputs").GetRawText(), attached.GetProperty("outputs").GetRawText());
        Assert.Equal("captured_settings_only", attached.GetProperty("additionalFileEvidenceStatus").GetString());
        Assert.Equal(AdditionalFileEvidence.Schema,
            attached.GetProperty("additionalFileEvidence").GetProperty("schema").GetString());

        var oldCapture = data.Capture(retain: false);
        var missing = AdditionalFileEvidence.Attach(original, oldCapture, data.Identity, "post-build");
        Assert.Equal("capture_witness_unavailable", missing.GetProperty("additionalFileEvidenceStatus").GetString());
        Assert.Equal(JsonValueKind.Null, missing.GetProperty("additionalFileEvidence").ValueKind);
        Assert.Equal(original.GetProperty("stamp").GetRawText(), missing.GetProperty("stamp").GetRawText());

        var large = JsonSerializer.SerializeToElement(new
        {
            existing = new string('x', NativeExecutionBridge.MaxFrameBytes / 2 + 1)
        }, Web);
        var unchanged = AdditionalFileEvidence.Attach(large, capture, data.Identity, "post-build");
        Assert.Equal(large.GetRawText(), unchanged.GetRawText());
        Assert.False(unchanged.TryGetProperty("additionalFileEvidence", out _));
    }

    private sealed class Fixture : IDisposable
    {
        internal static readonly byte[] BinaryInput = [0xff, 0x00, 0x80, 0x0d, 0x0a];
        private static readonly string[] Names = ["src/Input.cs", "settings/z.bin", "settings/a.bin", "out/Tests.dll"];
        internal string Root { get; } = Path.Combine(Path.GetTempPath(), "additional-evidence-" + Guid.NewGuid().ToString("N"));
        internal CompileMember[] Compile { get; } = [new("src/Input.cs", null)];
        internal string[] References { get; } = ["Library/Library.csproj|net10.0"];
        internal BuildArtifactManifest Manifest { get; }
        internal BuildArtifactReadScope Scope { get; }
        internal NativeExecutionIdentity Identity { get; }

        internal Fixture()
        {
            Directory.CreateDirectory(Root);
            foreach (var name in Names)
            {
                var path = Physical(name);
                Directory.CreateDirectory(Path.GetDirectoryName(path)!);
                File.WriteAllBytes(path, name == "settings/a.bin" ? BinaryInput : Encoding.UTF8.GetBytes("inert " + name));
            }
            var target = new ProjectTarget("Tests/Tests.csproj|net10.0", "Tests/Tests.csproj", "net10.0",
                "Debug", "AnyCPU", "", "10.0.100", true, Compile, References, new string('A', 64));
            Manifest = new BuildArtifactManifest(target, true, "fixture-map/v1", "fixture-runner/v1",
                "fixture-adapter/v1", [new BuildArtifactCompile("src/Input.cs", null)], [], [],
                ["settings/z.bin", "settings/a.bin"], [], ["out/Tests.dll"]);
            Scope = new BuildArtifactReadScope([Root], Names.Select(name =>
                new BuildArtifactPathMapping(name, Physical(name))).ToArray(), MappingTrusted: true);
            Identity = new NativeExecutionIdentity("fixture-build", Root, "fixture-root", target.Id,
                Guid.Parse("47000000-0000-0000-0000-000000000008"), 12, 3,
                new string('B', 64), new string('C', 64), new string('D', 64), new string('D', 64), "fixture-budget");
        }

        internal string Physical(string name) => Path.Combine(Root, name.Replace('/', Path.DirectorySeparatorChar));
        internal BuildArtifactCaptureResult Capture(bool retain = true) =>
            BuildArtifactCapture.Capture(Manifest, Scope, retainIdentityWitness: retain);
        internal void DeleteCapturedFiles()
        {
            foreach (var name in Names) File.Delete(Physical(name));
        }
        public void Dispose() => Directory.Delete(Root, recursive: true);
    }
}
