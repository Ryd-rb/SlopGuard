using System.Globalization;
using SlopGuard.Daemon;
using Xunit;

namespace SlopGuard.IntegrationTests;

public sealed class BuildOutputIdentityTests
{
    private static readonly string A = new('A', 64);
    private static readonly string B = new('B', 64);
    private static readonly string C = new('C', 64);
    private static readonly string D = new('D', 64);

    [Fact]
    public void IdenticalVerifiedObservationsCanBeReused()
    {
        var original = BuildOutputIdentity.Capture(Sample());
        var repeated = BuildOutputIdentity.Capture(Sample());

        Assert.Equal(BuildOutputIdentity.DomainVersion, original.DomainVersion);
        Assert.True(original.Complete);
        Assert.Empty(original.Problems);
        Assert.Equal(64, original.Sha256.Length);
        Assert.Equal(original.Sha256, repeated.Sha256);
        Assert.True(BuildOutputIdentity.Compare(original, repeated).Reusable);
    }

    [Theory]
    [InlineData("asset", "runtime-assets")]
    [InlineData("output", "outputs")]
    [InlineData("adapter", "adapter")]
    [InlineData("mapping", "mapping")]
    [InlineData("ordered-compile", "compile")]
    [InlineData("project-reference", "project-references")]
    [InlineData("referenced-assembly", "reference-assemblies")]
    [InlineData("settings", "settings")]
    [InlineData("runner", "runner")]
    [InlineData("sdk", "target")]
    public void ChangedBuildDependenciesInvalidateSourceIdenticalReuse(string change, string section)
    {
        var before = Sample();
        var after = change switch
        {
            "asset" => before with { RuntimeAssets = [new BuildIdentityFile("assets/data.json", D)] },
            "output" => before with
            {
                DeclaredOutputs = [new BuildDeclaredOutput("out/Tests.dll", D, BuildOutputObservation.ByteVerified)]
            },
            "adapter" => before with { AdapterIdentity = "xunit/3.2.0" },
            "mapping" => before with { PathMappingIdentity = "root-map/v2" },
            "ordered-compile" => before with { OrderedCompileInputs = before.OrderedCompileInputs!.Reverse().ToArray() },
            "project-reference" => before with { Target = before.Target with { References = ["src/NewLib.csproj|net10.0"] } },
            "referenced-assembly" => before with { ReferenceAssemblies = [new BuildIdentityFile("ref/Lib.dll", D)] },
            "settings" => before with { SettingsSha256 = D },
            "runner" => before with { RunnerIdentity = "VSTest/18.0" },
            "sdk" => before with { Target = before.Target with { SdkVersion = "10.0.405" } },
            _ => throw new ArgumentOutOfRangeException(nameof(change))
        };

        var oldStamp = BuildOutputIdentity.Capture(before);
        var newStamp = BuildOutputIdentity.Capture(after);
        var comparison = BuildOutputIdentity.Compare(oldStamp, newStamp);
        Assert.True(oldStamp.Complete);
        Assert.True(newStamp.Complete);
        Assert.NotEqual(oldStamp.Sha256, newStamp.Sha256);
        Assert.False(comparison.Reusable);
        Assert.Contains(comparison.Problems, reason =>
            reason.Kind == BuildIdentityProblemKind.Mismatch && reason.Field == section);
    }

    [Fact]
    public void UnorderedSetsHaveStableIdentityButCompileOrderIsSemantic()
    {
        var first = Sample();
        var other = first with
        {
            Target = first.Target with
            {
                Compile = first.Target.Compile.Reverse().ToArray(),
                References = ["src/Z.csproj|net10.0", "src/App.csproj|net10.0"]
            },
            ReferenceAssemblies = [new BuildIdentityFile("ref/Z.dll", B), new BuildIdentityFile("ref/Lib.dll", B)],
            RuntimeAssets = [new BuildIdentityFile("assets/z.json", C), new BuildIdentityFile("assets/data.json", C)],
            DeclaredOutputs =
            [
                new BuildDeclaredOutput("out/Z.pdb", A, BuildOutputObservation.ByteVerified),
                new BuildDeclaredOutput("out/Tests.dll", A, BuildOutputObservation.ByteVerified)
            ]
        };
        var reordered = other with
        {
            Target = other.Target with { References = other.Target.References.Reverse().ToArray() },
            ReferenceAssemblies = other.ReferenceAssemblies!.Reverse().ToArray(),
            RuntimeAssets = other.RuntimeAssets!.Reverse().ToArray(),
            DeclaredOutputs = other.DeclaredOutputs!.Reverse().ToArray()
        };

        var original = BuildOutputIdentity.Capture(other);
        var shuffled = BuildOutputIdentity.Capture(reordered);
        Assert.True(original.Complete);
        Assert.True(shuffled.Complete);
        Assert.Equal(original.Sha256, shuffled.Sha256);
        Assert.True(BuildOutputIdentity.Compare(original, shuffled).Reusable);
    }

    [Fact]
    public void EquivalentLogicalPathsAndDigestCaseAreCultureIndependent()
    {
        var first = Sample();
        var equivalent = first with
        {
            Target = first.Target with
            {
                Id = @"tests\Tests.csproj|net10.0",
                ProjectPath = "tests/Tests.csproj",
                Compile = [new CompileMember("tests/Alpha.cs", null), new CompileMember(@"tests\Beta.cs", "Shared/Beta.cs")],
                References = [@"src\App.csproj|net10.0"],
                EvaluationFingerprint = A.ToLowerInvariant()
            },
            OrderedCompileInputs =
            [
                new BuildCompileInput("tests/Alpha.cs", null, B.ToLowerInvariant()),
                new BuildCompileInput("tests/Beta.cs", @"Shared\Beta.cs", C.ToLowerInvariant())
            ],
            ReferenceAssemblies = [new BuildIdentityFile(@"ref\Lib.dll", B.ToLowerInvariant())],
            RuntimeAssets = [new BuildIdentityFile(@"assets\data.json", C.ToLowerInvariant())],
            DeclaredOutputs = [new BuildDeclaredOutput(@"out\Tests.dll", A.ToLowerInvariant(),
                BuildOutputObservation.ByteVerified)]
        };
        var saved = CultureInfo.CurrentCulture;
        try
        {
            CultureInfo.CurrentCulture = CultureInfo.GetCultureInfo("fr-FR");
            var left = BuildOutputIdentity.Capture(first);
            CultureInfo.CurrentCulture = CultureInfo.GetCultureInfo("tr-TR");
            var right = BuildOutputIdentity.Capture(equivalent);
            Assert.True(left.Complete);
            Assert.True(right.Complete);
            Assert.Equal(left.Sha256, right.Sha256);
        }
        finally
        {
            CultureInfo.CurrentCulture = saved;
        }
    }

    [Fact]
    public void UnknownInputsAreExplicitAndNeverReusableEvenAgainstThemselves()
    {
        var incomplete = Sample() with { RuntimeAssets = null, SettingsSha256 = null };
        var stamp = BuildOutputIdentity.Capture(incomplete);
        var comparison = BuildOutputIdentity.Compare(stamp, stamp);

        Assert.False(stamp.Complete);
        Assert.False(comparison.Reusable);
        Assert.Contains(stamp.Problems, issue => issue.Kind == BuildIdentityProblemKind.Incomplete &&
            issue.Field == "runtime");
        Assert.Contains(stamp.Problems, issue => issue.Kind == BuildIdentityProblemKind.Incomplete &&
            issue.Field == "settings.digest");
    }

    [Theory]
    [InlineData((int)BuildOutputObservation.ExpectedOnly)]
    [InlineData((int)BuildOutputObservation.Unavailable)]
    public void ExpectedOrMissingOutputsAreNotByteVerified(int observation)
    {
        var input = Sample() with
        {
            DeclaredOutputs = [new BuildDeclaredOutput("out/Tests.dll", A, (BuildOutputObservation)observation)]
        };
        var stamp = BuildOutputIdentity.Capture(input);
        Assert.False(stamp.Complete);
        Assert.False(BuildOutputIdentity.Compare(stamp, stamp).Reusable);
        Assert.Contains(stamp.Problems, issue => issue.Field == "outputs.not_byte_verified" &&
            issue.Kind == BuildIdentityProblemKind.Incomplete);
    }

    [Theory]
    [InlineData("../outside/A.cs")]
    [InlineData("/absolute/A.cs")]
    [InlineData(@"C:\root\A.cs")]
    [InlineData("tests//A.cs")]
    [InlineData("tests/bad. /A.cs")]
    public void PathsEscapingLogicalWorkspaceAreUntrusted(string path)
    {
        var input = Sample();
        var stamp = BuildOutputIdentity.Capture(input with
        {
            OrderedCompileInputs = [new BuildCompileInput(path, null, B), input.OrderedCompileInputs![1]]
        });
        Assert.False(stamp.Complete);
        Assert.Contains(stamp.Problems, issue => issue.Kind == BuildIdentityProblemKind.UntrustedPath);
    }

    [Fact]
    public void PartialProjectGraphFailsClosed()
    {
        var stamp = BuildOutputIdentity.Capture(Sample() with { ProjectGraphComplete = false });
        Assert.False(stamp.Complete);
        Assert.Contains(stamp.Problems, issue => issue.Field == "target.graph" &&
            issue.Kind == BuildIdentityProblemKind.Incomplete);
    }

    [Fact]
    public void MissingCollectionProvenanceFailsClosed()
    {
        var stamp = BuildOutputIdentity.Capture(Sample() with { CollectionMode = BuildCollectionMode.Unknown });
        Assert.False(stamp.Complete);
        Assert.Contains(stamp.Problems, issue => issue.Field == "mapping.collection_mode" &&
            issue.Kind == BuildIdentityProblemKind.Incomplete);
    }

    [Fact]
    public void MissingTrustAndInconsistentEvaluatedMembershipFailClosed()
    {
        var input = Sample();
        var stamp = BuildOutputIdentity.Capture(input with
        {
            PathMappingTrusted = false,
            OrderedCompileInputs = [new BuildCompileInput("tests/Different.cs", null, B), input.OrderedCompileInputs![1]]
        });
        Assert.False(stamp.Complete);
        Assert.Contains(stamp.Problems, issue => issue.Kind == BuildIdentityProblemKind.UntrustedPath &&
            issue.Field == "mapping.trust");
        Assert.Contains(stamp.Problems, issue => issue.Kind == BuildIdentityProblemKind.Mismatch &&
            issue.Field == "compile.membership");
    }

    [Fact]
    public void InvalidHashesAndDuplicateDeclarationsFailClosed()
    {
        var input = Sample() with
        {
            DeclaredOutputs =
            [
                new BuildDeclaredOutput("out/Tests.dll", "not a digest", BuildOutputObservation.ByteVerified),
                new BuildDeclaredOutput(@"out\Tests.dll", B, BuildOutputObservation.ByteVerified)
            ]
        };
        var stamp = BuildOutputIdentity.Capture(input);
        Assert.False(stamp.Complete);
        Assert.Contains(stamp.Problems, issue => issue.Kind == BuildIdentityProblemKind.UnsupportedIdentity &&
            issue.Field == "outputs.duplicate");
        Assert.Contains(stamp.Problems, issue => issue.Kind == BuildIdentityProblemKind.UnsupportedIdentity &&
            issue.Field.StartsWith("outputs[", StringComparison.Ordinal));
    }

    [Fact]
    public void ReasonsAreBoundedAndEveryInvalidInputRemainsNonReusable()
    {
        var input = Sample() with
        {
            RuntimeAssets = Enumerable.Range(0, 100).Select(i =>
                new BuildIdentityFile($"assets/item{i}.dat", null)).ToArray()
        };
        var stamp = BuildOutputIdentity.Capture(input);
        Assert.False(stamp.Complete);
        Assert.Equal(BuildOutputIdentity.MaxProblems, stamp.Problems.Count);
        Assert.False(BuildOutputIdentity.Compare(stamp, stamp).Reusable);
    }

    [Fact]
    public void WrongDomainOrTamperedSectionCannotAuthorizeReuse()
    {
        var stamp = BuildOutputIdentity.Capture(Sample());
        foreach (var invalid in new[]
        {
            stamp with { DomainVersion = 0 },
            stamp with { Sha256 = D },
            stamp with { Sections = stamp.Sections.Reverse().ToArray() },
            stamp with { Complete = false }
        })
        {
            var comparison = BuildOutputIdentity.Compare(stamp, invalid);
            Assert.False(comparison.Reusable);
            Assert.Contains(comparison.Problems, issue =>
                issue.Kind == BuildIdentityProblemKind.UnsupportedIdentity);
        }
    }

    [Fact]
    public void LengthPrefixedSectionsDoNotConflateDifferentFieldBoundaries()
    {
        var left = Sample() with { RunnerIdentity = "ab", AdapterIdentity = "c" };
        var right = Sample() with { RunnerIdentity = "a", AdapterIdentity = "bc" };
        Assert.NotEqual(BuildOutputIdentity.Capture(left).Sha256, BuildOutputIdentity.Capture(right).Sha256);
    }

    [Fact]
    public void UnknownRidDiffersFromExplicitlyAbsentRid()
    {
        var valid = Sample();
        var unknown = valid with { Target = valid.Target with { RuntimeIdentifier = null! } };
        var observed = BuildOutputIdentity.Capture(unknown);
        Assert.False(observed.Complete);
        Assert.NotEqual(BuildOutputIdentity.Capture(valid).Sha256, observed.Sha256);
    }

    private static BuildIdentityInputs Sample() => new(
        new ProjectTarget("tests/Tests.csproj|net10.0", @"tests\Tests.csproj", "net10.0", "Debug", "AnyCPU", "",
            "10.0.404", true,
            [new CompileMember(@"tests\Alpha.cs", null), new CompileMember("tests/Beta.cs", "Shared/Beta.cs")],
            ["src/App.csproj|net10.0"], A),
        true, "root-map/v1", true, BuildCollectionMode.LiveCheckout, B, "VSTest/17.14.1", "xunit/3.1.5",
        [new BuildCompileInput(@"tests\Alpha.cs", null, B),
            new BuildCompileInput("tests/Beta.cs", "Shared/Beta.cs", C)],
        [new BuildIdentityFile("ref/Lib.dll", B)],
        [new BuildIdentityFile("assets/data.json", C)],
        [new BuildDeclaredOutput("out/Tests.dll", A, BuildOutputObservation.ByteVerified)]);
}
