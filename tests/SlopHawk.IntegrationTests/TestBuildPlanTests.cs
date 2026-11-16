using System.Security.Cryptography;
using System.Text.Json;
using SlopHawk.Daemon;
using Xunit;

namespace SlopHawk.IntegrationTests;

public sealed class TestBuildPlanTests
{
    [Fact]
    public void ExactNoProcessPlanUsesConcreteMixedFrameworksAndPathsWithSpaces()
    {
        using var f = new Fixture();
        var library = f.Target("Library with spaces/Library.csproj", "net8.0", false,
            [new CompileMember("Shared folder/path with spaces.cs", "Linked/path with spaces.cs")]);
        var test = f.Target("Unit Tests/Unit Tests.csproj", "net10.0", true, [], library.Id);
        var graph = f.Graph(library, test);
        var result = f.Plan(graph, test.Id);
        Assert.True(result.Ready, $"{result.Blocker}: {result.Detail}");
        var plan = result.Plan!;
        Assert.Equal(new[] { library.Id, test.Id }, plan.InvokedTargets.Select(t => t.TargetId));
        Assert.Equal(new[] { "net8.0", "net10.0" }, plan.InvokedTargets.Select(t => t.Framework));
        Assert.Equal(new[] { "net8.0", "net10.0" }, plan.Assets.Select(a => a.AssetTarget));
        Assert.All(plan.Assets, a => Assert.Equal(64, a.Sha256.Length));
        Assert.Equal(ExecutionOperation.Build | ExecutionOperation.CompilerExtensions, plan.Requirements.Operations);
        Assert.True(plan.Requirements.UsesProcesses);
        Assert.Equal(f.ExpectedRoots(library, test), plan.Requirements.OutputRoots);
        Assert.Equal(f.Dotnet, plan.Command.Executable);
        Assert.Equal(Path.GetDirectoryName(f.ProjectPath(test)), plan.Command.WorkingDirectory);
        Assert.Equal(new[] { "exec", Path.Combine(Path.GetDirectoryName(f.Dotnet)!, "sdk", "10.0.404", "MSBuild.dll"),
            f.ProjectPath(test), "-t:Build", "-p:TargetFramework=net10.0", "-p:Configuration=Debug",
            "-p:Platform=AnyCPU", "-p:UseSharedCompilation=false",
            "-nologo", "-noAutoResponse", "-nodeReuse:false" }, plan.Command.Arguments);
        Assert.Equal(plan.Command.Arguments, plan.Command.ToStartInfo().ArgumentList.ToArray());
        Assert.All(plan.InvokedTargets, t => Assert.False(File.Exists(t.TargetAssembly)));
    }

    [Fact]
    public void RuntimeIdentifierIsExplicitlyPinnedToTheCommandAndAssets()
    {
        using var f = new Fixture();
        var test = f.Target("Tests/T.csproj", "net10.0", true) with { RuntimeIdentifier = "linux-x64" };
        f.WriteAssets(test);
        var result = f.Plan(f.Graph(test), test.Id);
        Assert.True(result.Ready, $"{result.Blocker}: {result.Detail}");
        Assert.Equal("net10.0/linux-x64", Assert.Single(result.Plan!.Assets).AssetTarget);
        Assert.Contains("-p:RuntimeIdentifier=linux-x64", result.Plan.Command.Arguments);
    }

    [Fact]
    public void AStaleSnapshotAndIncompleteGraphNeverBecomeReady()
    {
        using var f = new Fixture();
        var test = f.Target("Tests/T.csproj", "net10.0", true);
        var current = f.Graph(test);
        var snapshot = f.Snapshot(current);
        Assert.Equal(TestBuildBlocker.EvaluationUnavailable,
            f.Plan(new ProjectGraph(current.Targets, []), test.Id, snapshot).Blocker);
        var incomplete = new ProjectGraph([test], [new ProjectEvaluationIssue(test.ProjectPath, "unresolved_reference_target")]);
        Assert.Equal(TestBuildBlocker.IncompleteGraph, f.Plan(incomplete, test.Id).Blocker);
    }

    [Fact]
    public void UnresolvedOrCyclicProjectGraphEdgesBlockPreparation()
    {
        using var f = new Fixture();
        var dangling = f.Target("Tests/T.csproj", "net10.0", true, [], "Missing/Project.csproj|net10.0");
        Assert.Equal(TestBuildBlocker.UnresolvedReference, f.Plan(f.Graph(dangling), dangling.Id).Blocker);
        var a = f.Target("Tests/A.csproj", "net10.0", true, [], "Lib/B.csproj|net10.0");
        var b = f.Target("Lib/B.csproj", "net10.0", false, [], a.Id);
        Assert.Equal(TestBuildBlocker.UnresolvedReference, f.Plan(f.Graph(a, b), a.Id).Blocker);
    }

    [Fact]
    public void AssetsAreRequiredAndMustMatchFrameworkRidProjectAndMappedReferences()
    {
        using var f = new Fixture();
        var lib = f.Target("Lib/L.csproj", "net10.0", false);
        var test = f.Target("Tests/T.csproj", "net10.0", true, [], lib.Id);
        var graph = f.Graph(lib, test);
        File.Delete(f.AssetsPath(test));
        Assert.Equal(TestBuildBlocker.MissingAssets, f.Plan(graph, test.Id).Blocker);
        f.WriteAssets(test, assetTarget: "net8.0");
        Assert.Equal(TestBuildBlocker.IncompatibleAssets, f.Plan(graph, test.Id).Blocker);
        f.WriteAssets(test, projectPath: Path.Combine(f.Root, "Wrong.csproj"));
        Assert.Equal(TestBuildBlocker.IncompatibleAssets, f.Plan(graph, test.Id).Blocker);
        f.WriteAssets(test, references: []);
        Assert.Equal(TestBuildBlocker.IncompatibleAssets, f.Plan(graph, test.Id).Blocker);
    }

    [Fact]
    public void SdkAndWorkerGrantHaveToCoverBuildCompilerExtensionsAndEveryOutput()
    {
        using var f = new Fixture();
        var test = f.Target("Tests/T.csproj", "net10.0", true);
        var graph = f.Graph(test);
        var invalidSdk = f.Snapshot(graph, dotnet: Path.Combine(f.Root, "dotnet"));
        Assert.Equal(TestBuildBlocker.UnverifiedSdk, f.Plan(graph, test.Id, invalidSdk).Blocker);
        Assert.Equal(TestBuildBlocker.DeniedRoots,
            f.Plan(graph, test.Id, operations: ExecutionOperation.Build).Blocker);
        Assert.Equal(TestBuildBlocker.DeniedRoots,
            f.Plan(graph, test.Id, grantRoots: [f.Results]).Blocker);
    }

    [Fact]
    public void CollidingDirectoriesAndProtectedGitStateCannotBeLeased()
    {
        using var f = new Fixture();
        var library = f.Target("Lib/L.csproj", "net10.0", false);
        var test = f.Target("Tests/T.csproj", "net10.0", true, [], library.Id);
        var graph = f.Graph(library, test);
        var collision = f.Data(graph);
        collision[test.Id] = collision[test.Id] with
        {
            OutputDirectory = collision[library.Id].OutputDirectory,
            TargetAssembly = Path.Combine(collision[library.Id].OutputDirectory, "T.dll"),
            OutDir = collision[library.Id].OutputDirectory, OutputPath = collision[library.Id].OutputDirectory
        };
        Assert.Equal(TestBuildBlocker.OutputCollision, f.Plan(graph, test.Id, data: collision).Blocker);
        var solo = f.Graph(test with { References = [] });
        f.WriteAssets(solo.Targets[0]);
        var denied = f.Data(solo);
        var protectedPath = Path.Combine(f.Root, ".git", "bin");
        denied[test.Id] = denied[test.Id] with { OutputDirectory = protectedPath,
            TargetAssembly = Path.Combine(protectedPath, "T.dll"), OutDir = protectedPath, OutputPath = protectedPath };
        Assert.Equal(TestBuildBlocker.DeniedRoots, f.Plan(solo, test.Id, data: denied).Blocker);
    }

    [Fact]
    public void UnsupportedImportsAndUnboundedWritePathsAreExplicitBlockers()
    {
        using var f = new Fixture();
        var test = f.Target("Tests/T.csproj", "net10.0", true);
        var graph = f.Graph(test);
        var data = f.Data(graph);
        data[test.Id] = data[test.Id] with { Imports = [Path.Combine(f.Root, "custom.targets")] };
        Assert.Equal(TestBuildBlocker.UnsupportedImports, f.Plan(graph, test.Id, data: data).Blocker);
        data[test.Id] = data[test.Id] with { Imports = [], HasUnknownWrites = true };
        Assert.Equal(TestBuildBlocker.UnknownWrites, f.Plan(graph, test.Id, data: data).Blocker);
        data[test.Id] = data[test.Id] with { HasUnknownWrites = false, OutDir = Path.Combine(f.Root, "untracked") };
        Assert.Equal(TestBuildBlocker.UnknownWrites, f.Plan(graph, test.Id, data: data).Blocker);
        File.WriteAllText(f.ProjectPath(test), "<Project Sdk=\"Microsoft.NET.Sdk\"><Target BeforeTargets=\"Build\" Name=\"Custom\" /></Project>");
        Assert.Equal(TestBuildBlocker.UnsupportedImports, f.Plan(graph, test.Id).Blocker);
        File.WriteAllText(f.ProjectPath(test), "<Project Sdk=\"Microsoft.NET.Sdk\"><ItemGroup>" +
            "<Content Include=\"file.txt\" CopyToOutputDirectory=\"Always\" /></ItemGroup></Project>");
        Assert.Equal(TestBuildBlocker.UnknownWrites, f.Plan(graph, test.Id).Blocker);
        File.WriteAllText(f.ProjectPath(test), "<Project Sdk=\"Microsoft.NET.Sdk\" />");
        var obj = Path.GetDirectoryName(f.AssetsPath(test))!;
        File.WriteAllText(Path.Combine(obj, "T.csproj.nuget.g.props"),
            "<Project><Target Name=\"HiddenTarget\" /></Project>");
        Assert.Equal(TestBuildBlocker.UnsupportedImports, f.Plan(graph, test.Id).Blocker);
    }

    [Fact]
    public void MtpAndLegacyTestTargetsAreNotAssignedVstestBuildCommands()
    {
        using var f = new Fixture();
        var test = f.Target("Tests/T.csproj", "net10.0", true);
        var graph = f.Graph(test);
        var data = f.Data(graph);
        data[test.Id] = data[test.Id] with { UsesMicrosoftTestingPlatform = true };
        Assert.Equal(TestBuildBlocker.UnsupportedTarget, f.Plan(graph, test.Id, data: data).Blocker);
        var legacy = f.Target("Old/O.csproj", "net8.0", true);
        Assert.Equal(TestBuildBlocker.UnsupportedTarget, f.Plan(f.Graph(legacy), legacy.Id).Blocker);
    }

    [Fact]
    public async Task RealEvaluatedSdkStyleTestTargetIsPreparedWithoutLaunchingBuild()
    {
        using var f = new Fixture();
        var test = f.Target("Real SDK Target/Real SDK Target.csproj", "net10.0", true);
        var obj = Path.GetDirectoryName(f.AssetsPath(test))!;
        File.WriteAllText(Path.Combine(obj, Path.GetFileName(test.ProjectPath) + ".nuget.g.targets"), "<Project />");
        var evaluator = new ProjectEvaluator(f.Workspace);
        var graph = await evaluator.EvaluateAsync(null, CancellationToken.None);
        Assert.True(graph.Complete, string.Join(";", graph.Issues.Select(i => i.Reason)));
        var concrete = Assert.Single(graph.Targets);
        Assert.Equal(test.Id, concrete.Id);
        var projectDirectory = Path.GetDirectoryName(f.ProjectPath(test))!;
        var grant = new ExecutionGrant("fixture", RootIdentity.Read(f.Root),
            ExecutionOperation.Build | ExecutionOperation.CompilerExtensions,
            [f.Results, Path.Combine(projectDirectory, "bin"), Path.Combine(projectDirectory, "obj")]);
        var result = evaluator.PrepareBuild(graph, concrete.Id, f.Results, grant);
        Assert.True(result.Ready, $"{result.Blocker}: {result.Detail}");
        Assert.False(File.Exists(Assert.Single(result.Plan!.InvokedTargets).TargetAssembly));
        Assert.Contains("-t:Build", result.Plan.Command.Arguments);
        Assert.DoesNotContain(result.Plan.Command.Arguments, argument =>
            argument.Contains("restore", StringComparison.OrdinalIgnoreCase));
    }

    private sealed class Fixture : IDisposable
    {
        private readonly string container = Path.Combine(Path.GetTempPath(), "sg14-test-" + Guid.NewGuid().ToString("N"));
        internal string Root { get; }
        internal Workspace Workspace { get; }
        internal string Dotnet { get; }
        internal string Results { get; }

        internal Fixture()
        {
            Root = Path.Combine(container, "checkout");
            Directory.CreateDirectory(Root);
            Workspace = new Workspace(Root, Path.Combine(container, "state"));
            Dotnet = Path.Combine(container, "toolchain", OperatingSystem.IsWindows() ? "dotnet.exe" : "dotnet");
            Directory.CreateDirectory(Path.GetDirectoryName(Dotnet)!);
            File.WriteAllText(Dotnet, "no process may be launched from this launcher");
            var sdk = Path.Combine(Path.GetDirectoryName(Dotnet)!, "sdk", "10.0.404");
            Directory.CreateDirectory(sdk);
            File.WriteAllText(Path.Combine(sdk, "MSBuild.dll"), "SDK marker");
            Results = Path.Combine(container, "results with spaces");
        }

        internal ProjectTarget Target(string relative, string framework, bool isTest,
            IReadOnlyList<CompileMember>? compile = null, params string[] refs)
        {
            var path = Path.Combine(Root, relative.Replace('/', Path.DirectorySeparatorChar));
            Directory.CreateDirectory(Path.GetDirectoryName(path)!);
            File.WriteAllText(path, "<Project Sdk=\"Microsoft.NET.Sdk\"><PropertyGroup>" +
                "<TargetFramework>" + framework + "</TargetFramework><IsTestProject>" +
                (isTest ? "true" : "false") + "</IsTestProject></PropertyGroup></Project>");
            var target = new ProjectTarget(relative + "|" + framework, relative, framework, "Debug", "AnyCPU", "",
                "10.0.404", isTest, compile ?? [], refs, "fixture");
            WriteAssets(target);
            return target;
        }

        internal string ProjectPath(ProjectTarget target) => Path.Combine(Root, target.ProjectPath.Replace('/', Path.DirectorySeparatorChar));
        internal string AssetsPath(ProjectTarget target) => Path.Combine(Path.GetDirectoryName(ProjectPath(target))!, "obj", "project.assets.json");
        internal ProjectGraph Graph(params ProjectTarget[] targets) => new(targets, []);
        internal Dictionary<string, EvaluatedBuildTarget> Data(ProjectGraph graph)
        {
            var result = new Dictionary<string, EvaluatedBuildTarget>(StringComparer.Ordinal);
            foreach (var t in graph.Targets)
            {
                var projectDirectory = Path.GetDirectoryName(ProjectPath(t))!;
                var output = Path.Combine(projectDirectory, "bin", "Debug", t.Framework);
                var obj = Path.Combine(projectDirectory, "obj");
                result.Add(t.Id, new EvaluatedBuildTarget(t.Id, t.Framework, t.Configuration, t.Platform,
                    t.RuntimeIdentifier, t.SdkVersion, output, Path.Combine(obj, "Debug", t.Framework), obj,
                    Path.Combine(output, Path.GetFileNameWithoutExtension(t.ProjectPath) + ".dll"), AssetsPath(t), [])
                { OutDir = output, OutputPath = output });
            }
            return result;
        }

        internal BuildEvaluationSnapshot Snapshot(ProjectGraph graph, string? dotnet = null,
            IReadOnlyDictionary<string, EvaluatedBuildTarget>? data = null) =>
            new(graph, RootIdentity.Read(Root), dotnet ?? Dotnet, data ?? Data(graph));

        internal TestBuildPreparation Plan(ProjectGraph graph, string selectedId,
            BuildEvaluationSnapshot? snapshot = null, IReadOnlyDictionary<string, EvaluatedBuildTarget>? data = null,
            ExecutionOperation operations = ExecutionOperation.Build | ExecutionOperation.CompilerExtensions,
            IReadOnlyList<string>? grantRoots = null)
        {
            var grant = new ExecutionGrant("fixture", RootIdentity.Read(Root), operations,
                (grantRoots ?? ExpectedRoots(graph.Targets.ToArray())).ToArray());
            return new TestBuildPlanner(Workspace).Prepare(graph, selectedId, Results,
                grant, snapshot ?? Snapshot(graph, data: data));
        }

        internal string[] ExpectedRoots(params ProjectTarget[] targets) =>
            targets.SelectMany(t =>
            {
                var data = Data(new ProjectGraph([t], []))[t.Id];
                return new[] { data.OutputDirectory, data.IntermediateDirectory, data.BaseIntermediateDirectory };
            }).Append(Results).Distinct(Workspace.PathComparer).Order(Workspace.PathComparer).ToArray();

        internal void WriteAssets(ProjectTarget target, string? assetTarget = null, string? projectPath = null,
            IReadOnlyList<string>? references = null)
        {
            var assets = AssetsPath(target);
            Directory.CreateDirectory(Path.GetDirectoryName(assets)!);
            var refs = new Dictionary<string, object>(Workspace.PathComparer);
            foreach (var path in references ?? target.References.Select(id => id[..id.LastIndexOf('|')]))
                refs.Add(Path.Combine(Root, path.Replace('/', Path.DirectorySeparatorChar)), new { });
            var data = new Dictionary<string, object>
            {
                ["version"] = 3,
                ["targets"] = new Dictionary<string, object> { [assetTarget ?? target.Framework +
                    (target.RuntimeIdentifier.Length == 0 ? "" : "/" + target.RuntimeIdentifier)] = new { } },
                ["project"] = new { restore = new { projectPath = projectPath ?? ProjectPath(target),
                    outputPath = Path.GetDirectoryName(assets)!,
                    frameworks = new Dictionary<string, object> { [target.Framework] = new { projectReferences = refs } } },
                    frameworks = new Dictionary<string, object> { [target.Framework] = new { } } }
            };
            File.WriteAllText(assets, JsonSerializer.Serialize(data));
        }

        public void Dispose() => Directory.Delete(container, recursive: true);
    }
}
