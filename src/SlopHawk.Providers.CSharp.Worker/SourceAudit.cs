using System.Security.Cryptography;
using System.Text;
using System.Text.Json;
using Microsoft.CodeAnalysis;
using Microsoft.CodeAnalysis.CSharp;
using Microsoft.CodeAnalysis.Text;

namespace SlopHawk.Providers.CSharp.Worker;

internal static class SourceAudit
{
    private const int MaxFiles = 10_000;
    private const int MaxFileBytes = 8 * 1024 * 1024;
    private const int MaxManifestBytes = 2 * 1024 * 1024;
    private const long MaxTotalBytes = 256 * 1024 * 1024;
    private static readonly UTF8Encoding Utf8 = new(false, true);
    private static readonly JsonSerializerOptions Json = new(JsonSerializerDefaults.Web);

    internal static int Run(string root, string manifest)
    {
        try
        {
            root = Path.GetFullPath(root);
            for (var directory = new DirectoryInfo(root); directory is not null; directory = directory.Parent)
                if ((directory.Attributes & FileAttributes.ReparsePoint) != 0)
                    throw new IOException("Source root is redirected.");
            if (new FileInfo(manifest).Length > MaxManifestBytes)
                throw new IOException("Source manifest exceeds its byte limit.");
            var request = JsonSerializer.Deserialize<AuditRequest>(File.ReadAllBytes(manifest), Json);
            if (request?.Files is not { Length: > 0 and <= MaxFiles } ||
                request.Defines is { Length: > 64 } ||
                request.Files.Any(path => string.IsNullOrWhiteSpace(path)) ||
                request.Files.Distinct(OperatingSystem.IsWindows() ? StringComparer.OrdinalIgnoreCase :
                    StringComparer.Ordinal).Count() != request.Files.Length)
                throw new ArgumentException("Invalid, empty, or duplicate source manifest.");

            var parse = new CSharpParseOptions(LanguageVersion.CSharp14, preprocessorSymbols: request.Defines);
            long reviewedBytes = 0;
            var reports = new List<AuditDocument>(request.Files.Length);
            foreach (string relative in request.Files)
            {
                string file = Admit(root, relative);
                using var stream = File.OpenRead(file);
                if (stream.Length > MaxFileBytes || reviewedBytes + stream.Length > MaxTotalBytes)
                {
                    reports.Add(new(relative, null, "byte_budget", [], []));
                    continue;
                }

                byte[] bytes = new byte[stream.Length];
                stream.ReadExactly(bytes);
                if (stream.ReadByte() != -1)
                    throw new IOException("Source changed during the scan.");
                reviewedBytes += bytes.Length;
                string digest = Convert.ToHexString(SHA256.HashData(bytes));
                string source;
                try
                {
                    var content = bytes.AsSpan();
                    source = Utf8.GetString(content.StartsWith(Encoding.UTF8.Preamble) ? content[3..] : content);
                }
                catch (DecoderFallbackException)
                {
                    reports.Add(new(relative, digest, "invalid_utf8", [], []));
                    continue;
                }

                var tree = CSharpSyntaxTree.ParseText(SourceText.From(source, Utf8), parse, relative);
                var errors = tree.GetDiagnostics().Where(diagnostic => diagnostic.Severity == DiagnosticSeverity.Error)
                    .Select(diagnostic => diagnostic.Id).Distinct().Order().ToArray();
                reports.Add(errors.Length > 0
                    ? new(relative, digest, "parse_error", errors, [])
                    : new(relative, digest, "reviewed", [], SquaredDeltaOverflow.Find(tree)));
            }
            Console.WriteLine(JsonSerializer.Serialize(new
            {
                format = "slophawk.csharp-source-audit-v1",
                defines = parse.PreprocessorSymbolNames,
                documents = reports
            }, Json));
            return reports.All(report => report.Status == "reviewed") ? 0 : 1;
        }
        catch (Exception error) when (error is IOException or ArgumentException or UnauthorizedAccessException
            or JsonException or NotSupportedException)
        {
            Console.Error.WriteLine(error.Message);
            return 2;
        }
    }

    private static string Admit(string root, string relative)
    {
        var parts = relative.Split('/');
        if (Path.IsPathRooted(relative) || !relative.EndsWith(".cs", StringComparison.OrdinalIgnoreCase)
            || parts.Any(part => part is "" or "." or ".." || part.IndexOfAny(Path.GetInvalidFileNameChars()) >= 0
                || part.Contains('\\')))
            throw new ArgumentException($"Invalid manifest path {relative}.");

        string current = root;
        foreach (string part in parts)
        {
            current = Path.Combine(current, part);
            if ((File.GetAttributes(current) & FileAttributes.ReparsePoint) != 0)
                throw new IOException($"Source path is redirected: {relative}.");
        }
        return current;
    }

    private sealed record AuditRequest(string[] Files, string[]? Defines);
    private sealed record AuditDocument(string Path, string? Sha256, string Status, string[] ParseErrors,
        SourceAuditFinding[] Findings);
}
