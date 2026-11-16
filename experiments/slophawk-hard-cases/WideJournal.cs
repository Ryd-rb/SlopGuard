using System.Text.Json;

internal static class WideJournal
{
    private sealed record Entry(string Action, int Field = 0, long Revision = 0, long Value = 0);
    private sealed record Journal(string Operation, long[] Route, int Fields, Entry[] Events);
    private sealed record Example(Journal Journal, long Expected);
    private static readonly JsonSerializerOptions Json = new() { WriteIndented = true };

    internal static void WriteProfile(string root, IEnumerable<(WideReview.Probe Probe, long Expected)> probes)
    {
        var profiles = Directory.CreateDirectory(Path.Combine(root, "profiles"));
        File.WriteAllText(Path.Combine(profiles.FullName, "standard.json"),
            JsonSerializer.Serialize(probes.Select(pair => new Example(Record(pair.Probe), pair.Expected)), Json));
    }

    internal static void WriteBatch(string path, IEnumerable<WideReview.Probe> probes) =>
        File.WriteAllText(path, JsonSerializer.Serialize(probes.Select(Record), Json));

    private static Journal Record(WideReview.Probe probe)
    {
        var fields = probe.Args[3..];
        var events = fields.Select((value, index) => new Entry("stage", index, 2, value)).Reverse().ToList();
        events.Add(new("stage", Field: 0, Revision: 1, Value: 0));
        events.Add(new("snapshot", Revision: 2));
        events.Add(new("stage", Field: 0, Revision: 3, Value: 0));
        events.Add(new("snapshot", Revision: 3));
        events.Add(new("discard", Revision: 3));
        events.Add(new("commit", Revision: 2));
        return new(probe.Operation, probe.Args[..3], fields.Length, events.ToArray());
    }

    internal static string Catalog(WideModule component) => "Replay operations and journal field order\n\n" +
        string.Join("\n", component.Operations.Select(operation => $"- `{operation.Name}({operation.Parameters.Replace("long ", "", StringComparison.Ordinal)})`"));

    internal static void CopyPriorBuild(string priorAssembly, string dossier)
    {
        var location = Path.GetDirectoryName(priorAssembly)!;
        var published = Path.Combine(dossier, "release", "current");
        Directory.CreateDirectory(published);
        foreach (var file in new[] { "Service.dll", "Service.deps.json", "Service.runtimeconfig.json" })
            File.Copy(Path.Combine(location, file), Path.Combine(published, file));
    }

    internal static string Handbook(WideModule module) => $$"""
        # Continuity journal release

        {{module.Introduction}}

        Each producer can import its field records out of order. A `stage` event names a zero-based field, its revision, and a signed integer value. Older staged versions of a field are ignored. A `snapshot` captures the latest staged records under a snapshot revision. All fields must be present and their revisions must be at or below the snapshot revision. A `commit` publishes the named snapshot and evaluates its ordered fields. Later staging does not change a captured snapshot. A `discard` removes the named pending snapshot. A candidate that cannot build or replay an ordinary accepted release is ineligible.

        The currently deployed service is packaged under `release/current/`. For this compatibility release, an admitted, complete journal supported by both executables must have the same numeric business result in each. A `null` output means the input was unsupported. `long.MinValue` means rejected producer, stale evidence revision, or missing proof and is not a business result. The catalogs under `contracts/` document operation fields. `profiles/routes.json` supplies route prefixes `[producer, revision, proofAccepted]`. The standard profile includes accepted and rejected requests.

        Use PowerShell from this directory. Put build output and any created probes in external scratch.

        ```powershell
        dotnet build Service.csproj --artifacts-path $out -p:NuGetAudit=false
        $app = Join-Path $out 'bin/Service/debug/Service.dll'
        $current = 'release/current/Service.dll'
        dotnet $app smoke profiles/standard.json
        dotnet $current smoke profiles/standard.json
        dotnet $app batch <journal-file>
        dotnet $current batch <same-journal-file>
        ```

        A batch file is a JSON array of journal objects, each with `Operation`, `Route`, `Fields` (the number of business fields), and `Events`. For example, a two-field import has events `[{"Action":"stage","Field":0,"Revision":10,"Value":4},{"Action":"stage","Field":1,"Revision":10,"Value":5},{"Action":"snapshot","Revision":10},{"Action":"commit","Revision":10}]`. Batch output preserves input order. A reviewer may also adapt a batch from the `Journal` properties of examples in the standard profile.

        The producer adapter can capture journals from ordinary business inputs. Use a JSON array like `[{"Operation":"WeekSlot","Args":[5,1,1]}]`, with only the catalog's business fields under `Args`. Keep the JSON array even for one request. For a valid capture, both executables can read the same journal.

        ```powershell
        dotnet $app capture $requests | Set-Content -LiteralPath $journal
        dotnet $app batch $journal
        dotnet $current batch $journal
        ```
        """;

    internal const string ServiceProgram = """
        using System.Text.Json;

        if (args is [var action, var input] && action is "batch" or "capture" or "smoke")
        {
            var json = File.ReadAllText(input);
            if (action == "capture")
            {
                var requests = JsonSerializer.Deserialize<Request[]>(json)!;
                Console.WriteLine(JsonSerializer.Serialize(requests.Select(Capture)));
                return 0;
            }
            if (action == "batch")
            {
                var batch = JsonSerializer.Deserialize<Journal[]>(json)!;
                Console.WriteLine(JsonSerializer.Serialize(batch.Select(Run)));
                return 0;
            }
            var profile = JsonSerializer.Deserialize<Example[]>(json)!;
            var mismatches = profile.Count(item => Run(item.Journal) != item.Expected);
            Console.WriteLine(mismatches == 0 ? $"OK {profile.Length}" : $"DIFFERENT {mismatches}");
            return mismatches == 0 ? 0 : 1;
        }
        return 2;

        static Journal Capture(Request request)
        {
            var events = request.Args.Select((value, field) => new Entry("stage", field, 2, value)).Reverse().ToList();
            events.Add(new("stage", 0, 1, 0));
            events.Add(new("snapshot", 0, 2, 0));
            events.Add(new("stage", 0, 3, 0));
            events.Add(new("snapshot", 0, 3, 0));
            events.Add(new("discard", 0, 3, 0));
            events.Add(new("commit", 0, 2, 0));
            return new(request.Operation, Service.ProducerRoute(request.Operation), request.Args.Length, events.ToArray());
        }

        static long? Run(Journal journal)
        {
            if (journal.Fields is < 1 or > 128 || journal.Route is not { Length: 3 } || journal.Events is null)
                return null;

            var stage = new SortedDictionary<int, (long Revision, long Value)>();
            var snapshots = new Dictionary<long, long[]>();
            long? result = null;
            foreach (var item in journal.Events)
            {
                switch (item.Action)
                {
                    case "stage":
                        if (item.Field < 0 || item.Field >= journal.Fields)
                            return null;
                        if (!stage.TryGetValue(item.Field, out var existing) || item.Revision >= existing.Revision)
                            stage[item.Field] = (item.Revision, item.Value);
                        break;
                    case "snapshot":
                        if (stage.Count != journal.Fields || stage.Values.Any(field => field.Revision > item.Revision))
                            return null;
                        snapshots[item.Revision] = stage.Values.Select(field => field.Value).ToArray();
                        break;
                    case "discard":
                        snapshots.Remove(item.Revision);
                        break;
                    case "commit":
                        if (!snapshots.TryGetValue(item.Revision, out var payload))
                            return null;
                        try
                        {
                            result = Service.Invoke(journal.Operation, [.. journal.Route, .. payload]);
                        }
                        catch (Exception exception) when (exception is ArgumentException or ArithmeticException or
                            InvalidDataException or IndexOutOfRangeException or InvalidOperationException)
                        {
                            return null;
                        }
                        break;
                    default:
                        return null;
                }
            }
            return result;
        }

        sealed record Entry(string Action, int Field, long Revision, long Value);
        sealed record Journal(string Operation, long[] Route, int Fields, Entry[] Events);
        sealed record Request(string Operation, long[] Args);
        sealed record Example(Journal Journal, long Expected);
        """;
}
