using System.Text.Json;
using SlopHawk.Daemon;

if (args.Length < 2 || !Path.IsPathFullyQualified(args[0]))
{
    Console.Error.WriteLine("Usage: Inspector ROOT plan PROJECT | reuse | run ARCHIVE | coverage ARCHIVE TARGET SOURCE LINE");
    return 2;
}

var root = Path.GetFullPath(args[0]);
switch (args[1..])
{
    case ["plan", var project]:
        await Plan(project);
        break;
    case ["reuse"]:
        Reuse();
        break;
    case ["run", var run]:
        Run(run);
        break;
    case ["coverage", var archive, var target, var source, var line] when int.TryParse(line, out var number):
        Coverage(archive, target, source, number);
        break;
    default:
        Console.Error.WriteLine("Commands: plan PROJECT | reuse | run ARCHIVE | coverage ARCHIVE TARGET SOURCE LINE");
        return 2;
}
return 0;

async Task Plan(string project)
{
    var checkout = PilotArchive.Child(root, "dispatch");
    var scratch = Path.Combine(Path.GetTempPath(), "hawk-plan-" + Guid.NewGuid());
    var evaluator = new ProjectEvaluator(new Workspace(checkout, Path.Combine(scratch, "state")));
    var graph = await evaluator.EvaluateAsync();
    var selected = graph.Targets.Single(item => item.ProjectPath == project);
    var resultRoot = Path.Combine(scratch, "results");
    var allowed = graph.Targets.Select(item => Path.GetDirectoryName(PilotArchive.Child(checkout, item.ProjectPath))!)
        .SelectMany(directory => new[] { Path.Combine(directory, "bin"), Path.Combine(directory, "obj") })
        .Distinct().Append(resultRoot).ToArray();
    var grant = new ExecutionGrant("benchmark-inspection", RootIdentity.Read(checkout),
        ExecutionOperation.Build | ExecutionOperation.CompilerExtensions, allowed);
    var result = evaluator.PrepareBuild(graph, selected.Id, resultRoot, grant);
    Print(new { selected.Id, result.Ready, Blocker = result.Blocker.ToString(), result.Detail, graph.Complete });
}

void Reuse()
{
    var packet = PilotArchive.Read<RatingPacket>(root, "records/rating.json");
    var checkout = PilotArchive.Child(root, "rating");
    var files = packet.Paths.Select(path => new BuildArtifactPathMapping(path, PilotArchive.Child(checkout, path))).ToArray();
    var scope = new BuildArtifactReadScope([checkout], files, MappingTrusted: true);
    var capture = BuildArtifactCapture.Capture(packet.Manifest, scope);
    var comparison = BuildOutputIdentity.Compare(packet.PriorStamp, capture.Stamp);
    Print(new { comparison.Reusable, capture.Stamp.Complete, comparison.Problems, DeclaredRuntimeAssets = packet.Manifest.RuntimeAssets });
}

void Run(string name)
{
    var packet = PilotArchive.Read<RunPacket>(root, $"records/runs/{name}.json");
    try
    {
        packet.Receipt.Validate(packet.Attempt, TestOutcome.Passed, packet.AdmittedLease);
    }
    catch (IOException error)
    {
        Print(new { ReceiptValid = false, Reason = error.Message });
        return;
    }
    // Validation binds the archived attempt. Equality below compares the separate live-fence export.
    Print(new { ReceiptValid = true, SameLiveFence = packet.Attempt.Fence == packet.Live, Execution = packet.Receipt.Execution.ToString() });
}

void Coverage(string archive, string target, string source, int line)
{
    var location = $"records/coverage/{archive}";
    var input = PilotArchive.Read<CoveragePacket>(root, $"{location}/capture.json");
    var lease = input.Lease with { ResultsDirectory = PilotArchive.Child(root, location) };
    var captured = CoverageReceiptCapture.Capture(input.Attempt, lease, new CoverageArtifactDeclaration(input.Report)).Result;
    Print(new { Import = captured.State.ToString(), captured.Reason, input.Attempt.Target, RequestedTarget = target,
        Source = source, Line = line, LineState = captured.LineState(target, source, line).ToString() });
}

static void Print<T>(T value) => Console.WriteLine(JsonSerializer.Serialize(value));
