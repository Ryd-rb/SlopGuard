using System.Diagnostics;
using System.Text.Json;
using SlopHawk.Core;
using SlopHawk.WorkerFixture;
using Xunit;

namespace SlopHawk.IntegrationTests;

public sealed class ScalarPathCheckTests
{
    [Fact]
    public async Task GraphPlanReplaysTheSameCandidateAndExposesLedgerAndStructuralFallback()
    {
        using var fixture = new Fixture();
        var build = PathOf(await Run("build", fixture.Spec, fixture.Scratch), "Build");
        var off = PathOf(await Run("plan", fixture.Spec, fixture.Scratch, build, "graph-off"), "Plan");
        var on = PathOf(await Run("plan", fixture.Spec, fixture.Scratch, build, "graph-on"), "Plan");
        var offRun = await Run("check", off);
        var onRun = await Run("check", on);
        var results = onRun.Json.GetProperty("Result");
        Assert.Equal(1, onRun.Exit);
        Assert.Equal(offRun.Json.GetProperty("Result").GetProperty("Cases").GetInt32(), results.GetProperty("Cases").GetInt32());
        var violation = Assert.Single(results.GetProperty("Reproduced").EnumerateArray());
        Assert.Equal(1, violation.GetProperty("Observed").GetInt32());
        Assert.Equal("policy.md#ordinary-rate", violation.GetProperty("Source").GetString());
        Assert.Equal(new[] { 173, 311 }, violation.GetProperty("Case").GetProperty("Expectation").GetProperty("Args")
            .EnumerateArray().Select(item => item.GetInt32()).ToArray());
        Assert.False(string.IsNullOrEmpty(violation.GetProperty("Case").GetProperty("PredictedGoal").GetString()));
        var review = await Run("review", PathOf(onRun, "Review", 1));
        Assert.True(review.Json.GetProperty("Current").GetBoolean());
        Assert.Equal("daemon sentinel", File.ReadAllText(fixture.DaemonReport));

        var fallback = await Run("plan", fixture.Spec, fixture.Scratch, "-", "graph-on");
        var fallbackPath = PathOf(fallback, "Plan", 1);
        Assert.Equal(0, fallback.Json.GetProperty("Cases").GetInt32());
        using var structural = JsonDocument.Parse(File.ReadAllText(Path.Combine(Path.GetDirectoryName(fallbackPath)!, "graph.json")));
        Assert.False(structural.RootElement.GetProperty("BuildAttested").GetBoolean());
        Assert.Equal("Structural", structural.RootElement.GetProperty("Graphs").GetProperty("rate").GetProperty("Status").GetString());
        Assert.Equal(1, (await Run("check", fallbackPath)).Exit);
    }

    [Fact]
    public async Task ChangedSourceAndAnIncompleteSealBlockExecution()
    {
        using var fixture = new Fixture();
        var build = PathOf(await Run("build", fixture.Spec, fixture.Scratch), "Build");
        var first = PathOf(await Run("plan", fixture.Spec, fixture.Scratch, build, "graph-on"), "Plan");
        var second = PathOf(await Run("plan", fixture.Spec, fixture.Scratch, build, "graph-off"), "Plan");
        File.Delete(Path.Combine(Path.GetDirectoryName(second)!, "candidates.json"));
        Assert.Equal(1, (await Run("check", second)).Exit);
        File.AppendAllText(Path.Combine(Path.GetDirectoryName(fixture.Spec)!, "Rates.cs"), "\n// new source revision\n");
        var stale = await Run("check", first);
        Assert.Equal(1, stale.Exit);
        Assert.Contains("changed", stale.Error, StringComparison.OrdinalIgnoreCase);
        var plan = await Run("plan", fixture.Spec, fixture.Scratch, build, "graph-on");
        Assert.Equal(1, plan.Exit);
        Assert.Equal(0, plan.Json.GetProperty("Cases").GetInt32());
    }

    [Fact]
    public async Task ReorderedAdapterResponsesDoNotBecomeStableFindings()
    {
        using var fixture = new Fixture(reorder: true);
        var build = PathOf(await Run("build", fixture.Spec, fixture.Scratch), "Build");
        var plan = PathOf(await Run("plan", fixture.Spec, fixture.Scratch, build, "graph-off"), "Plan");
        var run = await Run("check", plan);
        Assert.Equal(1, run.Exit);
        var review = run.Json.GetProperty("Result");
        Assert.Empty(review.GetProperty("Reproduced").EnumerateArray());
        Assert.True(review.GetProperty("Unstable").GetInt32() > 0);
    }

    private static string PathOf((int Exit, JsonElement Json, string Error) result, string property, int expectedExit = 0)
    {
        Assert.True(result.Exit == expectedExit, $"{result.Exit}: {result.Error} {result.Json}");
        return result.Json.GetProperty(property).GetString()!;
    }

    private static async Task<(int Exit, JsonElement Json, string Error)> Run(params string[] args)
    {
        var start = Commands.Create(["paths", .. args]);
        start.ArgumentList[0] = Path.Combine(AppContext.BaseDirectory, "slophawk.dll");
        start.RedirectStandardOutput = true;
        start.RedirectStandardError = true;
        using var process = Process.Start(start)!;
        using var timeout = new CancellationTokenSource(TimeSpan.FromSeconds(30));
        var output = process.StandardOutput.ReadToEndAsync(timeout.Token);
        var error = process.StandardError.ReadToEndAsync(timeout.Token);
        await process.WaitForExitAsync(timeout.Token);
        var json = await output;
        return (process.ExitCode, string.IsNullOrWhiteSpace(json) ? default : JsonSerializer.Deserialize<JsonElement>(json), await error);
    }

    private sealed class Fixture : IDisposable
    {
        private readonly string root = Directory.CreateTempSubdirectory("slophawk-path-check-").FullName;
        public string Spec { get; }
        public string Scratch { get; }
        public string DaemonReport { get; }

        public Fixture(bool reorder = false)
        {
            var source = Directory.CreateDirectory(Path.Combine(root, "source")).FullName;
            Scratch = Directory.CreateDirectory(Path.Combine(root, "scratch")).FullName;
            Directory.CreateDirectory(Path.Combine(source, ".slophawk"));
            DaemonReport = Path.Combine(source, ".slophawk", "results.json");
            File.WriteAllText(DaemonReport, "daemon sentinel");
            File.WriteAllText(Path.Combine(source, "Rates.csproj"), "<Project Sdk=\"Microsoft.NET.Sdk\" />");
            File.WriteAllText(Path.Combine(source, "Rates.cs"), """
                using System;
                using System.IO;
                using System.Linq;
                using System.Text.Json;
                namespace Metering;
                public static class Rates
                {
                #if REORDER
                    public static int Charge(int x, int y) => x;
                #else
                    public static int Charge(int x, int y) => x == 173 && y == 311 ? 1 : 0;
                #endif
                    public static int Charge(int x) => 2;
                    public static void Main(string[] args)
                    {
                        var requests = JsonSerializer.Deserialize<Request[]>(File.ReadAllText(args[0]))!;
                #if REORDER
                        Array.Reverse(requests);
                #endif
                        Console.WriteLine(JsonSerializer.Serialize(requests.Select(request =>
                            Charge(checked((int)request.Args[0]), checked((int)request.Args[1]))).ToArray()));
                    }
                    public sealed record Request(string Operation, long[] Args);
                }
                """);
            Write("rules.json", new[] { new IntegerContract("rate", ["x", "y"],
                "x >= 0 && x <= 500 && y >= 0 && y <= 500", reorder ? "x" : "0", "policy.md#ordinary-rate") });
            Write("published.json", new[] { new ProbeExpectation("rate", [0, 0], JsonSerializer.SerializeToElement(0)) });
            var dotnet = new FileInfo(typeof(object).Assembly.Location).Directory!.Parent!.Parent!.Parent!.FullName;
            var refs = Directory.GetDirectories(Path.Combine(dotnet, "packs", "Microsoft.NETCore.App.Ref"), "10.0.*")
                .MaxBy(path => Version.Parse(Path.GetFileName(path)))!;
            Spec = Write("spec.json", new
            {
                Build = new
                {
                    Project = "Rates.csproj", Sources = new[] { "Rates.cs" }, ReferencePack = Path.Combine(refs, "ref", "net10.0"),
                    Defines = reorder ? new[] { "REORDER" } : Array.Empty<string>()
                },
                Targets = new[] { new { Operation = "rate", Document = "Rates.cs", Signature = "Metering.Rates.Charge(int, int)" } },
                Rules = "rules.json", Published = "published.json", PerOperation = 24
            });

            string Write<T>(string name, T value)
            {
                var path = Path.Combine(source, name);
                File.WriteAllText(path, JsonSerializer.Serialize(value));
                return path;
            }
        }

        public void Dispose() => Directory.Delete(root, recursive: true);
    }
}
