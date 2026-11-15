using System.Security.Cryptography;
using System.Text.Json;
using System.Text.Json.Serialization;

namespace SlopGuard.Daemon;

[Flags]
internal enum ExecutionOperation
{
    Evaluate = 1, Build = 2, Test = 4, Restore = 8, Network = 16, CompilerExtensions = 32
}

internal sealed record WorkerRequirements(ExecutionOperation Operations, bool UsesProcesses, IReadOnlyList<string> OutputRoots);

internal sealed record ExecutionGrant(string Revision, string RootIdentity, ExecutionOperation Operations, string[] OutputRoots,
    bool FromProfile = true)
{
    internal void Require(WorkerRequirements requirements)
    {
        if (!Operations.HasFlag(requirements.Operations))
            throw new UnauthorizedAccessException("The workspace execution profile does not grant the requested operations.");
        if (requirements.OutputRoots.Any(path => !OutputRoots.Any(root => ExecutionProfile.Contains(root, path))))
            throw new UnauthorizedAccessException("The workspace execution profile does not grant the requested output roots.");
    }
}

internal static class ExecutionProfile
{
    private const int MaxProfileBytes = 16 * 1024;
    private static readonly JsonSerializerOptions Json = new(JsonSerializerDefaults.Web)
    {
        UnmappedMemberHandling = JsonUnmappedMemberHandling.Disallow,
        Converters = { new JsonStringEnumConverter<ExecutionOperation>(JsonNamingPolicy.CamelCase, allowIntegerValues: false) }
    };

    internal static string PathFor(Workspace workspace) => Path.Combine(workspace.StateDirectory, "execution.json");

    internal static ExecutionGrant Read(Workspace workspace, string expectedRoot)
    {
        try
        {
            if (RootIdentity.Read(workspace.Root) != expectedRoot)
                throw new UnauthorizedAccessException("The workspace root changed. Review its execution profile.");
            Workspace.CheckDirectory(workspace.StateDirectory);
            var path = PathFor(workspace);
            if ((File.GetAttributes(path) & (FileAttributes.Directory | FileAttributes.ReparsePoint)) != 0)
                throw new UnauthorizedAccessException("The workspace execution profile must be a physical file.");
            using var stream = new FileStream(path, FileMode.Open, FileAccess.Read, FileShare.Read | FileShare.Delete);
            if (stream.Length is <= 0 or > MaxProfileBytes)
                throw new UnauthorizedAccessException("The workspace execution profile exceeds its size limit or is empty.");
            var bytes = new byte[(int)stream.Length];
            stream.ReadExactly(bytes);
            if (stream.ReadByte() != -1)
                throw new UnauthorizedAccessException("The workspace execution profile changed during reading.");
            var profile = JsonSerializer.Deserialize<Profile>(bytes, Json);
            if (profile is null || profile.SchemaVersion != 1 || profile.RootIdentity != expectedRoot ||
                profile.Operations is null || profile.Operations.Any(operation => !Enum.IsDefined(operation)) ||
                profile.OutputRoots is null || profile.OutputRoots.Length > 32)
                throw new UnauthorizedAccessException("The workspace execution profile is invalid or bound to another root.");

            var operations = profile.Operations.Aggregate((ExecutionOperation)0, (allowed, operation) => allowed | operation);
            return new ExecutionGrant(Convert.ToHexString(SHA256.HashData(bytes)), expectedRoot, operations,
                NormalizeRoots(workspace, profile.OutputRoots));
        }
        catch (FileNotFoundException ex)
        {
            throw new UnauthorizedAccessException("The workspace execution profile is missing.", ex);
        }
        catch (Exception ex) when (ex is IOException or JsonException or ArgumentException or NotSupportedException)
        {
            throw new UnauthorizedAccessException("The workspace execution profile is unreadable or invalid.", ex);
        }
    }

    internal static string[] NormalizeRoots(Workspace workspace, IReadOnlyList<string> roots)
    {
        if (roots.Count > 32)
            throw new UnauthorizedAccessException("Too many worker output roots were requested.");
        var paths = new SortedSet<string>(Workspace.PathComparer);
        foreach (var root in roots)
        {
            if (string.IsNullOrWhiteSpace(root) || !Path.IsPathFullyQualified(root))
                throw new UnauthorizedAccessException("Worker output roots must be absolute physical directory paths.");
            var full = Path.TrimEndingDirectorySeparator(Path.GetFullPath(root));
            Workspace.CheckDirectory(full, allowMissing: true);
            foreach (var protectedPath in new[] { workspace.UserStateDirectory,
                         Path.Combine(workspace.Root, ".git"), Path.Combine(workspace.Root, ".slopguard") })
            {
                if (Contains(full, protectedPath) || Contains(protectedPath, full))
                    throw new UnauthorizedAccessException("A worker output root overlaps SlopGuard or Git state.");
            }
            paths.Add(full);
        }
        return paths.ToArray();
    }

    internal static bool Contains(string root, string path)
    {
        var relative = Path.GetRelativePath(root, path);
        return !Path.IsPathRooted(relative) && relative != ".." &&
            !relative.StartsWith(".." + Path.DirectorySeparatorChar, StringComparison.Ordinal);
    }

    private sealed record Profile(int SchemaVersion, string RootIdentity, ExecutionOperation[] Operations, string[] OutputRoots);
}
