using System.Text.Json;
using System.Text.Json.Serialization;

internal sealed record WideExample(long[] Args, long Expected);
internal sealed record WideOperation(string Name, string Parameters, string Contract, string Shipping, string? Repair,
    WideExample Nominal, WideExample? Trigger);
internal sealed record WideModule(string Name, string Introduction, WideOperation[] Operations);

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

    private sealed record Probe(string Operation, long[] Args);
    private sealed record Example(string Operation, long[] Args, long Expected);
    private sealed record Oracle(string Operation, int Arity, bool Seeded, long[]? Args, long? Shipping, long? Corrected);
    private sealed record Finding(string Operation, long[] Args, long Actual, long Expected, string Cause);
    private sealed record Submission(Finding[] Findings);
    private static readonly JsonSerializerOptions ReportJson = new()
    {
        PropertyNameCaseInsensitive = true,
        NumberHandling = JsonNumberHandling.AllowReadingFromString
    };

    internal static async Task Package(string directory, Func<string, string[], Task<string>> dotnet)
    {
        if (Directory.Exists(directory) || File.Exists(directory))
            throw new IOException("The corpus needs a new destination.");
        Directory.CreateDirectory(directory);
        var modules = WideCorpus.Modules();
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
            if (operations.Select(operation => operation.Name).Distinct(StringComparer.Ordinal).Count() != operations.Length)
                throw new InvalidDataException("Operation identifiers must be unique within each dossier.");
            var witnesses = operations.Where(operation => operation.Trigger is not null).ToArray();
            var smoke = operations.Select(operation => new Example(operation.Name, operation.Nominal.Args, operation.Nominal.Expected)).ToArray();
            var probes = witnesses.Select(operation => new Probe(operation.Name, operation.Trigger!.Args)).ToArray();
            Write(dossier, "profiles/standard.json", smoke);
            Write(author, "qualification.json", probes);
            WriteApp(dossier, operations, corrected: false);
            WriteApp(repair, operations, corrected: true);
            var contracts = string.Join("\n\n", operations.Select(operation => $"- `{operation.Name}({operation.Parameters.Replace("long ", "", StringComparison.Ordinal)})` {operation.Contract}"));
            Put(dossier, "README.md", $$"""
                # {{module.Name}} service handoff

                {{module.Introduction}}

                The application exposes the operations below. Inputs are integer JSON arrays in the documented argument order. Status predicates use 0 and 1.

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
            Write(author, "ordinary.json", smoke.Select(item => new Probe(item.Operation, item.Args)).ToArray());
            var nominalBefore = await Replay(original, author, "ordinary.json", dotnet);
            var nominalAfter = await Replay(fixedBinary, author, "ordinary.json", dotnet);
            var countBefore = errors.Count;
            for (var index = 0; index < smoke.Length; index++)
                if (nominalBefore[index] != smoke[index].Expected || nominalAfter[index] != smoke[index].Expected)
                    errors.Add($"{module.Name}/{smoke[index].Operation}: public expected {smoke[index].Expected}, observed {nominalBefore[index]} -> {nominalAfter[index]}.");
            var before = await Replay(original, author, "qualification.json", dotnet);
            var after = await Replay(fixedBinary, author, "qualification.json", dotnet);
            var scores = new List<Oracle>();
            foreach (var operation in operations)
            {
                var arity = operation.Parameters.Split(',').Length;
                var trigger = operation.Trigger;
                if (trigger is null)
                {
                    scores.Add(new(operation.Name, arity, false, null, null, null));
                    continue;
                }

                var index = Array.IndexOf(witnesses, operation);
                if (index < 0 || before[index] is null || before[index] == after[index] || after[index] != trigger.Expected ||
                    string.IsNullOrWhiteSpace(operation.Repair) || operation.Repair == operation.Shipping)
                    errors.Add($"{module.Name}/{operation.Name}: private expected corrected {trigger.Expected}, observed {before[index]} -> {after[index]}.");
                scores.Add(new(operation.Name, arity, true, trigger.Args, before[index], after[index]));
            }
            Write(author, "oracle.json", scores);
            if (errors.Count != countBefore)
                continue;
            var smokePath = Path.Combine(dossier, "profiles/standard.json");
            if ((await dotnet(dossier, [original, "smoke", smokePath])).Trim() != $"OK {smoke.Length}" ||
                (await dotnet(dossier, [fixedBinary, "smoke", smokePath])).Trim() != $"OK {smoke.Length}")
                errors.Add($"The public smoke entry point did not pass in both builds for {module.Name}.");
            else
                Console.WriteLine($"{module.Name}: {witnesses.Length} independent triggers qualified; {smoke.Length} public checks passed in both builds.");
        }
        if (errors.Count != 0)
            throw new InvalidDataException(string.Join(Environment.NewLine, errors));
    }

    internal static async Task Adjudicate(string corpus, string reports, string destination, Func<string, string[], Task<string>> dotnet)
    {
        if (File.Exists(destination) || Directory.Exists(destination))
            throw new IOException("Choose a new adjudication output.");
        Directory.CreateDirectory(destination);
        foreach (var module in WideCorpus.Modules())
        {
            var author = Child(corpus, $"author/{module.Name}");
            var oracle = Read<Oracle[]>(author, "oracle.json").ToDictionary(item => item.Operation, StringComparer.Ordinal);
            var raw = File.ReadAllText(Child(reports, module.Name + ".json"));
            var submission = JsonSerializer.Deserialize<Submission>(raw, ReportJson)?.Findings
                ?? throw new InvalidDataException($"No findings array for {module.Name}.");
            var usable = submission.Where(finding => finding is not null && finding.Operation is not null &&
                oracle.TryGetValue(finding.Operation, out var entry) && finding.Args?.Length == entry.Arity).ToArray();
            Write(destination, module.Name + "-input.json", usable.Select(item => new Probe(item.Operation, item.Args)).ToArray());
            var original = await Replay(Binary(Path.Combine(author, "candidate-bin")), destination, module.Name + "-input.json", dotnet);
            var repaired = await Replay(Binary(Path.Combine(author, "corrected-bin")), destination, module.Name + "-input.json", dotnet);
            var rows = usable.Select((item, index) => new
            {
                item.Operation, item.Args, item.Actual, item.Expected, item.Cause, oracle[item.Operation].Seeded,
                ActualObserved = original[index], CorrectedObserved = repaired[index],
                ExactCounterexample = original[index] is long actual && repaired[index] is long expected &&
                    actual != expected && item.Actual == actual && item.Expected == expected
            }).ToArray();
            Write(destination, module.Name + "-measured.json", rows);
            Write(destination, module.Name + "-unbound.json", submission.Except(usable).ToArray());
            Console.WriteLine($"{module.Name}: {rows.Where(item => item.Seeded && item.ExactCounterexample).Select(item => item.Operation).Distinct().Count()} exact reported counterexamples from {submission.Length} reports.");
        }
    }

    private static async Task<long?[]> Replay(string binary, string root, string relative, Func<string, string[], Task<string>> dotnet) =>
        JsonSerializer.Deserialize<long?[]>((await dotnet(root, [binary, "batch", Child(root, relative)])).Trim())
        ?? throw new IOException(relative);

    private static void WriteApp(string root, WideOperation[] operations, bool corrected)
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
        Put(root, "Service.Dispatch.cs", """
            internal static partial class Service
            {
                internal static long Invoke(string operation, long[] values) => operation switch
                {
            """ + "\n" + string.Join('\n', selectors) + "\n" + """
                    _ => throw new InvalidDataException("Unexpected operation or arguments.")
                };
            }
            """);
        foreach (var operation in operations)
        {
            var body = corrected && operation.Repair is not null ? operation.Repair : operation.Shipping;
            Put(root, $"operations/{operation.Name}.cs", $$"""
                internal static partial class Service
                {
                    private static long {{operation.Name}}({{operation.Parameters}})
                    {
                {{string.Join('\n', body.Split('\n').Select(line => "        " + line.TrimEnd()))}}
                    }
                }
                """);
        }
        Put(root, "Program.cs", """
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
