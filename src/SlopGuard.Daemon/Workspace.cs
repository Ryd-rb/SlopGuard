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

    public Workspace(string root) : this(root, Path.Combine(
        Environment.GetFolderPath(Environment.SpecialFolder.LocalApplicationData,
            Environment.SpecialFolderOption.DoNotVerify), "SlopGuard", "state"))
    {
    }

    internal Workspace(string root, string stateDirectory)
    {
        Root = Path.TrimEndingDirectorySeparator(Path.GetFullPath(root));
        CheckDirectory(Root);

        var identity = OperatingSystem.IsWindows() ? Root.ToUpperInvariant() : Root;
        var bytes = Encoding.UTF8.GetBytes($"{Environment.UserName}\0{identity}");
        InstanceName = $"SlopGuard-{Convert.ToHexString(SHA256.HashData(bytes))[..32]}";
        if (!Path.IsPathFullyQualified(stateDirectory))
            throw new IOException("SlopGuard could not resolve the per-user state directory.");
        StateDirectory = Path.GetFullPath(Path.Combine(stateDirectory, InstanceName));
        var relativeState = Path.GetRelativePath(Root, StateDirectory);
        if (!Path.IsPathRooted(relativeState) && relativeState.Split(Path.DirectorySeparatorChar)[0] != "..")
            throw new IOException("SlopGuard state requires a location outside the checkout.");
    }

    public string Root { get; }
    public string InstanceName { get; }
    internal string StateDirectory { get; }
    internal string InventoryPath => Path.Combine(StateDirectory, "inventory.sqlite3");

    internal static void CheckDirectory(string path, bool allowMissing = false)
    {
        for (var part = new DirectoryInfo(path); part is not null; part = part.Parent)
        {
            FileAttributes flags;
            try
            {
                flags = File.GetAttributes(part.FullName);
            }
            catch (IOException ex) when (allowMissing && ex is FileNotFoundException or DirectoryNotFoundException)
            {
                continue;
            }
            if ((flags & (FileAttributes.Directory | FileAttributes.ReparsePoint)) != FileAttributes.Directory)
                throw new IOException($"Use a physical directory instead of '{part.FullName}'.");
        }
    }

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
