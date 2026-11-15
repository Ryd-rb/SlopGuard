using System.Security.Cryptography;
using System.Text;
using SlopGuard.Core;

namespace SlopGuard.Daemon;

internal sealed class SourceIndex(Workspace workspace, ISourceChecker checker)
{
    internal const int MaxFiles = 20_000;
    internal const int MaxPathCharacters = 2_000_000;
    private const int MaxIndexedDiagnostics = 4_000;
    private const int MaxReportedDiagnostics = 2_000;
    private const int MaxSourceBytes = 1_048_576;
    private const int MaxVisitedEntries = 200_000;

    internal async Task<SourceScan> ScanAsync(
        string[]? paths, SourceScan? prior, CancellationToken cancellationToken)
    {
        var files = new Dictionary<string, SyntaxEntry>(Workspace.PathComparer);
        var unknown = new HashSet<string>(Workspace.PathComparer);
        if (paths is not null && prior is not null)
        {
            foreach (var entry in prior.Files)
                files.Add(entry.Key, entry.Value);
            foreach (var path in paths)
                files.Remove(workspace.Relative(path));
        }

        var pathCharacters = files.Keys.Sum(path => path.Length);
        var diagnosticCount = files.Values.Sum(entry => entry.Diagnostics.Count);
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
                var (hash, bytes) = digest.Value;
                SyntaxEntry entry;
                if (prior is not null && prior.Files.TryGetValue(relative, out var cached) &&
                    cached.Digest == hash && !cached.Truncated)
                {
                    entry = cached;
                }
                else
                {
                    using var stream = new MemoryStream(bytes, writable: false);
                    using var reader = new StreamReader(stream, Encoding.UTF8, detectEncodingFromByteOrderMarks: true);
                    var source = await reader.ReadToEndAsync(cancellationToken);
                    var analysis = checker.Analyze(source, cancellationToken);
                    entry = new SyntaxEntry(hash, analysis.Diagnostics, analysis.Truncated);
                }

                var remaining = MaxIndexedDiagnostics - diagnosticCount;
                if (entry.Diagnostics.Count > remaining)
                    entry = entry with { Diagnostics = entry.Diagnostics.Take(remaining).ToArray(), Truncated = true };
                files.Add(relative, entry);
                diagnosticCount += entry.Diagnostics.Count;
                pathCharacters += relative.Length;
            }
            catch (Exception ex) when (ex is IOException or UnauthorizedAccessException)
            {
                MarkUnknown(unknown, relative);
            }
        }

        return new SourceScan(files, unknown);
    }

    internal WorkspaceReport CreateReport(SourceScan scan, Guid epoch, long revision, long committedRevision,
        GitRevision git)
    {
        var findings = new List<Finding>();
        // Reserve room for report metadata and conservatively charge for JSON escaping.
        var byteBudget = ReportFile.MaximumBytes - 1_048_576;
        var diagnosticCount = 0;
        var truncated = false;
        foreach (var (path, entry) in scan.Files.OrderBy(pair => pair.Key, Workspace.PathComparer))
        {
            truncated |= entry.Truncated;
            diagnosticCount += entry.Diagnostics.Count;
            foreach (var diagnostic in entry.Diagnostics)
            {
                if (findings.Count == MaxReportedDiagnostics)
                    break;
                var cost = 256 + 6 * (path.Length + diagnostic.Rule.Length + diagnostic.Message.Length);
                if (cost > byteBudget)
                    continue;
                byteBudget -= cost;
                findings.Add(new Finding(path, entry.Digest, diagnostic.Rule,
                    diagnostic.Message, diagnostic.Line, diagnostic.Column));
            }
        }

        return new WorkspaceReport(ReportFile.SchemaVersion, ReportFile.Producer, workspace.Root, epoch, revision,
            git.Branch, git.Commit, checker.Id, checker.Language, scan.Files.Count,
            scan.Partial, truncated || diagnosticCount > findings.Count, findings)
        {
            CommittedRevision = committedRevision
        };
    }

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
                else if (checker.Matches(path))
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

    private async Task<(string Hash, byte[] Bytes)?> ReadSourceAsync(
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
            return (Convert.ToHexString(SHA256.HashData(bytes)), bytes);
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

internal sealed record SyntaxEntry(string Digest, IReadOnlyList<Diagnostic> Diagnostics, bool Truncated);

internal sealed record SourceScan(Dictionary<string, SyntaxEntry> Files, HashSet<string> UncertainScopes)
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
