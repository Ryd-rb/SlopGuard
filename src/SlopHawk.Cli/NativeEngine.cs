using System.Diagnostics;
using System.Security.Cryptography;
using System.Text.Json;
using System.Text.Json.Serialization;
using SlopHawk.Daemon;

namespace SlopHawk.Cli;

/// <summary>Explicit, digest-pinned native opt-in. Default public commands remain legacy.</summary>
public static class NativeEngine
{
    private static readonly JsonSerializerOptions Json = new(JsonSerializerDefaults.Web)
    {
        UnmappedMemberHandling = JsonUnmappedMemberHandling.Disallow
    };
    private sealed record Profile(int SchemaVersion, string Platform, string BinaryPath,
        string BinarySha256, string ReceiptPath, string ReceiptSha256, string GrantReference);
    private sealed record Receipt(int SchemaVersion, string Platform, string LegacyBinarySha256,
        string BinarySha256, bool MatrixComplete, bool CompatibilityComplete, bool ContainmentComplete,
        bool RecoveryComplete, bool PerformanceComplete, string[] QualifiedCommands,
        string[] QualifiedFormats, string ReceiptId);

    // Public verbs are unchanged; this explicit wrapper must never silently turn
    // a missing capability into a legacy fallback or permit a repository profile.
    public static async Task<int> RunAsync(string[] args)
    {
        if (args.Length < 4 || args[0] != "engine" || args[1] is not ("native" or "transfer" or "rollback"))
            throw new ArgumentException("Use 'slophawk engine native <external-profile> <SHA256> -- <command> [args]' " +
                "or 'slophawk engine transfer|rollback <external-profile> <SHA256> --root <checkout>'.");
        var profileBytes = ReadExternalFile(args[2], 16 * 1024, Environment.CurrentDirectory);
        var profileDigest = Sha(profileBytes);
        if (profileDigest != args[3])
            throw new UnauthorizedAccessException("The external native engine profile does not match its explicit SHA-256.");
        var profile = JsonSerializer.Deserialize<Profile>(profileBytes, Json)
            ?? throw new IOException("The selected native profile is empty.");
        var platform = OperatingSystem.IsWindows() ? "windows" : OperatingSystem.IsLinux() ? "linux" : "unsupported";
        if (profile.SchemaVersion != 1 || profile.Platform != platform || platform == "unsupported" ||
            !IsSha(profile.BinarySha256) || !IsSha(profile.ReceiptSha256) ||
            string.IsNullOrWhiteSpace(profile.GrantReference) || profile.GrantReference.Length > 128)
            throw new IOException("The selected native profile is incompatible with this platform.");
        var receiptBytes = ReadExternalFile(profile.ReceiptPath, 1024 * 1024, Environment.CurrentDirectory);
        if (Sha(receiptBytes) != profile.ReceiptSha256)
            throw new IOException("The owner qualification receipt changed after the profile was pinned.");
        var receipt = JsonSerializer.Deserialize<Receipt>(receiptBytes, Json)
            ?? throw new IOException("The owner qualification receipt is empty.");
        if (receipt.SchemaVersion != 1 || receipt.Platform != platform ||
            !IsSha(receipt.LegacyBinarySha256) || receipt.BinarySha256 != profile.BinarySha256 || !receipt.MatrixComplete ||
            !receipt.CompatibilityComplete || !receipt.ContainmentComplete ||
            !receipt.RecoveryComplete || !receipt.PerformanceComplete ||
            receipt.QualifiedCommands is null || receipt.QualifiedFormats is null ||
            receipt.QualifiedCommands.Length > 64 || receipt.QualifiedFormats.Length > 64 ||
            receipt.QualifiedCommands.Distinct(StringComparer.Ordinal).Count() != receipt.QualifiedCommands.Length ||
            receipt.ReceiptId is not { Length: > 0 and <= 128 })
            throw new IOException("The exact native release is not qualified for this OS and binary.");
        var binary = ReadPhysicalBinaryPath(profile.BinaryPath, Environment.CurrentDirectory);
        using (var stream = File.OpenRead(binary))
        {
            if (Convert.ToHexString(SHA256.HashData(stream)) != profile.BinarySha256)
                throw new IOException("The selected native executable does not match its qualified SHA-256.");
        }

        if (args[1] is "transfer" or "rollback")
        {
            if (args is not ["engine", _, _, _, "--root", _])
                throw new ArgumentException("State transitions require an explicit --root <checkout>.");
            var workspace = new Workspace(args[5]);
            RequireExternalWorkspaceFiles(workspace, args[2], profile.ReceiptPath, binary);
            if (!receipt.QualifiedFormats.Contains("migration-state.v1", StringComparer.Ordinal))
                throw new IOException("The owner did not qualify the native state interchange.");
            if (args[1] == "transfer")
            {
                MigrationStateBridge.TransferToNative(workspace, binary, profile.BinarySha256,
                    profileDigest, profile.GrantReference);
                Console.WriteLine("The Rust writer has claimed the schema-v5 state.");
            }
            else
            {
                MigrationStateBridge.RollbackToLegacy(workspace, binary, profile.BinarySha256,
                    profileDigest, profile.GrantReference);
                Console.WriteLine("The native writer drained and returned schema-v5 state to the managed writer.");
            }
            return 0;
        }
        if (args.Length < 6 || args[4] != "--")
            throw new ArgumentException("Native selection requires '--' followed by an existing public command.");
        var commandArgs = args[5..];
        if (commandArgs.Length == 0)
            throw new ArgumentException("Missing public command for native selection.");
        var command = commandArgs[0];
        if (!receipt.QualifiedCommands.Contains(command, StringComparer.Ordinal))
            throw new IOException($"The owner did not qualify native command '{command}'. No legacy fallback was used.");
        if (command is "start" or "status" or "results" or "recheck" or "stop" or "mcp")
        {
            if (commandArgs.Length is not (1 or 3) || commandArgs.Length == 3 && commandArgs[1] != "--root")
                throw new ArgumentException("Native workspace commands require [--root <checkout>].");
            var workspace = new Workspace(commandArgs.Length == 3 ? commandArgs[2] : Environment.CurrentDirectory);
            RequireExternalWorkspaceFiles(workspace, args[2], profile.ReceiptPath, binary);
            var state = MigrationStateBridge.ReadNativeStateDirectory(workspace, profile.BinarySha256, profileDigest);
            return await ForwardAsync(binary, ["--native-workspace-v1", workspace.Root, state, profileDigest, command]);
        }
        if (command is not ("check" or "check-contracts" or "check-observe" or "check-retest" or "check-review" or "shadow" or "contracts" or "probes" or "paths" or "projects"))
            throw new IOException($"Native command '{command}' has no Rust-owned entry point in this release.");
        return await ForwardAsync(binary, commandArgs);
    }

    private static void RequireExternalWorkspaceFiles(Workspace workspace, params string[] paths)
    {
        foreach (var path in paths)
            if (Within(workspace.Root, Path.GetFullPath(path)))
                throw new IOException("Native engine configuration and binaries must be outside the target checkout.");
    }

    private static async Task<int> ForwardAsync(string executable, string[] args)
    {
        var start = new ProcessStartInfo(executable)
        {
            UseShellExecute = false, CreateNoWindow = true,
            WorkingDirectory = Environment.CurrentDirectory,
            RedirectStandardOutput = true, RedirectStandardError = true
        };
        foreach (var arg in args)
            start.ArgumentList.Add(arg);
        using var process = Process.Start(start) ?? throw new IOException("The qualified native CLI did not start.");
        var stdout = process.StandardOutput.BaseStream.CopyToAsync(Console.OpenStandardOutput());
        var stderr = process.StandardError.BaseStream.CopyToAsync(Console.OpenStandardError());
        await Task.WhenAll(stdout, stderr, process.WaitForExitAsync());
        return process.ExitCode;
    }

    private static string ReadPhysicalBinaryPath(string path, string checkout)
    {
        if (!Path.IsPathFullyQualified(path))
            throw new IOException("Native executable must be an absolute physical path.");
        var full = Path.GetFullPath(path);
        if (Within(checkout, full) || (File.GetAttributes(full) &
            (FileAttributes.ReparsePoint | FileAttributes.Directory)) != 0)
            throw new IOException("Native executable cannot live in the checkout or be a link.");
        MigrationStateBridge.RequirePhysicalDirectory(Path.GetDirectoryName(full)!);
        return full;
    }

    private static byte[] ReadExternalFile(string path, int maxBytes, string checkout)
    {
        var full = ReadPhysicalBinaryPath(path, checkout);
        using var stream = File.OpenRead(full);
        if (stream.Length is <= 0 || stream.Length > maxBytes)
            throw new IOException("The selected external profile or receipt exceeds its bounded format.");
        var bytes = new byte[(int)stream.Length];
        stream.ReadExactly(bytes);
        if (stream.ReadByte() != -1)
            throw new IOException("The selected external profile or receipt changed while reading.");
        return bytes;
    }

    private static bool Within(string root, string path)
    {
        var relative = Path.GetRelativePath(Path.GetFullPath(root), path);
        return !Path.IsPathRooted(relative) && relative != ".." &&
            !relative.StartsWith(".." + Path.DirectorySeparatorChar, StringComparison.Ordinal);
    }
    private static bool IsSha(string digest) => digest.Length == 64 && digest.All(c =>
        c is >= '0' and <= '9' or >= 'A' and <= 'F');
    private static string Sha(byte[] bytes) => Convert.ToHexString(SHA256.HashData(bytes));
}
