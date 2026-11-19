using System.Buffers.Binary;
using System.Diagnostics;
using System.Security.Cryptography;
using System.Text;
using System.Text.Json;
using SlopHawk.Providers.CSharp.Worker;
using Xunit;

namespace SlopHawk.IntegrationTests;

public sealed class RoslynFactWorkerTests
{
    private static readonly JsonSerializerOptions JsonOptions = new(JsonSerializerDefaults.Web);
    private static readonly string Root = Path.GetFullPath(Path.Combine(Path.GetTempPath(), "slophawk-worker-synthetic"));

    [Fact]
    public async Task SubprocessReturnsBoundCrossFileCallAndExitsWithoutCreatingSources()
    {
        var request = Request(Source("A", "class A { void Run() { B.Touch(); } }"),
            Source("B", "class B { public static void Touch() {} }"));
        var result = await Execute(RequestFrame(request));
        Assert.Equal(0, result.ExitCode);
        Assert.True(result.Exited);
        Assert.Equal("complete", Terminal(result).GetProperty("status").GetString());
        Assert.Equal("slophawk.csharp.roslyn.experimental",
            Terminal(result).GetProperty("provider").GetProperty("id").GetString());
        Assert.StartsWith("10.0.", Terminal(result).GetProperty("provider").GetProperty("sdk").GetString());
        Assert.Contains(result.Facts, item =>
            item.GetProperty("capability").GetString() == "calls" &&
            item.GetProperty("value").GetProperty("binding").GetString() == "direct" &&
            item.GetProperty("value").GetProperty("calleeId").GetString() is { Length: 64 });
        Assert.Equal(request.WorkspaceEpoch,
            result.Facts[0].GetProperty("scope").GetProperty("workspaceEpoch").GetString());
        Assert.Equal(request.CommittedRevision,
            result.Facts[0].GetProperty("scope").GetProperty("committedRevision").GetString());
        Assert.Equal(request.DocumentId,
            result.Facts[0].GetProperty("scope").GetProperty("documentId").GetString());
        Assert.False(Directory.Exists(Root)); // all source contents were supplied as bytes
    }

    [Fact]
    public async Task ValidArrayTypesDoNotEmitZeroWidthTokenFacts()
    {
        var result = await Execute(RequestFrame(Request(Source("A",
            "class A { readonly int[] values = new int[1]; System.Type Type => typeof(System.Collections.Generic.List<>); }"))));

        Assert.Equal(0, result.ExitCode);
        Assert.Equal("complete", Terminal(result).GetProperty("capabilities").GetProperty("tokens").GetString());
        var tokens = result.Facts.Where(fact => fact.GetProperty("capability").GetString() == "tokens").ToArray();
        Assert.Contains(tokens, token => token.GetProperty("value").GetProperty("text").GetString() == "[");
        Assert.All(tokens, token =>
            Assert.True(token.GetProperty("span").GetProperty("byteSpan").GetProperty("length").GetInt32() > 0));
    }

    [Fact]
    public async Task SourceAuditReportsUnboundedDeltasAndAcceptsAProvenTwoAxisGuard()
    {
        const string code = """
            class Distances
            {
                long Unbounded(int a, int b, int c, int d)
                {
                    long dx = (long)a - b;
                    long dy = (long)c - d;
                    long result = dx * dx + dy * dy;
                    return result;
                }

                long Guarded(int a, int b, int c, int d)
                {
                    long dx = (long)a - b;
                    long dy = (long)c - d;
                    if (Math.Abs(dy) > int.MaxValue || Math.Abs(dx) > int.MaxValue) return long.MaxValue;
                    long result = dy * dy + dx * dx;
                    return result;
                }

                long OneAxis(int a, int b, int c, int d)
                {
                    long dx = (long)a - b;
                    long dy = (long)c - d;
                    if (Math.Abs(dx) > int.MaxValue || Math.Abs(dx) > int.MaxValue) return long.MaxValue;
                    long result = dx * dx + dy * dy;
                    return result;
                }

                long LocalFixture()
                {
                    int a = 12, b = 4, c = 8, d = 4;
                    long dx = (long)a - b;
                    long dy = (long)c - d;
                    long result = dx * dx + dy * dy;
                    return result;
                }
            }
            """;
        var workspace = Directory.CreateTempSubdirectory("slophawk-source-audit-");
        try
        {
            var source = Path.Combine(workspace.FullName, "Distances.cs");
            var manifest = Path.Combine(workspace.FullName, "manifest.json");
            File.WriteAllText(source, code);
            File.WriteAllText(manifest, """{"files":["Distances.cs"]}""");
            var info = new ProcessStartInfo(DotnetExecutable())
            {
                RedirectStandardOutput = true,
                RedirectStandardError = true,
                UseShellExecute = false
            };
            foreach (string arg in new[] { WorkerBinary(), "--source-audit-v1", "--root", workspace.FullName, "--manifest", manifest })
                info.ArgumentList.Add(arg);
            using var process = Process.Start(info)!;
            using var timeout = new CancellationTokenSource(TimeSpan.FromSeconds(30));
            var stdout = process.StandardOutput.ReadToEndAsync(timeout.Token);
            var stderr = process.StandardError.ReadToEndAsync(timeout.Token);
            await process.WaitForExitAsync(timeout.Token);
            Assert.True(process.ExitCode == 0, await stderr);
            using var receipt = JsonDocument.Parse(await stdout);
            var document = Assert.Single(receipt.RootElement.GetProperty("documents").EnumerateArray());
            Assert.Equal("reviewed", document.GetProperty("status").GetString());
            Assert.Equal(Convert.ToHexString(SHA256.HashData(File.ReadAllBytes(source))),
                document.GetProperty("sha256").GetString());
            var lines = document.GetProperty("findings").EnumerateArray()
                .Select(finding => finding.GetProperty("line").GetInt32()).ToArray();
            Assert.Equal(new[] { 7, 25 }, lines);
        }
        finally
        {
            workspace.Delete(recursive: true);
        }
    }

    [Fact]
    public async Task UnresolvedAndDynamicEdgesArePartialRatherThanBoundCalls()
    {
        var result = await Execute(RequestFrame(Request(Source("A",
            "class C { void Run(dynamic d) { Missing(); d.Invoke(1); } }"))));
        Assert.Equal(0, result.ExitCode);
        Assert.Equal("partial", Terminal(result).GetProperty("status").GetString());
        var bindings = result.Facts.Where(item => item.GetProperty("capability").GetString() == "calls")
            .Select(item => item.GetProperty("value").GetProperty("binding").GetString()).ToArray();
        Assert.Contains("unresolved", bindings);
        Assert.Contains("dynamic", bindings);
        Assert.Equal("partial", Terminal(result).GetProperty("capabilities").GetProperty("calls").GetString());
    }

    [Fact]
    public async Task DefinesChangeTheActiveDeclarationWithinTheSameSourceBytes()
    {
        const string code = "class Choice {\n#if FEATURE\n void Enabled(){}\n#else\n void Disabled(){}\n#endif\n}";
        var enabled = await Execute(RequestFrame(Request(Source("A", code)) with
        {
            Context = Context() with { Defines = ["FEATURE"] }
        }));
        var disabled = await Execute(RequestFrame(Request(Source("A", code))));
        var namesEnabled = Names(enabled);
        var namesDisabled = Names(disabled);
        Assert.Contains("Enabled", namesEnabled);
        Assert.DoesNotContain("Disabled", namesEnabled);
        Assert.Contains("Disabled", namesDisabled);
        Assert.DoesNotContain("Enabled", namesDisabled);
        Assert.Equal(Terminal(enabled).GetProperty("scope").GetProperty("sourceSha256").GetString(),
            Terminal(disabled).GetProperty("scope").GetProperty("sourceSha256").GetString());
    }

    [Fact]
    public async Task AuthorizedLinkedAndSuppliedGeneratedProvenanceArePreserved()
    {
        var outside = Path.GetFullPath(Path.Combine(Path.GetTempPath(), "slophawk-linked-fixture"));
        var source = Source("A", "class Linked { }", provenance: "linked", physicalRoot: outside);
        var linked = await Execute(RequestFrame(Request(source) with { LinkedRoots = [outside] }));
        Assert.Equal("complete", Terminal(linked).GetProperty("status").GetString());
        Assert.Equal("linked", Terminal(linked).GetProperty("input").GetProperty("documents")[0]
            .GetProperty("provenance").GetString());

        var generated = await Execute(RequestFrame(Request(Source("A", "class Generated { }",
            provenance: "generated", physicalRoot: Root))));
        Assert.Equal("complete", Terminal(generated).GetProperty("status").GetString());
        Assert.Equal("generated", Terminal(generated).GetProperty("input")
            .GetProperty("documents")[0].GetProperty("provenance").GetString());

        var rejected = await Execute(RequestFrame(Request(source)));
        Assert.Equal(1, rejected.ExitCode);
        Assert.Equal("physical_scope", Terminal(rejected).GetProperty("errors")[0].GetString());
    }

    [Fact]
    public async Task Utf8BomCrLfAndUnicodeHaveExactOriginalByteOffsets()
    {
        var bytes = Encoding.UTF8.GetPreamble().Concat(Encoding.UTF8.GetBytes(
            "class Café { string Smile = \"🙂\";\r\n void Go(){} }")).ToArray();
        var result = await Execute(RequestFrame(Request(Source("A", bytes))));
        Assert.Equal("complete", Terminal(result).GetProperty("status").GetString());
        var declaration = result.Facts.Single(item =>
            item.GetProperty("capability").GetString() == "declarations" &&
            item.GetProperty("value").GetProperty("name").GetString() == "Café");
        var span = declaration.GetProperty("span");
        var byteSpan = span.GetProperty("byteSpan");
        Assert.True(byteSpan.GetProperty("known").GetBoolean());
        Assert.Equal(9, byteSpan.GetProperty("start").GetInt32()); // three BOM bytes
        Assert.Equal(5, byteSpan.GetProperty("length").GetInt32()); // UTF-8 é uses two bytes
        Assert.Equal(6, span.GetProperty("utf16").GetProperty("start").GetInt32());
        var go = result.Facts.Single(item =>
            item.GetProperty("capability").GetString() == "declarations" &&
            item.GetProperty("value").GetProperty("name").GetString() == "Go");
        Assert.Equal(1, go.GetProperty("span").GetProperty("utf16").GetProperty("startLine").GetInt32());
        var expectedStart = Encoding.UTF8.GetString(bytes).IndexOf("Go", StringComparison.Ordinal);
        Assert.True(expectedStart > 0);
        Assert.Equal(Encoding.UTF8.GetByteCount(Encoding.UTF8.GetString(bytes)[..expectedStart]),
            go.GetProperty("span").GetProperty("byteSpan").GetProperty("start").GetInt32());
    }

    [Fact]
    public async Task UnknownByteMappingMakesAnExplicitGap()
    {
        var result = await Execute(RequestFrame(Request(Source("A", "class A {}") with { ByteMapping = "unavailable" })));
        Assert.Equal("partial", Terminal(result).GetProperty("status").GetString());
        Assert.Contains(result.Facts, item => item.GetProperty("span").GetProperty("byteSpan")
            .GetProperty("gap").GetString() == "caller_declared_unmapped");
        Assert.Equal("partial", Terminal(result).GetProperty("capabilities")
            .GetProperty("byteSpans").GetString());
    }

    [Fact]
    public async Task OversizedAndTruncatedFrameAndUnknownVersionFailPromptly()
    {
        var oversized = new byte[4];
        BinaryPrimitives.WriteInt32LittleEndian(oversized, 1024 * 1024 + 1);
        var big = await Execute(oversized);
        Assert.Equal(1, big.ExitCode);
        Assert.Equal("frame_length", Terminal(big).GetProperty("errors")[0].GetString());
        var truncated = new byte[7];
        BinaryPrimitives.WriteInt32LittleEndian(truncated, 23);
        var shortFrame = await Execute(truncated);
        Assert.Equal(1, shortFrame.ExitCode);
        Assert.Equal("truncated_frame", Terminal(shortFrame).GetProperty("errors")[0].GetString());
        var version = await Execute(RequestFrame(Request(Source("A", "class A {}")) with { Version = 2 }));
        Assert.Equal(1, version.ExitCode);
        Assert.Equal("protocol_version", Terminal(version).GetProperty("errors")[0].GetString());
        var eof = await Execute([]);
        Assert.Equal(1, eof.ExitCode);
        Assert.Equal("truncated_frame", Terminal(eof).GetProperty("errors")[0].GetString());
        var nullSource = await Execute(RequestFrame(Request() with { Sources = [null] }));
        Assert.Equal(1, nullSource.ExitCode);
        Assert.Empty(nullSource.Facts);
        Assert.Equal("source_required", Terminal(nullSource).GetProperty("errors")[0].GetString());
    }

    [Fact]
    public async Task SourceDigestMismatchFailsWithoutPublishingFacts()
    {
        var bad = Source("A", "class A {}");
        var result = await Execute(RequestFrame(Request(bad with { Sha256 = new string('A', 64) })));
        Assert.Equal(1, result.ExitCode);
        Assert.Empty(result.Facts);
        Assert.Equal("source_identity_mismatch", Terminal(result).GetProperty("errors")[0].GetString());
    }

    [Fact]
    public async Task RecordAndReferenceClosureLimitsNeverClaimCompleteness()
    {
        var limited = await Execute(RequestFrame(Request(Source("A",
            "class A { int a; int b; void Run() { a=b; } }")) with
        { Limits = new FactBudget { MaxRecords = 3, MaxFrontiers = 8, MaxSyntaxNodes = 100, MaxDiagnostics = 4 } }));
        Assert.Equal(0, limited.ExitCode);
        Assert.True(limited.Facts.Count <= 3);
        Assert.Equal("partial", Terminal(limited).GetProperty("status").GetString());
        Assert.Contains(Terminal(limited).GetProperty("gaps").EnumerateArray(),
            item => item.GetString() == "output_record_or_byte_limit");
        var incomplete = await Execute(RequestFrame(Request(Source("A", "class A {}")) with
        { Context = Context() with { ReferenceClosureComplete = false } }));
        Assert.Equal("partial", Terminal(incomplete).GetProperty("status").GetString());
        Assert.Contains(Terminal(incomplete).GetProperty("gaps").EnumerateArray(),
            item => item.GetString() == "extractor:MissingCompilationReferences");
    }

    private static string[] Names(RunResult result) => result.Facts
        .Where(item => item.GetProperty("capability").GetString() == "declarations")
        .Select(item => item.GetProperty("value").GetProperty("name").GetString()!).ToArray();

    private static JsonElement Terminal(RunResult result)
    {
        Assert.NotEmpty(result.Frames);
        Assert.Equal("result", result.Frames[^1].GetProperty("kind").GetString());
        return result.Frames[^1];
    }

    private static FactSource Source(string id, string text, string provenance = "normal", string? physicalRoot = null) =>
        Source(id, Encoding.UTF8.GetBytes(text), provenance, physicalRoot);

    private static FactSource Source(string id, byte[] bytes, string provenance = "normal", string? physicalRoot = null) => new()
    {
        DocumentId = id,
        LogicalPath = "src/" + id + ".cs",
        PhysicalPath = Path.Combine(physicalRoot ?? Root, "src", id + ".cs"),
        Provenance = provenance,
        BytesBase64 = Convert.ToBase64String(bytes),
        Sha256 = Convert.ToHexString(SHA256.HashData(bytes)),
        ByteMapping = "exactUtf8"
    };

    private static FactContext Context() => new()
    {
        Key = "fixture|net10.0|Debug",
        TargetFramework = "net10.0",
        AssemblyName = "SyntheticFixture",
        LanguageVersion = "CSharp14",
        ReferencePack = ReferencePack(),
        Defines = [],
        ReferenceClosureComplete = true,
        SourceClosureComplete = true
    };

    private static FactRequest Request(params FactSource[] sources) => new()
    {
        Version = 1,
        Kind = "analyze",
        WorkspaceEpoch = "epoch-42",
        CommittedRevision = "revision-abcd",
        WorkspaceRoot = Root,
        LinkedRoots = [],
        DocumentId = "A",
        Context = Context(),
        Sources = sources,
        Limits = new FactBudget { MaxRecords = 600, MaxFrontiers = 24,
            MaxSyntaxNodes = 10000, MaxDiagnostics = 8 }
    };

    private static byte[] RequestFrame(FactRequest request)
    {
        var json = JsonSerializer.SerializeToUtf8Bytes(request, JsonOptions);
        var result = new byte[json.Length + 4];
        BinaryPrimitives.WriteInt32LittleEndian(result, json.Length);
        json.CopyTo(result.AsSpan(4));
        return result;
    }

    private sealed record RunResult(int ExitCode, bool Exited, IReadOnlyList<JsonElement> Frames)
    {
        public IReadOnlyList<JsonElement> Facts => Frames.Where(frame =>
            frame.GetProperty("kind").GetString() == "fact").ToArray();
    }

    private static async Task<RunResult> Execute(byte[] request)
    {
        using var process = new Process();
        process.StartInfo.FileName = DotnetExecutable();
        process.StartInfo.ArgumentList.Add(WorkerBinary());
        process.StartInfo.UseShellExecute = false;
        process.StartInfo.RedirectStandardInput = true;
        process.StartInfo.RedirectStandardOutput = true;
        process.StartInfo.RedirectStandardError = true;
        Assert.True(process.Start());
        var stdout = ReadAll(process.StandardOutput.BaseStream);
        var stderr = process.StandardError.ReadToEndAsync();
        using var timeout = new CancellationTokenSource(TimeSpan.FromSeconds(20));
        try
        {
            await process.StandardInput.BaseStream.WriteAsync(request, timeout.Token);
            process.StandardInput.Close();
            await process.WaitForExitAsync(timeout.Token);
        }
        catch (OperationCanceledException)
        {
            if (!process.HasExited) process.Kill(entireProcessTree: true);
            throw;
        }
        var bytes = await stdout;
        _ = await stderr;
        var frames = new List<JsonElement>();
        for (var offset = 0; offset < bytes.Length;)
        {
            Assert.True(offset + 4 <= bytes.Length, "Truncated output frame prefix.");
            var length = BinaryPrimitives.ReadInt32LittleEndian(bytes.AsSpan(offset, 4));
            Assert.InRange(length, 1, 1024 * 1024);
            offset += 4;
            Assert.True(offset + length <= bytes.Length, "Truncated output frame body.");
            using var json = JsonDocument.Parse(bytes.AsMemory(offset, length));
            frames.Add(json.RootElement.Clone());
            offset += length;
        }
        Assert.Equal(1, frames.Count(frame => frame.GetProperty("kind").GetString() == "result"));
        return new RunResult(process.ExitCode, process.HasExited, frames);
    }

    private static async Task<byte[]> ReadAll(Stream stream)
    {
        using var buffer = new MemoryStream();
        await stream.CopyToAsync(buffer);
        return buffer.ToArray();
    }

    private static string DotnetRoot()
    {
        var environment = Environment.GetEnvironmentVariable("DOTNET_ROOT");
        if (!string.IsNullOrWhiteSpace(environment) && Directory.Exists(environment)) return environment;
        var runtime = Path.GetDirectoryName(typeof(object).Assembly.Location)!;
        return Path.GetFullPath(Path.Combine(runtime, "..", "..", ".."));
    }

    private static string DotnetExecutable()
    {
        var binary = Path.Combine(DotnetRoot(), OperatingSystem.IsWindows() ? "dotnet.exe" : "dotnet");
        return File.Exists(binary) ? binary : "dotnet";
    }

    private static string ReferencePack()
    {
        var packs = Path.Combine(DotnetRoot(), "packs", "Microsoft.NETCore.App.Ref");
        var versions = Directory.GetDirectories(packs).OrderByDescending(path => Path.GetFileName(path), StringComparer.Ordinal);
        return versions.Select(path => Path.Combine(path, "ref", "net10.0"))
            .First(Directory.Exists);
    }

    private static string WorkerBinary()
    {
        var binary = typeof(Providers.CSharp.Worker.Program).Assembly.Location;
        Assert.True(File.Exists(Path.ChangeExtension(binary, ".runtimeconfig.json")),
            "The referenced worker runtime configuration must be copied alongside the integration tests.");
        return binary;
    }
}
