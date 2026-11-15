using System.Security.Cryptography;
using System.Text;
using SlopGuard.Core;

namespace SlopGuard.Daemon;

internal sealed class SourceIndex(Workspace workspace, ISourceChecker checker)
{
    private const int MaxFiles = 20_000;
    private const int MaxIndexedDiagnostics = 4_000;
    private const int MaxReportedDiagnostics = 2_000;
    private const int MaxSourceBytes = 1_048_576;
    private const int MaxIndexedPathCharacters = 2_000_000;

    private readonly Dictionary<string, Entry> files = new(Workspace.PathComparer);
    private int indexedDiagnostics;
    private int indexedPathCharacters;
    private bool fileLimitReached;

    internal async Task RefreshAsync(string[] paths, bool fullScan, CancellationToken cancellationToken)
    {
        if (!fullScan)
        {
            foreach (var path in paths)
                await RefreshFileAsync(path, cancellationToken);
            return;
        }

        var seen = new HashSet<string>(Workspace.PathComparer);
        fileLimitReached = false;
        var pathCharacters = 0;
        foreach (var path in EnumerateSources())
        {
            cancellationToken.ThrowIfCancellationRequested();
            if (seen.Count == MaxFiles || pathCharacters + path.Length > MaxIndexedPathCharacters)
            {
                fileLimitReached = true;
                break;
            }
            seen.Add(path);
            pathCharacters += path.Length;
        }

        foreach (var path in files.Keys.Where(path => !seen.Contains(path)).ToArray())
            Remove(path);
        foreach (var path in seen)
            await RefreshFileAsync(path, cancellationToken);
    }

    internal WorkspaceReport CreateReport(Guid epoch, long revision, GitRevision git)
    {
        var findings = new List<Finding>();
        // Reserve room for report metadata and conservatively charge for JSON escaping.
        var byteBudget = ReportFile.MaximumBytes - 1_048_576;
        foreach (var (path, entry) in files.OrderBy(pair => pair.Key, Workspace.PathComparer))
        {
            var relativePath = workspace.Relative(path);
            foreach (var diagnostic in entry.Diagnostics)
            {
                if (findings.Count == MaxReportedDiagnostics)
                    break;
                var cost = 256 + 6 * (relativePath.Length + diagnostic.Rule.Length + diagnostic.Message.Length);
                if (cost > byteBudget)
                    continue;
                byteBudget -= cost;
                findings.Add(new Finding(relativePath, entry.Digest!, diagnostic.Rule,
                    diagnostic.Message, diagnostic.Line, diagnostic.Column));
            }
            if (findings.Count == MaxReportedDiagnostics)
                break;
        }

        return new WorkspaceReport(ReportFile.SchemaVersion, ReportFile.Producer, workspace.Root, epoch, revision,
            git.Branch, git.Commit, checker.Id, checker.Language, files.Count,
            fileLimitReached || files.Values.Any(entry => entry.Digest is null),
            indexedDiagnostics > findings.Count || files.Values.Any(entry => entry.Truncated), findings);
    }

    private IEnumerable<string> EnumerateSources()
    {
        var pending = new Stack<IEnumerator<string>>();
        try
        {
            pending.Push(Directory.EnumerateFileSystemEntries(workspace.Root).GetEnumerator());
            while (pending.TryPeek(out var children))
            {
                if (!children.MoveNext())
                {
                    pending.Pop().Dispose();
                    continue;
                }

                var path = children.Current;
                if (!workspace.IsIncluded(path))
                    continue;
                var flags = Attributes(path);
                if (flags is null || (flags.Value & FileAttributes.ReparsePoint) != 0)
                    continue;
                if ((flags.Value & FileAttributes.Directory) != 0)
                    pending.Push(Directory.EnumerateFileSystemEntries(path).GetEnumerator());
                else if (checker.Matches(path))
                    yield return path;
            }
        }
        finally
        {
            foreach (var enumerator in pending)
                enumerator.Dispose();
        }
    }

    private async Task RefreshFileAsync(string path, CancellationToken cancellationToken)
    {
        var flags = Attributes(path);
        if (flags is null || (flags.Value & (FileAttributes.ReparsePoint | FileAttributes.Directory)) != 0
            || HasLinkedAncestor(path))
        {
            Remove(path);
            return;
        }
        if ((files.Count == MaxFiles || indexedPathCharacters + path.Length > MaxIndexedPathCharacters)
            && !files.ContainsKey(path))
        {
            fileLimitReached = true;
            return;
        }

        await using var stream = new FileStream(path, FileMode.Open, FileAccess.Read,
            FileShare.ReadWrite | FileShare.Delete, bufferSize: 16_384, useAsync: true);
        var length = stream.Length;
        if (length > MaxSourceBytes)
        {
            Set(path, new Entry(null, [], false));
            return;
        }

        var bytes = new byte[(int)length];
        await stream.ReadExactlyAsync(bytes, cancellationToken);
        if (stream.Length != length)
            throw new IOException("A source file changed during analysis.");
        var digest = Convert.ToHexString(SHA256.HashData(bytes));
        if (files.TryGetValue(path, out var existing) && existing.Digest == digest && !existing.Truncated)
            return;

        // StreamReader detects byte order marks for legacy C# files as well as UTF-8.
        using var textStream = new MemoryStream(bytes, writable: false);
        using var reader = new StreamReader(textStream, Encoding.UTF8, detectEncodingFromByteOrderMarks: true);
        var text = await reader.ReadToEndAsync(cancellationToken);
        var analysis = checker.Analyze(text, cancellationToken);
        var allowance = MaxIndexedDiagnostics - indexedDiagnostics + (existing?.Diagnostics.Count ?? 0);
        var diagnostics = analysis.Diagnostics.Take(allowance).ToArray();
        Set(path, new Entry(digest, diagnostics, analysis.Truncated || diagnostics.Length < analysis.Diagnostics.Count));
    }

    private bool HasLinkedAncestor(string path)
    {
        for (var parent = Path.GetDirectoryName(path); parent is not null
                && !Workspace.PathComparer.Equals(parent, workspace.Root); parent = Path.GetDirectoryName(parent))
        {
            if ((Attributes(parent) & FileAttributes.ReparsePoint) != 0)
                return true;
        }
        return false;
    }

    private static FileAttributes? Attributes(string path)
    {
        try
        {
            return File.GetAttributes(path);
        }
        catch (FileNotFoundException)
        {
            return null;
        }
        catch (DirectoryNotFoundException)
        {
            return null;
        }
    }

    private void Set(string path, Entry entry)
    {
        Remove(path);
        files.Add(path, entry);
        indexedDiagnostics += entry.Diagnostics.Count;
        indexedPathCharacters += path.Length;
    }

    private void Remove(string path)
    {
        if (files.Remove(path, out var old))
        {
            indexedDiagnostics -= old.Diagnostics.Count;
            indexedPathCharacters -= path.Length;
        }
    }

    private sealed record Entry(string? Digest, IReadOnlyList<Diagnostic> Diagnostics, bool Truncated);
}
