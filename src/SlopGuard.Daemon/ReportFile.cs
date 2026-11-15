using System.Text.Json;
using SlopGuard.Core;

namespace SlopGuard.Daemon;

internal sealed class ReportFile(Workspace workspace)
{
    internal const string Producer = "SlopGuard";
    internal const int SchemaVersion = 2;
    internal const int MaximumBytes = 8 * 1024 * 1024;
    internal static JsonSerializerOptions Json { get; } = new(JsonSerializerDefaults.Web);

    private readonly string directory = Path.Combine(workspace.Root, ".slopguard");
    private readonly string report = Path.Combine(workspace.Root, ".slopguard", "results.json");
    private readonly string staging = Path.Combine(workspace.Root, ".slopguard.pending.json");

    internal void Withdraw()
    {
        RequireOrdinaryDirectory(workspace.Root, allowMissing: false);
        if (!RequireOrdinaryDirectory(directory, allowMissing: true))
            return;
        DeleteOwnedReport(report);
    }

    internal bool Publish(WorkspaceReport snapshot, Func<bool> isCurrent)
    {
        RequireOrdinaryDirectory(workspace.Root, allowMissing: false);
        RequireOrdinaryDirectory(directory, allowMissing: true);
        Directory.CreateDirectory(directory);
        if (Directory.EnumerateFileSystemEntries(directory).Any(path => !Workspace.PathComparer.Equals(path, report)))
            throw new IOException(".slopguard contains an unrecognized path.");
        DeleteOwnedReport(staging);
        VerifyOwnership(report);

        var bytes = JsonSerializer.SerializeToUtf8Bytes(snapshot, Json);
        if (bytes.Length > MaximumBytes)
            throw new IOException("The current report exceeds the publication budget.");

        var created = false;
        try
        {
            using (var stream = new FileStream(staging, FileMode.CreateNew, FileAccess.Write, FileShare.Delete))
            {
                created = true;
                stream.Write(bytes);
            }
            if (!isCurrent())
                return false;
            File.Move(staging, report, overwrite: true);
            return true;
        }
        finally
        {
            if (created)
                File.Delete(staging);
        }
    }

    private void DeleteOwnedReport(string path)
    {
        if (VerifyOwnership(path))
            File.Delete(path);
    }

    private bool VerifyOwnership(string path)
    {
        FileAttributes flags;
        try
        {
            flags = File.GetAttributes(path);
        }
        catch (FileNotFoundException)
        {
            return false;
        }
        if ((flags & (FileAttributes.Directory | FileAttributes.ReparsePoint)) != 0)
            throw new IOException($"Unexpected report entry '{Path.GetFileName(path)}'.");
        using var stream = new FileStream(path, FileMode.Open, FileAccess.Read, FileShare.ReadWrite | FileShare.Delete);
        if (stream.Length <= MaximumBytes)
        {
            try
            {
                using var json = JsonDocument.Parse(stream);
                var value = json.RootElement;
                if (value.ValueKind == JsonValueKind.Object
                    && value.TryGetProperty("producer", out var producer) && producer.ValueKind == JsonValueKind.String
                    && producer.GetString() == Producer
                    && value.TryGetProperty("root", out var root) && root.ValueKind == JsonValueKind.String
                    && Workspace.PathComparer.Equals(root.GetString(), workspace.Root))
                    return true;
            }
            catch (JsonException)
            {
                // Leave files whose ownership cannot be verified untouched.
            }
        }
        throw new IOException($"SlopGuard could not verify ownership of '{Path.GetFileName(path)}'.");
    }

    private static bool RequireOrdinaryDirectory(string path, bool allowMissing)
    {
        try
        {
            var flags = File.GetAttributes(path);
            if ((flags & (FileAttributes.Directory | FileAttributes.ReparsePoint)) != FileAttributes.Directory)
                throw new IOException($"Unexpected directory type for '{path}'.");
            return true;
        }
        catch (FileNotFoundException) when (allowMissing)
        {
            return false;
        }
        catch (DirectoryNotFoundException) when (allowMissing)
        {
            return false;
        }
    }
}
