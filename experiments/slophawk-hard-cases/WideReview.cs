using System.Text.Json;
using System.Text.Json.Serialization;

internal sealed record WideExample(long[] Args, long Expected);
internal sealed record WideOperation(string Name, string Parameters, string Contract, string Shipping, string? Repair,
    WideExample Nominal, WideExample? Trigger, string? Source = null);
internal sealed record WideModule(string Name, string Introduction, WideOperation[] Operations, WideModule[]? Components = null);
internal enum WideRound { Standalone, Routed, Continuity, Deployment }

internal static partial class WideCorpus
{
    private static WideOperation Fault(string name, string parameters, string contract, string shipping, string repair,
        long[] ordinary, long ordinaryValue, long[] trigger, long correctValue) =>
        new(name, parameters, contract, shipping, repair, new(ordinary, ordinaryValue), new(trigger, correctValue));

    private static WideOperation Valid(string name, string parameters, string contract, string shipping,
        long[] ordinary, long ordinaryValue) => new(name, parameters, contract, shipping, null, new(ordinary, ordinaryValue), null);
}

internal static class WideReview
{
    private const string Project = "Service.csproj";
    private static string Binary(string output) => Path.Combine(output, "bin/Service/debug/Service.dll");

    internal sealed record Probe(string Operation, long[] Args);
    private sealed record Example(string Operation, long[] Args, long Expected);
    private sealed record Oracle(string Operation, int Arity, bool Seeded, long[]? Args, long? Shipping, long? Corrected, long[]? Route);
    private sealed record Finding(string Operation, long[] Args, long Actual, long Expected, string Cause);
    private sealed record Submission(Finding[] Findings);
    private static readonly JsonSerializerOptions ReportJson = new()
    {
        PropertyNameCaseInsensitive = true,
        NumberHandling = JsonNumberHandling.AllowReadingFromString
    };

    internal static async Task Package(string directory, Func<string, string[], Task<string>> dotnet, WideRound round)
    {
        if (Directory.Exists(directory) || File.Exists(directory))
            throw new IOException("The corpus needs a new destination.");
        Directory.CreateDirectory(directory);
        var modules = Modules(round);
        var integrated = round != WideRound.Standalone;
        var deployment = round == WideRound.Deployment;
        if (modules.Select(module => module.Name).Distinct(StringComparer.Ordinal).Count() != modules.Length ||
            modules.Sum(module => module.Operations.Count(operation => operation.Trigger is not null)) < 100)
            throw new InvalidDataException("The trial requires at least 100 independently indexed seeds.");

        var errors = new List<string>();
        foreach (var module in modules)
        {
            var dossier = Child(directory, $"data/{module.Name}");
            var author = Child(directory, $"author/{module.Name}");
            var repair = Path.Combine(author, "corrected");
            var operations = module.Operations;
            var routes = integrated ? operations.ToDictionary(operation => operation.Name,
                operation => RouteFor(module.Components!, operation), StringComparer.Ordinal) : null;
            if (operations.Select(operation => operation.Name).Distinct(StringComparer.Ordinal).Count() != operations.Length)
                throw new InvalidDataException("Operation identifiers must be unique within each dossier.");
            var witnesses = operations.Where(operation => operation.Trigger is not null).ToArray();
            var smoke = operations.Select(operation => new Example(operation.Name, Bind(operation, operation.Nominal.Args), operation.Nominal.Expected)).ToList();
            if (integrated)
                smoke.AddRange(operations.Select(operation => new Example(operation.Name,
                    [.. routes![operation.Name].Take(2), 0, .. operation.Nominal.Args], long.MinValue)));
            var probes = witnesses.Select(operation => new Probe(operation.Name, Bind(operation, operation.Trigger!.Args))).ToArray();
            if (deployment)
                WideJournal.WriteProfile(dossier, smoke.Select(item => (new Probe(item.Operation, item.Args), item.Expected)));
            else
                Write(dossier, "profiles/standard.json", smoke);
            Write(author, "qualification.json", probes);
            WriteApp(dossier, operations, corrected: false, routes, deployment);
            WriteApp(repair, operations, corrected: true, routes, deployment);
            var contracts = integrated
                ? "Review the component contracts under `contracts/` together with the producer catalog in `profiles/routes.json`.\n\n" +
                  "A service request starts with `producer, evidenceRevision, proofAccepted`, followed by the business fields. The route catalog supplies each operation's producer and revision. A different producer, different revision, or proof flag other than 1 makes the request inapplicable and returns `long.MinValue`. An admitted request supplies the business fields to its configured operation."
                : Contracts(operations);
            if (integrated)
            {
                Write(dossier, "profiles/routes.json", routes!.Select(item => new { Operation = item.Key, Producer = item.Value[0], Revision = item.Value[1] }));
                foreach (var component in module.Components!)
                    Put(dossier, $"contracts/{component.Name}.md", $"# {component.Name}\n\n{component.Introduction}\n\n" +
                        (deployment ? WideJournal.Catalog(component) : Contracts(component.Operations)));
            }
            Put(dossier, "README.md", deployment ? WideJournal.Handbook(module) : $$"""
                # {{module.Name}} service handoff

                {{module.Introduction}}

                Inputs are integer JSON arrays in the documented argument order. Status predicates use 0 and 1.

                {{contracts}}

                From this directory in PowerShell, choose `$out` in a fresh external scratch directory. After building, use the assembly to replay the standard profile or any documented input.

                ```powershell
                dotnet build Service.csproj --artifacts-path $out -p:NuGetAudit=false
                $app = Join-Path $out 'bin/Service/debug/Service.dll'
                dotnet $app smoke profiles/standard.json
                dotnet $app probe OPERATION '[1,2,3]'
                dotnet $app batch <external-json-file>
                ```

                Batch files contain objects such as `[{"Operation":"OPERATION","Args":[1,2,3]}]`. Replace the example operation and arguments with an applicable call. Preserve this checkout and keep new files in external scratch.
                """);

            var deployed = Path.Combine(author, "candidate-bin");
            var fixedOutput = Path.Combine(author, "corrected-bin");
            await dotnet(dossier, ["build", Project, "--artifacts-path", deployed, "-p:NuGetAudit=false"]);
            await dotnet(repair, ["build", Project, "--artifacts-path", fixedOutput, "-p:NuGetAudit=false"]);
            var original = Binary(deployed);
            var fixedBinary = Binary(fixedOutput);
            if (deployment)
                WideJournal.CopyPriorBuild(fixedBinary, dossier);
            Write(author, "ordinary.json", smoke.Select(item => new Probe(item.Operation, item.Args)).ToArray());
            var nominalBefore = await Replay(original, author, "ordinary.json", dotnet, deployment);
            var nominalAfter = await Replay(fixedBinary, author, "ordinary.json", dotnet, deployment);
            var countBefore = errors.Count;
            for (var index = 0; index < smoke.Count; index++)
                if (nominalBefore[index] != smoke[index].Expected || nominalAfter[index] != smoke[index].Expected)
                    errors.Add($"{module.Name}/{smoke[index].Operation}: public expected {smoke[index].Expected}, observed {nominalBefore[index]} -> {nominalAfter[index]}.");
            var before = await Replay(original, author, "qualification.json", dotnet, deployment);
            var after = await Replay(fixedBinary, author, "qualification.json", dotnet, deployment);
            var scores = new List<Oracle>();
            foreach (var operation in operations)
            {
                var arity = operation.Parameters.Split(',').Length;
                var trigger = operation.Trigger;
                if (trigger is null)
                {
                    scores.Add(new(operation.Name, arity, false, null, null, null, routes?[operation.Name]));
                    continue;
                }

                var index = Array.IndexOf(witnesses, operation);
                if (index < 0 || before[index] is null || before[index] == after[index] || after[index] != trigger.Expected ||
                    string.IsNullOrWhiteSpace(operation.Repair) || operation.Repair == operation.Shipping)
                    errors.Add($"{module.Name}/{operation.Name}: private expected corrected {trigger.Expected}, observed {before[index]} -> {after[index]}.");
                scores.Add(new(operation.Name, arity, true, probes[index].Args, before[index], after[index], routes?[operation.Name]));
            }
            Write(author, "oracle.json", scores);
            if (errors.Count != countBefore)
                continue;
            var smokePath = Path.Combine(dossier, "profiles/standard.json");
            if ((await dotnet(dossier, [original, "smoke", smokePath])).Trim() != $"OK {smoke.Count}" ||
                (await dotnet(dossier, [fixedBinary, "smoke", smokePath])).Trim() != $"OK {smoke.Count}")
                errors.Add($"The public smoke entry point did not pass in both builds for {module.Name}.");
            else
                Console.WriteLine($"{module.Name}: {witnesses.Length} independent triggers qualified; {smoke.Count} public checks passed in both builds.");

            long[] Bind(WideOperation operation, long[] args) => routes is null ? args : [.. routes[operation.Name], .. args];
        }
        if (errors.Count != 0)
            throw new InvalidDataException(string.Join(Environment.NewLine, errors));
    }

    internal static async Task Adjudicate(string corpus, string reports, string destination, Func<string, string[], Task<string>> dotnet,
        WideRound round)
    {
        if (File.Exists(destination) || Directory.Exists(destination))
            throw new IOException("Choose a new adjudication output.");
        Directory.CreateDirectory(destination);
        foreach (var module in Modules(round))
        {
            var author = Child(corpus, $"author/{module.Name}");
            var oracle = Read<Oracle[]>(author, "oracle.json").ToDictionary(item => item.Operation, StringComparer.Ordinal);
            using var raw = JsonDocument.Parse(File.ReadAllText(Child(reports, module.Name + ".json")));
            var submission = (raw.RootElement.ValueKind == JsonValueKind.Array
                ? raw.RootElement.Deserialize<Finding[]>(ReportJson)
                : raw.RootElement.Deserialize<Submission>(ReportJson)?.Findings)
                ?? throw new InvalidDataException($"No findings array for {module.Name}.");
            var usable = submission.Where(finding => finding is not null && finding.Operation is not null &&
                oracle.TryGetValue(finding.Operation, out var entry) && finding.Args is not null &&
                (finding.Args.Length == entry.Arity || entry.Route is not null && finding.Args.Length == entry.Arity + entry.Route.Length)).ToArray();
            var probes = usable.Select(item =>
            {
                var binding = oracle[item.Operation];
                return new Probe(item.Operation, binding.Route is not null && item.Args.Length == binding.Arity
                    ? [.. binding.Route, .. item.Args] : item.Args);
            }).ToArray();
            Write(destination, module.Name + "-input.json", probes);
            var original = await Replay(Binary(Path.Combine(author, "candidate-bin")), destination, module.Name + "-input.json", dotnet, round == WideRound.Deployment);
            var repaired = await Replay(Binary(Path.Combine(author, "corrected-bin")), destination, module.Name + "-input.json", dotnet, round == WideRound.Deployment);
            var rows = usable.Select((item, index) => new
            {
                item.Operation, item.Args, ReplayedArgs = probes[index].Args, item.Actual, item.Expected, item.Cause, oracle[item.Operation].Seeded,
                ActualObserved = original[index], CorrectedObserved = repaired[index],
                ExactCounterexample = original[index] is long actual && repaired[index] is long expected &&
                    actual != expected && item.Actual == actual && item.Expected == expected
            }).ToArray();
            Write(destination, module.Name + "-measured.json", rows);
            Write(destination, module.Name + "-unbound.json", submission.Except(usable).ToArray());
            Console.WriteLine($"{module.Name}: {rows.Where(item => item.Seeded && item.ExactCounterexample).Select(item => item.Operation).Distinct().Count()} exact reported counterexamples from {submission.Length} reports.");
        }
    }

    private static WideModule[] Modules(WideRound round) => round switch
    {
        WideRound.Standalone => WideCorpus.Modules(),
        WideRound.Routed => WideCorpus.IntegratedModules(),
        WideRound.Continuity or WideRound.Deployment => WideCorpus.LifecycleModules(),
        _ => throw new ArgumentOutOfRangeException(nameof(round))
    };

    private static async Task<long?[]> Replay(string binary, string root, string relative, Func<string, string[], Task<string>> dotnet,
        bool deployment)
    {
        if (deployment)
        {
            var input = Read<Probe[]>(root, relative);
            relative += ".journal.json";
            WideJournal.WriteBatch(Child(root, relative), input);
        }
        return JsonSerializer.Deserialize<long?[]>((await dotnet(root, [binary, "batch", Child(root, relative)])).Trim())
            ?? throw new IOException(relative);
    }

    private static string Contracts(WideOperation[] operations) => string.Join("\n\n", operations.Select(operation =>
        $"- `{operation.Name}({operation.Parameters.Replace("long ", "", StringComparison.Ordinal)})` {operation.Contract}"));

    private static long[] RouteFor(WideModule[] components, WideOperation operation)
    {
        var producer = Array.FindIndex(components, component => component.Name == operation.Source);
        if (producer < 0)
            throw new InvalidDataException(operation.Name);
        return [producer + 1, 300 + producer * 11, 1];
    }

    private static void WriteApp(string root, WideOperation[] operations, bool corrected, Dictionary<string, long[]>? routes,
        bool deployment)
    {
        Put(root, Project, """
            <Project Sdk="Microsoft.NET.Sdk">
              <PropertyGroup><OutputType>Exe</OutputType><TargetFramework>net10.0</TargetFramework><ImplicitUsings>enable</ImplicitUsings><Nullable>enable</Nullable></PropertyGroup>
            </Project>
            """);
        var selectors = operations.Select(operation =>
        {
            var count = operation.Parameters.Split(',').Length;
            return $"        \"{operation.Name}\" when values.Length == {count} => {operation.Name}({string.Join(", ", Enumerable.Range(0, count).Select(index => $"values[{index}]"))}),";
        });
        var entry = routes is null ? "internal static long Invoke" : "private static long Business";
        Put(root, "Service.Dispatch.cs", $$"""
            internal static partial class Service
            {
                {{entry}}(string operation, long[] values) => operation switch
                {
            """ + "\n" + string.Join('\n', selectors) + "\n" + """
                    _ => throw new InvalidDataException("Unexpected operation or arguments.")
                };
            }
            """);
        if (routes is not null)
        {
            var branches = routes.Select(item => $"        \"{item.Key}\" => ({item.Value[0]}L, {item.Value[1]}L),");
            var producer = deployment ? """
                    internal static long[] ProducerRoute(string operation)
                    {
                        var route = Find(operation);
                        return [route.Producer, route.Revision, 1];
                    }

                """ : "";
            Put(root, "Delivery.cs", """
                internal static partial class Service
                {
                """ + "\n" + producer + """
                    internal static long Invoke(string operation, long[] request)
                    {
                        var route = Find(operation);
                        if (request.Length < 3 || request[0] != route.Producer || request[1] != route.Revision || request[2] != 1)
                            return long.MinValue;
                        return Business(operation, request[3..]);
                    }

                    private static (long Producer, long Revision) Find(string operation) => operation switch
                    {
                """ + "\n" + string.Join('\n', branches) + "\n" + """
                        _ => throw new InvalidDataException("Unexpected operation.")
                    };
                }
                """);
        }
        foreach (var operation in operations)
        {
            var body = corrected && operation.Repair is not null ? operation.Repair : operation.Shipping;
            var source = operation.Source is null ? "operations" : $"operations/{operation.Source}";
            Put(root, $"{source}/{operation.Name}.cs", $$"""
                internal static partial class Service
                {
                    private static long {{operation.Name}}({{operation.Parameters}})
                    {
                {{string.Join('\n', body.Split('\n').Select(line => "        " + line.TrimEnd()))}}
                    }
                }
                """);
        }
        Put(root, "Program.cs", deployment ? WideJournal.ServiceProgram : """
            using System.Text.Json;

            if (args is ["probe", var operation, var values])
            {
                Console.WriteLine(Service.Invoke(operation, JsonSerializer.Deserialize<long[]>(values)!));
                return 0;
            }
            if (args is ["batch", var input])
            {
                var requests = JsonSerializer.Deserialize<Probe[]>(File.ReadAllText(input))!;
                var results = requests.Select(item => Evaluate(item.Operation, item.Args)).ToArray();
                Console.WriteLine(JsonSerializer.Serialize(results));
                return 0;
            }
            if (args is ["smoke", var profile])
            {
                var examples = JsonSerializer.Deserialize<Example[]>(File.ReadAllText(profile))!;
                foreach (var item in examples)
                {
                    var observed = Evaluate(item.Operation, item.Args);
                    if (observed != item.Expected)
                    {
                        Console.Error.WriteLine($"{item.Operation}: expected {item.Expected}, observed {observed}");
                        return 1;
                    }
                }
                Console.WriteLine($"OK {examples.Length}");
                return 0;
            }
            return 2;

            static long? Evaluate(string operation, long[] input)
            {
                try { return Service.Invoke(operation, input); }
                catch (Exception error) when (error is ArgumentException or ArithmeticException or InvalidDataException or IndexOutOfRangeException)
                { return null; }
            }

            internal sealed record Probe(string Operation, long[] Args);
            internal sealed record Example(string Operation, long[] Args, long Expected);
            """);
    }

    private static string Child(string root, string relative) => PilotArchive.Child(root, relative);
    private static T Read<T>(string root, string path) => PilotArchive.Read<T>(root, path);
    private static void Write<T>(string root, string path, T value) => PilotArchive.Write(root, path, value);

    private static void Put(string root, string path, string text)
    {
        var file = Child(root, path);
        Directory.CreateDirectory(Path.GetDirectoryName(file)!);
        File.WriteAllText(file, text + "\n");
    }
}
