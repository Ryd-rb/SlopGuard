using System.Security.Cryptography;
using System.Text;

namespace SlopGuard.Daemon;

public sealed class Workspace
{
    private static readonly HashSet<string> ExcludedNames = new(StringComparer.OrdinalIgnoreCase)
    {
        ".git", ".slopguard", ".slopguard.pending.json", ".artifacts", ".idea", ".vs", "bin", "obj",
        "TestResults", "node_modules"
    };

    internal static StringComparer PathComparer { get; } =
        OperatingSystem.IsWindows() ? StringComparer.OrdinalIgnoreCase : StringComparer.Ordinal;

    public Workspace(string root)
    {
        Root = Path.TrimEndingDirectorySeparator(Path.GetFullPath(root));
        // Resolve alias directories before deriving the singleton key.
        var directory = new DirectoryInfo(Root);
        if (!directory.Exists)
            throw new DirectoryNotFoundException(Root);
        for (DirectoryInfo? part = directory; part is not null; part = part.Parent)
        {
            if (part.Attributes.HasFlag(FileAttributes.ReparsePoint))
                throw new IOException($"Use the workspace's physical path instead of '{part.FullName}'.");
        }

        var identity = OperatingSystem.IsWindows() ? Root.ToUpperInvariant() : Root;
        var bytes = Encoding.UTF8.GetBytes($"{Environment.UserName}\0{identity}");
        InstanceName = $"SlopGuard-{Convert.ToHexString(SHA256.HashData(bytes))[..32]}";
    }

    public string Root { get; }
    public string InstanceName { get; }

    internal bool IsIncluded(string fullPath)
    {
        var relative = Path.GetRelativePath(Root, fullPath);
        if (relative is "." or ".." || Path.IsPathRooted(relative))
            return false;
        var parts = relative.Split(Path.DirectorySeparatorChar);
        return parts[0] != ".." && !parts.Any(ExcludedNames.Contains);
    }

    internal string Relative(string fullPath) =>
        Path.GetRelativePath(Root, fullPath).Replace(Path.DirectorySeparatorChar, '/');
}
