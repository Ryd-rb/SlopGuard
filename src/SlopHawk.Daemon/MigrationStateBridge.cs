using System.Diagnostics;
using System.Security.Cryptography;
using System.Text;
using System.Text.Json;
using System.Text.Json.Serialization;

namespace SlopHawk.Daemon;

/// <summary>Imports a stopped workspace's schema-v5 inventory into the Rust daemon.</summary>
public static class MigrationStateBridge
{
    private static readonly JsonSerializerOptions Json = new(JsonSerializerDefaults.Web)
    {
        UnmappedMemberHandling = JsonUnmappedMemberHandling.Disallow
    };

    public static void ImportLegacyInventory(Workspace workspace, string nativePath, string nativeSha256)
    {
        nativeSha256 = nativeSha256.ToUpperInvariant();
        WithWriterLock(workspace, () =>
        {
            RequireLegacyWriter(workspace);
            var root = RootIdentity.Read(workspace.Root);
            if (!new WorkerAdmission(workspace).CanRecover())
                throw new IOException("An admitted worker has not proved process-tree drainage.");
            var payload = JsonSerializer.SerializeToUtf8Bytes(new SourceInventory(workspace).ExportMigration(root), Json);
            RequireNativeImporter(workspace, nativePath, nativeSha256);
            var start = new ProcessStartInfo(nativePath)
            {
                UseShellExecute = false, CreateNoWindow = true,
                RedirectStandardInput = true, RedirectStandardError = true
            };
            start.ArgumentList.Add("--import-schema-v5");
            start.ArgumentList.Add("--root");
            start.ArgumentList.Add(workspace.Root);
            using var process = Process.Start(start) ?? throw new IOException("The native importer did not start.");
            using var deadline = new CancellationTokenSource(TimeSpan.FromSeconds(45));
            var error = ReadStderrAsync(process.StandardError.BaseStream, deadline.Token);
            try
            {
                process.StandardInput.BaseStream.WriteAsync(payload, deadline.Token).AsTask().GetAwaiter().GetResult();
                process.StandardInput.Close();
                process.WaitForExitAsync(deadline.Token).GetAwaiter().GetResult();
                if (process.ExitCode != 0)
                    throw new IOException("The native state import failed. " + error.GetAwaiter().GetResult());
                error.GetAwaiter().GetResult();
            }
            finally
            {
                if (!process.HasExited)
                {
                    try { process.Kill(entireProcessTree: true); }
                    catch (InvalidOperationException) { /* Exited during cancellation. */ }
                    if (!process.WaitForExit(3_000))
                        throw new IOException("The native importer remained active after its deadline.");
                }
            }
        });
    }

    private static void RequireLegacyWriter(Workspace workspace)
    {
        // A committed or staged writer marker blocks another schema-v5 import.
        var marker = Path.Combine(workspace.StateDirectory, "migration-engine.json");
        if (PathEntryExists(marker) || PathEntryExists(marker + ".pending"))
            throw new IOException("A native writer marker or pending transfer already exists. Preserve both inventories for recovery.");
    }

    private static bool PathEntryExists(string path)
    {
        try { _ = File.GetAttributes(path); return true; }
        catch (FileNotFoundException) { return false; }
        catch (DirectoryNotFoundException) { return false; }
    }

    private static void WithWriterLock(Workspace workspace, Action operation)
    {
        using var mutex = new Mutex(false, workspace.InstanceName);
        var acquired = false;
        try
        {
            try { acquired = mutex.WaitOne(TimeSpan.FromSeconds(15)); }
            catch (AbandonedMutexException) { acquired = true; }
            if (!acquired)
                throw new IOException("The previous daemon did not release its writer authority. Stop the original daemon with its installed binary and retry.");
            operation();
        }
        finally
        {
            if (acquired)
                mutex.ReleaseMutex();
        }
    }

    private static async Task<string> ReadStderrAsync(Stream stream, CancellationToken token)
    {
        var buffer = new byte[4097];
        var total = 0;
        while (total < buffer.Length)
        {
            var count = await stream.ReadAsync(buffer.AsMemory(total), token);
            if (count == 0)
                return new string(Encoding.UTF8.GetString(buffer, 0, total).Where(c => !char.IsControl(c)).Take(240).ToArray());
            total += count;
        }
        throw new IOException("Native importer diagnostics exceeded the 4 KiB limit.");
    }

    private static void RequireNativeImporter(Workspace workspace, string path, string digest)
    {
        if (!Path.IsPathFullyQualified(path) || !File.Exists(path) ||
            !Path.IsPathFullyQualified(Path.GetDirectoryName(path)!) ||
            !IsDigest(digest) || ExecutionProfile.Contains(workspace.Root, path))
            throw new IOException("A digest-checked external native binary is required.");
        Workspace.CheckDirectory(Path.GetDirectoryName(path)!);
        if ((File.GetAttributes(path) & (FileAttributes.ReparsePoint | FileAttributes.Directory)) != 0 ||
            !string.Equals(DigestFile(path), digest, StringComparison.Ordinal))
            throw new IOException("The selected native importer changed or is not a physical file.");
    }

    private static string DigestFile(string path)
    {
        using var stream = File.OpenRead(path);
        return Convert.ToHexString(SHA256.HashData(stream));
    }

    private static bool IsDigest(string value) => value.Length == 64 && value.All(c =>
        c is >= '0' and <= '9' or >= 'A' and <= 'F');
}
