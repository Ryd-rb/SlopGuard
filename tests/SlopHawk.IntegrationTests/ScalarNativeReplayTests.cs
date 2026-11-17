using System.Diagnostics;
using System.Security.Cryptography;
using System.Text;
using System.Text.Json;
using Microsoft.CodeAnalysis;
using Microsoft.CodeAnalysis.CSharp;
using Microsoft.CodeAnalysis.CSharp.Syntax;
using SlopHawk.Providers.CSharp;
using Xunit;

namespace SlopHawk.IntegrationTests;

// Opt-in native end-to-end qualification is enabled only after the owner's deferred builds.
public sealed class ScalarNativeReplayTests
{
    private const string Code = """
        namespace Fixture;
        public static class Runner
        {
            public static int Local(int x) { int Nested(int z) => unchecked(z + 1); return Nested(x); }
            public static int Assignment(int x, bool enabled) { int y = x; y = enabled ? x + 7 : x; y -= 5; return y == 395 ? 1 : 0; }
            public static int Checked(int x) => checked(x + 1) > 0 ? 1 : 0;
            public static int Opaque(int x, bool b) => b && x != 0 && 12 / x > 4 ? 1 : 0;
            public static int Loop(int x) { while (x > 5) x--; return x; }
            public static void Main() { }
        }
        """;

    [Fact]
    public void SameCompilationEmitsCandidateAndProjectsExactRoslynFacts()
    {
        using var fixture = new Fixture();
        var options = new ScalarBuildOptions("Demo.csproj", ["Demo.cs"], ReferencePack(), ["RELEASE_CASE"]);
        var first = ScalarCompilation.Create(fixture.Source, options);
        var again = ScalarCompilation.Create(fixture.Source, options);
        Assert.Equal(Convert.ToHexString(SHA256.HashData(first.Emit())),
            Convert.ToHexString(SHA256.HashData(again.Emit())));
        Assert.Equal(first.Inputs.Compiler, again.Inputs.Compiler);
        Assert.Equal(first.Inputs.Project.Sha256, again.Inputs.Project.Sha256);
        Assert.Equal(Convert.ToHexString(SHA256.HashData(File.ReadAllBytes(Path.Combine(fixture.Source, "Demo.cs")))),
            first.Inputs.Sources.Single().Sha256);
        Assert.Equal(new[] { "RELEASE_CASE" }, first.Inputs.Defines);
        Assert.Contains(first.Inputs.References, input => input.Name == "System.Runtime.dll");
        var facts = first.ProjectFacts("Demo.cs", "Fixture.Runner.Assignment(int, bool)");
        Assert.True(facts.Bound, facts.Reason);
        Assert.True(facts.Eligible);
        Assert.Equal(new[] { "int", "bool" }, facts.Parameters.Select(parameter => parameter.Type));
        Assert.Contains(facts.Blocks.SelectMany(block => block.Nodes), node => node.Kind == "assign");
        Assert.Equal(facts.ToWire([[0, 1]], 128, 16), again.ProjectFacts("Demo.cs",
            "Fixture.Runner.Assignment(int, bool)").ToWire([[0, 1]], 128, 16));
    }

    [Fact]
    public void LocalFunctionsAssignmentsOverflowUnknownsAndZeroBudgetsRemainExplicit()
    {
        using var fixture = new Fixture();
        var compilation = ScalarCompilation.Create(fixture.Source,
            new("Demo.csproj", ["Demo.cs"], ReferencePack()));
        var tree = CSharpSyntaxTree.ParseText(Code, path: "Demo.cs");
        var shadow = CSharpCompilation.Create("selection", [tree],
            [MetadataReference.CreateFromFile(typeof(object).Assembly.Location)],
            new CSharpCompilationOptions(OutputKind.DynamicallyLinkedLibrary));
        var local = tree.GetRoot().DescendantNodes().OfType<LocalFunctionStatementSyntax>().Single();
        var signature = shadow.GetSemanticModel(tree).GetDeclaredSymbol(local)!
            .ToDisplayString(SymbolDisplayFormat.CSharpErrorMessageFormat);
        Assert.True(compilation.ProjectFacts("Demo.cs", signature).Bound);
        Assert.False(compilation.ProjectFacts("missing.cs", signature).Bound);
        var assignment = compilation.ProjectFacts("Demo.cs", "Fixture.Runner.Assignment(int, bool)");
        Assert.Contains(assignment.Blocks.SelectMany(b => b.Nodes), node => node.Kind == "compound");
        Assert.Contains("limits\t0\t0", assignment.ToWire([], 0, 0));
        var checkedFacts = compilation.ProjectFacts("Demo.cs", "Fixture.Runner.Checked(int)");
        Assert.Contains(checkedFacts.Blocks.SelectMany(b => b.Nodes), node => node.Kind == "hole" &&
            node.Diagnostic.Contains("checked", StringComparison.Ordinal));
        var opaque = compilation.ProjectFacts("Demo.cs", "Fixture.Runner.Opaque(int, bool)");
        Assert.Contains(opaque.Blocks.SelectMany(b => b.Nodes), node => node.Kind == "hole" &&
            node.Diagnostic.Contains("Divide", StringComparison.Ordinal));
        var loop = compilation.Explore("Demo.cs", "Fixture.Runner.Loop(int)", [[6]], 32, 8, CancellationToken.None);
        Assert.Contains(loop.Unknown, item => item.StartsWith("Loop frontier", StringComparison.Ordinal));
    }

    [Fact]
    public async Task OriginalBuildReplayRejectsReorderingStaleClosureAndTamperedSeal()
    {
        if (!Native(out var executable, out var worker)) return; // Owner explicitly opts into the native fixture.
        using var fixture = new Fixture();
        var build = await Run(executable, "build", fixture.Spec, fixture.Scratch, worker);
        Assert.Equal(0, build.Exit);
        var buildManifest = build.Json.GetProperty("Build").GetString()!;
        var off = await Run(executable, "plan", fixture.Spec, fixture.Scratch, buildManifest, "graph-off", worker);
        var on = await Run(executable, "plan", fixture.Spec, fixture.Scratch, buildManifest, "graph-on", worker);
        Assert.Equal(0, off.Exit);
        Assert.Equal(0, on.Exit);
        Assert.Equal(off.Json.GetProperty("Cases").GetInt32(), on.Json.GetProperty("Cases").GetInt32());
        var check = await Run(executable, "check", on.Json.GetProperty("Plan").GetString()!, worker);
        Assert.Equal(1, check.Exit);
        Assert.Contains("unknown", check.Json.GetProperty("Result").GetProperty("MeasuredCoverage").GetString()!);
        var reviewed = await Run(executable, "review", check.Json.GetProperty("Review").GetString()!, worker);
        Assert.True(reviewed.Json.GetProperty("Current").GetBoolean());
        File.AppendAllText(Path.Combine(fixture.Source, "Demo.cs"), "\n// changed\n");
        Assert.Equal(2, (await Run(executable, "check", off.Json.GetProperty("Plan").GetString()!, worker)).Exit);
        File.AppendAllText(Path.Combine(Path.GetDirectoryName(buildManifest)!, "build.json"), " ");
        Assert.Equal(2, (await Run(executable, "plan", fixture.Spec, fixture.Scratch,
            buildManifest, "graph-on", worker)).Exit);
    }

    [Fact]
    public async Task ReverseOrderedAdapterResultsStayUnconfirmed()
    {
        if (!Native(out var executable, out var worker)) return;
        using var fixture = new Fixture(reorder: true);
        var build = await Run(executable, "build", fixture.Spec, fixture.Scratch, worker);
        var plan = await Run(executable, "plan", fixture.Spec, fixture.Scratch,
            build.Json.GetProperty("Build").GetString()!, "graph-off", worker);
        var check = await Run(executable, "check", plan.Json.GetProperty("Plan").GetString()!, worker);
        Assert.Equal(1, check.Exit);
        var result = check.Json.GetProperty("Result");
        Assert.Equal("incomplete", result.GetProperty("Outcome").GetString());
        Assert.Empty(result.GetProperty("Reproduced").EnumerateArray());
        Assert.True(result.GetProperty("Unstable").GetInt32() > 0);
        File.AppendAllText(Path.Combine(fixture.Source, "rules.json"), " ");
        Assert.Equal(2, (await Run(executable, "check",
            plan.Json.GetProperty("Plan").GetString()!, worker)).Exit);
    }

    private static bool Native(out string executable, out string worker)
    {
        executable = Environment.GetEnvironmentVariable("SLOPHAWK_NATIVE_FOREGROUND") ?? "";
        worker = Environment.GetEnvironmentVariable("SLOPHAWK_NATIVE_WORKER") ?? "";
        if (Environment.GetEnvironmentVariable("SLOPHAWK_NATIVE_QUALIFY") != "1") return false;
        Assert.True(File.Exists(executable) && File.Exists(worker));
        return true;
    }

    private static async Task<(int Exit, JsonElement Json)> Run(string executable, params string[] args)
    {
        var start = new ProcessStartInfo(executable) { UseShellExecute = false,
            RedirectStandardOutput = true, RedirectStandardError = true };
        foreach (var arg in args) start.ArgumentList.Add(arg);
        using var process = Process.Start(start)!;
        using var deadline = new CancellationTokenSource(TimeSpan.FromMinutes(5));
        var stdout = process.StandardOutput.ReadToEndAsync(deadline.Token);
        var stderr = process.StandardError.ReadToEndAsync(deadline.Token);
        await process.WaitForExitAsync(deadline.Token);
        var content = await stdout;
        var error = await stderr;
        return (process.ExitCode, string.IsNullOrWhiteSpace(content)
            ? JsonSerializer.SerializeToElement(new { Error = error })
            : JsonSerializer.Deserialize<JsonElement>(content));
    }

    private sealed class Fixture : IDisposable
    {
        private readonly string root = Directory.CreateTempSubdirectory("native-scalar-").FullName;
        public string Source { get; }
        public string Scratch { get; }
        public string Spec { get; }
        public Fixture(bool reorder = false)
        {
            Source = Directory.CreateDirectory(Path.Combine(root, "source")).FullName;
            Scratch = Directory.CreateDirectory(Path.Combine(root, "scratch")).FullName;
            File.WriteAllText(Path.Combine(Source, "Demo.csproj"), "<Project Sdk=\"Microsoft.NET.Sdk\" />");
            File.WriteAllText(Path.Combine(Source, "Demo.cs"), Code.Replace("public static void Main() { }", """
                public static void Main(string[] args)
                {
                    var values = System.Text.Json.JsonSerializer.Deserialize<Request[]>(System.IO.File.ReadAllText(args[0]))!;
                    #if REORDER
                    System.Array.Reverse(values);
                    #endif
                    System.Console.WriteLine(System.Text.Json.JsonSerializer.Serialize(values.Select(Observe).ToArray()));
                }
                private static int Observe(Request request)
                {
                    #if REORDER
                    return (int)request.Args[0];
                    #else
                    return Assignment((int)request.Args[0], request.Args[1] != 0);
                    #endif
                }
                public sealed record Request(string Operation, long[] Args);
                """).Replace("namespace Fixture;", "using System.Linq;\nnamespace Fixture;"));
            File.WriteAllText(Path.Combine(Source, "rules.json"), JsonSerializer.Serialize(new[]
            {
                new { Operation = "rate", Parameters = new[] { "x", "enabled" },
                    Domain = "x >= 0 && x <= 500 && (enabled == 0 || enabled == 1)",
                    Expected = reorder ? "x" : "0", Source = "policy.md#rate" }
            }));
            File.WriteAllText(Path.Combine(Source, "published.json"),
                "[{\"Operation\":\"rate\",\"Args\":[0,0],\"Expected\":0}]");
            Spec = Path.Combine(Source, "spec.json");
            File.WriteAllText(Spec, JsonSerializer.Serialize(new
            {
                Build = new { Project = "Demo.csproj", Sources = new[] { "Demo.cs" },
                    ReferencePack = ReferencePack(), Defines = reorder ? new[] { "REORDER" } : Array.Empty<string>() },
                Targets = new[] { new { Operation = "rate", Document = "Demo.cs",
                    Signature = "Fixture.Runner.Assignment(int, bool)" } },
                Rules = "rules.json", Published = "published.json", PerOperation = 24
            }));
        }
        public void Dispose() => Directory.Delete(root, recursive: true);
    }

    private static string ReferencePack()
    {
        var dotnet = new FileInfo(typeof(object).Assembly.Location).Directory!.Parent!.Parent!.Parent!.FullName;
        return Path.Combine(Directory.GetDirectories(Path.Combine(dotnet, "packs", "Microsoft.NETCore.App.Ref"), "10.0.*")
            .MaxBy(path => Version.Parse(Path.GetFileName(path)))!, "ref", "net10.0");
    }
}
