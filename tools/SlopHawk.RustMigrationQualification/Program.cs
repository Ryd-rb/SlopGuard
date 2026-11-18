using System.Text.Json;
using System.Text.Json.Serialization;

namespace SlopHawk.RustMigrationQualification;

internal static class Program
{
    internal static JsonSerializerOptions Json { get; } = new(JsonSerializerDefaults.Web)
    {
        WriteIndented = true,
        UnmappedMemberHandling = JsonUnmappedMemberHandling.Disallow
    };

    public static async Task<int> Main(string[] args)
    {
        try
        {
            if (args is ["sample", var spec, var output])
            {
                var run = await Sampler.CaptureAsync(Read<SampleSpec>(spec));
                Write(output, run);
                return run.ScopeComplete ? 0 : 1;
            }
            if (args is ["benchmark-resident" or "benchmark-resident-events", var legacy, var native, var scratch])
                return await ResidentBenchmark.RunAsync(legacy, native, scratch, includeSoak: args[0] == "benchmark-resident");
            if (args is ["qualify", var input, var receipt, var report])
            {
                var result = Qualification.Evaluate(Read<QualificationInput>(input));
                Write(receipt, result.Receipt);
                Write(report, result.Report);
                return result.Report.Qualified ? 0 : 1;
            }
            Console.Error.WriteLine("Usage: qualification sample <spec.json> <capture.json> | " +
                "qualify <evidence.json> <receipt.json> <report.json> | " +
                "benchmark-resident|benchmark-resident-events <legacy> <native> <new-scratch>");
            return 2;
        }
        catch (Exception ex) when (ex is IOException or JsonException or ArgumentException or
                                   UnauthorizedAccessException or InvalidOperationException or FormatException or
                                   System.ComponentModel.Win32Exception)
        {
            Console.Error.WriteLine(ex.Message);
            return 1;
        }
    }

    internal static T Read<T>(string path)
    {
        using var stream = File.OpenRead(path);
        if (stream.Length is <= 0 or > 16 * 1024 * 1024)
            throw new IOException("Qualification input exceeds the 16 MiB bound.");
        return JsonSerializer.Deserialize<T>(stream, Json)
            ?? throw new IOException("Qualification input was empty.");
    }

    internal static void Write<T>(string path, T value)
    {
        if (File.Exists(path))
            throw new IOException($"Refusing to overwrite an existing qualification artifact: {path}");
        using var stream = new FileStream(path, FileMode.CreateNew, FileAccess.Write, FileShare.None);
        JsonSerializer.Serialize(stream, value, Json);
        stream.Flush(flushToDisk: true);
    }
}
