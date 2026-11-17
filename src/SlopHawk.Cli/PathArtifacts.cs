using System.Security.Cryptography;
using System.Text.Json;

namespace SlopHawk.Cli;

internal sealed record PathArtifact(string File, long Length, string Sha256);
internal sealed record PathSeal(string Format, string Kind, PathArtifact[] Files);

internal sealed class PathArtifacts
{
    private const string Format = "slophawk-scalar-run-v1";
    private const int MaximumJsonBytes = 16 * 1024 * 1024;
    private readonly string published;
    public string Staging { get; }

    public PathArtifacts(string scratch, string folder)
    {
        var root = Path.Combine(scratch, ".slophawk", folder);
        RequireDirectory(root, allowMissing: true);
        Directory.CreateDirectory(root);
        RequireDirectory(root);
        var id = Guid.NewGuid().ToString("N");
        published = Path.Combine(root, id);
        Staging = Path.Combine(root, $"{id}.pending");
        Directory.CreateDirectory(Staging);
    }

    public Task WriteAsync<T>(string name, T value) =>
        ForegroundCheck.PublishJsonAsync(Path.Combine(Staging, name), value);

    public async Task<string> PublishAsync(string kind)
    {
        var options = new EnumerationOptions { RecurseSubdirectories = true, AttributesToSkip = FileAttributes.ReparsePoint };
        var files = Directory.EnumerateFiles(Staging, "*", options).Take(1025)
            .Select(file => Path.GetRelativePath(Staging, file).Replace('\\', '/'))
            .Order(StringComparer.Ordinal).Select(file => Fingerprint(Staging, file)).ToArray();
        if (files.Length is 0 or > 1024)
            throw new IOException("The path run has an unexpected number of artifacts.");
        await WriteAsync("manifest.json", new PathSeal(Format, kind, files));
        Directory.Move(Staging, published);
        return Path.Combine(published, "manifest.json");
    }

    public static string Read(string manifest, string kind, params string[] required)
    {
        manifest = Path.GetFullPath(manifest);
        var directory = Path.GetDirectoryName(manifest)!;
        RequireDirectory(directory);
        if (Path.GetFileName(manifest) != "manifest.json" || !Guid.TryParseExact(Path.GetFileName(directory), "N", out _))
            throw new IOException("Provide a published path manifest.");
        var seal = ReadJson<PathSeal>(manifest);
        if (seal.Format != Format || seal.Kind != kind || seal.Files is not { Length: > 0 and <= 1024 } ||
            seal.Files.Any(file => file is null || file.File == "manifest.json") ||
            seal.Files.Select(file => file.File).Distinct(StringComparer.OrdinalIgnoreCase).Count() != seal.Files.Length ||
            required.Any(name => !seal.Files.Any(file => file.File == name)))
            throw new IOException("Unexpected or incomplete path manifest.");
        foreach (var file in seal.Files)
            if (Fingerprint(directory, file.File) != file)
                throw new IOException("A path artifact disagrees with its published manifest.");
        return directory;
    }

    public static T ReadJson<T>(string file) => ReadJson<T>(file, out _);

    public static T ReadJson<T>(string file, out string digest)
    {
        RequireOrdinaryFile(file);
        using var stream = File.OpenRead(file);
        if (stream.Length > MaximumJsonBytes)
            throw new IOException("A path checker input exceeds the JSON byte limit.");
        var bytes = new byte[(int)stream.Length];
        stream.ReadExactly(bytes);
        if (stream.ReadByte() != -1)
            throw new IOException("A path checker input changed while it was read.");
        digest = Convert.ToHexString(SHA256.HashData(bytes));
        ReadOnlySpan<byte> text = bytes;
        if (text.StartsWith("\uFEFF"u8))
            text = text[3..];
        return JsonSerializer.Deserialize<T>(text, ForegroundCheck.Json) ??
            throw new JsonException($"Missing path checker data in {file}.");
    }

    public static string Hash(string file)
    {
        RequireOrdinaryFile(file);
        using var input = File.OpenRead(file);
        return Convert.ToHexString(SHA256.HashData(input));
    }

    public static string ResolveInput(string root, string relative)
    {
        if (string.IsNullOrWhiteSpace(relative) || Path.IsPathRooted(relative))
            throw new ArgumentException("Paths in the spec must be relative to the spec directory.");
        var absolute = Path.GetFullPath(relative, root);
        if (!IsWithin(root, absolute))
            throw new ArgumentException("An input must stay inside the spec directory.");
        RequireOrdinaryFile(absolute);
        return absolute;
    }

    public static bool IsWithin(string root, string path)
    {
        var relative = Path.GetRelativePath(root, path);
        return !Path.IsPathRooted(relative) && relative.Split(Path.DirectorySeparatorChar)[0] != "..";
    }

    public static void RequireDirectory(string path, bool allowMissing = false)
    {
        for (var directory = new DirectoryInfo(path); directory is not null; directory = directory.Parent)
        {
            FileAttributes attributes;
            try
            {
                attributes = File.GetAttributes(directory.FullName);
            }
            catch (IOException ex) when (allowMissing && ex is DirectoryNotFoundException or FileNotFoundException)
            {
                continue;
            }
            if ((attributes & (FileAttributes.Directory | FileAttributes.ReparsePoint)) != FileAttributes.Directory)
                throw new IOException($"Use a physical directory for path checker inputs and scratch: {directory.FullName}");
        }
    }

    private static PathArtifact Fingerprint(string directory, string name)
    {
        if (string.IsNullOrEmpty(name) || name.Split('/').Any(part => part is "" or "." or ".." ||
            part.Any(c => !char.IsAsciiLetterOrDigit(c) && c is not '.' and not '-' and not '_')))
            throw new IOException("Invalid path artifact name.");
        var file = Path.Combine(directory, name);
        return new(name, new FileInfo(file).Length, Hash(file));
    }

    private static void RequireOrdinaryFile(string file)
    {
        RequireDirectory(Path.GetDirectoryName(Path.GetFullPath(file))!);
        if ((File.GetAttributes(file) & (FileAttributes.Directory | FileAttributes.ReparsePoint)) != 0)
            throw new IOException("Provide an ordinary file for path checker inputs and artifacts.");
    }
}
