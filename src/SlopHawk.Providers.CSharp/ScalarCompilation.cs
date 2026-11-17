using System.Collections.Immutable;
using System.Security.Cryptography;
using System.Text;
using Microsoft.CodeAnalysis;
using Microsoft.CodeAnalysis.CSharp;
using Microsoft.CodeAnalysis.Emit;
using Microsoft.CodeAnalysis.Text;

namespace SlopHawk.Providers.CSharp;

public sealed record ScalarBuildOptions(string Project, string[] Sources, string ReferencePack,
    string[]? Defines = null, bool Checked = false, bool Optimize = true);

public sealed record ScalarCompilerFile(string Name, string Sha256);

public sealed record ScalarCompilerInputs(string Target, string Compiler, ScalarCompilerFile Project,
    ScalarCompilerFile[] Sources, ScalarCompilerFile[] References, string[] Defines, bool Checked, bool Optimize);

/// <summary>One closed, deterministic .NET 10 compilation. The emitted candidate and graph use these same trees.</summary>
public sealed class ScalarCompilation
{
    private readonly CSharpCompilation compilation;
    public ScalarCompilerInputs Inputs { get; }

    private ScalarCompilation(CSharpCompilation compilation, ScalarCompilerInputs inputs)
    {
        this.compilation = compilation;
        Inputs = inputs;
    }

    public static ScalarCompilation Create(string root, ScalarBuildOptions options)
    {
        if (options.Sources is not { Length: > 0 and <= 128 } ||
            options.Defines is { Length: > 64 } ||
            string.IsNullOrWhiteSpace(options.ReferencePack))
            throw new ArgumentException("Supply a reference pack, one to 128 source files, and at most 64 defines.");
        var project = ReadSource(root, options.Project);
        if (!project.File.Name.EndsWith(".csproj", StringComparison.OrdinalIgnoreCase))
            throw new ArgumentException("The controlled project must name a .csproj file.");
        var sources = options.Sources.Select(path => ReadSource(root, path)).ToArray();
        if (sources.Any(source => !source.File.Name.EndsWith(".cs", StringComparison.OrdinalIgnoreCase)) ||
            sources.Select(source => source.File.Name).Distinct(StringComparer.OrdinalIgnoreCase).Count() != sources.Length)
            throw new ArgumentException("Controlled source files must be distinct C# files.");

        var pack = Path.GetFullPath(options.ReferencePack, root);
        var references = Directory.GetFiles(pack, "*.dll").Order(StringComparer.Ordinal).ToArray();
        if (Path.GetFileName(pack) != "net10.0" || references.Length is < 2 or > 256 ||
            !File.Exists(Path.Combine(pack, "System.Runtime.dll")))
            throw new ArgumentException("Use the framework reference directory for Microsoft.NETCore.App.Ref/ref/net10.0.");
        var referenceFiles = new List<ScalarCompilerFile>();
        var metadata = new List<MetadataReference>();
        foreach (var path in references)
        {
            var bytes = ReadFile(path);
            var name = Path.GetFileName(path);
            referenceFiles.Add(new(name, Hash(bytes)));
            metadata.Add(MetadataReference.CreateFromImage(ImmutableArray.Create(bytes), filePath: name));
        }

        var defines = options.Defines ?? [];
        var parseOptions = new CSharpParseOptions(LanguageVersion.CSharp14, preprocessorSymbols: defines);
        var trees = sources.Select(source =>
        {
            using var stream = new MemoryStream(source.Bytes, writable: false);
            return CSharpSyntaxTree.ParseText(SourceText.From(stream, Encoding.UTF8, SourceHashAlgorithm.Sha256),
                parseOptions, source.File.Name);
        });
        var output = new CSharpCompilationOptions(OutputKind.ConsoleApplication,
            optimizationLevel: options.Optimize ? OptimizationLevel.Release : OptimizationLevel.Debug,
            checkOverflow: options.Checked, deterministic: true, nullableContextOptions: NullableContextOptions.Enable);
        var compiler = string.Join('/', new[] { typeof(CSharpCompilation).Assembly, typeof(Compilation).Assembly }
            .Select(assembly =>
            {
                using var file = File.OpenRead(assembly.Location);
                return $"{assembly.GetName().Version}/{Convert.ToHexString(SHA256.HashData(file))}";
            }));
        return new(CSharpCompilation.Create("candidate", trees, metadata, output),
            new("net10.0", compiler,
                project.File, sources.Select(source => source.File).ToArray(), [.. referenceFiles],
                defines, options.Checked, options.Optimize));
    }

    public byte[] Emit()
    {
        using var result = new MemoryStream();
        var emitted = compilation.Emit(result, options: new EmitOptions(debugInformationFormat: DebugInformationFormat.Embedded));
        if (!emitted.Success)
            throw new ArgumentException("Controlled compilation failed. " +
                string.Join(Environment.NewLine, emitted.Diagnostics.Where(d => d.Severity == DiagnosticSeverity.Error).Take(12)));
        return result.ToArray();
    }

    public ScalarPathResult Explore(string document, string signature, IReadOnlyList<long[]> seeds,
        int work, int cases, CancellationToken cancellationToken) =>
        ScalarPaths.Explore(compilation, document.Replace('\\', '/'), signature, seeds, work, cases, cancellationToken);

    /// <summary>Project Roslyn facts from the exact compilation used for <see cref="Emit"/>.</summary>
    public ScalarPathFactProjection ProjectFacts(string documentPath, string signature,
        CancellationToken cancellationToken = default) =>
        ScalarPaths.ProjectFacts(compilation, documentPath.Replace('\\', '/'), signature, cancellationToken);

    private static (ScalarCompilerFile File, byte[] Bytes) ReadSource(string root, string relative)
    {
        if (string.IsNullOrWhiteSpace(relative) || Path.IsPathRooted(relative))
            throw new ArgumentException("Project and source paths must be relative to the spec directory.");
        var file = Path.GetFullPath(relative, root);
        var name = Path.GetRelativePath(root, file).Replace('\\', '/');
        if (name.Split('/').Any(part => part == ".."))
            throw new ArgumentException("Source paths must stay inside the spec directory.");
        var bytes = ReadFile(file);
        return (new(name, Hash(bytes)), bytes);
    }

    private static byte[] ReadFile(string path)
    {
        const int limit = 8 * 1024 * 1024;
        using var stream = File.OpenRead(path);
        if (stream.Length > limit)
            throw new IOException($"A controlled compilation input exceeds {limit} bytes.");
        var bytes = new byte[(int)stream.Length];
        stream.ReadExactly(bytes);
        if (stream.ReadByte() != -1)
            throw new IOException("A controlled compilation input changed while it was read.");
        return bytes;
    }

    private static string Hash(byte[] bytes) => Convert.ToHexString(SHA256.HashData(bytes));
}
