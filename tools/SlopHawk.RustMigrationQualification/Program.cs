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
            if (args is ["benchmark-rust", var installed, var rustScratch])
                return await ResidentBenchmark.RunAsync(installed, rustScratch);
            if (args is ["benchmark-rust-checker", var checker, var worker, var rustCheckerScratch])
                return await CheckerBenchmark.RunAsync(checker, worker, rustCheckerScratch);
            if (args is ["--measure-checker", var checkerSpec])
            {
                var request = Read<CheckerBenchmark.Work>(checkerSpec);
                try { return await CheckerBenchmark.RunWorkAsync(request); }
                catch (Exception error) when (error is IOException or OperationCanceledException or JsonException or KeyNotFoundException)
                {
                    Write(request.Receipt + ".failed", error.Message);
                    throw new IOException("Measured checker failed.", error);
                }
            }
            Console.Error.WriteLine("Usage: qualification sample <spec.json> <capture.json> | " +
                "benchmark-rust <slophawk> <new-scratch> | " +
                "benchmark-rust-checker <slophawk> <fixture-dll> <new-scratch>");
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
