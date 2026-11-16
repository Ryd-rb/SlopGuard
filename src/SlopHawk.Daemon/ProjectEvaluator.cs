using System.Diagnostics;
using System.Security.Cryptography;
using System.Text;
using System.Text.Json;
using NuGet.Frameworks;

namespace SlopHawk.Daemon;

public sealed class ProjectEvaluator(Workspace workspace)
{
    private const int MaxEntries = 100_000;
    private const int MaxProjects = 64;
    private const int MaxTargets = 128;
    private const int MaxMetadataFiles = 1_024;
    private const int MaxSourceItems = 20_000;
    private const int MaxGraphCharacters = 2_000_000;
    private const int MaxMetadataBytes = 2 * 1024 * 1024;
    private BuildEvaluationSnapshot? buildSnapshot;
    private const string Properties = "TargetFramework,TargetFrameworks,Configuration,Platform,RuntimeIdentifier," +
        "UsingMicrosoftNETSdk,NETCoreSdkVersion,IsTestProject,TargetFrameworkMoniker,TargetPlatformIdentifier," +
        "MSBuildAllProjects,DirectoryBuildPropsPath,DirectoryBuildTargetsPath,DirectoryPackagesPropsPath," +
        "ProjectAssetsFile,DefineConstants,LangVersion,Nullable,PlatformTarget,AssetTargetFallback," +
        "OutputPath,OutDir,TargetPath,TargetDir,IntermediateOutputPath,BaseIntermediateOutputPath," +
        "IsTestingPlatformApplication,UseMicrosoftTestingPlatformRunner,GeneratePackageOnBuild," +
        "UseArtifactsOutput,DocumentationFile,ErrorLog,PreBuildEvent,PostBuildEvent," +
        "CustomBeforeMicrosoftCommonTargets,CustomAfterMicrosoftCommonTargets";

    // Consumes the exact graph instance returned by this evaluator's last authorized
    // evaluation; planning is read-only and must not launch an MSBuild process.
    internal TestBuildPreparation PrepareBuild(ProjectGraph graph, string selectedTargetId,
        string attemptResultsRoot, ExecutionGrant grant) =>
        new TestBuildPlanner(workspace).Prepare(graph, selectedTargetId, attemptResultsRoot, grant, buildSnapshot);

    // Calling this API explicitly grants project evaluation for the supplied workspace.
    public async Task<ProjectGraph> EvaluateAsync(CancellationToken cancellationToken = default)
    {
        if (!OperatingSystem.IsWindowsVersionAtLeast(10))
            return await EvaluateAsync(null, cancellationToken);
        try
        {
            var grant = new ExecutionGrant("explicit-evaluation", RootIdentity.Read(workspace.Root),
                ExecutionOperation.Evaluate, [], FromProfile: false);
            var request = new WorkerAdmission(workspace).TryAcquire(grant,
                new WorkerRequirements(ExecutionOperation.Evaluate, true, []));
            if (request.Lease is null)
                return new ProjectGraph([], [new ProjectEvaluationIssue(".", request.Waiting!)]);
            await using var worker = request.Lease;
            return await EvaluateAsync(worker, cancellationToken);
        }
        catch (Microsoft.Data.Sqlite.SqliteException ex)
        {
            throw new IOException("The evaluation worker reservation could not be recorded.", ex);
        }
    }

    internal async Task<ProjectGraph> EvaluateAsync(WorkerLease? worker, CancellationToken cancellationToken)
    {
        buildSnapshot = null;
        var rootIdentity = RootIdentity.Read(workspace.Root);
        using var deadline = CancellationTokenSource.CreateLinkedTokenSource(cancellationToken);
        deadline.CancelAfter(TimeSpan.FromSeconds(45));
        var token = deadline.Token;
        var issues = new List<ProjectEvaluationIssue>();
        var dotnet = FindDotnet();
        if (dotnet is null)
            return new ProjectGraph([], [new ProjectEvaluationIssue(".", "dotnet_unavailable")]);
        var evaluated = new List<RawTarget>();
        var inputs = new SortedDictionary<string, string>(Workspace.PathComparer);
        var sourceCount = 0;
        long graphCharacters = 0;
        try
        {
            var projects = Discover(issues, token);
            foreach (var path in projects.Metadata)
                await AddInputAsync(path, required: true);
            foreach (var path in projects.Projects)
            {
                if (RootIdentity.Read(workspace.Root) != rootIdentity)
                    throw new IOException("The workspace was replaced during project evaluation.");
                try
                {
                    var outer = await QueryAsync(dotnet, path, null, worker, token);
                    var frameworks = Value(outer.Properties, "TargetFrameworks")
                        .Split(';', StringSplitOptions.RemoveEmptyEntries | StringSplitOptions.TrimEntries);
                    if (frameworks.Length == 0)
                        frameworks = [Value(outer.Properties, "TargetFramework")];
                    foreach (var framework in frameworks.Distinct(StringComparer.OrdinalIgnoreCase))
                    {
                        if (evaluated.Count >= MaxTargets)
                        {
                            issues.Add(new ProjectEvaluationIssue(".", "target_budget"));
                            return Finish();
                        }
                        if (!SafeFramework(framework))
                        {
                            issues.Add(new ProjectEvaluationIssue(workspace.Relative(path), "unsupported_framework"));
                            continue;
                        }
                        var result = Value(outer.Properties, "TargetFramework") == framework
                            ? outer : await QueryAsync(dotnet, path, framework, worker, token);
                        var target = CreateTarget(path, framework, result);
                        if (target is null)
                        {
                            issues.Add(new ProjectEvaluationIssue(workspace.Relative(path), "unsupported_project_context"));
                            continue;
                        }
                        sourceCount += target.Target.Compile.Count;
                        graphCharacters += target.Target.Compile.Sum(member => (long)member.Path.Length + (member.Link?.Length ?? 0))
                            + target.References.Sum(reference => (long)reference.Path.Length + (reference.Framework?.Length ?? 0));
                        if (sourceCount > MaxSourceItems || graphCharacters > MaxGraphCharacters)
                        {
                            issues.Add(new ProjectEvaluationIssue(".", "membership_budget"));
                            return Finish();
                        }
                        foreach (var property in new[] { "DirectoryBuildPropsPath", "DirectoryBuildTargetsPath",
                                     "DirectoryPackagesPropsPath", "ProjectAssetsFile" })
                        {
                            var inputPath = Value(result.Properties, property);
                            if (InsideRoot(inputPath))
                                await AddInputAsync(inputPath, required: property != "ProjectAssetsFile");
                            else if (inputPath.Length != 0 && property != "ProjectAssetsFile")
                                issues.Add(new ProjectEvaluationIssue(workspace.Relative(path), "external_project_input"));
                        }
                        foreach (var import in Value(result.Properties, "MSBuildAllProjects").Split(';',
                                     StringSplitOptions.RemoveEmptyEntries | StringSplitOptions.TrimEntries))
                        {
                            if (InsideRoot(import))
                                await AddInputAsync(import, required: true);
                        }
                        evaluated.Add(target);
                    }
                }
                catch (Exception ex) when (ex is IOException or System.ComponentModel.Win32Exception or TimeoutException)
                {
                    issues.Add(new ProjectEvaluationIssue(workspace.Relative(path), "evaluation_failed"));
                }
                catch (UnauthorizedAccessException) when (worker is null)
                {
                    issues.Add(new ProjectEvaluationIssue(workspace.Relative(path), "evaluation_inaccessible"));
                }
            }
        }
        catch (OperationCanceledException) when (!cancellationToken.IsCancellationRequested)
        {
            issues.Add(new ProjectEvaluationIssue(".", "evaluation_timeout"));
        }

        return Finish();

        ProjectGraph Finish()
        {
            if (RootIdentity.Read(workspace.Root) != rootIdentity)
                throw new IOException("The workspace was replaced during project evaluation.");
            var graph = BuildGraph(evaluated, inputs, issues);
            var descriptions = new Dictionary<string, EvaluatedBuildTarget>(StringComparer.Ordinal);
            foreach (var target in evaluated)
                descriptions.TryAdd(target.Target.Id, target.Build);
            buildSnapshot = new BuildEvaluationSnapshot(graph, rootIdentity, dotnet, descriptions);
            return graph;
        }

        async Task AddInputAsync(string path, bool required)
        {
            var relative = workspace.Relative(path);
            if (inputs.ContainsKey(relative))
                return;
            if (inputs.Count == MaxMetadataFiles)
            {
                if (!issues.Any(issue => issue.Reason == "metadata_budget"))
                    issues.Add(new ProjectEvaluationIssue(".", "metadata_budget"));
                return;
            }
            try
            {
                Workspace.CheckDirectory(Path.GetDirectoryName(path)!);
                if ((File.GetAttributes(path) & (FileAttributes.Directory | FileAttributes.ReparsePoint)) != 0)
                    throw new IOException("The project input requires a physical file.");
                await using var stream = File.OpenRead(path);
                if (stream.Length > MaxMetadataBytes)
                    throw new IOException("The project input exceeds its size budget.");
                inputs.Add(relative, Convert.ToHexString(await SHA256.HashDataAsync(stream, token)));
            }
            catch (IOException ex) when (!required && ex is FileNotFoundException or DirectoryNotFoundException)
            {
                inputs.Add(relative, "missing");
            }
            catch (Exception ex) when (ex is IOException or UnauthorizedAccessException)
            {
                issues.Add(new ProjectEvaluationIssue(relative, "metadata_unavailable"));
            }
        }
    }

    private (string[] Projects, string[] Metadata) Discover(List<ProjectEvaluationIssue> issues, CancellationToken token)
    {
        var projects = new List<string>();
        var metadata = new List<string>();
        var directories = new Stack<string>();
        directories.Push(workspace.Root);
        var visited = 0;
        while (directories.TryPop(out var directory))
        {
            try
            {
                Workspace.CheckDirectory(directory);
                foreach (var path in Directory.EnumerateFileSystemEntries(directory))
                {
                    token.ThrowIfCancellationRequested();
                    if (++visited > MaxEntries)
                    {
                        issues.Add(new ProjectEvaluationIssue(".", "discovery_budget"));
                        return Sorted();
                    }
                    if (!workspace.IsIncluded(path))
                        continue;
                    var flags = File.GetAttributes(path);
                    if ((flags & FileAttributes.ReparsePoint) != 0)
                    {
                        if ((flags & FileAttributes.Directory) != 0 || IsProjectInput(path))
                            issues.Add(new ProjectEvaluationIssue(workspace.Relative(path), "linked_project_input"));
                        continue;
                    }
                    if ((flags & FileAttributes.Directory) != 0)
                        directories.Push(path);
                    else if (IsProjectInput(path))
                    {
                        if (Path.GetExtension(path).Equals(".csproj", StringComparison.OrdinalIgnoreCase))
                            projects.Add(path);
                        metadata.Add(path);
                        if (projects.Count > MaxProjects || metadata.Count > MaxMetadataFiles)
                        {
                            issues.Add(new ProjectEvaluationIssue(".", "project_budget"));
                            return Sorted();
                        }
                    }
                }
            }
            catch (Exception ex) when (ex is IOException or UnauthorizedAccessException)
            {
                issues.Add(new ProjectEvaluationIssue(workspace.Relative(directory), "discovery_incomplete"));
            }
        }
        return Sorted();

        (string[], string[]) Sorted() =>
            (projects.Order(Workspace.PathComparer).Take(MaxProjects).ToArray(),
                metadata.Order(Workspace.PathComparer).Take(MaxMetadataFiles).ToArray());
    }

    internal static bool IsProjectInput(string path) =>
        Path.GetExtension(path).ToUpperInvariant() is ".CSPROJ" or ".SLN" or ".SLNX" or ".PROPS" or ".TARGETS"
            or ".EDITORCONFIG" or ".RUNSETTINGS" ||
        Path.GetFileName(path).ToUpperInvariant() is "GLOBAL.JSON" or "PACKAGES.LOCK.JSON" or "NUGET.CONFIG";

    private bool InsideRoot(string path)
    {
        if (!Path.IsPathFullyQualified(path))
            return false;
        var relative = workspace.Relative(path);
        return !Path.IsPathRooted(relative) && relative is not ("." or "..") &&
            !relative.StartsWith("../", StringComparison.Ordinal);
    }

    private RawTarget? CreateTarget(string path, string framework, Evaluation result)
    {
        var properties = result.Properties;
        var sdk = Value(properties, "NETCoreSdkVersion");
        var shortFramework = NuGetFramework.ParseFolder(framework);
        if (!IsTrue(properties, "UsingMicrosoftNETSdk") || !sdk.StartsWith("10.", StringComparison.Ordinal) ||
            !framework.Equals(Value(properties, "TargetFramework"), StringComparison.OrdinalIgnoreCase) ||
            Value(properties, "TargetPlatformIdentifier").Length != 0 || shortFramework.IsUnsupported ||
            !shortFramework.Equals(NuGetFramework.Parse(Value(properties, "TargetFrameworkMoniker"))) ||
            shortFramework.Framework is not (".NETCoreApp" or ".NETStandard"))
            return null;

        var project = workspace.Relative(path);
        var configuration = Value(properties, "Configuration");
        var platform = Value(properties, "Platform");
        var rid = Value(properties, "RuntimeIdentifier");
        var id = project + "|" + framework;
        if (id.Length > 128)
            id = TestCause.Hash(id);
        var compile = new List<CompileMember>();
        var references = new List<RawReference>();
        var issues = new List<ProjectEvaluationIssue>();
        foreach (var item in Items(result.Items, "Compile"))
        {
            var member = Value(item, "FullPath");
            if (!InsideRoot(member) || !workspace.IsIncluded(member))
                issues.Add(new ProjectEvaluationIssue(project, "external_or_excluded_source"));
            else
                compile.Add(new CompileMember(workspace.Relative(member), Value(item, "Link") is { Length: > 0 } link ? link : null));
        }
        foreach (var item in Items(result.Items, "ProjectReference"))
        {
            var reference = Value(item, "FullPath");
            var setFramework = Value(item, "SetTargetFramework");
            if (!InsideRoot(reference) || !workspace.IsIncluded(reference) ||
                new[] { "AdditionalProperties", "Properties", "GlobalPropertiesToRemove", "UndefineProperties",
                    "SetConfiguration", "SetPlatform", "SetRuntimeIdentifier", "Targets" }.Any(name => Value(item, name).Length > 0) ||
                setFramework.Length > 0 && !setFramework.StartsWith("TargetFramework=", StringComparison.OrdinalIgnoreCase) ||
                setFramework.Length == 0 && IsTrue(item, "SkipGetTargetFrameworkProperties"))
            {
                issues.Add(new ProjectEvaluationIssue(project, "unresolved_reference_context"));
                continue;
            }
            var explicitFramework = setFramework.Length == 0 ? null : setFramework["TargetFramework=".Length..];
            references.Add(new RawReference(workspace.Relative(reference), explicitFramework));
        }
        var build = new EvaluatedBuildTarget(id, framework, configuration, platform, rid, sdk,
            Value(properties, "TargetDir"), Value(properties, "IntermediateOutputPath"),
            Value(properties, "BaseIntermediateOutputPath"), Value(properties, "TargetPath"),
            Value(properties, "ProjectAssetsFile"),
            Value(properties, "MSBuildAllProjects").Split(';', StringSplitOptions.RemoveEmptyEntries | StringSplitOptions.TrimEntries))
        {
            UsesMicrosoftTestingPlatform = IsTrue(properties, "IsTestingPlatformApplication") ||
                IsTrue(properties, "UseMicrosoftTestingPlatformRunner"),
            HasUnsupportedImports = new[] { "DirectoryBuildPropsPath", "DirectoryBuildTargetsPath",
                "DirectoryPackagesPropsPath" }.Any(name => Value(properties, name).Length != 0),
            HasUnknownWrites = IsTrue(properties, "GeneratePackageOnBuild") ||
                IsTrue(properties, "UseArtifactsOutput") ||
                new[] { "DocumentationFile", "ErrorLog", "PreBuildEvent", "PostBuildEvent" }
                    .Any(name => Value(properties, name).Length != 0),
            OutDir = Value(properties, "OutDir"),
            OutputPath = Value(properties, "OutputPath"),
            CustomBeforeMicrosoftCommonTargets = Value(properties, "CustomBeforeMicrosoftCommonTargets"),
            CustomAfterMicrosoftCommonTargets = Value(properties, "CustomAfterMicrosoftCommonTargets")
        };
        return new RawTarget(new ProjectTarget(id, project, framework, configuration, platform, rid, sdk,
            IsTrue(properties, "IsTestProject"),
            compile.Distinct().OrderBy(member => member.Path, Workspace.PathComparer).ThenBy(member => member.Link, StringComparer.Ordinal).ToArray(),
            [], TestCause.Hash(properties.GetRawText())), shortFramework, references.ToArray(), issues.Distinct().ToArray(), build);
    }

    private static ProjectGraph BuildGraph(List<RawTarget> evaluated, SortedDictionary<string, string> inputs,
        List<ProjectEvaluationIssue> issues)
    {
        var reducer = new FrameworkReducer();
        var projects = evaluated.ToLookup(target => target.Target.ProjectPath, Workspace.PathComparer);
        var metadata = TestCause.Hash(JsonSerializer.Serialize(inputs));
        var targets = new List<ProjectTarget>();
        foreach (var source in evaluated)
        {
            issues.AddRange(source.Issues);
            var references = new SortedSet<string>(StringComparer.Ordinal);
            foreach (var reference in source.References)
            {
                var candidates = projects[reference.Path].Where(target =>
                    target.Target.SdkVersion == source.Target.SdkVersion &&
                    target.Target.Configuration == source.Target.Configuration &&
                    target.Target.Platform == source.Target.Platform &&
                    target.Target.RuntimeIdentifier == source.Target.RuntimeIdentifier).ToArray();
                var nearest = reference.Framework is null
                    ? reducer.GetNearest(source.Framework, candidates.Select(target => target.Framework))
                    : null;
                var matches = candidates.Where(target => reference.Framework is null
                    ? target.Framework.Equals(nearest)
                    : target.Target.Framework.Equals(reference.Framework, StringComparison.OrdinalIgnoreCase)).ToArray();
                if (matches.Length == 1)
                    references.Add(matches[0].Target.Id);
                else
                    issues.Add(new ProjectEvaluationIssue(source.Target.ProjectPath, "unresolved_reference_target"));
            }
            targets.Add(source.Target with
            {
                References = references.ToArray(),
                EvaluationFingerprint = TestCause.Hash(source.Target.EvaluationFingerprint + metadata)
            });
        }
        return new ProjectGraph(targets.OrderBy(target => target.Id, StringComparer.Ordinal).ToArray(),
            issues.Distinct().Take(256).ToArray());
    }

    private static bool SafeFramework(string framework) => framework.Length is > 0 and <= 64 &&
        framework.All(character => char.IsAsciiLetterOrDigit(character) || character is '.' or '-' or '_');

    private static bool IsTrue(JsonElement element, string property) =>
        Value(element, property).Equals("true", StringComparison.OrdinalIgnoreCase);

    private static string Value(JsonElement element, string property) =>
        element.TryGetProperty(property, out var value) && value.ValueKind == JsonValueKind.String
            ? value.GetString()! : string.Empty;

    private static JsonElement.ArrayEnumerator Items(JsonElement element, string name) =>
        element.GetProperty(name).EnumerateArray();

    private async Task<Evaluation> QueryAsync(string dotnet, string path, string? framework, WorkerLease? worker,
        CancellationToken token)
    {
        Workspace.CheckDirectory(Path.GetDirectoryName(path)!);
        if ((File.GetAttributes(path) & FileAttributes.ReparsePoint) != 0)
            throw new IOException("The project resolves through a link.");
        var info = new ProcessStartInfo(dotnet)
        {
            WorkingDirectory = Path.GetDirectoryName(path)!,
            UseShellExecute = false,
            CreateNoWindow = true,
            RedirectStandardOutput = true,
            RedirectStandardError = true
        };
        foreach (var argument in new[] { "msbuild", path, "-nologo", "-noAutoResponse", "-nodeReuse:false",
                     "-verbosity:quiet", "-getProperty:" + Properties, "-getItem:Compile,ProjectReference" })
            info.ArgumentList.Add(argument);
        if (framework is not null)
            info.ArgumentList.Add("-property:TargetFramework=" + framework);
        info.Environment["DOTNET_NOLOGO"] = "1";
        info.Environment["DOTNET_CLI_TELEMETRY_OPTOUT"] = "1";
        using var command = CancellationTokenSource.CreateLinkedTokenSource(token);
        command.CancelAfter(TimeSpan.FromSeconds(10));
        try
        {
            var result = worker is null ? await RunExplicitAsync(info, command)
                : await worker.RunAsync(info, MaxMetadataBytes, command.Token);
            if (result.ExitCode != 0)
                throw new IOException("MSBuild project evaluation failed.");
            using var json = JsonDocument.Parse(result.StandardOutput);
            var root = json.RootElement;
            var properties = root.GetProperty("Properties").Clone();
            var items = root.GetProperty("Items").Clone();
            if (properties.ValueKind != JsonValueKind.Object ||
                Items(items, "Compile").Any(item => item.ValueKind != JsonValueKind.Object) ||
                Items(items, "ProjectReference").Any(item => item.ValueKind != JsonValueKind.Object))
                throw new IOException("MSBuild evaluation output is invalid.");
            return new Evaluation(properties, items);
        }
        catch (OperationCanceledException) when (!token.IsCancellationRequested)
        {
            throw new IOException("MSBuild evaluation exceeded its output or time budget.");
        }
        catch (Exception ex) when (ex is JsonException or KeyNotFoundException or InvalidOperationException)
        {
            throw new IOException("MSBuild evaluation output is invalid.", ex);
        }
    }

    private static async Task<WorkerResult> RunExplicitAsync(ProcessStartInfo info, CancellationTokenSource command)
    {
        using var process = Process.Start(info) ?? throw new IOException("The project evaluator could not start.");
        try
        {
            var stdout = ReadBoundedAsync(process.StandardOutput, MaxMetadataBytes, command);
            var stderr = ReadBoundedAsync(process.StandardError, 32_768, command);
            await Task.WhenAll(stdout, stderr, process.WaitForExitAsync(command.Token));
            return new WorkerResult(process.ExitCode, await stdout, await stderr);
        }
        finally
        {
            if (!process.HasExited)
            {
                process.Kill(entireProcessTree: true);
                await process.WaitForExitAsync().WaitAsync(TimeSpan.FromSeconds(3));
            }
        }
    }

    private static async Task<string> ReadBoundedAsync(StreamReader reader, int limit, CancellationTokenSource deadline)
    {
        var output = new StringBuilder();
        var buffer = new char[4096];
        while (true)
        {
            var read = await reader.ReadAsync(buffer, deadline.Token);
            if (read == 0)
                return output.ToString();
            if (output.Length + read > limit)
            {
                await deadline.CancelAsync();
                throw new IOException("MSBuild evaluation output exceeded its budget.");
            }
            output.Append(buffer, 0, read);
        }
    }

    private string? FindDotnet()
    {
        var name = OperatingSystem.IsWindows() ? "dotnet.exe" : "dotnet";
        foreach (var directory in (Environment.GetEnvironmentVariable("PATH") ?? "").Split(Path.PathSeparator))
        {
            var path = directory.Trim().Trim('"');
            if (Path.IsPathFullyQualified(path))
            {
                var candidate = Path.GetFullPath(Path.Combine(path, name));
                if (!InsideRoot(candidate) && File.Exists(candidate))
                    return candidate;
            }
        }
        return null;
    }

    private sealed record Evaluation(JsonElement Properties, JsonElement Items);
    private sealed record RawReference(string Path, string? Framework);
    private sealed record RawTarget(ProjectTarget Target, NuGetFramework Framework, RawReference[] References,
        ProjectEvaluationIssue[] Issues, EvaluatedBuildTarget Build);
}
