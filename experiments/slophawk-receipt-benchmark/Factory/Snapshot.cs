using System.Security.Cryptography;
using System.Text.Json;

internal sealed record SnapshotEntry(string Path, string Kind, string? Sha256, long CreatedTicks, long ModifiedTicks);

internal static class Snapshot
{
    internal static void Report(string directory)
    {
        var root = new DirectoryInfo(Path.GetFullPath(directory));
        var entries = new List<SnapshotEntry>();
        Visit(root);

        var ordered = entries.OrderBy(entry => entry.Path, StringComparer.Ordinal).ToArray();
        var fingerprint = Convert.ToHexString(SHA256.HashData(JsonSerializer.SerializeToUtf8Bytes(ordered)));
        Console.WriteLine(JsonSerializer.Serialize(new { Fingerprint = fingerprint, Entries = ordered }));

        void Visit(FileSystemInfo entry)
        {
            entry.Refresh();
            if (entry.Attributes.HasFlag(FileAttributes.ReparsePoint))
                throw new IOException("Linked archive entries cannot be fingerprinted.");

            var relative = Path.GetRelativePath(root.FullName, entry.FullName).Replace('\\', '/');
            if (entry is DirectoryInfo folder)
            {
                entries.Add(new SnapshotEntry(relative, "directory", null,
                    entry.CreationTimeUtc.Ticks, entry.LastWriteTimeUtc.Ticks));
                foreach (var child in folder.EnumerateFileSystemInfos())
                    Visit(child);
            }
            else
            {
                using var stream = File.OpenRead(entry.FullName);
                var digest = Convert.ToHexString(SHA256.HashData(stream));
                entries.Add(new SnapshotEntry(relative, "file", digest,
                    entry.CreationTimeUtc.Ticks, entry.LastWriteTimeUtc.Ticks));
            }
        }
    }
}
