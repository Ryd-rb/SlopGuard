using System.Security.Cryptography;

namespace SlopHawk.Daemon;

internal sealed class SourceScanner(Workspace workspace)
{
    internal const int MaxFiles = 20_000;
    internal const int MaxPathCharacters = 2_000_000;
    private const int MaxSourceBytes = 1_048_576;
    private const int MaxVisitedEntries = 200_000;

    internal async Task<SourceScan> ScanAsync(
        string[]? paths, SourceScan? prior, CancellationToken cancellationToken)
    {
        var files = new Dictionary<string, string>(Workspace.PathComparer);
        var unknown = new HashSet<string>(Workspace.PathComparer);
        if (paths is not null && prior is not null)
        {
            foreach (var entry in prior.Files)
                files.Add(entry.Key, entry.Value);
            foreach (var path in paths)
                files.Remove(workspace.Relative(path));
        }

        var pathCharacters = files.Keys.Sum(path => path.Length);
        var candidates = paths ?? EnumerateSources(unknown);
        foreach (var path in candidates)
        {
            cancellationToken.ThrowIfCancellationRequested();
            var relative = workspace.Relative(path);
            if (files.Count >= MaxFiles || pathCharacters + relative.Length > MaxPathCharacters)
            {
                MarkUnknown(unknown, "");
                break;
            }

            try
            {
                var digest = await ReadSourceAsync(path, paths is not null, cancellationToken);
                if (digest is null)
                    continue;
                files.Add(relative, digest);
                pathCharacters += relative.Length;
            }
            catch (Exception ex) when (ex is IOException or UnauthorizedAccessException)
            {
                MarkUnknown(unknown, relative);
            }
        }

        return new SourceScan(files, unknown);
    }

    internal static bool Matches(string path) =>
        Path.GetExtension(path).Equals(".cs", StringComparison.OrdinalIgnoreCase);

    private IEnumerable<string> EnumerateSources(HashSet<string> unknown)
    {
        var stack = new Stack<(string Directory, IEnumerator<string> Entries)>();
        Push(workspace.Root);
        var visited = 0;
        try
        {
            while (stack.TryPeek(out var current))
            {
                if (!MoveNext(current))
                {
                    stack.Pop().Entries.Dispose();
                    continue;
                }
                if (++visited > MaxVisitedEntries)
                {
                    MarkUnknown(unknown, "");
                    yield break;
                }

                var path = current.Entries.Current;
                if (!workspace.IsIncluded(path))
                    continue;

                FileAttributes flags;
                try
                {
                    flags = File.GetAttributes(path);
                }
                catch (Exception ex) when (ex is IOException or UnauthorizedAccessException)
                {
                    MarkUnknown(unknown, workspace.Relative(path));
                    continue;
                }
                if ((flags & FileAttributes.ReparsePoint) != 0)
                    MarkUnknown(unknown, workspace.Relative(path));
                else if ((flags & FileAttributes.Directory) != 0)
                    Push(path);
                else if (Matches(path))
                    yield return path;
            }
        }
        finally
        {
            foreach (var current in stack)
                current.Entries.Dispose();
        }

        void Push(string path)
        {
            try
            {
                stack.Push((path, Directory.EnumerateFileSystemEntries(path).GetEnumerator()));
            }
            catch (Exception ex) when (ex is IOException or UnauthorizedAccessException)
            {
                MarkUnknown(unknown, path == workspace.Root ? "" : workspace.Relative(path));
            }
        }

        bool MoveNext((string Directory, IEnumerator<string> Entries) current)
        {
            try
            {
                return current.Entries.MoveNext();
            }
            catch (Exception ex) when (ex is IOException or UnauthorizedAccessException)
            {
                MarkUnknown(unknown, current.Directory == workspace.Root ? "" : workspace.Relative(current.Directory));
                return false;
            }
        }
    }

    private async Task<string?> ReadSourceAsync(
        string path, bool targeted, CancellationToken cancellationToken)
    {
        try
        {
            if (targeted)
            {
                for (var parent = Path.GetDirectoryName(path); parent is not null &&
                     !Workspace.PathComparer.Equals(parent, workspace.Root); parent = Path.GetDirectoryName(parent))
                {
                    if ((File.GetAttributes(parent) & FileAttributes.ReparsePoint) != 0)
                        throw new IOException("A source ancestor is a link.");
                }
            }

            var flags = File.GetAttributes(path);
            if ((flags & FileAttributes.ReparsePoint) != 0)
                throw new IOException("The source is a link.");
            if ((flags & FileAttributes.Directory) != 0)
                return null;

            var before = File.GetLastWriteTimeUtc(path);
            await using var stream = new FileStream(path, FileMode.Open, FileAccess.Read,
                FileShare.ReadWrite | FileShare.Delete, bufferSize: 16_384, useAsync: true);
            var length = stream.Length;
            if (length > MaxSourceBytes)
                throw new IOException("The source exceeds the read budget.");

            var bytes = new byte[(int)length];
            await stream.ReadExactlyAsync(bytes, cancellationToken);
            if (stream.Length != length || File.GetLastWriteTimeUtc(path) != before)
                throw new IOException("A source file changed during analysis.");
            return Convert.ToHexString(SHA256.HashData(bytes));
        }
        catch (IOException ex) when (targeted && ex is FileNotFoundException or DirectoryNotFoundException)
        {
            return null;
        }
    }

    private static void MarkUnknown(HashSet<string> scopes, string path)
    {
        const int maxScopes = 512;
        if (scopes.Contains(""))
            return;
        if (path.Length == 0 || path.Length > 2_048 || scopes.Count == maxScopes)
        {
            scopes.Clear();
            scopes.Add("");
            return;
        }
        scopes.Add(path);
    }
}

internal sealed record SourceScan(Dictionary<string, string> Files, HashSet<string> UncertainScopes)
{
    internal bool Partial => UncertainScopes.Count != 0;

    internal static bool IsUncertain(HashSet<string> scopes, string path)
    {
        while (true)
        {
            if (scopes.Contains(path))
                return true;
            if (path.Length == 0)
                return false;
            var separator = path.LastIndexOf('/');
            path = separator < 0 ? "" : path[..separator];
        }
    }
}
