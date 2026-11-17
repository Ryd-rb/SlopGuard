using System.Buffers.Binary;
using System.Globalization;
using System.Security.Cryptography;
using System.Text;
using System.Text.Json;
using System.Text.Json.Serialization;

if (args.Length is < 1 or > 2)
{
    Console.Error.WriteLine("Usage: HoldoutGenerator <empty output directory> [seed]");
    return 2;
}
var root = Path.GetFullPath(args[0]);
if (Directory.Exists(root) && Directory.EnumerateFileSystemEntries(root).Any())
{
    Console.Error.WriteLine("Choose an empty output directory for this draw.");
    return 2;
}
var seed = args.Length == 2 ? ulong.Parse(args[1], NumberStyles.None, CultureInfo.InvariantCulture)
    : BinaryPrimitives.ReadUInt64LittleEndian(RandomNumberGenerator.GetBytes(sizeof(ulong)));
new Holdout(root, seed).Generate();
Console.WriteLine($"Draw seed {seed.ToString(CultureInfo.InvariantCulture)}");
return 0;

record Op(string Operation, string Method, string[] Parameters, string Domain, string Expected, string Description,
    string Baseline, string Reference, string[][] Examples, Variant[] Variants, Dictionary<string, string>? Let = null);
record Variant(string Body, long[] Witness);
record Request(string Operation, long[] Args);

partial class Catalog
{
    public static Op[] All() => [.. Orders(), .. Scheduling(), .. Controls()];
    static Variant V(string body, params long[] witness) => new(body, witness);
}

partial class Holdout(string root, ulong seed)
{
    private static readonly JsonSerializerOptions Json = new()
    {
        WriteIndented = true,
        DefaultIgnoreCondition = JsonIgnoreCondition.WhenWritingNull,
    };

    public void Generate()
    {
        var ops = Catalog.All();
        if (ops.Count(op => op.Variants.Length == 3) != 20 || ops.Count(op => op.Variants.Length == 0) != 6)
            throw new InvalidOperationException("Unexpected catalog size.");
        ulong state = seed;
        var selected = ops.Where(op => op.Variants.Length != 0)
            .ToDictionary(op => op.Operation, _ => DrawIndex(ref state));
        var ordinary = ops.SelectMany(op => op.Examples.Select(example => Parse(op, example))).ToArray();
        var witnesses = ops.Where(op => op.Variants.Length != 0)
            .Select(op => new Request(op.Operation, op.Variants[selected[op.Operation]].Witness)).ToArray();
        var allWitnesses = ops.SelectMany(op => op.Variants.Select((item, index) =>
            new { op.Operation, Variant = index + 1, Input = new Request(op.Operation, item.Witness) })).ToArray();

        WriteProject("Public/src", "ForgeLedger", RenderApp(ops, op => selected.TryGetValue(op.Operation, out var index)
            ? op.Variants[index].Body : op.Baseline));
        WriteProject("Private/ReferenceSrc", "ForgeLedger.Reference", RenderApp(ops, op => op.Reference));
        WriteProject("Private/Qualification", "ForgeLedger.Qualification", RenderQualification(ops, selected));
        Write("Public/rules.json", Format(ops.Select(op => new
        {
            op.Operation, op.Parameters, op.Domain, op.Expected,
            Source = "contract.md#" + op.Operation, op.Let
        })));
        Write("Public/README.md", Readme);
        Write("Public/contract.md", RenderContract(ops));
        Write("Public/examples.json", Format(ordinary));
        Write("Private/selected-witness-input.json", Format(witnesses));
        Write("Private/all-variant-witnesses.json", Format(allWitnesses));
        Write("Private/draw-receipt.json", Format(new
        {
            Fixture = "ForgeLedger",
            Algorithm = "SplitMix64 with rejection of zero, uniform residue modulo 3",
            Seed = seed,
            FamilyCount = selected.Count,
            CorrectControlCount = ops.Length - selected.Count,
            OrdinaryCaseCount = ordinary.Length,
            Families = ops.Where(op => selected.ContainsKey(op.Operation)).Select(op =>
                new { op.Operation, op.Method, Variant = selected[op.Operation] + 1 }),
            Controls = ops.Where(op => op.Variants.Length == 0).Select(op => new { op.Operation, op.Method })
        }));
    }

    private static int DrawIndex(ref ulong state)
    {
        ulong value;
        do
        {
            state = unchecked(state + 0x9E3779B97F4A7C15UL);
            value = state;
            value = unchecked((value ^ (value >> 30)) * 0xBF58476D1CE4E5B9UL);
            value = unchecked((value ^ (value >> 27)) * 0x94D049BB133111EBUL);
            value ^= value >> 31;
        } while (value == 0);
        return (int)(value % 3);
    }

    private static Request Parse(Op op, string[] args) =>
        new(op.Operation, args.Select(value => long.Parse(value, CultureInfo.InvariantCulture)).ToArray());

    private static string Format<T>(T obj) => JsonSerializer.Serialize(obj, Json) + "\n";

    private void WriteProject(string directory, string name, string source)
    {
        Write(directory + "/" + name + ".csproj", $$"""
            <Project Sdk="Microsoft.NET.Sdk">
              <PropertyGroup>
                <OutputType>Exe</OutputType>
                <TargetFramework>net10.0</TargetFramework>
                <ImplicitUsings>enable</ImplicitUsings>
                <Nullable>enable</Nullable>
              </PropertyGroup>
            </Project>
            """ + "\n");
        Write(directory + "/Program.cs", source);
    }

    private void Write(string file, string value)
    {
        var target = Path.Combine(root, file);
        Directory.CreateDirectory(Path.GetDirectoryName(target)!);
        File.WriteAllText(target, value, new UTF8Encoding(false));
    }

    private static string RenderContract(IEnumerable<Op> ops) => "# Fabrication order contracts\n\n"
        + "The plant planner accepts the inputs in each operation's admitted domain in rules.json. That file lists the parameter order and calculation rules. Integer measurements and monetary amounts are exact.\n\n"
        + string.Join("\n", ops.Select(op => $"## {op.Operation}\n\n{op.Description}\n"));

    private static string RenderApp(Op[] ops, Func<Op, string> implementation)
    {
        var arms = string.Join("\n", ops.Select(op =>
            $"""            "{op.Operation}" when args.Length == {op.Parameters.Length} => {op.Method}({string.Join(", ", op.Parameters.Select((_, i) => $"args[{i}]"))}),"""));
        var methods = string.Join("\n\n", ops.Select(op =>
            RenderMethod(op.Method, op.Parameters, implementation(op))));
        return $$"""
            using System.Numerics;
            using System.Text.Json;

            if (args is not ["batch", var input])
            {
                Console.Error.WriteLine("Usage: ForgeLedger batch <input.json>");
                return 2;
            }

            try
            {
                var options = new JsonSerializerOptions { PropertyNameCaseInsensitive = true };
                var requests = JsonSerializer.Deserialize<BatchOrder?[]>(File.ReadAllText(input), options);
                if (requests is null)
                {
                    Console.Error.WriteLine("Batch input must be a JSON array.");
                    return 2;
                }

                var results = requests.Select(FloorPlanning.Calculate).ToArray();
                Console.WriteLine(JsonSerializer.Serialize(results));
                return 0;
            }
            catch (Exception error) when (error is IOException or JsonException)
            {
                Console.Error.WriteLine(error.Message);
                return 2;
            }

            internal record BatchOrder(string? Operation, long[]? Args);

            internal static class FloorPlanning
            {
                public static long? Calculate(BatchOrder? order)
                {
                    if (order?.Args is not { } args)
                        return null;

                    return order.Operation switch
                    {
            {{arms}}
                        _ => null,
                    };
                }

            {{methods}}

            {{Helpers}}
            }
            """ + "\n";
    }

    private static string RenderMethod(string name, IEnumerable<string> parameters, string body, string resultType = "long") =>
        $"    private static {resultType} {name}({string.Join(", ", parameters.Select(parameter => $"long {parameter}"))})\n"
        + "    {\n"
        + string.Join("\n", SplitStatements(body).Select(statement => "        " + statement))
        + "\n    }";

    private static IEnumerable<string> SplitStatements(string body)
    {
        int start = 0;
        int nesting = 0;
        for (int i = 0; i < body.Length; i++)
        {
            var token = body[i];
            if (token is '(' or '{') nesting++;
            if (token is ')' or '}') nesting--;
            if (token != ';' || nesting != 0)
                continue;
            yield return body[start..(i + 1)].Trim();
            start = i + 1;
        }

        if (start < body.Length)
            yield return body[start..].Trim();
    }

    private const string Helpers = """
        private static long min(long a, long b) => Math.Min(a, b);
        private static long max(long a, long b) => Math.Max(a, b);
        private static long abs(long value) => Math.Abs(value);
        private static long clamp(long value, long lower, long upper) => Math.Clamp(value, lower, upper);
        private static long popcount(long value) => BitOperations.PopCount((ulong)value);

        private static long mod(long value, long period)
        {
            var remainder = value % period;
            return remainder < 0 ? remainder + period : remainder;
        }
    """;

    private const string Readme = """
        # ForgeLedger fabrication planning

        This adapter processes an offline batch of fabrication planning calculations. The bundled executable under bin is built for .NET 10.

        Run a batch:

        ```powershell
        dotnet bin/ForgeLedger.dll batch examples.json
        ```

        Build from the included source:

        ```powershell
        dotnet build src/ForgeLedger.csproj -c Release -o bin
        ```

        The input is a JSON array of objects with case-sensitive operation IDs in `Operation` and positional signed 64-bit integers in `Args`. Output is a JSON array of signed 64-bit integers or null in input order. Unknown operations and rows with missing or incorrectly sized argument arrays produce null. A malformed JSON batch exits with code 2. Each operation specifies the inputs it admits in rules.json. Its behavior outside that operation's domain is unspecified.

        See contract.md for the plant policies. rules.json gives the parameter order, domain, expected calculation, and public contract section for each operation. A `Let` expression introduces a local calculation used by the expected expression. Division truncates toward zero and `mod` is the nonnegative remainder for a positive divisor. All measurements use the whole units named in each contract. examples-expected.json gives the corresponding results for examples.json.
        """ + "\n";
}
