using System.Diagnostics;
using System.Text.Json;
using System.Text.Json.Nodes;
using SlopHawk.Core;
using SlopHawk.WorkerFixture;
using Xunit;

namespace SlopHawk.IntegrationTests;

public sealed class ForegroundEvidenceTests
{
    [NativeCliFact]
    public async Task ContractSearchSeparatesChangingReplayResultsFromStableFindings()
    {
        using var workspace = new TestWorkspace();
        string[] operations = ["steady", "changes", "disappears", "new-difference"];
        var rules = workspace.Write("rules.json", operations.Select(operation =>
            new IntegerContract(operation, ["x"], "x >= 0", "1", "rules.md")).ToArray());
        var examples = workspace.Write("examples.json", operations.Select(operation => Example(operation, 5, 1)).ToArray());

        var result = await Run("check-contracts", FixtureAssembly, rules, examples, workspace.Path, "2", "probe-batch");

        Assert.Equal(1, result.ExitCode);
        Assert.Equal(3, result.Json.GetProperty("ObservedDiscrepancies").GetInt32());
        Assert.Equal(3, result.Json.GetProperty("ReplayChanges").GetInt32());
        Assert.Equal(1, result.Json.GetProperty("Findings").GetInt32());
        Assert.Equal("published-examples", result.Json.GetProperty("QualificationBasis").GetString());
        Assert.Equal("supplied-rules", result.Json.GetProperty("ExpectationBasis").GetString());
        var stable = workspace.Read(result.Json.GetProperty("Report").GetString()!);
        var finding = Assert.Single(stable.GetProperty("Result").GetProperty("Findings").EnumerateArray());
        Assert.Equal("steady", finding.GetProperty("Input").GetProperty("Operation").GetString());
        var summary = workspace.Read(result.Json.GetProperty("Summary").GetString()!);
        Assert.Equal(3, summary.GetProperty("ReplayChanges").GetArrayLength());
        Assert.True(File.Exists(summary.GetProperty("ReplayReport").GetString()));
    }

    [NativeCliFact]
    public async Task ProgressPublishesScopedWorklistBeforeAnyChecksRun()
    {
        using var workspace = new TestWorkspace();
        var acknowledgments = workspace.Write("acknowledgments.json", Array.Empty<object>());
        var scope = workspace.Write("scope.json", new
        {
            Operations = new[] { "sample" },
            Ordinary = Array.Empty<object>(),
            Sources = new[] { new { Operation = "sample", Component = "rates", Contract = "rules.md", Implementation = "Rates.cs" } }
        });

        var progress = await Run("check-review", workspace.Path, acknowledgments, scope, "--progress");

        Assert.True(progress.ExitCode == 0, progress.Error);
        Assert.False(progress.Json.GetProperty("Complete").GetBoolean());
        Assert.False(progress.Json.GetProperty("RequiresRunSelection").GetBoolean());
        Assert.Equal(1, progress.Json.GetProperty("MissingCoverage").GetInt32());
        Assert.Equal("rates", progress.Json.GetProperty("NextComponent").GetString());
        var report = workspace.Read(progress.Json.GetProperty("Review").GetString()!);
        Assert.Empty(report.GetProperty("Runs").EnumerateArray());
        Assert.Empty(report.GetProperty("Coverage").EnumerateArray());
        Assert.Equal("sample", Assert.Single(report.GetProperty("MissingCoverage").EnumerateArray()).GetString());
    }

    [NativeCliFact]
    public async Task UnsupportedComparisonKeepsReviewIncompleteAlongsidePassingComparisons()
    {
        using var workspace = new TestWorkspace();
        var passing = workspace.Write("passing.json", new[] { Example("sample", 7, 7) });
        var observation = workspace.Write("observation.json", new[] { new ProbeInput("unsupported", [9]) });
        Assert.Equal(0, (await Run("check", FixtureAssembly, passing, workspace.Path, "probe-batch")).ExitCode);
        Assert.Equal(1, (await Run("check-observe", FixtureAssembly, observation, workspace.Path, "probe-batch")).ExitCode);
        Assert.Equal(0, (await Run("check-review", workspace.Path)).ExitCode);

        var unsupported = workspace.Write("unsupported.json", new[] { Example("unsupported", 9, 9) });
        var check = await Run("check", FixtureAssembly, unsupported, workspace.Path, "probe-batch");
        var review = await Run("check-review", workspace.Path);

        Assert.Equal(1, check.ExitCode);
        Assert.Equal(1, review.ExitCode);
        Assert.False(review.Json.GetProperty("Complete").GetBoolean());
        Assert.Equal(1, review.Json.GetProperty("UnsupportedComparisons").GetInt32());
        Assert.Equal(0, review.Json.GetProperty("Outstanding").GetInt32());
        Assert.Equal(0, review.Json.GetProperty("UnstableInputs").GetInt32());
    }

    [NativeCliFact]
    public async Task SelectedRunValidatesOnlyItsOwnAdapterEvidence()
    {
        using var workspace = new TestWorkspace();
        var cases = workspace.Write("cases.json", new[] { Example("sample", 7, 7) });
        var first = await Run("check", FixtureAssembly, cases, workspace.Path, "probe-batch");
        var other = await Run("check", FixtureAssembly, cases, workspace.Path, "alternate-probe-batch");
        Assert.Equal(0, first.ExitCode);
        Assert.Equal(0, other.ExitCode);
        var firstRun = first.Json.GetProperty("Run").GetString()!;
        var otherRun = other.Json.GetProperty("Run").GetString()!;
        File.Delete(Path.Combine(other.Json.GetProperty("Artifacts").GetString()!, "observed.json"));

        var review = await Run("check-review", workspace.Path, "--run", firstRun);
        var damaged = await Run("check-review", workspace.Path, "--run", otherRun);

        Assert.True(review.ExitCode == 0, review.Error);
        Assert.True(review.Json.GetProperty("Complete").GetBoolean());
        var report = workspace.Read(review.Json.GetProperty("Review").GetString()!);
        Assert.Equal(firstRun, Assert.Single(report.GetProperty("Runs").EnumerateArray()).GetString());
        Assert.Equal(1, damaged.ExitCode);
        Assert.Contains("observed.json", damaged.Error, StringComparison.Ordinal);
    }

    [NativeCliFact]
    public async Task ReviewDoesNotCombineCoverageFromSeparateAdapterRuns()
    {
        using var workspace = new TestWorkspace();
        var first = workspace.Write("first.json", new[] { Example("first", 7, 7) });
        var second = workspace.Write("second.json", new[] { Example("second", 9, 9) });
        var firstCheck = await Run("check", FixtureAssembly, first, workspace.Path, "probe-batch");
        var secondCheck = await Run("check", FixtureAssembly, second, workspace.Path, "alternate-probe-batch");
        Assert.Equal(0, firstCheck.ExitCode);
        Assert.Equal(0, secondCheck.ExitCode);
        var acknowledgments = workspace.Write("acknowledgments.json", Array.Empty<object>());
        var scope = workspace.Write("scope.json", new { Operations = new[] { "first", "second" }, Ordinary = Array.Empty<object>() });

        var aggregate = await Run("check-review", workspace.Path, acknowledgments, scope);
        var scoped = await Run("check-review", workspace.Path, acknowledgments, scope, "--run", firstCheck.Json.GetProperty("Run").GetString()!);

        Assert.Equal(1, aggregate.ExitCode);
        Assert.True(aggregate.Json.GetProperty("RequiresRunSelection").GetBoolean());
        Assert.Equal(2, aggregate.Json.GetProperty("EvidenceSets").GetArrayLength());
        Assert.Equal(1, scoped.ExitCode);
        Assert.False(scoped.Json.GetProperty("Complete").GetBoolean());
        Assert.False(scoped.Json.GetProperty("RequiresRunSelection").GetBoolean());
        var review = workspace.Read(scoped.Json.GetProperty("Review").GetString()!);
        Assert.Equal("second", Assert.Single(review.GetProperty("MissingCoverage").EnumerateArray()).GetString());
    }

    [NativeCliFact]
    public async Task ReviewRejectsResultsInconsistentWithSavedObservations()
    {
        using var workspace = new TestWorkspace();
        var input = workspace.Write("cases.json", new[] { Example("steady", 1, 1) });
        var check = await Run("check", FixtureAssembly, input, workspace.Path, "probe-batch");
        var reportPath = check.Json.GetProperty("Report").GetString()!;
        var report = JsonNode.Parse(await File.ReadAllTextAsync(reportPath))!;
        report["Result"]!["Findings"] = new JsonArray();
        await File.WriteAllTextAsync(reportPath, report.ToJsonString());

        var review = await Run("check-review", workspace.Path);

        Assert.Equal(1, review.ExitCode);
        Assert.Contains("disagrees with the saved inputs and observations", review.Error, StringComparison.Ordinal);
    }

    [NativeCliFact]
    public async Task ReviewFlagsConflictingObservationsEvenWhenBothChecksMatchTheirExpectations()
    {
        using var workspace = new TestWorkspace();
        var single = workspace.Write("single.json", new[] { Example("changes", 0, 3) });
        var multiple = workspace.Write("multiple.json",
            new[] { Example("changes", 0, 2), Example("other", 1, 1), Example("other", 2, 2),
                Example("other", 3, 3), Example("other", 4, 4) });
        Assert.Equal(0, (await Run("check", FixtureAssembly, single, workspace.Path, "probe-batch")).ExitCode);
        Assert.Equal(0, (await Run("check", FixtureAssembly, multiple, workspace.Path, "probe-batch")).ExitCode);

        var review = await Run("check-review", workspace.Path);

        Assert.Equal(1, review.ExitCode);
        Assert.Equal(0, review.Json.GetProperty("Outstanding").GetInt32());
        Assert.Equal(1, review.Json.GetProperty("UnstableInputs").GetInt32());
        Assert.False(review.Json.GetProperty("Complete").GetBoolean());
    }

    [NativeCliFact]
    public async Task ChangedRetestStaysOutsideTheReviewEvidenceAndDoesNotAcknowledgeTheFinding()
    {
        using var workspace = new TestWorkspace();
        var inputs = Enumerable.Range(0, 5).Select(index => Example("changes", index, 1)).ToArray();
        var original = await Run("check", FixtureAssembly, workspace.Write("cases.json", inputs), workspace.Path, "probe-batch");
        var decisions = workspace.Write("decisions.json", new[] { new { Index = 0, Reason = "Contract returns one." } });
        var acknowledgments = workspace.Write("acknowledgments.json", Array.Empty<object>());
        var run = original.Json.GetProperty("Run").GetString()!;

        var retest = await Run("check-retest", FixtureAssembly, workspace.Path, run, decisions, acknowledgments);

        Assert.Equal(1, retest.ExitCode);
        Assert.Contains("did not reproduce", retest.Error, StringComparison.Ordinal);
        Assert.Single(Directory.GetDirectories(Path.Combine(workspace.Path, ".slophawk", "checks")));
        Assert.Single(Directory.GetDirectories(Path.Combine(workspace.Path, ".slophawk", "retest-attempts")));
        Assert.Equal(0, workspace.Read(acknowledgments).GetArrayLength());
    }

    private static string FixtureAssembly => typeof(Commands).Assembly.Location;

    private static ProbeExpectation Example(string operation, long argument, long expected) =>
        new(operation, [argument], JsonSerializer.SerializeToElement(expected));

    private static async Task<(int ExitCode, JsonElement Json, string Error)> Run(params string[] args)
    {
        var start = NativeCliFactAttribute.Command(args);
        start.RedirectStandardOutput = true;
        start.RedirectStandardError = true;
        using var process = Process.Start(start)!;
        using var timeout = new CancellationTokenSource(TimeSpan.FromSeconds(30));
        var output = process.StandardOutput.ReadToEndAsync(timeout.Token);
        var error = process.StandardError.ReadToEndAsync(timeout.Token);
        await process.WaitForExitAsync(timeout.Token);
        var json = await output;
        return (process.ExitCode, string.IsNullOrWhiteSpace(json) ? default : JsonSerializer.Deserialize<JsonElement>(json),
            await error);
    }

    private sealed class TestWorkspace : IDisposable
    {
        internal string Path { get; } = Directory.CreateTempSubdirectory("slophawk-evidence-").FullName;

        internal string Write<T>(string name, T value)
        {
            var path = System.IO.Path.Combine(Path, name);
            File.WriteAllText(path, JsonSerializer.Serialize(value));
            return path;
        }

        internal JsonElement Read(string path) => JsonSerializer.Deserialize<JsonElement>(File.ReadAllText(path));
        public void Dispose() => Directory.Delete(Path, recursive: true);
    }
}
