using System.Text.Json;
using System.Text.Json.Serialization;
using SlopHawk.Daemon;

internal sealed record AuditIndex(AuditItem[] Items);
internal sealed record AuditItem(string Id, string Component, string Claim);
internal sealed record BuildInputs(BuildArtifactManifest Manifest, string[] MappedFiles);
internal sealed record RunInputs(TestAttempt Admitted, TestOutcome Outcome, string Lease, VstestProcessFacts Process);
internal sealed record CoverageInputs(CoverageSupervisedAttempt Attempt, CoverageResultsLease Lease,
    string Report, string Document, int Line);
internal sealed record Observation(
    string State, string[] Facts, CoberturaImportResult? Coverage = null);

internal static class Evidence
{
    private static readonly JsonSerializerOptions Json = new()
    {
        WriteIndented = true,
        Converters = { new JsonStringEnumConverter() }
    };

    internal static string Evaluate(string root, AuditItem item)
    {
        var packet = Packet(root, item);
        var observation = Observe(packet, item.Component);
        return $"{item.Id} {item.Component} {observation.State} | {string.Join(" | ", observation.Facts)}";
    }

    internal static Observation Observe(string packet, string component) => component switch
    {
        "build" => Build(packet),
        "run" => Run(packet),
        "coverage" => Coverage(packet),
        _ => throw new InvalidDataException("Unknown evidence component.")
    };

    private static Observation Build(string packet)
    {
        var prior = Read<BuildIdentityStamp>(packet, "prior-stamp.json");
        var observed = Capture(packet, Read<BuildInputs>(packet, "current.json"));
        var reuse = BuildOutputIdentity.Compare(prior, observed.Stamp);
        var facts = reuse.Problems.Select(problem => $"{problem.Kind}/{problem.Field}")
            .Concat(observed.Evidence.Where(file => file.Status != BuildArtifactObservationStatus.ByteVerified)
                .Select(file => $"{file.LogicalPath}={file.Status}"))
            .Distinct().ToArray();
        return new Observation(reuse.Reusable ? "Reusable" : observed.Stamp.Complete ? "Mismatch" : "Incomplete", facts);
    }

    internal static BuildArtifactCaptureResult Capture(string packet, BuildInputs input)
    {
        var physical = Child(packet, "current");
        var mappings = input.MappedFiles.Select(path => new BuildArtifactPathMapping(path, Child(physical, path))).ToArray();
        return BuildArtifactCapture.Capture(input.Manifest, new BuildArtifactReadScope([physical], mappings, MappingTrusted: true));
    }

    private static Observation Run(string packet)
    {
        var input = Read<RunInputs>(packet, "admission.json");
        var receipt = Read<TestRunReceiptDetails>(packet, "receipt.json");
        var path = Child(packet, "run.trx");
        var file = new FileInfo(path);
        if (file.Exists && file.Length > VstestTrxClassifier.MaxArtifactBytes)
            throw new InvalidDataException("The archived test artifact exceeds the benchmark read budget.");

        // Archived supervisor facts supply the attempt identity and times. The current file
        // observation supplies the write time, never the claimed receipt.
        var process = input.Process with { ArtifactLastWriteUtc = file.Exists ? file.LastWriteTimeUtc : null };
        var platform = VstestTrxClassifier.Classify(process, VstestTrxClassifier.Parse(file.Exists ? File.ReadAllBytes(path) : null));
        var facts = new List<string> { $"replayed={platform.Disposition}", $"artifact={platform.Artifact.Health}", platform.Reason };
        try
        {
            receipt.Validate(input.Admitted, input.Outcome, input.Lease);
        }
        catch (IOException error)
        {
            facts.Add(error.Message);
            return new Observation("ReceiptRejected", facts.ToArray());
        }

        // This audit cross-check is glue between two component results, not a scheduler operation.
        if (receipt.Platform != platform.Disposition || receipt.ArtifactSha256 != platform.Artifact.Sha256 ||
            receipt.Counters != platform.Artifact.Counters || receipt.RunId != platform.Artifact.RunId ||
            receipt.ArtifactBytes != platform.Artifact.ByteLength)
        {
            facts.Add("stored receipt disagrees with replayed artifact or classification");
            return new Observation("ArchiveDisagrees", facts.ToArray());
        }

        return new Observation("ReceiptMatches", facts.ToArray());
    }

    private static Observation Coverage(string packet)
    {
        var input = Read<CoverageInputs>(packet, "capture.json");
        var lease = input.Lease with { ResultsDirectory = Child(packet, input.Lease.ResultsDirectory) };
        var capture = CoverageReceiptCapture.Capture(input.Attempt, lease, new CoverageArtifactDeclaration(input.Report));
        var result = capture.Result;
        var line = result.LineState(input.Attempt.Target, input.Document, input.Line);
        var facts = new List<string>
        {
            result.Reason, $"{input.Document}:{input.Line}={line}"
        };
        if (result.Totals is { } totals)
            facts.Add($"run outcome={totals.Outcome}");
        foreach (var aggregate in result.BranchAggregates)
            facts.Add($"{aggregate.SourcePath}:{aggregate.Line} branches={aggregate.Covered}/{aggregate.Valid} (aggregate)");
        foreach (var unknown in result.UnknownScope)
            facts.Add($"{unknown.CollectorPath}: {unknown.Reason}");
        return new Observation(result.State.ToString(), facts.ToArray(), result);
    }

    internal static string Packet(string root, AuditItem item) => Child(root, "packets/" + item.Id);

    internal static string Child(string root, string relative)
    {
        if (Path.IsPathRooted(relative) || relative.Split('/', '\\').Any(part => part is "" or "." or ".."))
            throw new InvalidDataException("Expected a bounded relative archive path.");

        return Path.GetFullPath(Path.Combine(root, relative));
    }

    internal static T Read<T>(string root, string relative) =>
        JsonSerializer.Deserialize<T>(File.ReadAllText(Child(root, relative)), Json)
        ?? throw new InvalidDataException($"Missing payload in {relative}.");

    internal static void Write<T>(string root, string relative, T value) =>
        File.WriteAllText(Child(root, relative), JsonSerializer.Serialize(value, Json) + "\n");
}
