using System.Diagnostics;
using System.Security.Cryptography;
using System.Text;
using System.Text.Json;
using System.Text.Json.Nodes;
using SlopHawk.Core;
using SlopHawk.WorkerFixture;
using Xunit;

namespace SlopHawk.IntegrationTests;

[Collection(NativeCliCollection.Name)]
public sealed class ScalarPathCheckTests
{
    [NativeCliFact(scalarWorker: true)]
    public async Task NativeForegroundRoutesBuildPlanCheckAndReviewThroughTheScalarWorker()
    {
        using var fixture = new Fixture();
        var output = await Native("build", fixture.Spec, fixture.Scratch);
        Assert.Equal(new[] { "Build", "Candidate" }, output.Json.EnumerateObject().Select(property => property.Name));
        var build = PathOf(output, "Build");
        var planned = await Native("plan", fixture.Spec, fixture.Scratch, build, "graph-correlated-v1");
        var plan = PathOf(planned, "Plan");
        Assert.Equal("graph-correlated-v1", planned.Json.GetProperty("Mode").GetString());
        using var planDocument = JsonDocument.Parse(await File.ReadAllTextAsync(Artifact(plan, "plan.json")));
        var evidence = planDocument.RootElement.GetProperty("PlannerEvidence");
        Assert.Equal("slophawk-scalar-planner-evidence-v1", evidence.GetProperty("Format").GetString());
        Assert.Equal("graph-correlated-v1", evidence.GetProperty("Mode").GetString());
        Assert.Equal("correlated-equalities-v1/replay-v1", evidence.GetProperty("PlannerRevision").GetString());
        Assert.Equal(Convert.ToHexString(SHA256.HashData(File.ReadAllBytes(
            Environment.GetEnvironmentVariable("SLOPHAWK_NATIVE_MIGRATION_BINARY")!))),
            evidence.GetProperty("PlannerSha256").GetString());
        foreach (var field in new[] { "InspectionSha256", "ScalarFactsSha256", "GraphsSha256" })
            Assert.Matches("^[0-9A-F]{64}$", evidence.GetProperty(field).GetString()!);
        using var planSeal = JsonDocument.Parse(await File.ReadAllTextAsync(plan));
        Assert.Equal("slophawk-scalar-run-v2", planSeal.RootElement.GetProperty("Format").GetString());
        using var candidates = JsonDocument.Parse(await File.ReadAllTextAsync(
            Path.Combine(Path.GetDirectoryName(plan)!, "candidates.json")));
        var request = Path.Combine(fixture.Scratch, "checker-plan.json");
        await File.WriteAllTextAsync(request, JsonSerializer.Serialize(new
        {
            checker = "scalar_correlated",
            facts = candidates.RootElement.GetProperty("ScalarFacts").GetProperty("rate").GetString()
        }));
        var preview = await NativeTool("checker-plan", "--request", request);
        Assert.Equal(0, preview.Exit);
        Assert.Equal("graph-correlated-v1", preview.Json.GetProperty("policy").GetString());
        Assert.True(preview.Json.GetProperty("result").GetProperty("bound").GetBoolean());
        Assert.NotEmpty(preview.Json.GetProperty("result").GetProperty("candidates").EnumerateArray());
        var checkedRun = await Native("check", plan);
        var review = await Native("review", PathOf(checkedRun, "Review", 1));
        using var reviewSeal = JsonDocument.Parse(await File.ReadAllTextAsync(PathOf(checkedRun, "Review", 1)));
        Assert.Equal("slophawk-scalar-run-v2", reviewSeal.RootElement.GetProperty("Format").GetString());
        Assert.True(review.Json.GetProperty("Current").GetBoolean());
        Assert.Equal(JsonValueKind.Null, review.Json.GetProperty("HistoricalReason").ValueKind);
        var observed = Assert.Single(review.Json.GetProperty("Result").GetProperty("Reproduced").EnumerateArray());
        Assert.Equal(1, observed.GetProperty("Observed").GetInt32());
        var structural = await Native("plan", fixture.Spec, fixture.Scratch, "-", "graph-correlated-v1");
        var structuralPlan = PathOf(structural, "Plan", 1);
        using var structuralDocument = JsonDocument.Parse(await File.ReadAllTextAsync(Artifact(structuralPlan, "plan.json")));
        Assert.False(structuralDocument.RootElement.TryGetProperty("PlannerEvidence", out _));
        using var graphs = JsonDocument.Parse(await File.ReadAllTextAsync(Path.Combine(
            Path.GetDirectoryName(structuralPlan)!, "graph.json")));
        Assert.False(graphs.RootElement.GetProperty("BuildAttested").GetBoolean());
        Assert.Equal("Structural", graphs.RootElement.GetProperty("Graphs").GetProperty("rate").GetProperty("Status").GetString());
        Assert.Equal(1, (await Native("check", structuralPlan)).Exit);
        Assert.Equal("daemon sentinel", File.ReadAllText(fixture.DaemonReport));

        var source = Path.GetDirectoryName(fixture.Spec)!;
        var contracts = await NativeTool("check-contracts", "--diverse", PathOf(output, "Candidate"),
            Path.Combine(source, "rules.json"), Path.Combine(source, "published.json"), fixture.Scratch, "24");
        Assert.InRange(contracts.Exit, 0, 1);
        Assert.InRange(contracts.Json.GetProperty("Compared").GetInt32(), 1, 24);
        using var contractSummary = JsonDocument.Parse(await File.ReadAllTextAsync(
            contracts.Json.GetProperty("Summary").GetString()!));
        Assert.Equal("diverse-v1", contractSummary.RootElement.GetProperty("CaseSelection").GetString());
        var reviewPath = PathOf(checkedRun, "Review", 1);
        await File.AppendAllTextAsync(Path.Combine(source, "Rates.cs"), "\n// changed");
        var historical = await Native("review", reviewPath);
        Assert.Equal(1, historical.Exit);
        Assert.False(historical.Json.GetProperty("Current").GetBoolean());
        Assert.False(string.IsNullOrEmpty(historical.Json.GetProperty("HistoricalReason").GetString()));
        Assert.True(JsonElement.DeepEquals(review.Json.GetProperty("Result"), historical.Json.GetProperty("Result")));
        await File.AppendAllTextAsync(Path.Combine(Path.GetDirectoryName(reviewPath)!, "review.json"), " ");
        var tampered = await Native("review", reviewPath);
        Assert.Equal(1, tampered.Exit);
        Assert.Contains("path_check_failed", tampered.Error);

        Task<(int Exit, JsonElement Json, string Error)> Native(params string[] args) => NativeTool("paths", args);

        async Task<(int Exit, JsonElement Json, string Error)> NativeTool(string command, params string[] args)
        {
            var start = PathCommand(command, args);
            return await Run(start);
        }
    }

    [NativeCliFact(scalarWorker: true)]
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
        Assert.False(string.IsNullOrEmpty(violation.GetProperty("RetestJob").GetString()));
        Assert.NotEqual(results.GetProperty("InitialJob").GetString(), violation.GetProperty("RetestJob").GetString());
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

    [NativeCliFact(scalarWorker: true)]
    public async Task ATargetWithoutRunnableCasesKeepsTheCheckAndReviewIncomplete()
    {
        using var fixture = new Fixture(missingTarget: true);
        var build = PathOf(await Run("build", fixture.Spec, fixture.Scratch), "Build");
        var plan = PathOf(await Run("plan", fixture.Spec, fixture.Scratch, build, "graph-off"), "Plan");
        var check = await Run("check", plan);
        var review = await Run("review", PathOf(check, "Review", 1));

        Assert.Equal(1, review.Exit);
        Assert.True(review.Json.GetProperty("Current").GetBoolean());
        var result = review.Json.GetProperty("Result");
        Assert.Equal("incomplete", result.GetProperty("Outcome").GetString());
        Assert.True(result.GetProperty("Compared").GetInt32() > 0);
        Assert.Empty(result.GetProperty("Reproduced").EnumerateArray());
        var targets = result.GetProperty("Scope").EnumerateArray().ToArray();
        Assert.Equal(2, targets.Length);
        Assert.True(targets[0].GetProperty("PlannedCases").GetInt32() > 0);
        Assert.Equal("missing", targets[1].GetProperty("Operation").GetString());
        Assert.False(targets[1].GetProperty("Bound").GetBoolean());
        Assert.Equal(0, targets[1].GetProperty("PlannedCases").GetInt32());
    }

    [NativeCliFact(scalarWorker: true)]
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
        Assert.StartsWith("path_check_failed:", stale.Error);
        Assert.Equal(JsonValueKind.Undefined, stale.Json.ValueKind);
        var plan = await Run("plan", fixture.Spec, fixture.Scratch, build, "graph-on");
        Assert.Equal(1, plan.Exit);
        Assert.StartsWith("path_check_failed:", plan.Error);
        Assert.Equal(JsonValueKind.Undefined, plan.Json.ValueKind);
    }

    [NativeCliFact(scalarWorker: true)]
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

    [NativeCliFact(scalarWorker: true)]
    public async Task CorrelatedEvidenceRejectsAuthenticatedModeAndMissingFacts()
    {
        using var fixture = new Fixture();
        var build = PathOf(await Run("build", fixture.Spec, fixture.Scratch), "Build");
        var modePlan = PathOf(await Run("plan", fixture.Spec, fixture.Scratch, build, "graph-correlated-v1"), "Plan");
        RewriteJson(Artifact(modePlan, "plan.json"), value => value["Mode"] = "graph-off");
        Reseal(modePlan);
        AssertRejected(await Run("check", modePlan));

        var factsPlan = PathOf(await Run("plan", fixture.Spec, fixture.Scratch, build, "graph-correlated-v1"), "Plan");
        RewriteJson(Artifact(factsPlan, "candidates.json"), value => value["ScalarFacts"]!.AsObject().Remove("rate"));
        Reseal(factsPlan);
        AssertRejected(await Run("check", factsPlan));
        Assert.Equal("daemon sentinel", File.ReadAllText(fixture.DaemonReport));
    }

    [NativeCliFact(scalarWorker: true)]
    public async Task PlannerDriftAndOldCorrelatedReceiptsKeepHistoricalResultsWithoutFreshExecution()
    {
        using var fixture = new Fixture();
        var build = PathOf(await Run("build", fixture.Spec, fixture.Scratch), "Build");
        var plan = PathOf(await Run("plan", fixture.Spec, fixture.Scratch, build, "graph-correlated-v1"), "Plan");
        var review = PathOf(await Run("check", plan), "Review", 1);
        Assert.True((await Run("review", review)).Json.GetProperty("Current").GetBoolean());

        RewriteJson(Artifact(plan, "plan.json"), value =>
            value["PlannerEvidence"]!["PlannerRevision"] = "future-planner/99");
        Reseal(plan);
        RelinkReview(review, plan, "slophawk-scalar-run-v2");
        AssertRejected(await Run("check", plan));
        AssertHistorical(await Run("review", review), review);

        // Recreate the old producer's v1 correlated receipt. There is no invented
        // binding or migration default: its retained comparisons stay historical.
        RewriteJson(Artifact(plan, "plan.json"), value => value.Remove("PlannerEvidence"));
        Reseal(plan, "slophawk-scalar-run-v1");
        RelinkReview(review, plan, "slophawk-scalar-run-v1");
        var oldCheck = await Run("check", plan);
        AssertRejected(oldCheck);
        Assert.Contains("sealed planner evidence", oldCheck.Error);
        var oldReview = await Run("review", review);
        AssertHistorical(oldReview, review);
        Assert.Contains("sealed planner evidence", oldReview.Json.GetProperty("HistoricalReason").GetString()!);

        // Missing evidence in a new-format receipt is malformed, not an old input.
        Reseal(plan, "slophawk-scalar-run-v2");
        RelinkReview(review, plan, "slophawk-scalar-run-v2");
        AssertRejected(await Run("check", plan));
        AssertRejected(await Run("review", review));
    }

    private static string Artifact(string manifest, string name) =>
        Path.Combine(Path.GetDirectoryName(manifest)!, name);

    private static void AssertRejected((int Exit, JsonElement Json, string Error) result)
    {
        Assert.Equal(1, result.Exit);
        Assert.StartsWith("path_check_failed:", result.Error);
        Assert.Equal(JsonValueKind.Undefined, result.Json.ValueKind);
    }

    private static void AssertHistorical((int Exit, JsonElement Json, string Error) result, string manifest)
    {
        Assert.Equal(1, result.Exit);
        Assert.False(result.Json.GetProperty("Current").GetBoolean());
        Assert.False(string.IsNullOrEmpty(result.Json.GetProperty("HistoricalReason").GetString()));
        using var retained = JsonDocument.Parse(File.ReadAllText(Artifact(manifest, "review.json")));
        Assert.True(JsonElement.DeepEquals(retained.RootElement, result.Json.GetProperty("Result")));
    }

    private static void RewriteJson(string path, Action<JsonObject> edit)
    {
        var value = JsonNode.Parse(File.ReadAllText(path))!.AsObject();
        edit(value);
        File.WriteAllText(path, value.ToJsonString());
    }

    private static void RelinkReview(string review, string plan, string format)
    {
        RewriteJson(Artifact(review, "review.json"), value =>
            value["PlanSha256"] = Convert.ToHexString(SHA256.HashData(File.ReadAllBytes(plan))));
        Reseal(review, format);
    }

    // Test-only producer of authenticated invalid/old receipts. This exercises
    // cross-document admission after the existing byte seal has been verified.
    private static void Reseal(string manifest, string? format = null)
    {
        using var existing = JsonDocument.Parse(File.ReadAllText(manifest));
        var seal = existing.RootElement;
        using var stream = new MemoryStream();
        using (var writer = new Utf8JsonWriter(stream))
        {
            writer.WriteStartObject();
            writer.WriteString("Format", format ?? seal.GetProperty("Format").GetString());
            writer.WriteString("Kind", seal.GetProperty("Kind").GetString());
            writer.WriteStartArray("Files");
            foreach (var entry in seal.GetProperty("Files").EnumerateArray())
            {
                var name = entry.GetProperty("File").GetString()!;
                var bytes = File.ReadAllBytes(Artifact(manifest, name));
                writer.WriteStartObject();
                writer.WriteString("File", name);
                writer.WriteNumber("Length", bytes.LongLength);
                writer.WriteString("Sha256", Convert.ToHexString(SHA256.HashData(bytes)));
                writer.WriteEndObject();
            }
            writer.WriteEndArray();
            writer.WriteStartObject("Identity");
            foreach (var name in new[] { "Protocol", "Operation", "Job", "Root", "Context", "Epoch", "Revision",
                "InputDigest", "ToolDigest", "PolicyDigest", "BudgetRef", "GrantRef" })
            {
                writer.WritePropertyName(name);
                // Preserve the Rust producer's string escaping, including Unicode paths.
                writer.WriteRawValue(seal.GetProperty("Identity").GetProperty(name).GetRawText());
            }
            writer.WriteEndObject();
            writer.WriteEndObject();
        }
        var unsigned = stream.ToArray();
        var mac = Convert.ToHexString(HMACSHA256.HashData(Encoding.UTF8.GetBytes(SealKey), unsigned));
        File.WriteAllText(manifest, Encoding.UTF8.GetString(unsigned)[..^1] + ",\"Mac\":\"" + mac + "\"}");
    }

    private static string PathOf((int Exit, JsonElement Json, string Error) result, string property, int expectedExit = 0)
    {
        Assert.True(result.Exit == expectedExit, $"{result.Exit}: {result.Error} {result.Json}");
        return result.Json.GetProperty(property).GetString()!;
    }

    private static Task<(int Exit, JsonElement Json, string Error)> Run(params string[] args) =>
        Run(PathCommand("paths", args));

    private static ProcessStartInfo PathCommand(string verb, string[] args)
    {
        var start = NativeCliFactAttribute.Command([verb, .. args]);
        var worker = Environment.GetEnvironmentVariable("SLOPHAWK_NATIVE_SCALAR_WORKER")!;
        start.Environment["SLOPHAWK_SCALAR_WORKER"] = worker;
        start.Environment["SLOPHAWK_SCALAR_WORKER_SHA256"] = Convert.ToHexString(
            System.Security.Cryptography.SHA256.HashData(File.ReadAllBytes(worker)));
        start.Environment["SLOPHAWK_PATHS_GRANT"] = "native-qualification-fixture";
        start.Environment["SLOPHAWK_PATHS_EPOCH"] = "scalar-fixture";
        start.Environment["SLOPHAWK_PATHS_REVISION"] = "1";
        start.Environment["SLOPHAWK_PATHS_BUDGET"] = "fixture-budget";
        start.Environment["SLOPHAWK_PATHS_SEAL_KEY"] = SealKey;
        return start;
    }

    private static readonly string SealKey = Convert.ToHexString(
        System.Security.Cryptography.RandomNumberGenerator.GetBytes(32));

    private static async Task<(int Exit, JsonElement Json, string Error)> Run(ProcessStartInfo start)
    {
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

        public Fixture(bool reorder = false, bool missingTarget = false)
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
            var rule = new IntegerContract("rate", ["x", "y"],
                "x >= 0 && x <= 500 && y >= 0 && y <= 500", reorder ? "x" : "0", "policy.md#ordinary-rate");
            var example = new ProbeExpectation("rate", [0, 0], JsonSerializer.SerializeToElement(0));
            var targets = new[] { new { Operation = "rate", Document = "Rates.cs", Signature = "Metering.Rates.Charge(int, int)" } };
            if (missingTarget)
                targets = [.. targets, new { Operation = "missing", Document = "Rates.cs", Signature = "Metering.Rates.Missing(int, int)" }];
            Write("rules.json", missingTarget ? new[] { rule, rule with { Operation = "missing" } } : [rule]);
            Write("published.json", missingTarget ? new[] { example, example with { Operation = "missing" } } : [example]);
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
                Targets = targets,
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
