using System.Security.Cryptography;
using System.Text.Json;
using SlopHawk.Daemon;

const string usage = "Inspector ROOT review REQUEST.json | plan PROJECT.csproj | reuse [REQUEST.json] | run RECORD.json | coverage CAPTURE.json TARGET SOURCE LINE";
if (args.Length < 2 || !Path.IsPathFullyQualified(args[0]))
{
    Console.Error.WriteLine(usage);
    return 2;
}

var root = Path.GetFullPath(args[0]);
try
{
    switch (args[1..])
    {
        case ["review", var path]:
            var request = ReadRequest(path);
            await Plan(request.Project);
            Reuse(request);
            Run(request.Run);
            Coverage(request.Coverage, request.Target, request.Document, request.Line);
            break;
        case ["plan", var project]:
            await Plan(project);
            break;
        case ["reuse"]:
            Reuse(null);
            break;
        case ["reuse", var path]:
            Reuse(ReadRequest(path));
            break;
        case ["run", var run]:
            Run(run);
            break;
        case ["coverage", var archive, var target, var source, var line] when int.TryParse(line, out var number) && number > 0:
            Coverage(archive, target, source, number);
            break;
        default:
            Console.Error.WriteLine(usage);
            return 2;
    }
}
catch (Exception error) when (error is IOException or InvalidDataException or ArgumentException or UnauthorizedAccessException or JsonException)
{
    Print(new { Error = error.Message, Usage = usage });
    return 2;
}
return 0;

PromotionRequest ReadRequest(string path) => PilotArchive.Read<PromotionRequest>(root, ArchivePath(path, "releases", ".json"));

async Task Plan(string project)
{
    var checkout = PilotArchive.Child(root, "dispatch");
    var scratch = Path.Combine(Path.GetTempPath(), "hawk-plan-" + Guid.NewGuid());
    var evaluator = new ProjectEvaluator(new Workspace(checkout, Path.Combine(scratch, "state")));
    var graph = await evaluator.EvaluateAsync();
    var requested = project.Replace('\\', '/');
    if (requested.StartsWith("dispatch/", StringComparison.OrdinalIgnoreCase))
        requested = requested["dispatch/".Length..];
    var matching = graph.Targets.Where(item => item.ProjectPath.Equals(requested, StringComparison.OrdinalIgnoreCase) ||
        item.Id.Equals(requested, StringComparison.OrdinalIgnoreCase)).Take(2).ToArray();
    if (matching is not [var selected])
    {
        Print(new { Check = "Plan", Requested = project, Error = "Select one evaluated project path or target ID.",
            Available = graph.Targets.Take(32).Select(target => new { Project = "dispatch/" + target.ProjectPath, target.Id }) });
        return;
    }

    var resultRoot = Path.Combine(scratch, "results");
    var allowed = graph.Targets.Select(item => Path.GetDirectoryName(PilotArchive.Child(checkout, item.ProjectPath))!)
        .SelectMany(directory => new[] { Path.Combine(directory, "bin"), Path.Combine(directory, "obj") })
        .Distinct().Append(resultRoot).ToArray();
    var grant = new ExecutionGrant("benchmark-inspection", RootIdentity.Read(checkout),
        ExecutionOperation.Build | ExecutionOperation.CompilerExtensions, allowed);
    var result = evaluator.PrepareBuild(graph, selected.Id, resultRoot, grant);
    Print(new { Check = "Plan", selected.Id, result.Ready, Blocker = result.Blocker.ToString(), result.Detail, graph.Complete });
}

void Reuse(PromotionRequest? request)
{
    var packet = PilotArchive.Read<RatingPacket>(root, "records/rating.json");
    var checkout = PilotArchive.Child(root, "rating");
    var files = packet.Paths.Select(path => new BuildArtifactPathMapping(path, PilotArchive.Child(checkout, path))).ToArray();
    var scope = new BuildArtifactReadScope([checkout], files, MappingTrusted: true);
    var observed = request is null ? null : ObservedRuntime(checkout, request.Route);
    var capture = BuildArtifactCapture.Capture(packet.Manifest, scope, observedRuntimeInputs: observed);
    var comparison = BuildOutputIdentity.Compare(packet.PriorStamp, capture.Stamp);
    var changed = observed?.Select(path => new { Path = path, Approved = Digest("records/approved/" + path), Current = Digest("rating/" + path) })
        .Where(file => file.Approved != file.Current).ToArray();
    Print(new { Check = "Reuse", comparison.Reusable,
        EvidenceScope = observed is null ? "declared files only" : "declared files plus selected consumer path check",
        comparison.Problems, DeclaredRuntimeAssets = packet.Manifest.RuntimeAssets, SelectedConsumerInputs = observed, ObservedRuntimeChanges = changed });
}

string Digest(string path)
{
    using var file = File.OpenRead(PilotArchive.Child(root, path));
    if (file.Length > 1024 * 1024)
        throw new InvalidDataException("A dossier input exceeds the 1 MiB inspection limit.");
    return Convert.ToHexString(SHA256.HashData(file));
}

static string[] ObservedRuntime(string checkout, string route)
{
    const string catalog = "catalog/index.json";
    using var index = JsonDocument.Parse(File.ReadAllText(PilotArchive.Child(checkout, catalog)));
    if (!index.RootElement.TryGetProperty(route, out var entry) || entry.GetString() is not { } selected)
        throw new InvalidDataException($"No catalog selection for route '{route}'.");
    using var profile = JsonDocument.Parse(File.ReadAllText(PilotArchive.Child(checkout, selected)));
    return profile.RootElement.TryGetProperty("redirect", out var value) && value.GetString() is { } redirect
        ? [catalog, selected, redirect] : [catalog, selected];
}

void Run(string name)
{
    var path = ArchivePath(name, "records/runs", ".json");
    var packet = PilotArchive.Read<RunPacket>(root, path);
    try
    {
        packet.Receipt.Validate(packet.Attempt, TestOutcome.Passed, packet.AdmittedLease);
    }
    catch (IOException error)
    {
        Print(new { Check = "Run", Path = path, ReceiptValid = false, Reason = error.Message });
        return;
    }
    Print(new { Check = "Run", Path = path, ReceiptValid = true, SameLiveFence = packet.Attempt.Fence == packet.Live,
        AdmittedEpoch = packet.Attempt.Fence.Epoch, LiveEpoch = packet.Live.Epoch, Execution = packet.Receipt.Execution.ToString() });
}

void Coverage(string archive, string target, string source, int line)
{
    var path = ArchivePath(archive, "records/coverage", "/capture.json");
    var location = path[..path.LastIndexOf('/')];
    var input = PilotArchive.Read<CoveragePacket>(root, path);
    var lease = input.Lease with { ResultsDirectory = PilotArchive.Child(root, location) };
    var captured = CoverageReceiptCapture.Capture(input.Attempt, lease, new CoverageArtifactDeclaration(input.Report)).Result;
    var digest = Digest(source);
    var recorded = captured.Lines.FirstOrDefault(counter => counter.Target == target && counter.SourcePath == source && counter.Number == line);
    Print(new { Check = "Coverage", Path = path, Import = captured.State.ToString(), captured.Reason, input.Attempt.Target,
        RequestedTarget = target, Source = source, Line = line, RecordedDigest = recorded?.SourceSha256, CurrentDigest = digest,
        ArchivedLineState = recorded?.State.ToString() ?? "Unknown",
        CurrentLineState = captured.LineState(target, source, line, digest).ToString() });
}

static string ArchivePath(string selector, string directory, string suffix)
{
    var normalized = selector.Replace('\\', '/');
    if (normalized.StartsWith(directory + "/", StringComparison.Ordinal) && normalized.EndsWith(suffix, StringComparison.OrdinalIgnoreCase))
        return normalized;
    if (normalized.Length is > 0 and <= 100 && normalized.All(character => char.IsAsciiLetterOrDigit(character) || character is '-' or '_'))
        return $"{directory}/{normalized}{suffix}";
    throw new ArgumentException($"Enter an archive ID or a dossier-relative path under {directory}/ ending in {suffix}.");
}

static void Print<T>(T value) => Console.WriteLine(JsonSerializer.Serialize(value));
