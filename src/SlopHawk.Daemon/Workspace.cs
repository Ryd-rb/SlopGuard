using System.Security.Cryptography;
using System.Text;

namespace SlopHawk.Daemon;

public sealed class Workspace
{
    private static readonly HashSet<string> ExcludedNames = new(StringComparer.OrdinalIgnoreCase)
    {
        ".git", ".slophawk", ".slophawk.pending.json", ".slopguard", ".slopguard.pending.json",
        ".artifacts", ".idea", ".vs", "bin", "obj",
        "TestResults", "node_modules"
    };

    internal static StringComparer PathComparer { get; } =
        OperatingSystem.IsWindows() ? StringComparer.OrdinalIgnoreCase : StringComparer.Ordinal;

    // Keep existing inventories, execution grants and daemon endpoints accessible across the rename.
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
            throw new IOException("SlopHawk could not resolve the per-user state directory.");
        StateDirectory = Path.GetFullPath(Path.Combine(stateDirectory, InstanceName));
        UserStateDirectory = Path.GetDirectoryName(StateDirectory)!;
        var relativeState = Path.GetRelativePath(Root, StateDirectory);
        if (!Path.IsPathRooted(relativeState) && relativeState.Split(Path.DirectorySeparatorChar)[0] != "..")
            throw new IOException("SlopHawk state requires a location outside the checkout.");
    }

    public string Root { get; }
    public string InstanceName { get; }
    internal string StateDirectory { get; }
    internal string UserStateDirectory { get; }
    internal string InventoryPath => Path.Combine(StateDirectory, "inventory.sqlite3");

    internal static void CreatePrivateDirectory(string path)
    {
        CheckDirectory(path, allowMissing: true);
        if (OperatingSystem.IsWindows())
            Directory.CreateDirectory(path);
        else
            Directory.CreateDirectory(path, UnixFileMode.UserRead | UnixFileMode.UserWrite | UnixFileMode.UserExecute);
        CheckDirectory(path);
    }

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
