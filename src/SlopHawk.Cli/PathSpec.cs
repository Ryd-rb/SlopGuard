using System.Security.Cryptography;
using System.Text.Json;
using SlopHawk.Core;
using SlopHawk.Providers.CSharp;

namespace SlopHawk.Cli;

internal sealed record PathTarget(string Operation, string Document, string Signature);
internal sealed record PathSpec(ScalarBuildOptions Build, PathTarget[] Targets, string Rules, string Published,
    int PerOperation = 256, int WorkLimit = 128, int RetestLimit = 16, int Seed = 1,
    string? RunVerb = null, string? PrepareVerb = null);

internal sealed record PathInputs(string Spec, string SpecSha256, string RulesSha256, string PublishedSha256,
    string CompilerSha256, string RuntimeVersion);
internal sealed record PathContext(PathSpec Spec, PathInputs Inputs, ScalarCompilation Compilation,
    ContractCases Catalog, ProbeExpectation[] Published)
{
    public static PathContext Open(string path, string scratch)
    {
        path = Path.GetFullPath(path);
        var root = Path.GetDirectoryName(path)!;
        PathArtifacts.RequireDirectory(scratch);
        if (PathArtifacts.IsWithin(root, scratch) || PathArtifacts.IsWithin(scratch, root))
            throw new ArgumentException("Use a separate writable scratch workspace outside the source directory.");
        var spec = PathArtifacts.ReadJson<PathSpec>(path, out var specHash);
        if (spec.Build is null || spec.Targets is not { Length: > 0 and <= 32 } ||
            spec.Targets.Any(target => target is null || string.IsNullOrWhiteSpace(target.Operation) ||
                string.IsNullOrWhiteSpace(target.Document) || string.IsNullOrWhiteSpace(target.Signature)) ||
            spec.Targets.Select(target => target.Operation).Distinct(StringComparer.Ordinal).Count() != spec.Targets.Length ||
            spec.PerOperation is < 1 or > 512 || spec.WorkLimit is < 1 or > 8192 || spec.RetestLimit is < 1 or > 32)
            throw new ArgumentException("Supply unique scalar targets and valid path operation, work and retest limits.");
        PathArtifacts.ResolveInput(root, spec.Build.Project);
        foreach (var source in spec.Build.Sources ?? [])
            PathArtifacts.ResolveInput(root, source);
        var rules = PathArtifacts.ReadJson<IntegerContract[]>(PathArtifacts.ResolveInput(root, spec.Rules), out var rulesHash);
        var published = PathArtifacts.ReadJson<ProbeExpectation[]>(PathArtifacts.ResolveInput(root, spec.Published), out var publishedHash);
        var compilation = ScalarCompilation.Create(root, spec.Build);
        return new(spec, new(path, specHash, rulesHash, publishedHash,
            Hash(JsonSerializer.SerializeToUtf8Bytes(compilation.Inputs)), Environment.Version.ToString()),
            compilation, new(rules, published), published);
    }

    public static string Hash(byte[] bytes) => Convert.ToHexString(SHA256.HashData(bytes));
}
