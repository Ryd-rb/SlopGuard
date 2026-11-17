using System.Buffers.Binary;
using System.Diagnostics;
using System.Security.Cryptography;
using System.Text;
using System.Text.Json;
using SlopHawk.Core;
using SlopHawk.Providers.CSharp;

namespace SlopHawk.Providers.CSharp.ScalarWorker;

// Private opt-in worker. Stdio is exclusively slophawk.migration-job/1, not the shipped Roslyn frame-v1.
// Only the host can supply the independent environment authority; repository content is never a grant.
internal sealed record JobIdentity(string Protocol, string Operation, string Job, string Root, string Context,
    string Epoch, string Revision, string InputDigest, string ToolDigest, string PolicyDigest,
    string BudgetRef, string GrantRef);
internal sealed record JobRequest(JobIdentity Identity, JsonElement Payload);
internal sealed record JobTerminal(JobIdentity Identity, string Status, string? Reason, string[] Omissions, object? Value);
internal sealed record TargetSpec(string Operation, string Document, string Signature);
internal sealed record ScalarSpec(ScalarBuildOptions Build, TargetSpec[] Targets, string Rules, string Published,
    int PerOperation = 256, int WorkLimit = 128, int RetestLimit = 16, int Seed = 1,
    string? RunVerb = null, string? PrepareVerb = null);
internal sealed record BuildReceipt(string CompilerSha256, string RuntimeVersion, string CandidateSha256,
    ScalarCompilerInputs CompilerInputs);
internal sealed record BuildRequest(string SpecPath, string OutputDir);
internal sealed record InspectRequest(string SpecPath, string? CandidatePath, bool GraphOn);
internal sealed record EvaluationRequest(string Operation, long[] Args);
internal sealed record EvaluateRequest(string SpecPath, EvaluationRequest[] Requests);
internal sealed record ReplayRequest(string SpecPath, string CandidatePath, string WorkDir, ProbeInput[] Inputs);
internal sealed record InspectedTarget(string Operation, string Signature, bool Bound, bool Eligible, string? Reason,
    string? RuleSource, string[] Parameters, string Facts, object? Graph, ProbeExpectation[] Original, string[] OrdinaryKeys);
internal sealed record InspectResult(string CompilerSha256, string CandidateSha256, string RuntimeVersion,
    ScalarCompilerInputs CompilerInputs, InspectedTarget[] Targets);
internal sealed record ReplayResult(JsonElement[] Observed, bool OrderingStable, int Replays,
    string[] Unknown, string CandidateSha256);

internal static class Program
{
    private const int MaxFrame = 16 * 1024 * 1024;
    private const int MaxOutput = 8 * 1024 * 1024;
    private static readonly JsonSerializerOptions Json = new() { PropertyNameCaseInsensitive = true };

    public static async Task<int> Main()
    {
        JobRequest? request = null;
        try
        {
            request = ReadFrame<JobRequest>();
            var (root, spec, specPath) = Validate(request);
            object result = request.Identity.Operation switch
            {
                "build" => Build(root, spec, Read<BuildRequest>(request.Payload), specPath),
                "inspect" => Inspect(root, spec, Read<InspectRequest>(request.Payload), specPath),
                "evaluate" => Evaluate(root, spec, Read<EvaluateRequest>(request.Payload), specPath),
                "replay" => await Replay(root, spec, Read<ReplayRequest>(request.Payload), specPath),
                _ => throw new ArgumentException("Unsupported migration operation.")
            };
            WriteFrame(new JobTerminal(request.Identity, "complete", null, [], result));
            return 0;
        }
        catch (OperationCanceledException)
        {
            if (request is not null)
                WriteFrame(new JobTerminal(request.Identity, "cancelled", "Operation cancelled.", [], null));
            return 2;
        }
        catch (Exception error) when (error is ArgumentException or IOException or JsonException or
                                       InvalidOperationException or UnauthorizedAccessException or
                                       System.ComponentModel.Win32Exception or CryptographicException)
        {
            var status = error switch
            {
                UnauthorizedAccessException => "deferred",
                IOException io when io.Message.Contains("changed", StringComparison.OrdinalIgnoreCase) ||
                    io.Message.Contains("digest", StringComparison.OrdinalIgnoreCase) ||
                    io.Message.Contains("original", StringComparison.OrdinalIgnoreCase) => "stale",
                ArgumentException => "unsupported",
                _ => "failed"
            };
            if (request is not null)
                WriteFrame(new JobTerminal(request.Identity, status, Limit(error.Message), ["worker operation"], null));
            return 2;
        }
    }

    private static (string Root, ScalarSpec Spec, string SpecPath) Validate(JobRequest request)
    {
        var identity = request.Identity ?? throw new ArgumentException("Missing job identity.");
        if (identity.Protocol != "slophawk.migration-job/1" ||
            identity.Operation is not ("build" or "inspect" or "evaluate" or "replay") ||
            string.IsNullOrWhiteSpace(identity.Job) || identity.Job.Length > 128 ||
            string.IsNullOrWhiteSpace(identity.Epoch) || string.IsNullOrWhiteSpace(identity.Revision) ||
            string.IsNullOrWhiteSpace(identity.BudgetRef) || string.IsNullOrWhiteSpace(identity.GrantRef) ||
            identity.GrantRef != Environment.GetEnvironmentVariable("SLOPHAWK_PATHS_GRANT") ||
            identity.Epoch != Environment.GetEnvironmentVariable("SLOPHAWK_PATHS_EPOCH") ||
            identity.Revision != Environment.GetEnvironmentVariable("SLOPHAWK_PATHS_REVISION") ||
            identity.BudgetRef != Environment.GetEnvironmentVariable("SLOPHAWK_PATHS_BUDGET"))
            throw new UnauthorizedAccessException("Missing or changed host-owned execution authority.");
        var root = Path.GetFullPath(identity.Root);
        if (!Path.IsPathFullyQualified(identity.Root) || root != identity.Context ||
            root != identity.Root || !Directory.Exists(root) ||
            (File.GetAttributes(root) & FileAttributes.ReparsePoint) != 0)
            throw new ArgumentException("Job root/context identity is not an ordinary absolute directory.");
        // The spec path is part of the request payload; no path inference from repository text.
        var providedSpec = request.Payload.GetProperty("specPath").GetString()
            ?? throw new ArgumentException("Missing spec path.");
        var specPath = RequireFile(root, providedSpec, allowAlternateName: true);
        if (!Path.GetDirectoryName(specPath)!.Equals(root, StringComparison.Ordinal))
            throw new ArgumentException("The spec must live at the declared source root.");
        var bytes = ReadBoundedFile(specPath, MaxFrame);
        if (Hex(bytes) != identity.InputDigest)
            throw new IOException("The pinned spec input digest changed.");
        var spec = JsonSerializer.Deserialize<ScalarSpec>(bytes, Json) ??
            throw new JsonException("Missing scalar spec.");
        if (spec.Build is null || spec.Targets is not { Length: > 0 and <= 32 } ||
            spec.Targets.Any(t => t is null || string.IsNullOrWhiteSpace(t.Operation) ||
                string.IsNullOrWhiteSpace(t.Signature) || string.IsNullOrWhiteSpace(t.Document)) ||
            spec.Targets.Select(t => t.Operation).Distinct(StringComparer.Ordinal).Count() != spec.Targets.Length ||
            spec.PerOperation is < 1 or > 512 || spec.WorkLimit is < 1 or > 8192 ||
            spec.RetestLimit is < 1 or > 32)
            throw new ArgumentException("Invalid scalar target or planning budget.");
        var policy = PolicyDigest(root, spec);
        if (policy != identity.PolicyDigest)
            throw new IOException("The public policy inputs changed.");
        var assembly = typeof(Program).Assembly.Location;
        if (Hex(ReadBoundedFile(assembly, MaxFrame)) != identity.ToolDigest)
            throw new IOException("The worker executable digest changed.");
        if (!root.Equals(Path.GetFullPath(Path.GetDirectoryName(specPath)!), StringComparison.Ordinal))
            throw new ArgumentException("The source and spec roots disagree.");
        return (root, spec, specPath);
    }

    private static object Build(string root, ScalarSpec spec, BuildRequest request, string specPath)
    {
        RequireExactSpec(specPath, request.SpecPath);
        var output = RequireOutput(root, request.OutputDir);
        var compiler = ScalarCompilation.Create(root, spec.Build);
        var binary = compiler.Emit();
        var candidate = Path.Combine(output, "candidate.dll");
        using (var stream = new FileStream(candidate, FileMode.CreateNew, FileAccess.Write, FileShare.None))
            stream.Write(binary);
        File.WriteAllText(Path.Combine(output, "candidate.runtimeconfig.json"), JsonSerializer.Serialize(new
        {
            runtimeOptions = new
            {
                tfm = "net10.0", rollForward = "Disable",
                framework = new { name = "Microsoft.NETCore.App", version = Environment.Version.ToString() }
            }
        }));
        var receipt = Receipt(compiler, binary);
        File.WriteAllText(Path.Combine(output, "build.json"), JsonSerializer.Serialize(receipt));
        return receipt;
    }

    private static InspectResult Inspect(string root, ScalarSpec spec, InspectRequest request, string specPath)
    {
        RequireExactSpec(specPath, request.SpecPath);
        var compiler = ScalarCompilation.Create(root, spec.Build);
        var binary = request.CandidatePath is null ? [] : compiler.Emit();
        if (request.CandidatePath is not null &&
            !Hex(ReadBoundedFile(request.CandidatePath, MaxFrame)).Equals(Hex(binary), StringComparison.Ordinal))
            throw new IOException("Inspection requires the exact original emitted candidate.");
        var catalog = Catalog(root, spec);
        var arities = catalog.ParameterCounts.Where(pair => spec.Targets.Any(t => t.Operation == pair.Key))
            .ToDictionary();
        using var stream = new MemoryStream(binary, writable: false);
        var literals = request.CandidatePath is null ? new Dictionary<string, long[]>() :
            ManagedBodyComparison.IntegerLiterals(stream, arities);
        var targets = new List<InspectedTarget>();
        foreach (var target in spec.Targets)
        {
            var seeds = Published(root, spec).Where(item => item.Operation == target.Operation).ToArray();
            var facts = compiler.ProjectFacts(target.Document, target.Signature);
            var parameters = facts.Parameters.Select(p => p.Type).ToArray();
            var compatible = facts.Bound && facts.Eligible &&
                arities.TryGetValue(target.Operation, out var arity) && arity == parameters.Length;
            var originals = compatible && request.CandidatePath is not null
                ? OriginalCases(catalog, spec, target.Operation, seeds, literals, parameters) : [];
            targets.Add(new(target.Operation, target.Signature, facts.Bound, compatible,
                facts.Reason ?? (!compatible ? "Unqualified public rule or unsupported signature." : null),
                catalog.Qualification.FirstOrDefault(q => q.Operation == target.Operation)?.Source,
                parameters, facts.ToWire(seeds.Select(s => s.Args).ToArray(), spec.WorkLimit,
                    request.GraphOn && compatible && request.CandidatePath is not null ? spec.PerOperation : 0), facts.Graph,
                originals, seeds.Select(s => Key(s.Args)).ToArray()));
        }
        var receipt = Receipt(compiler, binary);
        return new(receipt.CompilerSha256, receipt.CandidateSha256, receipt.RuntimeVersion,
            receipt.CompilerInputs, [.. targets]);
    }

    private static object Evaluate(string root, ScalarSpec spec, EvaluateRequest request, string specPath)
    {
        RequireExactSpec(specPath, request.SpecPath);
        if (request.Requests is not { Length: <= 16384 } ||
            request.Requests.Any(r => r is null || r.Args is null || r.Args.Length > 32))
            throw new ArgumentException("Invalid or oversized evaluation batch.");
        var catalog = Catalog(root, spec);
        return request.Requests.Select((r, i) => new { Index = i,
            Expectation = catalog.Evaluate(r.Operation, r.Args) }).ToArray();
    }

    private static async Task<ReplayResult> Replay(string root, ScalarSpec spec, ReplayRequest request, string specPath)
    {
        RequireExactSpec(specPath, request.SpecPath);
        if (request.Inputs is not { Length: > 0 and <= 16384 } ||
            request.Inputs.Any(i => i is null || string.IsNullOrWhiteSpace(i.Operation) || i.Args is null))
            throw new ArgumentException("The replay input must be a bounded nonempty ordered batch.");
        var workspace = RequireOutput(root, request.WorkDir);
        var compiler = ScalarCompilation.Create(root, spec.Build);
        var original = compiler.Emit();
        var originalHash = Hex(original);
        if (Hex(ReadBoundedFile(request.CandidatePath, MaxFrame)) != originalHash)
            throw new IOException("The replay binary is not the original controlled output.");
        using var deadline = new CancellationTokenSource(TimeSpan.FromSeconds(90));
        var observed = await RunAdapter(request.CandidatePath, spec, request.Inputs, workspace, deadline.Token);
        var reverse = await RunAdapter(request.CandidatePath, spec, request.Inputs.Reverse().ToArray(), workspace, deadline.Token);
        var stable = observed.Length == reverse.Length &&
            observed.Zip(reverse.Reverse()).All(pair => JsonElement.DeepEquals(pair.First, pair.Second));
        if (Hex(ReadBoundedFile(request.CandidatePath, MaxFrame)) != originalHash)
            throw new IOException("The replay candidate changed while executing.");
        return new(observed, stable, 2, stable
            ? ["No invocation-scoped branch trace was supplied; graph edges remain predictions."]
            : ["Adapter results changed when input order was reversed; reply order is untrustworthy.",
               "No invocation-scoped branch trace was supplied."], originalHash);
    }

    private static async Task<JsonElement[]> RunAdapter(string candidate, ScalarSpec spec, ProbeInput[] inputs,
        string workspace, CancellationToken cancellationToken)
    {
        var attempt = Path.Combine(workspace, Guid.NewGuid().ToString("N"));
        Directory.CreateDirectory(attempt);
        var requests = Path.Combine(attempt, "requests.json");
        File.WriteAllText(requests, JsonSerializer.Serialize(inputs));
        if (spec.PrepareVerb is not null)
        {
            var prepared = await Invoke(candidate, spec.PrepareVerb, requests, attempt, cancellationToken);
            using var document = JsonDocument.Parse(prepared);
            requests = Path.Combine(attempt, "prepared.json");
            File.WriteAllText(requests, document.RootElement.GetRawText());
        }
        var output = await Invoke(candidate, spec.RunVerb, requests, attempt, cancellationToken);
        var values = JsonSerializer.Deserialize<JsonElement[]>(output) ??
            throw new JsonException("The numeric adapter must return an ordered JSON array.");
        if (values.Length != inputs.Length)
            throw new IOException("Adapter response count does not match ordered request count.");
        return values;
    }

    private static async Task<string> Invoke(string candidate, string? verb, string input,
        string attempt, CancellationToken cancellationToken)
    {
        var start = new ProcessStartInfo("dotnet")
        {
            WorkingDirectory = attempt, UseShellExecute = false, CreateNoWindow = true,
            RedirectStandardOutput = true, RedirectStandardError = true
        };
        start.ArgumentList.Add("exec");
        start.ArgumentList.Add(candidate);
        if (verb is not null)
            start.ArgumentList.Add(verb);
        start.ArgumentList.Add(input);
        using var child = Process.Start(start) ?? throw new IOException("The original candidate did not start.");
        try
        {
            using var timeout = CancellationTokenSource.CreateLinkedTokenSource(cancellationToken);
            timeout.CancelAfter(TimeSpan.FromSeconds(30));
            var output = ReadBounded(child.StandardOutput, MaxOutput, timeout.Token);
            var error = ReadBounded(child.StandardError, 64 * 1024, timeout.Token);
            await Task.WhenAll(output, error, child.WaitForExitAsync(timeout.Token));
            if (child.ExitCode != 0)
                throw new IOException($"Original candidate exited {child.ExitCode}: {Limit(await error)}");
            return await output;
        }
        finally
        {
            if (!child.HasExited)
                child.Kill(entireProcessTree: true);
        }
    }

    private static async Task<string> ReadBounded(StreamReader reader, int limit, CancellationToken token)
    {
        var buffer = new char[4096];
        var text = new StringBuilder();
        int read;
        while ((read = await reader.ReadAsync(buffer, token)) != 0)
        {
            if (read > limit - text.Length)
                throw new IOException("Original-build replay exceeded its output limit.");
            text.Append(buffer, 0, read);
        }
        return text.ToString();
    }

    private static ProbeExpectation[] OriginalCases(ContractCases catalog, ScalarSpec spec, string operation,
        ProbeExpectation[] seeds, IReadOnlyDictionary<string, long[]> literals, IReadOnlyList<string> types)
    {
        var limit = spec.PerOperation;
        var selected = new HashSet<string>(StringComparer.Ordinal) { operation };
        var warmup = Math.Min(64, limit / 2);
        IEnumerable<ProbeExpectation> ordinary = warmup == 0
            ? Enumerable.Empty<ProbeExpectation>() : catalog.Generate(warmup, 0, selected);
        var guided = catalog.Generate(limit, spec.Seed, selected, guideWithBoundaries: true,
            implementationLiterals: literals);
        return seeds.Concat(ordinary).Concat(guided)
            .Where(item => Compatible(item.Args, types)).DistinctBy(item => Key(item.Args)).Take(limit).ToArray();
    }

    private static bool Compatible(long[] args, IReadOnlyList<string> types) => args.Length == types.Count &&
        args.Select((value, index) => value >= int.MinValue && value <= int.MaxValue &&
            (types[index] != "bool" || value is 0 or 1)).All(value => value);

    private static ContractCases Catalog(string root, ScalarSpec spec) =>
        new(JsonSerializer.Deserialize<IntegerContract[]>(ReadBoundedFile(RequireRelative(root, spec.Rules), MaxFrame), Json) ?? [],
            Published(root, spec));

    private static ProbeExpectation[] Published(string root, ScalarSpec spec) =>
        JsonSerializer.Deserialize<ProbeExpectation[]>(ReadBoundedFile(RequireRelative(root, spec.Published), MaxFrame), Json)
            ?? throw new JsonException("Missing published examples.");

    private static BuildReceipt Receipt(ScalarCompilation compiler, byte[] binary) =>
        new(Hex(JsonSerializer.SerializeToUtf8Bytes(compiler.Inputs)), Environment.Version.ToString(),
            Hex(binary), compiler.Inputs);

    private static string PolicyDigest(string root, ScalarSpec spec)
    {
        var rules = Hex(ReadBoundedFile(RequireRelative(root, spec.Rules), MaxFrame));
        var published = Hex(ReadBoundedFile(RequireRelative(root, spec.Published), MaxFrame));
        return Hex(Encoding.UTF8.GetBytes(rules + ":" + published));
    }

    private static string RequireRelative(string root, string relative)
    {
        if (string.IsNullOrWhiteSpace(relative) || Path.IsPathRooted(relative))
            throw new ArgumentException("Spec dependencies must be relative to the source root.");
        return RequireFile(root, Path.GetFullPath(relative, root), allowAlternateName: true);
    }

    private static string RequireFile(string root, string path, bool allowAlternateName)
    {
        var canonical = Path.GetFullPath(path);
        var relative = Path.GetRelativePath(root, canonical);
        if (Path.IsPathRooted(relative) || relative.Split(Path.DirectorySeparatorChar)[0] == ".." ||
            (File.GetAttributes(canonical) & (FileAttributes.Directory | FileAttributes.ReparsePoint)) != 0)
            throw new IOException("Use a regular input file inside the declared source root.");
        if (!allowAlternateName && Path.GetFileName(canonical) != "spec.json")
            throw new IOException("Unexpected spec file.");
        return canonical;
    }

    private static string RequireOutput(string root, string path)
    {
        var canonical = Path.GetFullPath(path);
        if (!Path.IsPathFullyQualified(path) || IsInside(root, canonical))
            throw new ArgumentException("Worker output must be outside the source directory.");
        if (!Directory.Exists(canonical) || (File.GetAttributes(canonical) & FileAttributes.ReparsePoint) != 0)
            throw new IOException("The output must be a precreated ordinary host-owned directory.");
        return canonical;
    }

    private static bool IsInside(string root, string path)
    {
        var relative = Path.GetRelativePath(root, path);
        return relative == "." || (!Path.IsPathRooted(relative) &&
            relative.Split(Path.DirectorySeparatorChar)[0] != "..");
    }

    private static void RequireExactSpec(string expected, string supplied)
    {
        if (Path.GetFullPath(supplied) != expected)
            throw new IOException("The job refers to a different source spec.");
    }

    private static T Read<T>(JsonElement payload) => payload.Deserialize<T>(Json) ??
        throw new JsonException("Missing migration request payload.");

    private static byte[] ReadBoundedFile(string path, int limit)
    {
        using var stream = File.OpenRead(path);
        if (stream.Length > limit)
            throw new IOException("A migration input exceeds the byte limit.");
        var bytes = new byte[(int)stream.Length];
        stream.ReadExactly(bytes);
        if (stream.ReadByte() != -1)
            throw new IOException("A migration input changed during its read.");
        return bytes;
    }

    private static string Hex(byte[] bytes) => Convert.ToHexString(SHA256.HashData(bytes));
    private static string Key(long[] args) => string.Join(',', args);
    private static string Limit(string text) => text.Length > 512 ? text[..512] : text;

    private static T ReadFrame<T>()
    {
        using var input = Console.OpenStandardInput();
        Span<byte> length = stackalloc byte[4];
        input.ReadExactly(length);
        var count = BinaryPrimitives.ReadInt32LittleEndian(length);
        if (count is < 1 or > MaxFrame)
            throw new IOException("Invalid bounded migration frame length.");
        var bytes = new byte[count];
        input.ReadExactly(bytes);
        if (input.ReadByte() != -1)
            throw new IOException("Unexpected trailing migration frame data.");
        return JsonSerializer.Deserialize<T>(bytes, Json) ?? throw new JsonException("Missing frame object.");
    }

    private static void WriteFrame<T>(T value)
    {
        var bytes = JsonSerializer.SerializeToUtf8Bytes(value, Json);
        if (bytes.Length > MaxFrame)
            throw new IOException("Migration terminal frame exceeds the byte limit.");
        using var output = Console.OpenStandardOutput();
        Span<byte> length = stackalloc byte[4];
        BinaryPrimitives.WriteInt32LittleEndian(length, bytes.Length);
        output.Write(length);
        output.Write(bytes);
        output.Flush();
    }
}
