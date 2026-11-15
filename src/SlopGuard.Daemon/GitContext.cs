using System.Diagnostics;

namespace SlopGuard.Daemon;

internal sealed record GitRevision(string? Branch, string? Commit);

internal sealed class GitContext(string root, string? gitDir, string? commonDir, string? executable)
{
    internal IEnumerable<string> MetadataDirectories =>
        new[] { gitDir, commonDir }.OfType<string>().Distinct(Workspace.PathComparer);

    internal static async Task<GitContext> DiscoverAsync(string root, CancellationToken cancellationToken)
    {
        var git = FindExecutable();
        if (git is null)
        {
            if (Path.Exists(Path.Combine(root, ".git")))
                throw new IOException("Git must be on PATH to watch a Git checkout.");
            return new GitContext(root, null, null, null);
        }
        var output = await GitAsync(git, root, cancellationToken, "rev-parse", "--absolute-git-dir",
            "--path-format=absolute", "--git-common-dir");

        var directories = output.Split('\n', StringSplitOptions.TrimEntries | StringSplitOptions.RemoveEmptyEntries);
        if (directories.Length == 2)
            return new GitContext(root, Path.GetFullPath(directories[0]), Path.GetFullPath(directories[1]), git);
        if (Path.Exists(Path.Combine(root, ".git")))
            throw new IOException("Git could not resolve workspace metadata.");
        return new GitContext(root, null, null, git);
    }

    internal async Task<GitRevision> ReadAsync(CancellationToken cancellationToken)
    {
        if (gitDir is null)
        {
            if (Path.Exists(Path.Combine(root, ".git")))
                throw new IOException("Git metadata appeared. Restart SlopGuard to attach to it.");
            return new GitRevision(null, null);
        }

        var headPath = Path.Combine(gitDir, "HEAD");
        var before = await File.ReadAllTextAsync(headPath, cancellationToken);
        if (File.Exists(Path.Combine(gitDir, "index.lock")) || File.Exists(headPath + ".lock"))
            throw new IOException("Git is updating the workspace.");

        var branch = before.StartsWith("ref: ", StringComparison.Ordinal) ? before[5..].Trim() : null;
        var commit = (await GitAsync(executable!, root, cancellationToken, "rev-parse", "--verify", "--quiet", "HEAD")).Trim();
        var after = await File.ReadAllTextAsync(headPath, cancellationToken);
        if (before != after || File.Exists(Path.Combine(gitDir, "index.lock")) || File.Exists(headPath + ".lock"))
            throw new IOException("Git changed during verification.");
        if (commit.Length == 0 && branch is null)
            throw new IOException("Git could not resolve detached HEAD.");
        return new GitRevision(branch, commit.Length == 0 ? null : commit);
    }

    internal static bool IsContextChange(string directory, string path)
    {
        var relative = Path.GetRelativePath(directory, path).Replace(Path.DirectorySeparatorChar, '/');
        return relative is "HEAD" or "HEAD.lock" or "index" or "index.lock" or "packed-refs" or "packed-refs.lock"
            or "refs" or "reftable" || relative.StartsWith("refs/", StringComparison.Ordinal)
            || relative.StartsWith("reftable/", StringComparison.Ordinal);
    }

    private static string? FindExecutable()
    {
        var directories = (Environment.GetEnvironmentVariable("PATH") ?? string.Empty).Split(Path.PathSeparator);
        var name = OperatingSystem.IsWindows() ? "git.exe" : "git";
        foreach (var entry in directories)
        {
            var directory = entry.Trim().Trim('"');
            if (!Path.IsPathFullyQualified(directory))
                continue;
            var candidate = Path.Combine(directory, name);
            if (File.Exists(candidate))
                return candidate;
        }
        return null;
    }

    private static async Task<string> GitAsync(
        string executable, string root, CancellationToken cancellationToken, params string[] args)
    {
        var info = new ProcessStartInfo(executable)
        {
            WorkingDirectory = root,
            UseShellExecute = false,
            CreateNoWindow = true,
            RedirectStandardOutput = true,
            RedirectStandardError = true,
        };
        foreach (var name in new[] { "GIT_DIR", "GIT_WORK_TREE", "GIT_COMMON_DIR", "GIT_INDEX_FILE" })
            info.Environment.Remove(name);
        info.ArgumentList.Add("--no-optional-locks");
        foreach (var argument in args)
            info.ArgumentList.Add(argument);

        using var process = Process.Start(info) ?? throw new IOException("Could not start Git.");
        using var timeout = CancellationTokenSource.CreateLinkedTokenSource(cancellationToken);
        timeout.CancelAfter(TimeSpan.FromSeconds(3));
        try
        {
            var stdout = process.StandardOutput.ReadToEndAsync(timeout.Token);
            var stderr = process.StandardError.ReadToEndAsync(timeout.Token);
            await process.WaitForExitAsync(timeout.Token);
            await stderr;
            return process.ExitCode == 0 ? await stdout : string.Empty;
        }
        catch (OperationCanceledException)
        {
            if (!process.HasExited)
                process.Kill();
            cancellationToken.ThrowIfCancellationRequested();
            throw new IOException("Git metadata lookup timed out.");
        }
    }
}
