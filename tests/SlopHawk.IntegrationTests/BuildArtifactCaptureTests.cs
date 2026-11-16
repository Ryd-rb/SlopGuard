using System.Security.Cryptography;
using System.Text;
using SlopHawk.Daemon;
using Xunit;

namespace SlopHawk.IntegrationTests;

public sealed class BuildArtifactCaptureTests
{
    [Fact]
    public void EquivalentByteRecapturePreservesOrderedInputsLinksAndLengths()
    {
        using var data = new Fixture();
        var first = data.Capture();
        var again = data.Capture();

        Assert.True(first.Stamp.Complete, string.Join(", ", first.Stamp.Problems));
        Assert.True(again.Stamp.Complete);
        Assert.True(BuildOutputIdentity.Compare(first.Stamp, again.Stamp).Reusable);
        Assert.Equal(BuildCollectionMode.LiveCheckout, data.CollectionMode);
        Assert.Equal(data.Manifest.OrderedCompileInputs!.Select(file => file.Path),
            first.Evidence.Where(file => file.Field.StartsWith("capture.compile[", StringComparison.Ordinal))
                .Select(file => file.LogicalPath));
        Assert.All(first.Evidence, file =>
        {
            Assert.Equal(BuildArtifactObservationStatus.ByteVerified, file.Status);
            var bytes = File.ReadAllBytes(file.PhysicalPath!);
            Assert.Equal(bytes.LongLength, file.Length);
            Assert.Equal(Convert.ToHexString(SHA256.HashData(bytes)), file.Sha256);
            Assert.NotNull(file.LastWriteUtc);
        });
        Assert.Contains(data.Manifest.OrderedCompileInputs!, file =>
            file.Path == "src/Linked.cs" && file.Link == "virtual/Linked.cs");
        Assert.Contains(first.Evidence, file => file.LogicalPath == "out/Tests.pdb");
        Assert.Contains(first.Evidence, file => file.LogicalPath == "out/Tests.deps.json");
    }

    [Theory]
    [InlineData("src/First.cs", "compile")]
    [InlineData("src/Linked.cs", "compile")]
    [InlineData("ref/Dependency.dll", "reference-assemblies")]
    [InlineData("assets/data.json", "runtime-assets")]
    [InlineData("settings/test.runsettings", "settings")]
    [InlineData("tools/testadapter.dll", "adapter")]
    [InlineData("out/Tests.dll", "outputs")]
    [InlineData("out/Tests.pdb", "outputs")]
    [InlineData("out/Tests.deps.json", "outputs")]
    public void ChangingRealBytesInvalidatesSameTargetReuse(string path, string section)
    {
        using var data = new Fixture();
        var before = data.Capture();
        Assert.True(before.Stamp.Complete);
        var file = data.Physical(path);
        var oldTimestamp = File.GetLastWriteTimeUtc(file);
        // A different byte sequence with exactly the same length AND timestamp is still different.
        var bytes = File.ReadAllBytes(file);
        bytes[0] ^= 0x1f;
        File.WriteAllBytes(file, bytes);
        File.SetLastWriteTimeUtc(file, oldTimestamp);

        var after = data.Capture();
        Assert.True(after.Stamp.Complete, string.Join(", ", after.Stamp.Problems));
        var compare = BuildOutputIdentity.Compare(before.Stamp, after.Stamp);
        Assert.False(compare.Reusable);
        Assert.Contains(compare.Problems, p => p.Kind == BuildIdentityProblemKind.Mismatch && p.Field == section);
    }

    [Fact]
    public void ReorderingCompilationInputsChangesIdentityButNotMembership()
    {
        using var data = new Fixture();
        var before = data.Capture();
        var modified = data.Manifest with
        {
            OrderedCompileInputs = data.Manifest.OrderedCompileInputs!.Reverse().ToArray()
        };
        var after = BuildArtifactCapture.Capture(modified, data.Scope);
        Assert.True(after.Stamp.Complete, string.Join(", ", after.Stamp.Problems));
        Assert.Contains(BuildOutputIdentity.Compare(before.Stamp, after.Stamp).Problems,
            p => p.Field == "compile" && p.Kind == BuildIdentityProblemKind.Mismatch);
    }

    [Fact]
    public void MissingRequiredFileIsIncompleteWithAnExplicitObservation()
    {
        using var data = new Fixture();
        var before = data.Capture();
        File.Delete(data.Physical("out/Tests.deps.json"));
        var after = data.Capture();
        Assert.False(after.Stamp.Complete);
        Assert.Contains(after.Evidence, item => item.LogicalPath == "out/Tests.deps.json" &&
            item.Status == BuildArtifactObservationStatus.Missing);
        Assert.Contains(after.Stamp.Problems, p => p.Kind == BuildIdentityProblemKind.Incomplete &&
            p.Field.Contains("capture.outputs", StringComparison.Ordinal));
        Assert.False(BuildOutputIdentity.Compare(before.Stamp, after.Stamp).Reusable);
        Assert.False(BuildOutputIdentity.Compare(after.Stamp, after.Stamp).Reusable);
    }

    [Fact]
    public void AbsentDependencyClosureIsNotTreatedAsKnownEmpty()
    {
        using var data = new Fixture();
        var incomplete = BuildArtifactCapture.Capture(data.Manifest with { SettingsFiles = null }, data.Scope);
        Assert.False(incomplete.Stamp.Complete);
        Assert.Contains(incomplete.Stamp.Problems, p => p.Field == "settings.digest");
        Assert.False(BuildOutputIdentity.Compare(incomplete.Stamp, incomplete.Stamp).Reusable);
    }

    [Fact]
    public void ReadErrorsAreRecordedWithoutUsingPartialDigests()
    {
        using var data = new Fixture();
        var result = BuildArtifactCapture.Capture(data.Manifest, data.Scope,
            betweenValidationReads: path =>
            {
                if (path == data.Physical("src/First.cs"))
                    throw new IOException("Synthetic read failure");
            });
        Assert.False(result.Stamp.Complete);
        Assert.Contains(result.Evidence, file => file.LogicalPath == "src/First.cs" &&
            file.Status == BuildArtifactObservationStatus.Unreadable && file.Sha256 is null);
    }

    [Fact]
    public void AChangedOrReplacedFileDuringCaptureFailsClosed()
    {
        using var data = new Fixture();
        var changed = false;
        var result = BuildArtifactCapture.Capture(data.Manifest, data.Scope,
            betweenValidationReads: path =>
            {
                if (changed || path != data.Physical("src/First.cs")) return;
                changed = true;
                var replacement = Path.Combine(data.Root, "temporary-replacement");
                File.WriteAllText(replacement, "replacement bytes");
                File.Move(path, path + ".previous");
                File.Move(replacement, path);
            });
        Assert.False(result.Stamp.Complete);
        Assert.Contains(result.Evidence, file => file.LogicalPath == "src/First.cs" &&
            file.Status == BuildArtifactObservationStatus.ChangedDuringCapture);
        Assert.False(BuildOutputIdentity.Compare(result.Stamp, result.Stamp).Reusable);
    }

    [Fact]
    public void MutationBetweenReadsIsDetectedEvenWhenLengthAndLastWriteTimeAreUnchanged()
    {
        using var data = new Fixture();
        var mutated = false;
        var result = BuildArtifactCapture.Capture(data.Manifest, data.Scope,
            betweenValidationReads: path =>
            {
                if (mutated || path != data.Physical("src/First.cs")) return;
                mutated = true;
                var originalTime = File.GetLastWriteTimeUtc(path);
                var bytes = File.ReadAllBytes(path);
                bytes[0] ^= 0x20;
                File.WriteAllBytes(path, bytes);
                File.SetLastWriteTimeUtc(path, originalTime);
            });
        Assert.False(result.Stamp.Complete);
        Assert.Contains(result.Evidence, item => item.LogicalPath == "src/First.cs" &&
            item.Status == BuildArtifactObservationStatus.ChangedDuringCapture);
    }

    [Theory]
    [InlineData("../external.cs")]
    [InlineData("src/../First.cs")]
    [InlineData("/absolute.cs")]
    [InlineData("src//First.cs")]
    [InlineData("src/file. ")]
    public void InvalidLogicalNamesCannotBeRead(string logical)
    {
        using var data = new Fixture();
        var manifest = data.Manifest with
        {
            OrderedCompileInputs = [new BuildArtifactCompile(logical, null),
                data.Manifest.OrderedCompileInputs![1]]
        };
        var result = BuildArtifactCapture.Capture(manifest, data.Scope);
        Assert.False(result.Stamp.Complete);
        Assert.Contains(result.Evidence, item => item.Status == BuildArtifactObservationStatus.Denied);
        Assert.Contains(result.Stamp.Problems, p => p.Kind == BuildIdentityProblemKind.UntrustedPath);
    }

    [Fact]
    public void ScopeMustExplicitlyAuthorizeTheMappedPhysicalFile()
    {
        using var data = new Fixture();
        using var outside = new Fixture();
        var mapping = data.Scope.Mappings.Select(m => m.LogicalPath == "src/First.cs"
            ? m with { PhysicalPath = outside.Physical("src/First.cs") } : m).ToArray();
        var result = BuildArtifactCapture.Capture(data.Manifest,
            data.Scope with { Mappings = mapping });
        Assert.False(result.Stamp.Complete);
        Assert.Contains(result.Stamp.Problems, p => p.Kind == BuildIdentityProblemKind.UntrustedPath);
        Assert.Contains(result.Evidence, item => item.Status == BuildArtifactObservationStatus.Denied);
    }

    [Fact]
    public void UntrustedOrAmbiguousMappingNeverAllowsReuse()
    {
        using var data = new Fixture();
        var untrusted = BuildArtifactCapture.Capture(data.Manifest, data.Scope with { MappingTrusted = false });
        Assert.False(untrusted.Stamp.Complete);
        var duplicate = BuildArtifactCapture.Capture(data.Manifest, data.Scope with
        {
            Mappings = data.Scope.Mappings.Concat([data.Scope.Mappings[0]]).ToArray()
        });
        Assert.False(duplicate.Stamp.Complete);
        Assert.Contains(duplicate.Stamp.Problems, problem => problem.Kind == BuildIdentityProblemKind.UntrustedPath);
    }

    [Fact]
    public void SameBytesAtAnotherAuthorizedPhysicalPathChangeMappingIdentity()
    {
        using var data = new Fixture();
        var before = data.Capture();
        var copy = data.Physical("src/Another.cs");
        File.Copy(data.Physical("src/First.cs"), copy);
        var mapping = data.Scope.Mappings.Select(m => m.LogicalPath == "src/First.cs"
            ? m with { PhysicalPath = copy } : m).ToArray();
        var after = BuildArtifactCapture.Capture(data.Manifest, data.Scope with { Mappings = mapping });
        Assert.True(after.Stamp.Complete, string.Join(", ", after.Stamp.Problems));
        Assert.Contains(BuildOutputIdentity.Compare(before.Stamp, after.Stamp).Problems,
            problem => problem.Field == "mapping" && problem.Kind == BuildIdentityProblemKind.Mismatch);
    }

    [Fact]
    public void LogicalCaseIsExactAndPhysicalContainmentUsesHostFileSystemRules()
    {
        using var data = new Fixture();
        var wrongCase = data.Manifest with
        {
            OrderedCompileInputs = [new BuildArtifactCompile("src/first.cs", null),
                data.Manifest.OrderedCompileInputs![1]]
        };
        var result = BuildArtifactCapture.Capture(wrongCase, data.Scope);
        Assert.False(result.Stamp.Complete);
        Assert.Contains(result.Evidence, x => x.Status == BuildArtifactObservationStatus.Denied);

        if (!OperatingSystem.IsWindows())
        {
            var fakeRoot = Path.Combine(data.Root.ToUpperInvariant(), "src/First.cs");
            var altered = data.Scope.Mappings.Select(m => m.LogicalPath == "src/First.cs"
                ? m with { PhysicalPath = fakeRoot } : m).ToArray();
            Assert.False(BuildArtifactCapture.Capture(data.Manifest,
                data.Scope with { Mappings = altered }).Stamp.Complete);
        }
    }

    [Fact]
    public void LinkedLeafAndLinkedDirectoryAreRejectedEvenIfTheyPointInsideAnApprovedRoot()
    {
        using var data = new Fixture();
        var link = data.Physical("src/alias.cs");
        try { File.CreateSymbolicLink(link, data.Physical("src/First.cs")); }
        catch (Exception ex) when (ex is UnauthorizedAccessException or PlatformNotSupportedException or IOException)
        { return; /* This platform does not allow synthetic links. */ }
        var mapping = data.Scope.Mappings.Select(m => m.LogicalPath == "src/First.cs"
            ? m with { PhysicalPath = link } : m).ToArray();
        var result = BuildArtifactCapture.Capture(data.Manifest, data.Scope with { Mappings = mapping });
        Assert.False(result.Stamp.Complete);
        Assert.Contains(result.Evidence, file => file.Status == BuildArtifactObservationStatus.Denied);
    }

    [Fact]
    public void LinkedParentDirectoryCannotBypassExplicitMapping()
    {
        using var data = new Fixture();
        var alias = Path.Combine(data.Root, "alias-dir");
        try { Directory.CreateSymbolicLink(alias, Path.Combine(data.Root, "src")); }
        catch (Exception ex) when (ex is UnauthorizedAccessException or PlatformNotSupportedException or IOException)
        { return; }
        var alternate = Path.Combine(alias, "First.cs");
        var mappings = data.Scope.Mappings.Select(m => m.LogicalPath == "src/First.cs"
            ? m with { PhysicalPath = alternate } : m).ToArray();
        var result = BuildArtifactCapture.Capture(data.Manifest, data.Scope with { Mappings = mappings });
        Assert.False(result.Stamp.Complete);
        Assert.Contains(result.Evidence, file => file.Status == BuildArtifactObservationStatus.Denied);
    }

    [Fact]
    public void BudgetLimitsAndCancellationStopObservation()
    {
        using var data = new Fixture();
        var fileBound = BuildArtifactCapture.Capture(data.Manifest, data.Scope,
            new BuildArtifactCaptureLimits(MaxFileBytes: 1));
        Assert.False(fileBound.Stamp.Complete);
        Assert.Contains(fileBound.Evidence, item => item.Status == BuildArtifactObservationStatus.BudgetExceeded);
        var totalBound = BuildArtifactCapture.Capture(data.Manifest, data.Scope,
            new BuildArtifactCaptureLimits(MaxTotalBytes: 10));
        Assert.False(totalBound.Stamp.Complete);
        Assert.Contains(totalBound.Evidence, item => item.Status == BuildArtifactObservationStatus.BudgetExceeded);
        var recordBound = BuildArtifactCapture.Capture(data.Manifest, data.Scope,
            new BuildArtifactCaptureLimits(MaxRecords: 2));
        Assert.False(recordBound.Stamp.Complete);
        Assert.Contains(recordBound.Stamp.Problems, p => p.Kind == BuildIdentityProblemKind.Incomplete ||
            p.Kind == BuildIdentityProblemKind.UntrustedPath);
        using var cancellation = new CancellationTokenSource();
        cancellation.Cancel();
        Assert.Throws<OperationCanceledException>(() => BuildArtifactCapture.Capture(
            data.Manifest, data.Scope, cancellationToken: cancellation.Token));
    }

    private sealed class Fixture : IDisposable
    {
        private static readonly string EvaluationHash = new('A', 64);
        internal string Root { get; } = Path.Combine(Path.GetTempPath(), "sg-13-" + Guid.NewGuid().ToString("N"));
        internal BuildArtifactManifest Manifest { get; }
        internal BuildArtifactReadScope Scope { get; }
        internal BuildCollectionMode CollectionMode => Scope.ImmutableSnapshotVerified
            ? BuildCollectionMode.ImmutableSnapshot : BuildCollectionMode.LiveCheckout;

        internal Fixture()
        {
            Directory.CreateDirectory(Root);
            var names = new[]
            {
                "src/First.cs", "src/Linked.cs", "ref/Dependency.dll", "assets/data.json",
                "settings/test.runsettings", "tools/testadapter.dll", "out/Tests.dll", "out/Tests.pdb",
                "out/Tests.deps.json"
            };
            for (var i = 0; i < names.Length; i++)
            {
                var file = Physical(names[i]);
                Directory.CreateDirectory(Path.GetDirectoryName(file)!);
                File.WriteAllBytes(file, Encoding.UTF8.GetBytes("capture bytes " + i));
            }
            var target = new ProjectTarget("tests/Tests.csproj|net10.0", "tests/Tests.csproj", "net10.0",
                "Debug", "AnyCPU", "", "10.0.404", true,
                [new CompileMember("src/First.cs", null), new CompileMember("src/Linked.cs", "virtual/Linked.cs")],
                [], EvaluationHash);
            Manifest = new BuildArtifactManifest(target, true, "fixture-map/v1", "VSTest/17.14.1",
                "adapter/v1", [new BuildArtifactCompile("src/First.cs", null),
                    new BuildArtifactCompile("src/Linked.cs", "virtual/Linked.cs")],
                ["ref/Dependency.dll"], ["assets/data.json"], ["settings/test.runsettings"],
                ["tools/testadapter.dll"], ["out/Tests.dll", "out/Tests.pdb", "out/Tests.deps.json"]);
            Scope = new BuildArtifactReadScope([Root], names.Select(name =>
                new BuildArtifactPathMapping(name, Physical(name))).ToArray(), MappingTrusted: true);
        }
        internal string Physical(string name) => Path.Combine(Root,
            name.Replace('/', Path.DirectorySeparatorChar));
        internal BuildArtifactCaptureResult Capture() => BuildArtifactCapture.Capture(Manifest, Scope);
        public void Dispose() => Directory.Delete(Root, recursive: true);
    }
}
