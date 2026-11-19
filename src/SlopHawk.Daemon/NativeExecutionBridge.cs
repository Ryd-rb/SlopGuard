using System.Security.Cryptography;
using System.Text;
using System.Text.Json;
using System.Text.Json.Serialization;

namespace SlopHawk.Daemon;

// This opt-in boundary is deliberately not wired to the scheduler, IPC or MCP. A caller
// must possess the daemon's current, independently checked publication fence. The JSON
// identity is a comparison token, never a grant to execute or publish anything.
public sealed record NativeExecutionIdentity(
    string JobId, string Root, string RootIdentity, string Context, Guid Epoch,
    long CommittedRevision, long FenceSequence, string InputSha256, string ToolSha256,
    string PolicySha256, string GrantRevision, string BudgetReference,
    long AttemptId = 0, int AttemptNumber = 0, string? ExecutableSha256 = null,
    string? RunnerSha256 = null);

public sealed record NativeExecutionClosure(
    string MappingIdentity, string RunnerIdentity, string AdapterIdentity,
    string[] ReferenceAssemblies, string[] RuntimeAssets, string[] SettingsFiles,
    string[] AdapterFiles, string[] RequiredOutputs, string[] Companions,
    string[] ObservedRuntimeInputs);

public sealed record NativeExecutionWork(
    string Root, string DotnetPath, string? SelectedTargetId = null, string? ResultsRoot = null,
    NativeExecutionClosure? Closure = null, string? SelectedInputFingerprint = null,
    string[]? CauseIds = null, string? GitBranch = null, string? GitCommit = null,
    string? RunnerPath = null, int TimeoutSeconds = 300)
{
    // Data-only opt-in; the old request JSON and fact shape remain unchanged.
    // This does not request another evaluation or establish AdditionalFiles attribution.
    [JsonIgnore(Condition = JsonIgnoreCondition.WhenWritingDefault)]
    public bool IncludeAdditionalFileEvidence { get; init; }
}

public sealed record NativeExecutionFact(string Kind, JsonElement Data);
public sealed record NativeExecutionResult(string Status, string Reason,
    IReadOnlyList<NativeExecutionFact> Facts, IReadOnlyList<string> Omissions)
{
    public static NativeExecutionResult Refuse(string status, string reason, params string[] omitted) =>
        new(status, reason, [], omitted);
}

/// <summary>
/// One evaluated job in one authenticated profile reservation. This facade remains in
/// SlopHawk.Daemon so that ProjectEvaluator's private last-evaluation snapshot, the
/// planner, collector and VSTest runner retain their original access and ownership.
/// The standalone execution worker only frames calls to this facade.
/// </summary>
public static class NativeExecutionBridge
{
    public const int MaxFrameBytes = 2 * 1024 * 1024;
    public const int MaxRequestBytes = 256 * 1024;
    public const int MaxJobBytes = 8 * 1024 * 1024;
    public const int MaxFacts = 128;
    private const long MaxToolBytes = 128L * 1024 * 1024;
    private static readonly JsonSerializerOptions Wire = new(JsonSerializerDefaults.Web);
    private static readonly StringComparison Paths = OperatingSystem.IsWindows()
        ? StringComparison.OrdinalIgnoreCase : StringComparison.Ordinal;

    public static bool ValidIdentity(NativeExecutionIdentity? identity, string? operation,
        NativeExecutionWork? work, string? rawPayload, out string reason)
    {
        reason = "bad_identity";
        if (identity is null || work is null || rawPayload is null ||
            Encoding.UTF8.GetByteCount(rawPayload) > MaxRequestBytes ||
            operation is not ("evaluate" or "build" or "test") ||
            !Bound(identity.JobId, 1, 128) || !Bound(identity.Root, 1, 4096) ||
            !Bound(identity.RootIdentity, 1, 160) || !Bound(identity.Context, 1, 128) ||
            identity.Epoch == Guid.Empty || identity.CommittedRevision < 0 || identity.FenceSequence < 0 ||
            !Digest(identity.InputSha256) || !Digest(identity.ToolSha256) ||
            !Digest(identity.PolicySha256) || !Digest(identity.GrantRevision) ||
            !Bound(identity.BudgetReference, 1, 128) ||
            identity.ExecutableSha256 is not null && !Digest(identity.ExecutableSha256) ||
            identity.RunnerSha256 is not null && !Digest(identity.RunnerSha256) ||
            !Bound(work.Root, 1, 4096) || !Bound(work.DotnetPath, 1, 4096) ||
            !Path.IsPathFullyQualified(work.Root) || !Path.IsPathFullyQualified(work.DotnetPath) ||
            !Path.GetFullPath(work.Root).Equals(Path.GetFullPath(identity.Root), Paths) ||
            !string.Equals(identity.InputSha256, Hex(SHA256.HashData(Encoding.UTF8.GetBytes(rawPayload))),
                StringComparison.OrdinalIgnoreCase) ||
            work.TimeoutSeconds is < 1 or > 1800 || !MatchesPayload(work, rawPayload))
            return false;
        if (operation == "evaluate")
        {
            if (work.IncludeAdditionalFileEvidence || identity.Context != "workspace" || identity.AttemptId != 0 ||
                identity.AttemptNumber != 0 || identity.ExecutableSha256 is not null ||
                identity.RunnerSha256 is not null)
                return false;
        }
        else
        {
            if (!Bound(work.SelectedTargetId, 1, 128) || identity.Context != work.SelectedTargetId ||
                !Bound(work.ResultsRoot, 1, 4096) || !Path.IsPathFullyQualified(work.ResultsRoot!) ||
                work.Closure is null || !ValidClosure(work.Closure))
                return false;
        }
        if (operation == "test" && (identity.AttemptId <= 0 || identity.AttemptNumber <= 0 ||
            !Digest(identity.ExecutableSha256) || !Digest(identity.RunnerSha256) ||
            !Digest(work.SelectedInputFingerprint) ||
            work.CauseIds is not { Length: > 0 and <= 128 } ||
            work.CauseIds.Any(cause => !Bound(cause, 1, 128)) ||
            !Bound(work.RunnerPath, 1, 4096) || !Path.IsPathFullyQualified(work.RunnerPath!)))
            return false;
        if (operation == "build" && (identity.AttemptId != 0 || identity.AttemptNumber != 0))
            return false;
        reason = "ok";
        return true;
    }

    private static bool MatchesPayload(NativeExecutionWork work, string rawPayload)
    {
        try
        {
            var decoded = JsonSerializer.Deserialize<NativeExecutionWork>(rawPayload, Wire);
            return decoded is not null &&
                JsonSerializer.Serialize(decoded, Wire) == JsonSerializer.Serialize(work, Wire);
        }
        catch (JsonException)
        {
            return false;
        }
    }

    private static bool ValidClosure(NativeExecutionClosure c) =>
        Bound(c.MappingIdentity, 1, 128) && Bound(c.RunnerIdentity, 1, 128) &&
        Bound(c.AdapterIdentity, 1, 128) &&
        new[] { c.ReferenceAssemblies, c.RuntimeAssets, c.SettingsFiles, c.AdapterFiles,
            c.RequiredOutputs, c.Companions, c.ObservedRuntimeInputs }.All(paths =>
                paths is { Length: <= 4096 } && paths.All(path =>
                    Bound(path, 1, 4096) && Path.IsPathFullyQualified(path))) &&
        c.RequiredOutputs.Length > 0 && c.Companions.Length <= VstestAttemptRunner.MaxCompanions;

    private static bool Bound(string? value, int min, int max) =>
        value is not null && value.Length >= min && value.Length <= max &&
        !value.Any(char.IsControl);

    private static bool Digest(string? value) => value is { Length: 64 } &&
        value.All(c => c is >= '0' and <= '9' or >= 'a' and <= 'f' or >= 'A' and <= 'F');

    private static string Hex(byte[] bytes) => Convert.ToHexString(bytes);

    // Caller has to check current epoch/committed revision/fence and the input and
    // policy digests again before accepting the complete terminal. This method
    // has no publication, scheduler, receipt-store or persistence side effects.
    public static Task<NativeExecutionResult> ExecuteAsync(NativeExecutionIdentity identity,
        string operation, NativeExecutionWork work, CancellationToken cancellationToken) =>
        ExecuteCoreAsync(identity, operation, work, null, cancellationToken);

    // The existing IntegrationTests friend assembly supplies an isolated per-test
    // state root without modifying the user's real execution profile.
    internal static Task<NativeExecutionResult> ExecuteForTestsAsync(Workspace workspace,
        NativeExecutionIdentity identity, string operation, NativeExecutionWork work,
        CancellationToken cancellationToken) =>
        ExecuteCoreAsync(identity, operation, work, workspace, cancellationToken);

    private static async Task<NativeExecutionResult> ExecuteCoreAsync(NativeExecutionIdentity identity,
        string operation, NativeExecutionWork work, Workspace? isolatedWorkspace,
        CancellationToken cancellationToken)
    {
        try
        {
            if (!Path.GetFullPath(identity.Root).Equals(Path.GetFullPath(work.Root), Paths) ||
                operation is not ("evaluate" or "build" or "test") ||
                operation == "evaluate" && work.IncludeAdditionalFileEvidence ||
                operation != "evaluate" && identity.Context != work.SelectedTargetId ||
                isolatedWorkspace is not null &&
                !isolatedWorkspace.Root.Equals(Path.GetFullPath(work.Root), Paths) ||
                (operation is "build" or "test") &&
                (work.Closure is null || string.IsNullOrWhiteSpace(work.SelectedTargetId) ||
                 string.IsNullOrWhiteSpace(work.ResultsRoot)))
                return NativeExecutionResult.Refuse("unsupported", "job_context_mismatch");
            var linux = OperatingSystem.IsLinux()
                ? await NativeLinuxExecutionScope.TryAttachAsync(cancellationToken) : null;
            if (!OperatingSystem.IsWindowsVersionAtLeast(10) && linux is null)
                return NativeExecutionResult.Refuse("unsupported", "process_scope_unavailable", "native_worker_scope");
            var workspace = isolatedWorkspace ?? new Workspace(work.Root);
            if (RootIdentity.Read(workspace.Root) != identity.RootIdentity)
                return NativeExecutionResult.Refuse("stale", "root_changed");
            var dotnet = linux is null ? ResolveDotnet(workspace) : Path.GetFullPath(work.DotnetPath);
            if (dotnet is null || linux is not null && (Path.GetFileName(dotnet) != "dotnet" ||
                ExecutionProfile.Contains(workspace.Root, dotnet) || !File.Exists(dotnet)))
                return NativeExecutionResult.Refuse("deferred", "dotnet_unavailable", "toolchain");
            if (!dotnet.Equals(Path.GetFullPath(work.DotnetPath), Paths) ||
                !string.Equals(HashFile(dotnet, cancellationToken), identity.ToolSha256, StringComparison.OrdinalIgnoreCase))
                return NativeExecutionResult.Refuse("stale", "tool_identity_changed");

            var grant = ExecutionProfile.Read(workspace, identity.RootIdentity);
            if (!grant.Revision.Equals(identity.GrantRevision, StringComparison.OrdinalIgnoreCase) ||
                !grant.Revision.Equals(identity.PolicySha256, StringComparison.OrdinalIgnoreCase))
                return NativeExecutionResult.Refuse("stale", "policy_changed");
            var operations = ExecutionOperation.Evaluate;
            if (operation is "build" or "test")
                operations |= ExecutionOperation.Build | ExecutionOperation.CompilerExtensions;
            if (operation == "test")
                operations |= ExecutionOperation.Test;
            // No Evaluate, Restore, Network or extension grant is inferred by the job.
            string[] roots = operation == "evaluate" ? [] : grant.OutputRoots;
            var requirements = new WorkerRequirements(operations, UsesProcesses: true, OutputRoots: roots);
            grant.Require(requirements);
            var (lease, waiting) = linux is null
                ? new WorkerAdmission(workspace).TryAcquire(grant, requirements)
                : (new WorkerLease(null, workspace, Guid.NewGuid(), grant, requirements, null, linux), null);
            if (lease is null)
                return NativeExecutionResult.Refuse("deferred", waiting ?? "waiting_worker");
            await using var worker = lease;
            using var deadline = CancellationTokenSource.CreateLinkedTokenSource(cancellationToken);
            deadline.CancelAfter(TimeSpan.FromSeconds(work.TimeoutSeconds));
            var token = deadline.Token;
            worker.Revalidate();
            var evaluator = new ProjectEvaluator(workspace);
            var graph = await evaluator.EvaluateAsync(worker, token, pinnedDotnet: dotnet);
            if (!graph.Complete)
                return NativeExecutionResult.Refuse("partial", "evaluation_incomplete",
                    graph.Issues.Select(issue => issue.Reason).Distinct().Take(32).ToArray());
            if (operation == "evaluate")
            {
                worker.Revalidate();
                CheckIdentity(workspace, dotnet, identity, token);
                return Complete(new NativeExecutionFact("evaluated_graph", JsonSerializer.SerializeToElement(graph, Wire)));
            }
            var selected = graph.Targets.SingleOrDefault(target => target.Id == identity.Context);
            if (selected is null)
                return NativeExecutionResult.Refuse("unsupported", "unknown_evaluated_target");
            var prepared = evaluator.PrepareBuild(graph, selected.Id, work.ResultsRoot!, grant);
            if (!prepared.Ready)
                return NativeExecutionResult.Refuse("unsupported", "build_" + prepared.Blocker!.Value.ToString().ToLowerInvariant());
            var plan = prepared.Plan!;
            if (!plan.Command.Executable.Equals(dotnet, Paths))
                return NativeExecutionResult.Refuse("stale", "selected_sdk_changed");
            worker.Revalidate();
            var run = await worker.RunAsync(plan.Command.ToStartInfo(), 16_384, token);
            worker.Revalidate();
            CheckIdentity(workspace, dotnet, identity, token);
            if (run.ExitCode != 0)
                return NativeExecutionResult.Refuse("failed", "build_nonzero_exit");
            if (plan.Assets.Any(asset => !string.Equals(HashFile(asset.Path, token), asset.Sha256,
                    StringComparison.OrdinalIgnoreCase)))
                return NativeExecutionResult.Refuse("stale", "build_assets_changed");
            var capture = Capture(workspace, identity, work, graph, selected, plan, token);
            if (!capture.Stamp.Complete)
                return NativeExecutionResult.Refuse("partial", "build_closure_incomplete",
                    capture.Stamp.Problems.Select(problem => problem.Field).Take(32).ToArray());
            var buildData = JsonSerializer.SerializeToElement(new
            {
                plan.SelectedTargetId, selected.Framework, selected.Configuration, selected.Platform,
                selected.RuntimeIdentifier, selected.SdkVersion, plan.Assets,
                plan.InvokedTargets, capture.Stamp, capture.Outputs
            }, Wire);
            if (work.IncludeAdditionalFileEvidence)
                buildData = AdditionalFileEvidence.Attach(buildData, capture, identity, "post-build");
            var buildFact = new NativeExecutionFact("verified_build", buildData);
            if (operation == "build")
            {
                worker.Revalidate();
                CheckIdentity(workspace, dotnet, identity, token);
                return Complete(buildFact);
            }
            if (!capture.Outputs.Any(output => output.Status == BuildArtifactObservationStatus.ByteVerified &&
                Path.GetFullPath(output.PhysicalPath!).Equals(Path.GetFullPath(plan.InvokedTargets[^1].TargetAssembly), Paths)))
                return NativeExecutionResult.Refuse("partial", "test_assembly_unobserved");
            var refreshed = Capture(workspace, identity, work, graph, selected, plan, token);
            if (!BuildOutputIdentity.Compare(capture.Stamp, refreshed.Stamp).Reusable)
                return NativeExecutionResult.Refuse("stale", "build_identity_changed");
            worker.RequireTestOutputRoot(Path.GetFullPath(work.ResultsRoot!));
            Workspace.CheckDirectory(work.ResultsRoot!, allowMissing: true);
            Directory.CreateDirectory(work.ResultsRoot!);
            Workspace.CheckDirectory(work.ResultsRoot!);
            var assembly = FindVerified(refreshed, plan.InvokedTargets[^1].TargetAssembly);
            if (!string.Equals(assembly.Sha256, identity.ExecutableSha256, StringComparison.OrdinalIgnoreCase))
                return NativeExecutionResult.Refuse("stale", "executable_identity_changed");
            var companions = work.Closure!.Companions.Select(path => FindVerified(refreshed, path)).ToArray();
            var runner = new VstestVerifiedFile(work.RunnerPath!, HashFile(work.RunnerPath!, token));
            if (!string.Equals(runner.Sha256, identity.RunnerSha256, StringComparison.OrdinalIgnoreCase))
                return NativeExecutionResult.Refuse("stale", "runner_identity_changed");
            var attempt = new TestAttempt(identity.AttemptId, identity.AttemptNumber,
                new ScheduleFence(identity.Epoch, identity.FenceSequence, identity.CommittedRevision,
                    identity.RootIdentity, new GitRevision(work.GitBranch, work.GitCommit)),
                new SelectedTarget(selected.Id, work.SelectedInputFingerprint!, work.CauseIds!), grant.Revision);
            var vstest = new VstestAttemptRequest(attempt,
                new VstestVerifiedFile(dotnet, identity.ToolSha256), runner, selected.SdkVersion,
                capture.Stamp, refreshed, assembly, companions, work.ResultsRoot!,
                TimeSpan.FromSeconds(work.TimeoutSeconds));
            var result = await VstestAttemptRunner.RunAsync(vstest, worker, token);
            if (result.Gate == VstestAttemptGate.DeniedAuthority)
                return NativeExecutionResult.Refuse("stale", "test_authority_revoked");
            if (result.Gate == VstestAttemptGate.Cancelled)
                return NativeExecutionResult.Refuse("cancelled", "test_cancelled");
            if (result.Gate != VstestAttemptGate.Launched || result.Classification is null)
                return NativeExecutionResult.Refuse("partial", "test_" + result.Gate.ToString().ToLowerInvariant());
            var disposition = result.Classification.Disposition;
            if (disposition is not (VstestDisposition.Passed or VstestDisposition.Failed or
                VstestDisposition.ZeroDiscovered or VstestDisposition.AllSkipped))
                return NativeExecutionResult.Refuse("partial", "test_" + disposition.ToString().ToLowerInvariant());
            // Validation here is not a SQLite Finish. The native owner must reconcile
            // this evidence with its actual admitted TestAttempt before publishing.
            var cls = result.Classification;
            var receipt = new TestRunReceiptDetails(attempt.Id, attempt.Fence.Revision, attempt.Fence.Epoch,
                attempt.Fence.Sequence, TestRunReceiptDetails.GitIdentity(attempt.Fence.Git),
                attempt.Fence.RootIdentity, attempt.Target.Id, attempt.Target.InputFingerprint,
                attempt.ProfileRevision, worker.Id.ToString("N"), capture.Stamp.DomainVersion,
                capture.Stamp.Sha256, runner.Sha256, cls.Artifact.Sha256,
                cls.Artifact.Health == VstestArtifactHealth.Complete ? cls.Artifact.ByteLength : null,
                cls.Artifact.RunId, disposition, cls.Runner, cls.Artifact.Health, cls.Execution,
                cls.Artifact.Counters, "observed");
            receipt.Validate(attempt, cls.SchedulerOutcome ?? TestOutcome.Blocked, worker.Id.ToString("N"));
            worker.Revalidate();
            CheckIdentity(workspace, dotnet, identity, token);
            var testFact = new NativeExecutionFact("admitted_vstest", JsonSerializer.SerializeToElement(new
            {
                result.AttemptId, result.AttemptNumber, result.Gate, result.Process,
                cls.Disposition, cls.Runner, cls.Execution,
                cls.Artifact.Health, cls.Artifact.Sha256, cls.Artifact.ByteLength,
                cls.Artifact.RunId, cls.Artifact.Counters, receipt
            }, Wire));
            return Complete(buildFact, testFact);
        }
        catch (OperationCanceledException)
        {
            return NativeExecutionResult.Refuse("cancelled", "deadline_or_cancellation");
        }
        catch (UnauthorizedAccessException)
        {
            return NativeExecutionResult.Refuse("unsupported", "grant_missing_or_revoked");
        }
        catch (Exception ex) when (ex is IOException or ArgumentException or InvalidOperationException or
                                   NotSupportedException or System.ComponentModel.Win32Exception or
                                   Microsoft.Data.Sqlite.SqliteException)
        {
            return NativeExecutionResult.Refuse("failed", "worker_io_or_tool_failure");
        }
    }

    private static NativeExecutionResult Complete(params NativeExecutionFact[] facts) =>
        new("complete", "ok", facts, []);

    private static void CheckIdentity(Workspace workspace, string dotnet,
        NativeExecutionIdentity identity, CancellationToken token)
    {
        if (RootIdentity.Read(workspace.Root) != identity.RootIdentity ||
            !string.Equals(HashFile(dotnet, token), identity.ToolSha256, StringComparison.OrdinalIgnoreCase) ||
            ExecutionProfile.Read(workspace, identity.RootIdentity).Revision != identity.GrantRevision)
            throw new UnauthorizedAccessException("Workspace, tool, or authority changed.");
    }

    internal static string? ResolveDotnetForTests(Workspace workspace, string path) =>
        ResolveDotnet(workspace, path);

    private static string? ResolveDotnet(Workspace workspace, string? suppliedPath = null)
    {
        var name = OperatingSystem.IsWindows() ? "dotnet.exe" : "dotnet";
        foreach (var entry in (suppliedPath ?? Environment.GetEnvironmentVariable("PATH") ?? "").Split(Path.PathSeparator))
        {
            var directory = entry.Trim().Trim('"');
            if (Path.IsPathFullyQualified(directory))
            {
                var path = Path.GetFullPath(Path.Combine(directory, name));
                if (!ExecutionProfile.Contains(workspace.Root, path) && File.Exists(path))
                    return path;
            }
        }
        return null;
    }

    private static string HashFile(string path, CancellationToken token)
    {
        Workspace.CheckDirectory(Path.GetDirectoryName(Path.GetFullPath(path))!);
        using var handle = PhysicalArtifactFile.OpenRead(path);
        using var stream = new FileStream(handle, FileAccess.Read);
        if (stream.Length is < 0 or > MaxToolBytes)
            throw new IOException("File exceeds bounded identity read.");
        using var hasher = IncrementalHash.CreateHash(HashAlgorithmName.SHA256);
        var buffer = new byte[64 * 1024];
        long read = 0;
        int n;
        while ((n = stream.Read(buffer)) > 0)
        {
            token.ThrowIfCancellationRequested();
            read += n;
            if (read > MaxToolBytes)
                throw new IOException("File exceeds bounded identity read.");
            hasher.AppendData(buffer, 0, n);
        }
        return Hex(hasher.GetHashAndReset());
    }

    private static BuildArtifactCaptureResult Capture(Workspace workspace,
        NativeExecutionIdentity identity, NativeExecutionWork work, ProjectGraph graph,
        ProjectTarget selected, TestBuildPlan plan, CancellationToken token)
    {
        var closure = work.Closure!;
        var mappings = new SortedDictionary<string, string>(StringComparer.Ordinal);
        string Add(string physical)
        {
            var path = Path.GetFullPath(physical);
            if (!ExecutionProfile.Contains(workspace.Root, path) ||
                path.Equals(workspace.Root, Paths) || !workspace.IsIncluded(path) &&
                !plan.InvokedTargets.Any(target =>
                    ExecutionProfile.Contains(target.OutputDirectory, path) ||
                    ExecutionProfile.Contains(target.BaseIntermediateDirectory, path)))
                throw new UnauthorizedAccessException("Capture path is not owned by the workspace.");
            Workspace.CheckDirectory(Path.GetDirectoryName(path)!);
            var relative = workspace.Relative(path);
            if (mappings.TryGetValue(relative, out var existing) && !existing.Equals(path, Paths))
                throw new IOException("Conflicting physical path mapping.");
            mappings[relative] = path;
            return relative;
        }
        var compile = selected.Compile.Select(member =>
            new BuildArtifactCompile(Add(Path.Combine(workspace.Root, member.Path.Replace('/',
                Path.DirectorySeparatorChar))), member.Link)).ToArray();
        var projectRefs = plan.InvokedTargets.Where(target => target.TargetId != selected.Id)
            .Select(target => target.TargetAssembly).ToArray();
        var referencePaths = closure.ReferenceAssemblies.Select(path => Path.GetFullPath(path)).ToArray();
        if (projectRefs.Any(path => !referencePaths.Contains(path, Workspace.PathComparer)))
            throw new IOException("Evaluated project reference is not in the explicit closure.");
        var required = plan.InvokedTargets.Select(target => target.TargetAssembly)
            .Concat(closure.RequiredOutputs).Select(path => Path.GetFullPath(path))
            .Distinct(Workspace.PathComparer).Select(Add).ToArray();
        var references = referencePaths.Select(Add).ToArray();
        var runtime = closure.RuntimeAssets.Select(Add).ToArray();
        var settings = plan.InvokedTargets.Select(target => target.ProjectPath)
            .Concat(plan.Assets.Select(asset => asset.Path)).Concat(closure.SettingsFiles)
            .Select(path => Path.GetFullPath(path)).Distinct(Workspace.PathComparer).Select(Add).ToArray();
        var adapters = closure.AdapterFiles.Select(Add).ToArray();
        if (work.SelectedTargetId is not null && closure.Companions.Any(path =>
            !closure.RequiredOutputs.Contains(path, Workspace.PathComparer)))
            throw new IOException("Runner companion missing from required outputs.");
        if (closure.ObservedRuntimeInputs.Any(path => !closure.RuntimeAssets.Contains(path,
                Workspace.PathComparer)))
            throw new IOException("Observed runtime input omitted from the closure.");
        var manifest = new BuildArtifactManifest(selected, graph.Complete,
            closure.MappingIdentity, closure.RunnerIdentity, closure.AdapterIdentity,
            compile, references, runtime, settings, adapters, required);
        var mapped = mappings.Select(pair => new BuildArtifactPathMapping(pair.Key, pair.Value)).ToArray();
        return BuildArtifactCapture.Capture(manifest,
            new BuildArtifactReadScope([workspace.Root], mapped, MappingTrusted: true),
            cancellationToken: token,
            observedRuntimeInputs: closure.ObservedRuntimeInputs.Select(Add).ToArray(),
            retainIdentityWitness: work.IncludeAdditionalFileEvidence);
    }

    private static VstestVerifiedFile FindVerified(BuildArtifactCaptureResult capture, string path)
    {
        var physical = Path.GetFullPath(path);
        var item = capture.Outputs.SingleOrDefault(output =>
            output.PhysicalPath is not null && output.PhysicalPath.Equals(physical, Paths) &&
            output.Status == BuildArtifactObservationStatus.ByteVerified);
        if (item?.Sha256 is null)
            throw new IOException("Required output was not verified.");
        return new VstestVerifiedFile(physical, item.Sha256);
    }
}
