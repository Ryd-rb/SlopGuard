using System.Globalization;
using System.Text.Json;
using SlopHawk.Core;

namespace SlopHawk.Cli;

internal static class ProbeCommands
{
    private static readonly JsonSerializerOptions Json = new() { PropertyNameCaseInsensitive = true };

    internal static async Task<int> RunAsync(string[] args)
    {
        using var output = Console.OpenStandardOutput();
        switch (args)
        {
            case ["probes", "plan", var seeds, var limit]:
                await WritePlan(seeds, limit, "0", output);
                return 0;
            case ["probes", "plan", var seeds, var limit, var randomSeed]:
                await WritePlan(seeds, limit, randomSeed, output);
                return 0;
            case ["probes", "plan", var seeds, var limit, var randomSeed, var scales]:
                await WritePlan(seeds, limit, randomSeed, output, await Read<long[]>(scales));
                return 0;
            case ["probes", "compare", var inputs, var candidate, var reference]:
                var results = ProbeReplay.Compare(await Read<ProbeInput[]>(inputs),
                    await Read<JsonElement[]>(candidate), await Read<JsonElement[]>(reference));
                await JsonSerializer.SerializeAsync(output, results);
                return 0;
            case ["probes", "methods", var candidate, var reference]:
                using (var candidateAssembly = File.OpenRead(candidate))
                using (var referenceAssembly = File.OpenRead(reference))
                    await JsonSerializer.SerializeAsync(output, ManagedBodyComparison.Compare(candidateAssembly, referenceAssembly));
                return 0;
            default:
                Console.Error.WriteLine("Run slophawk --help for probe command usage.");
                return 2;
        }
    }

    private static async Task WritePlan(string path, string limit, string seed, Stream output, long[]? scales = null)
    {
        if (!int.TryParse(limit, NumberStyles.None, CultureInfo.InvariantCulture, out var count) || count <= 0 ||
            !int.TryParse(seed, NumberStyles.Integer, CultureInfo.InvariantCulture, out var randomSeed))
            throw new ArgumentException("The probe count must be positive and the optional random seed must be a signed integer.");
        await JsonSerializer.SerializeAsync(output, ProbeReplay.Plan(await Read<ProbeInput[]>(path), count, randomSeed, scales));
    }

    private static async Task<T> Read<T>(string path)
    {
        await using var input = File.OpenRead(path);
        return await JsonSerializer.DeserializeAsync<T>(input, Json) ?? throw new JsonException("A JSON array was expected.");
    }
}
