using System.Diagnostics;
using System.Security.Cryptography;
using System.Text.Json;
using Microsoft.Data.Sqlite;
using SlopHawk.Daemon;

if (args is ["fingerprint", var snapshot] && Path.IsPathFullyQualified(snapshot))
{
    Snapshot.Report(snapshot);
    return 0;
}

if (args is ["matrix", var suite] && Path.IsPathFullyQualified(suite))
{
    await EffortMatrix.Package(suite, Dotnet);
    return 0;
}

if (args is ["incidents", var incidentSuite] && Path.IsPathFullyQualified(incidentSuite))
{
    await IncidentMatrix.Package(incidentSuite, Dotnet);
    return 0;
}

if (args is ["wide", var corpus] && Path.IsPathFullyQualified(corpus))
{
    await WideReview.Package(corpus, Dotnet);
    return 0;
}

if (args is ["adjudicate", var corpusPath, var reports, var score] &&
    Path.IsPathFullyQualified(corpusPath) && Path.IsPathFullyQualified(reports) && Path.IsPathFullyQualified(score))
{
    await WideReview.Adjudicate(corpusPath, reports, score, Dotnet);
    return 0;
}

var hard = args is ["pack-hard", _];
var pack = args is ["pack", _] || hard;
if (pack)
    args = args[1..];
if (args is not [var destination] || !Path.IsPathFullyQualified(destination) ||
    Directory.Exists(destination) || File.Exists(destination))
{
    Console.Error.WriteLine("Usage: Qualification [pack|pack-hard] NEW_ABSOLUTE_SCRATCH_DIRECTORY");
    return 2;
}

var root = Path.GetFullPath(destination);
Directory.CreateDirectory(root);
var sdk = (await Dotnet(root, "--version")).Trim();
var pilot = pack ? new Pilot(root, hard) : null;

await QualifyBuildPlan(Path.Combine(root, "planning"), pilot);
await QualifyReuse(pilot?.Rating ?? Path.Combine(root, "reuse"), sdk, pilot);
QualifyReceipt(Path.Combine(root, "reconciliation"), pilot);
QualifyCoverage(Path.Combine(root, "coverage"), pilot);
if (pilot is not null)
    await pilot.Replay(root, Dotnet);
Console.WriteLine("Four counterexamples and their neighboring controls qualified.");
return 0;

static async Task QualifyBuildPlan(string root, Pilot? pilot)
{
    var checkout = pilot?.Dispatch ?? Path.Combine(root, "checkout");
    const string current = "services/v2/Tariff/Tariff.csproj";
    const string legacy = "services/v1/Tariff/Tariff.csproj";
    const string router = "services/Route/Route.csproj";
    const string gate = "verification/Release.Check/Release.Check.csproj";

    Write(checkout, current, Project("<AssemblyName>RegionalTariff</AssemblyName>"));
    Write(checkout, legacy, Project("<AssemblyName>LegacyTariff</AssemblyName>"));
    Write(checkout, "services/v2/Tariff/Rules.cs", """
        namespace Tariff;
        public static class Rules { public static string Stamp => "D17"; }
        """);
    Write(checkout, "services/v1/Tariff/Rules.cs", """
        namespace Tariff;
        public static class Rules { public static string Stamp => "D16"; }
        """);
    Write(checkout, router, Project("", "../v2/Tariff/Tariff.csproj"));
    Write(checkout, "services/Route/Dispatch.cs", """
        namespace Route;
        public static class Dispatch { public static string QuoteStamp() => Tariff.Rules.Stamp; }
        """);
    Write(checkout, gate, Project("<OutputType>Exe</OutputType><IsTestProject>true</IsTestProject>", "../../services/Route/Route.csproj"));
    Write(checkout, "verification/Release.Check/Program.cs", """
        if (Route.Dispatch.QuoteStamp() != "D17")
            throw new System.InvalidOperationException("Unexpected release profile.");
        System.Console.WriteLine("D17");
        """);

    await Dotnet(checkout, "build", gate, "-p:NuGetAudit=false");
    await Dotnet(checkout, "restore", legacy, "-p:NuGetAudit=false");
    var dll = Path.Combine(checkout, "verification/Release.Check/bin/Debug/net10.0/Release.Check.dll");
    Require((await Dotnet(checkout, dll)).Trim() == "D17", "The ordinary smoke gate should pass.");

    var workspace = new Workspace(checkout, Path.Combine(root, "state"));
    var evaluator = new ProjectEvaluator(workspace);
    var graph = await evaluator.EvaluateAsync();
    Require(graph.Complete, "Expected a complete real project graph.");
    var selected = graph.Targets.Single(target => target.ProjectPath == gate);
    var route = graph.Targets.Single(target => target.ProjectPath == router);
    var regional = graph.Targets.Single(target => target.ProjectPath == current);
    Require(selected.References.SequenceEqual([route.Id]) && route.References.SequenceEqual([regional.Id]),
        "Expected the release gate to reach regional tariff through the router.");

    var results = Path.Combine(root, "attempt");
    var allowed = new[] { gate, router, current }.Select(path => Path.GetDirectoryName(Path.Combine(checkout, path))!)
        .SelectMany(dir => new[] { Path.Combine(dir, "bin"), Path.Combine(dir, "obj") }).Append(results).ToArray();
    var grant = new ExecutionGrant("qualification", RootIdentity.Read(checkout),
        ExecutionOperation.Build | ExecutionOperation.CompilerExtensions, allowed);
    TestBuildPreparation Plan() => evaluator.PrepareBuild(graph, selected.Id, results, grant);
    var initial = Plan();
    Require(initial.Ready, $"Clean receipt plan not ready: {initial.Blocker} {initial.Detail}");

    var currentAssets = Path.GetFullPath(Path.Combine(checkout, "services/v2/Tariff/obj/project.assets.json"));
    var legacyAssets = Path.GetFullPath(Path.Combine(checkout, "services/v1/Tariff/obj/project.assets.json"));
    var priorCurrent = File.ReadAllBytes(currentAssets);
    var priorLegacy = File.ReadAllBytes(legacyAssets);
    Require(RestorePath(priorCurrent) == Path.GetFullPath(Path.Combine(checkout, current)) &&
            RestorePath(priorLegacy) == Path.GetFullPath(Path.Combine(checkout, legacy)),
        "The copied restore files must be real, separately owned assets.");
    File.WriteAllBytes(legacyAssets, priorCurrent);
    Require(Plan().Ready, "A wrong artifact outside the selected closure should leave the plan ready.");
    File.WriteAllBytes(legacyAssets, priorLegacy);

    File.WriteAllBytes(currentAssets, priorLegacy);
    var blocked = Plan();
    Require(blocked.Blocker == TestBuildBlocker.IncompatibleAssets &&
            blocked.Detail?.Contains(currentAssets, StringComparison.OrdinalIgnoreCase) == true,
        $"Expected the planner to block the transitive restore: {blocked.Blocker} {blocked.Detail}");
    Require((await Dotnet(checkout, dll)).Trim() == "D17",
        "The unchanged compiled smoke executable should still pass over a stale restore archive.");
    File.WriteAllBytes(currentAssets, priorCurrent);
    Require(Plan().Ready, "Restoring the correct single artifact should repair the plan.");
    Console.WriteLine("Planning: smoke D17; wrong bytes under the active leaf block; identical substitution outside the closure stays ready.");
    if (pilot?.Harder == true)
    {
        const string calendar = "services/Calendar/Calendar.csproj";
        Write(checkout, calendar, Project(""));
        Write(checkout, "services/Calendar/Season.cs", "namespace Calendar; public static class Season { public const string Stamp = \"D17\"; }");
        Write(checkout, current, Project("<AssemblyName>RegionalTariff</AssemblyName>", "../../Calendar/Calendar.csproj"));
        Write(checkout, "services/v2/Tariff/Rules.cs", "namespace Tariff; public static class Rules { public static string Stamp => Calendar.Season.Stamp; }");
        await Dotnet(checkout, "build", gate, "-p:NuGetAudit=false");
        graph = await evaluator.EvaluateAsync();
        var selectedCalendar = graph.Targets.Single(target => target.ProjectPath == calendar);
        Require(graph.Complete && graph.Targets.Single(target => target.ProjectPath == current).References.Contains(selectedCalendar.Id),
            "The current evaluated tariff must reference the shared calendar.");
        var calendarRoot = Path.GetDirectoryName(Path.Combine(checkout, calendar))!;
        grant = grant with { OutputRoots = [.. allowed, Path.Combine(calendarRoot, "bin"), Path.Combine(calendarRoot, "obj")] };
        var updatedAssets = File.ReadAllBytes(currentAssets);
        Require(RestorePath(priorCurrent) == RestorePath(updatedAssets) && Plan().Ready,
            "Both restores should name the same project; the current graph should be ready.");
        File.WriteAllBytes(currentAssets, priorCurrent);
        var old = Plan();
        Require(old.Blocker == TestBuildBlocker.IncompatibleAssets && old.Detail?.EndsWith(" references", StringComparison.Ordinal) == true,
            $"Expected the old restore edge set to block: {old.Blocker} {old.Detail}");
        Require((await Dotnet(checkout, dll)).Trim() == "D17", "The rebuilt smoke should stay green with the stale receipt.");
        pilot.KeepPlan(graph, gate, legacy, currentAssets, priorCurrent, updatedAssets);
        Console.WriteLine("Handoff: same restore owner and framework; the restored reference set is stale.");
    }
    else
        pilot?.KeepPlan(graph, gate, legacy, currentAssets, priorLegacy, priorCurrent);
}

static async Task QualifyReuse(string root, string sdk, Pilot? pilot)
{
    Write(root, "Quote.csproj", Project("<OutputType>Exe</OutputType>"));
    Write(root, "Program.cs", """
        using System.IO;
        using System.Text.Json;

        var root = args[0];
        using var routes = JsonDocument.Parse(File.ReadAllText(Path.Combine(root, "catalog/index.json")));
        var name = routes.RootElement.GetProperty(args[1]).GetString()!;
        using var rate = JsonDocument.Parse(File.ReadAllText(Path.Combine(root, name)));
        System.Console.WriteLine(rate.RootElement.GetProperty("limit").GetInt32());
        """);
    Write(root, "catalog/index.json", """
        {"branch":"assets/regional.json","fallback":"assets/standard.json"}
        """);
    Write(root, "assets/regional.json", """{"limit":16}""");
    Write(root, "assets/standard.json", """{"limit":16}""");
    Write(root, "assets/local-override.json", """{"limit":16}""");
    await Dotnet(root, "build", "Quote.csproj", "-p:NuGetAudit=false");

    const string assembly = "bin/Debug/net10.0/Quote.dll";
    string[] outputs = [assembly, "bin/Debug/net10.0/Quote.deps.json", "bin/Debug/net10.0/Quote.runtimeconfig.json"];
    var target = new ProjectTarget("Quote.csproj|net10.0", "Quote.csproj", "net10.0", "Debug", "AnyCPU", "", sdk, false,
        [new("Program.cs", null)], [], Digest(Path.Combine(root, "Quote.csproj")));
    var manifest = new BuildArtifactManifest(target, true, "evaluated-profile-v1", "dotnet/app", "fixture",
        [new("Program.cs", null)], [], ["catalog/index.json"], [], [], outputs);
    var observedManifest = manifest with { RuntimeAssets = ["catalog/index.json", "assets/regional.json"] };
    BuildIdentityStamp Capture(BuildArtifactManifest selected)
    {
        var mapped = outputs.Append("Program.cs").Concat(selected.RuntimeAssets!).Select(path =>
            new BuildArtifactPathMapping(path, Path.GetFullPath(Path.Combine(root, path)))).ToArray();
        var scope = new BuildArtifactReadScope([root], mapped, MappingTrusted: true);
        var result = BuildArtifactCapture.Capture(selected, scope);
        Require(result.Stamp.Complete, "Scoped capture incomplete: " + string.Join(", ", result.Stamp.Problems));
        return result.Stamp;
    }

    async Task<string> Quote(string route) =>
        (await Dotnet(root, assembly, root, route)).Trim();

    var prior = Capture(manifest);
    var priorObserved = Capture(observedManifest);
    if (pilot?.Harder == false)
        pilot.KeepBuildBefore(manifest, prior, outputs);
    Require(await Quote("branch") == "16" && await Quote("fallback") == "16", "Baseline output should be 16.");
    ReplaceAtSameTime(Path.Combine(root, "assets/local-override.json"), """{"limit":17}""");
    Require(await Quote("branch") == "16" && BuildOutputIdentity.Compare(priorObserved, Capture(observedManifest)).Reusable,
        "The nearby inactive override should not affect this target.");
    ReplaceAtSameTime(Path.Combine(root, "assets/regional.json"), """{"limit":17}""");
    Require(await Quote("branch") == "17" && await Quote("fallback") == "16",
        "Only the selected branch should change in a fresh process.");
    Require(BuildOutputIdentity.Compare(prior, Capture(manifest)).Reusable,
        "Omitting the actual runtime input should reproduce the erroneous reuse.");
    var diff = BuildOutputIdentity.Compare(priorObserved, Capture(observedManifest));
    Require(!diff.Reusable && diff.Problems.Any(problem => problem.Field == "runtime-assets"),
        "An independently observed runtime input should invalidate the stamp.");
    Console.WriteLine("Reuse: fallback remains 16, branch becomes 17; omitted asset reuses, captured asset invalidates.");
    if (pilot?.Harder == true)
    {
        ReplaceAtSameTime(Path.Combine(root, "assets/regional.json"), """{"limit":16}""");
        var current = observedManifest with { RuntimeAssets = [.. observedManifest.RuntimeAssets!, "assets/standard.json"] };
        pilot.KeepBuildBefore(current, Capture(current), outputs);
    }
}

static void QualifyReceipt(string root, Pilot? pilot)
{
    var bad = Settle(Path.Combine(root, "cycle-1"), "delta", replaced: true);
    var good = Settle(Path.Combine(root, "cycle-2"), "ember", replaced: false);
    Require(bad == ("superseded", "passed", 1, 0L) && good == ("passed", "passed", 0, 0L),
        $"Expected historical pass with pending intent versus current pass with resolved intent: {bad} / {good}.");
    // The dummy summary looks only at the valid result code. Both records appear green.
    Require(bad.Outcome == good.Outcome, "The unscoped summary should mask the lifecycle distinction.");
    Console.WriteLine("Completion: both outcomes passed; only the current attempt resolved the pending intent.");
    if (pilot?.Harder == true)
    {
        var current = Settle(Path.Combine(root, "cycle-3"), "delta-current", replaced: false);
        Require(current == good, "The selected qualification for the harder dossier must be current.");
    }

    (string State, string? Outcome, int Pending, long Evidence) Settle(string scope, string lane, bool replaced)
    {
        var workspace = new Workspace(Directory.CreateDirectory(Path.Combine(scope, "checkout")).FullName,
            Path.Combine(scope, "state"));
        var store = new SourceInventory(workspace);
        store.Load();
        var old = new ScheduleFence(Guid.NewGuid(), 5, 9, RootIdentity.Read(workspace.Root),
            new GitRevision("refs/heads/release", new string('a', 40)));
        var cause = new TestCause(8, "src/Policy.cs", "src/Policy.cs", "earlier", "src/Policy.cs", "later", "graph_unavailable");
        var selected = new SelectedTarget("unit-net10", new string('D', 64), [cause.Id]);
        var work = new TestWork(new TestSelectionInput(old, [cause]), [], new PendingIntents(1, 8, 8, null), null);
        var selection = new ValidatedSelection(work, new TestSelection(true, [selected]));
        var attempt = new TestAttempt(11, 1, old, selected, "release");
        const string lease = "fixture-lease";
        var details = new TestRunReceiptDetails(11, old.Revision, old.Epoch, old.Sequence, TestRunReceiptDetails.GitIdentity(old.Git),
            old.RootIdentity, selected.Id, selected.InputFingerprint, "release", lease,
            BuildOutputIdentity.DomainVersion, new string('A', 64), new string('B', 64), new string('C', 64),
            214, Guid.NewGuid(), VstestDisposition.Passed, VstestRunnerHealth.ZeroExit, VstestArtifactHealth.Complete,
            VstestExecution.Passed, new VstestTrxCounters(2, 2, 2, 0, 0, 0), "passed");
        details.Validate(attempt, TestOutcome.Passed, lease);

        using var db = new SqliteConnection(new SqliteConnectionStringBuilder
        {
            DataSource = workspace.InventoryPath,
            Pooling = false
        }.ToString());
        db.Open();
        using (var seed = db.CreateCommand())
        {
            seed.CommandText = """
                UPDATE workspace SET revision=9, root_identity=$root;
                INSERT INTO test_intent VALUES (8, 'src/Policy.cs', 'src/Policy.cs', 'earlier', 'src/Policy.cs', 'later', 'graph_unavailable');
                INSERT INTO test_attempt(id, number, revision, epoch, target, fingerprint, causes, plan_key, state, lease_id, profile_revision)
                VALUES (11, 1, 9, $epoch, 'unit-net10', $fingerprint, $causes, 'plan-one', 'running', $lease, 'release');
                """;
            seed.Parameters.AddWithValue("$root", old.RootIdentity);
            seed.Parameters.AddWithValue("$epoch", old.Epoch.ToString());
            seed.Parameters.AddWithValue("$fingerprint", selected.InputFingerprint);
            seed.Parameters.AddWithValue("$causes", JsonSerializer.Serialize(selected.CauseIds));
            seed.Parameters.AddWithValue("$lease", lease);
            seed.ExecuteNonQuery();
        }

        // Move the live incarnation after the original receipt is validated. Keep the
        // commit, target, input hash, and revision equal to make each a plausible decoy.
        var live = replaced ? old with { Epoch = Guid.NewGuid() } : old;
        var settled = store.Finish(attempt, TestOutcome.Passed, work, selection, DateTimeOffset.UtcNow,
            () => live == attempt.Fence, details);
        var reopened = new SourceInventory(workspace);
        Require(reopened.ReadTestRunReceipt(attempt.Id) == details, "The original passed receipt must survive in both cases.");
        pilot?.KeepRun(lane, new RunPacket(attempt, live, lease, details));
        using var query = db.CreateCommand();
        query.CommandText = "SELECT count(*) FROM test_evidence";
        return (settled.Receipt.State, settled.Receipt.Outcome, reopened.Load().Pending.DetailedCount, (long)query.ExecuteScalar()!);
    }
}

static void QualifyCoverage(string root, Pilot? pilot)
{
    const string compatible = "compat-net10";
    const string shipping = "release-net10";
    const string source = "src/Renewal.cs";
    const string alias = "maps/src/Renewal.cs";
    var workspace = pilot?.Dossier ?? root;
    Write(workspace, source, "internal static class Renewal { public static int Window() => 16; }");
    var sourceDigest = Digest(Path.Combine(workspace, source));
    var other = Observe(compatible, "compat", 7, TestOutcome.Failed);
    var actual = Observe(shipping, "release", 0, TestOutcome.Passed);

    // A dashboard projection dropping the target returns a hit for the wrong request.
    var naive = other.Lines.Any(line => line.SourcePath == source && line.Number == 1 && line.Hits > 0);
    Require(naive && other.LineState(shipping, source, 1) == CoverageLineState.Unknown,
        "An observed sibling hit should leave the requested target unknown.");
    Require(other.LineState(compatible, source, 1) == CoverageLineState.Observed && other.Totals?.Outcome == TestOutcome.Failed,
        "A correctly attributed failed run still supplies its actual line hit.");
    Require(actual.LineState(shipping, source, 1) == CoverageLineState.Uncovered,
        "Zero on the requested target should prove uncovered.");
    Console.WriteLine("Coverage: sibling hit is observed on its own target, unknown on the requested target; an explicit requested zero is uncovered.");
    if (pilot?.Harder == true)
    {
        var previous = Observe(shipping, "release-archive", 7, TestOutcome.Passed);
        File.Copy(pilot.PathInDossier(source), pilot.PathInDossier("records/Renewal.prior.cs"));
        ReplaceAtSameTime(pilot.PathInDossier(source), "internal static class Renewal { public static int Window() => 17; }");
        sourceDigest = Digest(pilot.PathInDossier(source));
        var control = Observe(compatible, "compat-current", 7, TestOutcome.Failed);
        Require(previous.LineState(shipping, source, 1) == CoverageLineState.Observed &&
            previous.LineState(shipping, source, 1, sourceDigest) == CoverageLineState.Unknown &&
            control.LineState(compatible, source, 1, sourceDigest) == CoverageLineState.Observed,
            "Only current source bytes should satisfy a coverage query on the current source.");
        Console.WriteLine("Handoff: the archived target/path/line has a hit; current source bytes leave the result unknown.");
    }

    CoberturaImportResult Observe(string target, string folder, int hits, TestOutcome outcome)
    {
        var now = DateTimeOffset.UtcNow;
        var leaseRoot = pilot?.PathInDossier($"records/coverage/{folder}") ?? Path.Combine(root, folder);
        const string report = "coverage.xml";
        Write(leaseRoot, report, $"""
            <coverage lines-covered="{(hits == 0 ? 0 : 1)}" lines-valid="1" branches-covered="0" branches-valid="0" version="1.9">
              <packages><package name="unit"><classes><class name="Renewal" filename="{alias}">
                <lines><line number="1" hits="{hits}" /></lines>
              </class></classes></package></packages>
            </coverage>
            """);
        var attempt = new CoverageSupervisedAttempt(Guid.NewGuid(), 11, target, "build-" + target, "binary-" + target,
            "fixture", "1", "source-map", now.AddMinutes(-1), now.AddSeconds(1), now.AddSeconds(2),
            outcome, Completed: true, CollectorAvailable: true, [new(alias, target, source, sourceDigest)]);
        var lease = new CoverageResultsLease(attempt.Epoch, attempt.AttemptId, target, attempt.BuildIdentity, attempt.BinaryIdentity,
            attempt.Collector, attempt.CollectorVersion, attempt.SourceIdentity, leaseRoot, now.AddSeconds(-30), OwnedAndDrained: true);
        var captured = CoverageReceiptCapture.Capture(attempt, lease, new CoverageArtifactDeclaration(report));
        Require(captured.Result.State == CoberturaImportState.Imported && captured.Receipt is not null,
            $"The record itself should be valid: {captured.Result.State}/{captured.Result.Reason}");
        pilot?.KeepCoverage(folder, new CoveragePacket(attempt, lease with { ResultsDirectory = "" }, report));
        return captured.Result;
    }
}

static string Project(string properties, string? reference = null) => $"""
    <Project Sdk="Microsoft.NET.Sdk">
      <PropertyGroup>
        <TargetFramework>net10.0</TargetFramework>
        {properties}
      </PropertyGroup>
      {(reference is null ? "" : $"<ItemGroup><ProjectReference Include=\"{reference}\" /></ItemGroup>")}
    </Project>
    """;

static string RestorePath(byte[] bytes)
{
    using var document = JsonDocument.Parse(bytes);
    return document.RootElement.GetProperty("project").GetProperty("restore").GetProperty("projectPath").GetString()!;
}

static void ReplaceAtSameTime(string path, string value)
{
    var before = File.ReadAllBytes(path);
    var stamp = File.GetLastWriteTimeUtc(path);
    File.WriteAllText(path, value + "\n");
    File.SetLastWriteTimeUtc(path, stamp);
    var after = File.ReadAllBytes(path);
    Require(before.Length == after.Length && !before.SequenceEqual(after) && File.GetLastWriteTimeUtc(path) == stamp,
        "The replacement should change bytes with the same size and timestamp.");
}

static string Digest(string path) => Convert.ToHexString(SHA256.HashData(File.ReadAllBytes(path)));

static void Write(string root, string relative, string text)
{
    var path = Path.Combine(root, relative);
    Directory.CreateDirectory(Path.GetDirectoryName(path)!);
    File.WriteAllText(path, text + "\n");
}

static void Require(bool passed, string message)
{
    if (!passed)
        throw new InvalidOperationException(message);
}

static async Task<string> Dotnet(string directory, params string[] args)
{
    var start = new ProcessStartInfo("dotnet")
    {
        WorkingDirectory = directory, UseShellExecute = false,
        RedirectStandardOutput = true, RedirectStandardError = true
    };
    foreach (var argument in args)
        start.ArgumentList.Add(argument);
    using var process = Process.Start(start) ?? throw new IOException("Could not launch dotnet.");
    var stdout = process.StandardOutput.ReadToEndAsync();
    var stderr = process.StandardError.ReadToEndAsync();
    using var timeout = new CancellationTokenSource(TimeSpan.FromMinutes(2));
    try
    {
        await process.WaitForExitAsync(timeout.Token);
    }
    catch (OperationCanceledException)
    {
        if (!process.HasExited)
            process.Kill(entireProcessTree: true);
        throw;
    }
    var output = await stdout;
    if (process.ExitCode != 0)
        throw new IOException($"dotnet {string.Join(' ', args)} exited {process.ExitCode}.\n{output}\n{await stderr}");
    return output;
}
