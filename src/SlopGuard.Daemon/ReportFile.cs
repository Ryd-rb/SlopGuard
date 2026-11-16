using System.Security.Cryptography;
using System.Text.Json;
using SlopGuard.Core;

namespace SlopGuard.Daemon;

internal sealed class ReportFile
{
    private readonly Workspace workspace;
    private readonly Func<CancellationToken, Task>? afterStaging;

    // The optional seam is used only to hold a staged report at a reproducible
    // interleaving in regression tests. It is not configured by the daemon.
    internal ReportFile(Workspace workspace, Func<CancellationToken, Task>? afterStaging = null)
    {
        this.workspace = workspace;
        this.afterStaging = afterStaging;
        directory = Path.Combine(workspace.Root, ".slopguard");
        report = Path.Combine(directory, "results.json");
        staging = Path.Combine(workspace.Root, ".slopguard.pending.json");
    }

    internal const string Producer = "SlopGuard";
    internal const int SchemaVersion = 2;
    internal const int MaximumBytes = 8 * 1024 * 1024;
    internal static JsonSerializerOptions Json { get; } = new(JsonSerializerDefaults.Web);

    private readonly string directory;
    private readonly string report;
    private readonly string staging;

    internal void Withdraw()
    {
        RequireOrdinaryDirectory(workspace.Root, allowMissing: false);
        if (!RequireOrdinaryDirectory(directory, allowMissing: true))
            return;
        DeleteOwnedReport(report);
    }

    internal async Task<bool> PublishAsync(
        WorkspaceReport snapshot, string rootIdentity, Func<CancellationToken, Task<bool>> isLive,
        Func<Action, bool> tryCommit, CancellationToken cancellationToken)
    {
        Workspace.CheckDirectory(workspace.Root);
        if (RootIdentity.Read(workspace.Root) != rootIdentity)
            return false;
        RequireOrdinaryDirectory(directory, allowMissing: true);
        Directory.CreateDirectory(directory);
        Workspace.CheckDirectory(directory);
        var directoryIdentity = RootIdentity.Read(directory);
        CheckReportDirectory();
        DeleteOwnedReport(staging);
        VerifyOwnership(report);

        var bytes = JsonSerializer.SerializeToUtf8Bytes(snapshot, Json);
        if (bytes.Length > MaximumBytes)
            throw new IOException("The current report exceeds the publication budget.");
        var digest = SHA256.HashData(bytes);

        var created = false;
        try
        {
            using (var stream = new FileStream(staging, FileMode.CreateNew, FileAccess.Write, FileShare.Delete))
            {
                created = true;
                stream.Write(bytes);
            }
            if (afterStaging is not null)
                await afterStaging(cancellationToken);

            // The live Git/root/epoch check must follow staging, not precede it.
            // Re-check all writable paths and the in-memory sequence immediately
            // before the single atomic same-filesystem rename.
            if (!await isLive(cancellationToken))
                return false;
            if (RootIdentity.Read(workspace.Root) != rootIdentity ||
                RootIdentity.Read(directory) != directoryIdentity)
                throw new IOException("The report publication directory changed after staging.");
            Workspace.CheckDirectory(directory);
            CheckReportDirectory();
            VerifyOwnership(report);
            if (!StagingMatches(bytes.Length, digest))
                throw new IOException("The staged report changed before publication.");
            return tryCommit(() => File.Move(staging, report, overwrite: true));
        }
        finally
        {
            // A replaced or linked staging path is not ours to remove. In
            // particular, never follow a symlink installed during a late race.
            if (created && RootStillMatches(rootIdentity) && StagingMatches(bytes.Length, digest))
                File.Delete(staging);
        }
    }

    private void CheckReportDirectory()
    {
        if (Directory.EnumerateFileSystemEntries(directory)
            .Any(path => !Workspace.PathComparer.Equals(path, report)))
            throw new IOException(".slopguard contains an unrecognized path.");
    }

    private bool RootStillMatches(string identity)
    {
        try
        {
            return RootIdentity.Read(workspace.Root) == identity;
        }
        catch (Exception ex) when (ex is IOException or UnauthorizedAccessException)
        {
            return false;
        }
    }

    private bool StagingMatches(int length, byte[] digest)
    {
        try
        {
            if ((File.GetAttributes(staging) & (FileAttributes.Directory | FileAttributes.ReparsePoint)) != 0)
                return false;
            using var stream = new FileStream(staging, FileMode.Open, FileAccess.Read,
                FileShare.ReadWrite | FileShare.Delete);
            if (stream.Length != length)
                return false;
            // Read exactly the bounded staged payload. Hashing a mutable stream
            // until EOF could otherwise let a concurrent appender extend work.
            var bytes = new byte[length];
            stream.ReadExactly(bytes);
            return stream.ReadByte() == -1 && SHA256.HashData(bytes).AsSpan().SequenceEqual(digest);
        }
        catch (Exception ex) when (ex is IOException or UnauthorizedAccessException)
        {
            return false;
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
