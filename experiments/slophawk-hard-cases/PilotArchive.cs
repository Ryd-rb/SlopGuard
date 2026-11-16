using System.Text.Json;
using System.Text.Json.Serialization;
using SlopHawk.Daemon;

internal sealed record PromotionRequest(string Project, string Route, string Run, string Coverage,
    string Target, string Document, int Line, bool Paused = false);
internal sealed record RatingPacket(string[] Paths, BuildArtifactManifest Manifest, BuildIdentityStamp PriorStamp);
internal sealed record RunPacket(TestAttempt Attempt, ScheduleFence Live, string AdmittedLease, TestRunReceiptDetails Receipt);
internal sealed record CoveragePacket(CoverageSupervisedAttempt Attempt, CoverageResultsLease Lease, string Report);

internal static class PilotArchive
{
    private static readonly JsonSerializerOptions Json = new()
    {
        WriteIndented = true,
        Converters = { new JsonStringEnumConverter() }
    };

    internal static string Child(string root, string relative)
    {
        if (Path.IsPathRooted(relative) || relative.Split('/', '\\').Any(part => part is "" or "." or ".."))
            throw new InvalidDataException("Expected a bounded archive path.");
        return Path.GetFullPath(Path.Combine(root, relative));
    }

    internal static T Read<T>(string root, string relative) =>
        JsonSerializer.Deserialize<T>(File.ReadAllText(Child(root, relative)), Json)
        ?? throw new InvalidDataException(relative);

    internal static void Write<T>(string root, string relative, T value)
    {
        var destination = Child(root, relative);
        Directory.CreateDirectory(Path.GetDirectoryName(destination)!);
        File.WriteAllText(destination, JsonSerializer.Serialize(value, Json) + "\n");
    }
}
