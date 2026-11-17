using System.Collections.Immutable;
using System.Security.Cryptography;
using System.Text;
using Microsoft.CodeAnalysis;
using Microsoft.CodeAnalysis.CSharp;

namespace SlopHawk.Providers.CSharp.Worker;

internal sealed record LoadedSource(FactSource Identity, byte[] Bytes, string Digest, string Text,
    SourceByteMap Map, CSharpSyntaxTree Tree);

internal sealed record LoadedJob(FactRequest Request, FactBudget Budget, FactContext Context,
    FactScope Scope, LoadedSource Target, LoadedSource[] Sources,
    string ReferencePackDigest, IReadOnlyList<MetadataReference> References);

internal static class WorkerValidation
{
    private const int MaxSourceFiles = 24;
    private const int MaxSourceBytes = 512 * 1024;
    private const int MaxAllSourceBytes = 768 * 1024;
    private const long MaxReferencePackBytes = 64L * 1024 * 1024;

    public static LoadedJob Load(FactRequest request)
    {
        if (request.Version != FrameProtocol.Version || request.Kind != "analyze")
            throw new InvalidRequestException("protocol_version", "Unsupported request version or kind.");
        var epoch = Nonempty(request.WorkspaceEpoch, 128);
        var revision = Nonempty(request.CommittedRevision, 160);
        var documentId = Nonempty(request.DocumentId, 160);
        var root = AbsoluteDirectory(request.WorkspaceRoot);
        var roots = request.LinkedRoots ?? [];
        if (roots.Length > 16)
            throw new InvalidRequestException("linked_scope", "Too many caller-authorized linked roots.");
        var linkedRoots = roots.Select(AbsoluteDirectory).Distinct(StringComparer.Ordinal).ToArray();
        var context = request.Context ?? throw new InvalidRequestException("context", "Context is mandatory.");
        var key = Nonempty(context.Key, 256);
        if (context.TargetFramework != "net10.0" || context.LanguageVersion != "CSharp14")
            throw new InvalidRequestException("target", "This worker supports net10.0/CSharp14 only.");
        var assembly = Nonempty(context.AssemblyName, 128);
        if (assembly.Contains('/') || assembly.Contains('\\'))
            throw new InvalidRequestException("assembly_name", "Assembly name must not contain a path.");
        var defines = context.Defines ?? [];
        if (defines.Length > 64 || defines.Distinct(StringComparer.Ordinal).Count() != defines.Length ||
            defines.Any(value => string.IsNullOrWhiteSpace(value) || value.Length > 128 ||
                !SyntaxFacts.IsValidIdentifier(value)))
            throw new InvalidRequestException("defines", "Invalid or duplicate C# preprocessor defines.");
        var budget = request.Limits ?? new FactBudget();
        if (budget.MaxRecords is < 2 or > 2400 || budget.MaxFrontiers is < 3 or > 32 ||
            budget.MaxSyntaxNodes is < 1 or > 100_000 || budget.MaxDiagnostics is < 1 or > 32)
            throw new InvalidRequestException("limits", "Requested bounded budgets are out of range.");
        var sources = request.Sources ?? [];
        if (sources.Length is < 1 or > MaxSourceFiles)
            throw new InvalidRequestException("source_count", "Expected one to 24 explicitly supplied sources.");
        var knownIds = new HashSet<string>(StringComparer.Ordinal);
        var loaded = new List<LoadedSource>(sources.Length);
        var total = 0;
        var parseOptions = new CSharpParseOptions(LanguageVersion.CSharp14, preprocessorSymbols: defines);
        foreach (var source in sources)
        {
            if (source is null)
                throw new InvalidRequestException("source_required", "Every source must be an object.");
            var id = Nonempty(source.DocumentId, 160);
            if (!knownIds.Add(id))
                throw new InvalidRequestException("duplicate_document", "Each source needs a unique document ID.");
            var logical = Nonempty(source.LogicalPath, 512);
            if (Path.IsPathRooted(logical) || logical.Split('/', '\\').Any(part =>
                    part is "" or "." or "..") || !logical.EndsWith(".cs", StringComparison.OrdinalIgnoreCase))
                throw new InvalidRequestException("logical_path", "Logical source path must be a relative C# path.");
            var physical = AbsoluteDirectory(source.PhysicalPath);
            if (!physical.EndsWith(".cs", StringComparison.OrdinalIgnoreCase))
                throw new InvalidRequestException("physical_path", "Physical source identity must name a C# file.");
            if (source.Provenance is not ("normal" or "linked" or "generated"))
                throw new InvalidRequestException("provenance", "Unknown source provenance.");
            var insideWorkspace = Contained(root, physical);
            if (source.Provenance == "normal" && !insideWorkspace ||
                source.Provenance is "linked" or "generated" && !insideWorkspace &&
                !linkedRoots.Any(allowed => Contained(allowed, physical)))
                throw new InvalidRequestException("physical_scope", "Source path is outside the caller-declared physical scope.");
            if (source.ByteMapping is not ("exactUtf8" or "unavailable"))
                throw new InvalidRequestException("byte_mapping", "Unknown source byte mapping declaration.");
            var base64 = source.BytesBase64 ?? "";
            if (base64.Length == 0 || base64.Length > 4 * ((MaxSourceBytes + 2) / 3))
                throw new InvalidRequestException("source_limit", "Source bytes exceed the per-file limit.");
            byte[] bytes;
            try { bytes = Convert.FromBase64String(base64); }
            catch (FormatException)
            { throw new InvalidRequestException("source_base64", "Source bytes must be base64."); }
            if (bytes.Length > MaxSourceBytes || (total += bytes.Length) > MaxAllSourceBytes)
                throw new InvalidRequestException("source_limit", "Source bytes exceed the total source budget.");
            var digest = Convert.ToHexString(SHA256.HashData(bytes));
            if (source.Sha256 is null || source.Sha256.Length != 64 ||
                !source.Sha256.Equals(digest, StringComparison.OrdinalIgnoreCase))
                throw new InvalidRequestException("source_identity_mismatch", "Declared SHA-256 did not match supplied bytes.");
            var (text, byteMap) = SourceByteMap.Decode(bytes, source.ByteMapping == "exactUtf8");
            var tree = (CSharpSyntaxTree)CSharpSyntaxTree.ParseText(text, parseOptions,
                path: physical, encoding: Encoding.UTF8);
            loaded.Add(new LoadedSource(source, bytes, digest, text, byteMap, tree));
        }
        var target = loaded.SingleOrDefault(source => source.Identity.DocumentId == documentId)
            ?? throw new InvalidRequestException("target_document", "Target document must be in the source closure.");
        var (references, packDigest) = GetReferences(context.ReferencePack);
        return new LoadedJob(request, budget, context,
            new FactScope(epoch, revision, key, documentId, target.Digest),
            target, [.. loaded], packDigest, references);
    }

    private static (IReadOnlyList<MetadataReference>, string) GetReferences(string? path)
    {
        var folder = AbsoluteDirectory(path);
        if (Path.GetFileName(folder) != "net10.0" || !File.Exists(Path.Combine(folder, "System.Runtime.dll")))
            throw new InvalidRequestException("reference_pack", "Supply an explicit net10.0 reference-pack directory.");
        var files = Directory.EnumerateFiles(folder, "*.dll").Take(257).Order(StringComparer.Ordinal).ToArray();
        if (files.Length is < 2 or > 256)
            throw new InvalidRequestException("reference_pack", "Reference pack file count is outside bounds.");
        long bytes = 0;
        using var identity = IncrementalHash.CreateHash(HashAlgorithmName.SHA256);
        var references = new List<MetadataReference>(files.Length);
        foreach (var file in files)
        {
            var info = new FileInfo(file);
            if (!Contained(folder, file) || (info.Attributes & FileAttributes.ReparsePoint) != 0)
                throw new InvalidRequestException("reference_pack", "Reference files must be local to the declared pack.");
            using var stream = File.OpenRead(file);
            var length = stream.Length;
            if (length < 1 || length > MaxReferencePackBytes - bytes)
                throw new InvalidRequestException("reference_pack", "Reference-pack bytes are outside bounds.");
            var image = new byte[(int)length];
            stream.ReadExactly(image);
            if (stream.ReadByte() != -1)
                throw new InvalidRequestException("reference_pack", "A reference changed while reading the pack.");
            bytes += length;
            identity.AppendData(Encoding.UTF8.GetBytes(info.Name + "\0"));
            identity.AppendData(SHA256.HashData(image));
            references.Add(MetadataReference.CreateFromImage(ImmutableArray.Create(image), filePath: file));
        }
        return (references, Convert.ToHexString(identity.GetHashAndReset()));
    }

    private static string Nonempty(string? value, int maximum)
    {
        if (string.IsNullOrWhiteSpace(value) || value.Length > maximum || value.Contains('\0'))
            throw new InvalidRequestException("identity", "Required identity is blank, oversized or contains NUL.");
        return value;
    }

    private static string AbsoluteDirectory(string? path)
    {
        var input = Nonempty(path, 1024);
        if (!Path.IsPathFullyQualified(input))
            throw new InvalidRequestException("absolute_path", "Physical paths and roots must be fully qualified.");
        return Path.TrimEndingDirectorySeparator(Path.GetFullPath(input));
    }

    private static bool Contained(string root, string path)
    {
        var relative = Path.GetRelativePath(root, path);
        return relative == "." ||
            (relative != ".." && !relative.StartsWith(".." + Path.DirectorySeparatorChar, StringComparison.Ordinal) &&
            !Path.IsPathRooted(relative));
    }
}
