using System.Diagnostics;
using System.Security.Cryptography;
using System.Text.Json;
using System.Text.Json.Nodes;
using SlopHawk.Core;
using SlopHawk.WorkerFixture;
using Xunit;

namespace SlopHawk.IntegrationTests;

[Collection(NativeCliCollection.Name)]
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
        Assert.False(summary.TryGetProperty("SelectionAudit", out _));
        Assert.Equal(3, summary.GetProperty("ReplayChanges").GetArrayLength());
        Assert.True(File.Exists(summary.GetProperty("ReplayReport").GetString()));
        var reviewPath = result.Json.GetProperty("Review").GetString()!;
        Assert.Equal(reviewPath, summary.GetProperty("Review").GetString());
        var review = workspace.Read(reviewPath);
        Assert.False(review.TryGetProperty("SelectionAudit", out _));
        Assert.Equal("slophawk-contract-review-v1", review.GetProperty("Format").GetString());
        Assert.Equal(stable.GetProperty("Run").GetString(), review.GetProperty("Run").GetString());
        Assert.Equal(stable.GetProperty("Candidate").GetString(), review.GetProperty("Candidate").GetString());
        Assert.Equal(stable.GetProperty("Result").GetRawText(), review.GetProperty("Result").GetRawText());
        Assert.Equal(operations.Length, review.GetProperty("Operations").GetArrayLength());
        Assert.Equal(3, review.GetProperty("ReplayChanges").GetInt32());
        var requestsPath = result.Json.GetProperty("ReplayRequests").GetString()!;
        Assert.Equal(requestsPath, summary.GetProperty("ReplayRequests").GetString());
        Assert.Equal(requestsPath, review.GetProperty("ReplayRequests").GetString());
        var request = Assert.Single(workspace.Read(requestsPath).EnumerateArray());
        Assert.True(JsonElement.DeepEquals(finding.GetProperty("Input"), request));
    }

    [NativeCliFact]
    public async Task BoundaryPairsReplayOverflowAndAcceptTheWideCalculation()
    {
        using var workspace = new TestWorkspace();
        string[] names = ["narrow-span", "wide-span"];
        const string domain = "lo >= -2147483648 && hi <= 2147483647 && lo < hi";
        var endpoints = new Dictionary<string, long[]>
        {
            ["lo"] = [int.MinValue, int.MaxValue],
            ["hi"] = [int.MinValue, int.MaxValue]
        };
        var rules = workspace.Write("rules.json", names.Select(name =>
            new IntegerContract(name, ["lo", "hi"], domain, "hi - lo", "interval.md", Basis: endpoints)).ToArray());
        var examples = workspace.Write("examples.json", names.Select(name =>
            new ProbeExpectation(name, [0, 10], JsonSerializer.SerializeToElement(10))).ToArray());

        var result = await Run("check-contracts", "--boundary-pairs", FixtureAssembly, rules, examples,
            workspace.Path, "2", "probe-batch");

        Assert.True(result.ExitCode == 1, result.Error);
        Assert.Equal(4, Count(result.Json, "Compared"));
        Assert.Equal(0, Count(result.Json, "Unsupported"));
        Assert.Equal(0, Count(result.Json, "ReplayChanges"));
        var summary = workspace.Read(result.Json.GetProperty("Summary").GetString()!);
        Assert.Equal("boundary-pairs-v1", summary.GetProperty("CaseSelection").GetString());
        var review = workspace.Read(result.Json.GetProperty("Review").GetString()!);
        var finding = Assert.Single(review.GetProperty("Result").GetProperty("Findings").EnumerateArray());
        var input = finding.GetProperty("Input");
        Assert.Equal("narrow-span", input.GetProperty("Operation").GetString());
        Assert.Equal(new long[] { int.MinValue, int.MaxValue },
            input.GetProperty("Args").EnumerateArray().Select(arg => arg.GetInt64()).ToArray());
        Assert.Equal(-1, finding.GetProperty("Candidate").GetInt64());
        Assert.Equal(4_294_967_295L, finding.GetProperty("Reference").GetInt64());
    }

    [NativeCliFact]
    public async Task DiverseContractSearchAuditsExecutedCasesAndSeparateReplay()
    {
        using var workspace = new TestWorkspace();
        var rules = workspace.Write("rules.json", new[]
        {
            new IntegerContract("sample", ["x"], "x >= 0", "x", "rules.md")
        });
        var examples = workspace.Write("examples.json", new[] { Example("sample", 5, 5) });

        var result = await Run("check-contracts", "--diverse", FixtureAssembly, rules, examples,
            workspace.Path, "3", "probe-batch");

        Assert.True(result.ExitCode == 0, result.Error);
        Assert.False(result.Json.TryGetProperty("SelectionAudit", out _));
        var summary = workspace.Read(result.Json.GetProperty("Summary").GetString()!);
        var review = workspace.Read(result.Json.GetProperty("Review").GetString()!);
        var audit = AssertSelectionAudit(summary, review);
        var pass = Assert.Single(audit.GetProperty("Passes").EnumerateArray());
        Assert.Equal("sample", pass.GetProperty("Operation").GetString());
        Assert.Equal(0, Count(pass, "Pass"));
        Assert.Equal(3, Count(pass, "Budget"));
        Assert.Equal(12, Count(pass, "PoolLimit"));
        Assert.Equal("completed", pass.GetProperty("Generation").GetString());
        Assert.InRange(Count(pass, "Nominated"), 3, 12);
        Assert.Equal(0, Count(pass, "PreviouslyExecuted"));
        Assert.Equal(3, Count(pass, "Selected"));
        Assert.Equal(0, Count(pass, "SkippedAfterFinding"));
        Assert.Equal(0, Count(pass, "SkippedDuplicate"));
        Assert.Equal(3, Count(pass, "Executed"));
        Assert.Equal(3, Count(pass, "Compared"));
        Assert.Equal(0, Count(pass, "Unsupported"));
        Assert.Equal(0, Count(pass, "UnusedBudget"));

        var initial = Assert.Single(audit.GetProperty("Batches").EnumerateArray());
        Assert.Equal(0, Count(initial, "Pass"));
        Assert.Equal(3, Count(initial, "Executed"));
        Assert.Equal(pass.GetProperty("SelectedInputsSha256").GetString(),
            initial.GetProperty("InputsSha256").GetString());
        var initialRequests = AssertAuditedBatch(workspace, initial, "sample", unsupported: false);
        var replay = audit.GetProperty("Replay");
        Assert.Equal(1, Count(replay, "Executed"));
        Assert.Equal(1, Count(replay, "Compared"));
        Assert.Equal(0, Count(replay, "Unsupported"));
        Assert.NotEqual(initial.GetProperty("Run").GetString(), replay.GetProperty("Run").GetString());
        Assert.Equal(result.Json.GetProperty("Run").GetString(), replay.GetProperty("Run").GetString());
        Assert.Equal(result.Json.GetProperty("Report").GetString(), replay.GetProperty("Report").GetString());
        Assert.Equal(summary.GetProperty("ReplayReport").GetString(), replay.GetProperty("Report").GetString());
        var replayRequests = AssertAuditedBatch(workspace, replay, "sample", unsupported: false);
        Assert.True(JsonElement.DeepEquals(initialRequests[0], Assert.Single(replayRequests)));
        // Initial allocation and the separately confirmed witness have different budgets.
        Assert.Equal(3, Count(summary, "Compared"));
        Assert.Equal(3, Count(review, "Compared"));
        Assert.Equal(1, Count(review.GetProperty("Result"), "Compared"));
        Assert.Equal(0, Count(summary, "ObservedDiscrepancies"));
        Assert.Equal(0, Count(summary, "Findings"));
    }

    [NativeCliFact]
    public async Task DiverseUnsupportedObservationsSpendBudgetWithoutClaimingComparisons()
    {
        using var workspace = new TestWorkspace();
        var rules = workspace.Write("rules.json", new[]
        {
            new IntegerContract("unsupported", ["x"], "x >= 0", "x", "rules.md")
        });
        var examples = workspace.Write("examples.json", new[] { Example("unsupported", 5, 5) });

        var result = await Run("check-contracts", "--diverse", FixtureAssembly, rules, examples,
            workspace.Path, "3", "probe-batch");

        Assert.Equal(1, result.ExitCode);
        var summary = workspace.Read(result.Json.GetProperty("Summary").GetString()!);
        var review = workspace.Read(result.Json.GetProperty("Review").GetString()!);
        var audit = AssertSelectionAudit(summary, review);
        var pass = Assert.Single(audit.GetProperty("Passes").EnumerateArray());
        Assert.Equal("unsupported", pass.GetProperty("Operation").GetString());
        Assert.Equal("completed", pass.GetProperty("Generation").GetString());
        Assert.Equal(3, Count(pass, "Budget"));
        Assert.Equal(3, Count(pass, "Selected"));
        Assert.Equal(3, Count(pass, "Executed"));
        Assert.Equal(0, Count(pass, "Compared"));
        Assert.Equal(3, Count(pass, "Unsupported"));
        Assert.Equal(0, Count(pass, "UnusedBudget"));
        var initial = Assert.Single(audit.GetProperty("Batches").EnumerateArray());
        Assert.Equal(3, Count(initial, "Executed"));
        Assert.Equal(0, Count(initial, "Compared"));
        Assert.Equal(3, Count(initial, "Unsupported"));
        AssertAuditedBatch(workspace, initial, "unsupported", unsupported: true);
        Assert.Equal(JsonValueKind.Null, audit.GetProperty("Replay").ValueKind);
        Assert.Equal(JsonValueKind.Null, result.Json.GetProperty("Report").ValueKind);
        Assert.Equal(JsonValueKind.Null, result.Json.GetProperty("Run").ValueKind);
        Assert.Equal(JsonValueKind.Null, summary.GetProperty("ReplayReport").ValueKind);
        Assert.Equal(JsonValueKind.Null, review.GetProperty("Result").ValueKind);
        Assert.Equal(0, Count(summary, "Compared"));
        Assert.Equal(3, Count(summary, "Unsupported"));
        Assert.Equal(0, Count(summary, "ReplayUnsupported"));
        Assert.Equal(0, Count(summary, "Findings"));
    }

    [NativeCliFact]
    public async Task DiversePublishedSingletonDoesNotClaimGeneratedOrExecutedCoverage()
    {
        using var workspace = new TestWorkspace();
        var rules = workspace.Write("rules.json", new[]
        {
            new IntegerContract("sample", ["x"], "x == 5", "x", "rules.md")
        });
        var examples = workspace.Write("examples.json", new[] { Example("sample", 5, 5) });

        var result = await Run("check-contracts", "--diverse", FixtureAssembly, rules, examples,
            workspace.Path, "3", "probe-batch");

        Assert.Equal(1, result.ExitCode);
        Assert.Equal(0, Count(result.Json, "Unqualified"));
        var summary = workspace.Read(result.Json.GetProperty("Summary").GetString()!);
        var review = workspace.Read(result.Json.GetProperty("Review").GetString()!);
        var audit = AssertSelectionAudit(summary, review);
        var pass = Assert.Single(audit.GetProperty("Passes").EnumerateArray());
        Assert.Equal("sample", pass.GetProperty("Operation").GetString());
        Assert.Equal("completed", pass.GetProperty("Generation").GetString());
        Assert.Equal(3, Count(pass, "Budget"));
        Assert.Equal(12, Count(pass, "PoolLimit"));
        foreach (var counter in new[] { "Nominated", "PreviouslyExecuted", "Eligible", "Selected",
            "NotSelected", "SkippedAfterFinding", "SkippedDuplicate", "Executed", "Compared", "Unsupported" })
            Assert.Equal(0, Count(pass, counter));
        Assert.Equal(3, Count(pass, "UnusedBudget"));
        var emptyInputsSha256 = Convert.ToHexString(SHA256.HashData("[]"u8));
        Assert.Equal(emptyInputsSha256, pass.GetProperty("NominatedInputsSha256").GetString());
        Assert.Equal(emptyInputsSha256, pass.GetProperty("SelectedInputsSha256").GetString());
        Assert.Empty(audit.GetProperty("Batches").EnumerateArray());
        Assert.Equal(JsonValueKind.Null, audit.GetProperty("Replay").ValueKind);
        Assert.Equal(JsonValueKind.Null, result.Json.GetProperty("Report").ValueKind);
        Assert.Equal(JsonValueKind.Null, result.Json.GetProperty("Run").ValueKind);
        Assert.Equal(JsonValueKind.Null, summary.GetProperty("ReplayReport").ValueKind);
        Assert.Equal(JsonValueKind.Null, review.GetProperty("Result").ValueKind);
        Assert.Equal(0, Count(summary, "Compared"));
        Assert.Equal(0, Count(summary, "Unsupported"));
        Assert.Equal(0, Count(Assert.Single(summary.GetProperty("Operations").EnumerateArray()), "Compared"));
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

    private static int Count(JsonElement value, string name) => value.GetProperty(name).GetInt32();

    private static JsonElement AssertSelectionAudit(JsonElement summary, JsonElement review)
    {
        Assert.Equal("diverse-v1", summary.GetProperty("CaseSelection").GetString());
        var audit = summary.GetProperty("SelectionAudit");
        Assert.True(JsonElement.DeepEquals(audit, review.GetProperty("SelectionAudit")));
        Assert.Equal("slophawk-contract-selection-audit-v1", audit.GetProperty("Format").GetString());
        Assert.Equal("emitted-legal-nonpublished-cases", audit.GetProperty("GeneratorBasis").GetString());
        Assert.Equal("validated-observations", audit.GetProperty("ExecutionBasis").GetString());
        foreach (var pass in audit.GetProperty("Passes").EnumerateArray())
        {
            Assert.Equal(Count(pass, "Nominated"), Count(pass, "PreviouslyExecuted") + Count(pass, "Eligible"));
            Assert.Equal(Count(pass, "Eligible"), Count(pass, "Selected") + Count(pass, "NotSelected"));
            Assert.Equal(Count(pass, "Selected"), Count(pass, "Executed")
                + Count(pass, "SkippedAfterFinding") + Count(pass, "SkippedDuplicate"));
            Assert.Equal(Count(pass, "Budget"), Count(pass, "Executed") + Count(pass, "UnusedBudget"));
            Assert.Equal(Count(pass, "Executed"), Count(pass, "Compared") + Count(pass, "Unsupported"));
            Assert.Matches("^[0-9A-F]{64}$", pass.GetProperty("NominatedInputsSha256").GetString()!);
            Assert.Matches("^[0-9A-F]{64}$", pass.GetProperty("SelectedInputsSha256").GetString()!);
        }
        return audit;
    }

    private static JsonElement[] AssertAuditedBatch(TestWorkspace workspace, JsonElement batch,
        string operation, bool unsupported)
    {
        var reportPath = batch.GetProperty("Report").GetString()!;
        Assert.True(File.Exists(reportPath));
        var directory = Path.GetDirectoryName(reportPath)!;
        var requestsPath = Path.Combine(directory, "requests.json");
        var observedPath = Path.Combine(directory, "observed.json");
        var identityPath = Path.Combine(directory, "identity.json");
        Assert.True(File.Exists(requestsPath));
        Assert.True(File.Exists(observedPath));
        Assert.True(File.Exists(identityPath));
        var report = workspace.Read(reportPath);
        var requests = workspace.Read(requestsPath).EnumerateArray().ToArray();
        var observed = workspace.Read(observedPath).EnumerateArray().ToArray();
        var expected = report.GetProperty("Expected").EnumerateArray().ToArray();
        var identity = workspace.Read(identityPath);
        Assert.Equal(batch.GetProperty("Run").GetString(), report.GetProperty("Run").GetString());
        Assert.Equal(batch.GetProperty("Run").GetString(), identity.GetProperty("Run").GetString());
        Assert.Equal(Convert.ToHexString(SHA256.HashData(File.ReadAllBytes(requestsPath))),
            batch.GetProperty("InputsSha256").GetString());
        Assert.Equal(identity.GetProperty("RequestsSha256").GetString(), batch.GetProperty("InputsSha256").GetString());
        Assert.Equal(Count(batch, "Executed"), requests.Length);
        Assert.Equal(requests.Length, observed.Length);
        Assert.Equal(requests.Length, expected.Length);
        Assert.Equal(requests.Length, Count(report.GetProperty("Result"), "Cases"));
        Assert.Equal(Count(batch, "Compared"), Count(report.GetProperty("Result"), "Compared"));
        Assert.Equal(Count(batch, "Unsupported"), Count(report.GetProperty("Result"), "Unsupported"));
        Assert.Empty(report.GetProperty("Result").GetProperty("Findings").EnumerateArray());
        for (var index = 0; index < requests.Length; index++)
        {
            Assert.Equal(operation, requests[index].GetProperty("Operation").GetString());
            var argument = Assert.Single(requests[index].GetProperty("Args").EnumerateArray()).GetInt64();
            Assert.InRange(argument, 0L, long.MaxValue);
            Assert.NotEqual(5L, argument); // The published anchor never spends the new-case budget.
            Assert.Equal(argument, expected[index].GetInt64()); // Public identity rule, independent of the adapter.
            if (unsupported)
                Assert.Equal(JsonValueKind.Null, observed[index].ValueKind);
            else
                Assert.Equal(argument, observed[index].GetInt64());
        }
        return requests;
    }

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
