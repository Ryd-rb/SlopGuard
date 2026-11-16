using System.Diagnostics;
using System.Security.Cryptography;
using System.Text.Json;
using System.Xml;

namespace SlopHawk.Daemon;

// SG-14 data is deliberately transient. Nothing in these records is added to ProjectGraph,
// the inventory schema, or a durable scheduler receipt.
internal enum TestBuildBlocker
{
    EvaluationUnavailable,
    IncompleteGraph,
    UnknownTarget,
    UnsupportedTarget,
    UnresolvedReference,
    MismatchedEvaluation,
    UnverifiedSdk,
    UnknownWrites,
    UnsupportedImports,
    MissingOutputMetadata,
    MissingAssets,
    IncompatibleAssets,
    OutputCollision,
    DeniedRoots
}

internal sealed record EvaluatedBuildTarget(
    string TargetId, string Framework, string Configuration, string Platform, string RuntimeIdentifier,
    string SdkVersion, string OutputDirectory, string IntermediateDirectory,
    string BaseIntermediateDirectory, string TargetAssembly, string AssetsFile,
    IReadOnlyList<string> Imports)
{
    internal bool UsesMicrosoftTestingPlatform { get; init; }
    internal bool HasUnknownWrites { get; init; }
    internal bool HasUnsupportedImports { get; init; }
    internal string? OutDir { get; init; }
    internal string? OutputPath { get; init; }
    internal string? CustomBeforeMicrosoftCommonTargets { get; init; }
    internal string? CustomAfterMicrosoftCommonTargets { get; init; }
}

internal sealed record BuildEvaluationSnapshot(ProjectGraph Graph, string RootIdentity, string DotnetPath,
    IReadOnlyDictionary<string, EvaluatedBuildTarget> Targets);

internal sealed record TestBuildCommand(string Executable, string WorkingDirectory, IReadOnlyList<string> Arguments)
{
    // Conversion is inert; admission and process launching belong to a later slice.
    internal ProcessStartInfo ToStartInfo()
    {
        var start = new ProcessStartInfo(Executable)
        {
            WorkingDirectory = WorkingDirectory,
            UseShellExecute = false,
            CreateNoWindow = true,
            RedirectStandardOutput = true,
            RedirectStandardError = true
        };
        foreach (var argument in Arguments)
            start.ArgumentList.Add(argument);
        start.Environment["DOTNET_NOLOGO"] = "1";
        start.Environment["DOTNET_CLI_TELEMETRY_OPTOUT"] = "1";
        return start;
    }
}

internal sealed record ExpectedBuildAssets(string TargetId, string Path, string Sha256, string AssetTarget);

internal sealed record PlannedBuildTarget(string TargetId, string ProjectPath, string Framework,
    string Configuration, string Platform, string RuntimeIdentifier, string OutputDirectory,
    string IntermediateDirectory, string BaseIntermediateDirectory, string TargetAssembly);

internal sealed record TestBuildPlan(string SelectedTargetId, IReadOnlyList<PlannedBuildTarget> InvokedTargets,
    IReadOnlyList<ExpectedBuildAssets> Assets, string AttemptResultsRoot,
    WorkerRequirements Requirements, TestBuildCommand Command);

internal sealed record TestBuildPreparation(TestBuildPlan? Plan, TestBuildBlocker? Blocker, string? Detail)
{
    internal bool Ready => Plan is not null && Blocker is null;

    internal static TestBuildPreparation Blocked(TestBuildBlocker reason, string? detail = null) =>
        new(null, reason, detail);
}

internal sealed class TestBuildPlanner(Workspace workspace)
{
    private const long MaxAssetsBytes = 16 * 1024 * 1024;
    private const int MaxInvokedTargets = 128;

    internal TestBuildPreparation Prepare(ProjectGraph graph, string selectedTargetId, string attemptResultsRoot,
        ExecutionGrant grant, BuildEvaluationSnapshot? snapshot)
    {
        ArgumentNullException.ThrowIfNull(graph);
        ArgumentNullException.ThrowIfNull(grant);
        string currentIdentity;
        try
        {
            currentIdentity = RootIdentity.Read(workspace.Root);
        }
        catch (Exception ex) when (ex is IOException or UnauthorizedAccessException)
        {
            return TestBuildPreparation.Blocked(TestBuildBlocker.EvaluationUnavailable, ex.Message);
        }
        if (snapshot is null || !ReferenceEquals(graph, snapshot.Graph) ||
            !string.Equals(grant.RootIdentity, snapshot.RootIdentity, StringComparison.Ordinal) ||
            currentIdentity != snapshot.RootIdentity)
            return TestBuildPreparation.Blocked(TestBuildBlocker.EvaluationUnavailable, "evaluated snapshot or root changed");
        if (!graph.Complete)
            return TestBuildPreparation.Blocked(TestBuildBlocker.IncompleteGraph,
                string.Join(",", graph.Issues.Select(issue => issue.Reason)));

        var index = new Dictionary<string, ProjectTarget>(StringComparer.Ordinal);
        foreach (var target in graph.Targets)
            if (!index.TryAdd(target.Id, target))
                return TestBuildPreparation.Blocked(TestBuildBlocker.UnresolvedReference, "duplicate target ID");
        if (!index.TryGetValue(selectedTargetId, out var selected))
            return TestBuildPreparation.Blocked(TestBuildBlocker.UnknownTarget, selectedTargetId);
        if (!selected.IsTestProject || !string.Equals(selected.Framework, "net10.0", StringComparison.OrdinalIgnoreCase))
            return TestBuildPreparation.Blocked(TestBuildBlocker.UnsupportedTarget, "only an SDK-style net10.0 VSTest target is supported");

        // Walk only IDs already resolved by ProjectGraph.BuildGraph. Do not reselect a
        // nearest framework or invent a referenced project context at planning time.
        var visited = new Dictionary<string, int>(StringComparer.Ordinal);
        var ordered = new List<ProjectTarget>();
        TestBuildPreparation? error = null;
        bool Visit(ProjectTarget current)
        {
            if (visited.TryGetValue(current.Id, out var state))
            {
                if (state == 1)
                    error = TestBuildPreparation.Blocked(TestBuildBlocker.UnresolvedReference, "cyclic project references");
                return state == 2;
            }
            if (visited.Count >= MaxInvokedTargets)
            {
                error = TestBuildPreparation.Blocked(TestBuildBlocker.UnresolvedReference, "target limit exceeded");
                return false;
            }
            visited.Add(current.Id, 1);
            foreach (var id in current.References)
            {
                if (!index.TryGetValue(id, out var referenced))
                {
                    error = TestBuildPreparation.Blocked(TestBuildBlocker.UnresolvedReference, id);
                    return false;
                }
                if (!Visit(referenced))
                    return false;
            }
            visited[current.Id] = 2;
            ordered.Add(current);
            return true;
        }
        if (!Visit(selected))
            return error ?? TestBuildPreparation.Blocked(TestBuildBlocker.UnresolvedReference);

        if (!TrySdk(snapshot.DotnetPath, selected.SdkVersion, out var sdkDirectory))
            return TestBuildPreparation.Blocked(TestBuildBlocker.UnverifiedSdk, snapshot.DotnetPath);
        string resultsRoot;
        try
        {
            if (!Path.IsPathFullyQualified(attemptResultsRoot))
                return TestBuildPreparation.Blocked(TestBuildBlocker.MissingOutputMetadata, "attempt results root must be absolute");
            resultsRoot = AbsoluteDirectory(attemptResultsRoot, workspace.Root);
            Workspace.CheckDirectory(resultsRoot, allowMissing: true);
        }
        catch (Exception ex) when (ex is ArgumentException or IOException or UnauthorizedAccessException or NotSupportedException)
        {
            return TestBuildPreparation.Blocked(TestBuildBlocker.MissingOutputMetadata, "attempt results: " + ex.Message);
        }

        var outputs = new List<PlannedBuildTarget>();
        var expectedAssets = new List<ExpectedBuildAssets>();
        var roots = new List<string> { resultsRoot };
        foreach (var current in ordered)
        {
            if (!snapshot.Targets.TryGetValue(current.Id, out var evaluated))
                return TestBuildPreparation.Blocked(TestBuildBlocker.MismatchedEvaluation, current.Id);
            if (current.SdkVersion != selected.SdkVersion ||
                current.Framework != evaluated.Framework || current.Configuration != evaluated.Configuration ||
                current.Platform != evaluated.Platform || current.RuntimeIdentifier != evaluated.RuntimeIdentifier ||
                current.SdkVersion != evaluated.SdkVersion ||
                !SafeProperty(current.Configuration) || !SafeProperty(current.Platform) ||
                current.RuntimeIdentifier.Length > 0 && !SafeProperty(current.RuntimeIdentifier))
                return TestBuildPreparation.Blocked(TestBuildBlocker.MismatchedEvaluation, current.Id);
            if (evaluated.UsesMicrosoftTestingPlatform ||
                current.IsTestProject && !current.Framework.Equals("net10.0", StringComparison.OrdinalIgnoreCase))
                return TestBuildPreparation.Blocked(TestBuildBlocker.UnsupportedTarget, current.Id);
            if (evaluated.HasUnknownWrites)
                return TestBuildPreparation.Blocked(TestBuildBlocker.UnknownWrites, current.Id);
            if (evaluated.HasUnsupportedImports)
                return TestBuildPreparation.Blocked(TestBuildBlocker.UnsupportedImports, current.Id + ": directory import override");
            if (!ExpectedSdkHook(evaluated.CustomBeforeMicrosoftCommonTargets, sdkDirectory,
                    "Custom.Before.Microsoft.Common.targets") ||
                !ExpectedSdkHook(evaluated.CustomAfterMicrosoftCommonTargets, sdkDirectory,
                    "Custom.After.Microsoft.Common.targets"))
                return TestBuildPreparation.Blocked(TestBuildBlocker.UnsupportedImports, current.Id + ": SDK hook override");
            if (current.Framework.Length == 0 || current.ProjectPath.Length == 0 ||
                !Path.GetExtension(current.ProjectPath).Equals(".csproj", StringComparison.OrdinalIgnoreCase))
                return TestBuildPreparation.Blocked(TestBuildBlocker.UnsupportedTarget, current.Id);

            string projectPath, baseIntermediate, intermediate, output, assembly, assets;
            try
            {
                projectPath = AbsoluteFile(current.ProjectPath, workspace.Root);
                if (!workspace.IsIncluded(projectPath) || !File.Exists(projectPath))
                    return TestBuildPreparation.Blocked(TestBuildBlocker.UnsupportedTarget, current.ProjectPath);
                var projectDirectory = Path.GetDirectoryName(projectPath)!;
                output = AbsoluteDirectory(evaluated.OutputDirectory, projectDirectory);
                intermediate = AbsoluteDirectory(evaluated.IntermediateDirectory, projectDirectory);
                baseIntermediate = AbsoluteDirectory(evaluated.BaseIntermediateDirectory, projectDirectory);
                assembly = AbsoluteFile(evaluated.TargetAssembly, projectDirectory);
                assets = AbsoluteFile(evaluated.AssetsFile, projectDirectory);
                if (!ExecutionProfile.Contains(output, assembly) || output == assembly ||
                    !ExecutionProfile.Contains(baseIntermediate, intermediate) ||
                    !ExecutionProfile.Contains(baseIntermediate, assets) ||
                    Path.GetExtension(assembly) is not (".dll" or ".exe"))
                    return TestBuildPreparation.Blocked(TestBuildBlocker.UnknownWrites, $"{current.Id} paths: out={output} asm={assembly} intermediate={intermediate} base={baseIntermediate} assets={assets}");
                if (evaluated.OutDir is { Length: > 0 } outDir &&
                    !Workspace.PathComparer.Equals(output, AbsoluteDirectory(outDir, projectDirectory)))
                    return TestBuildPreparation.Blocked(TestBuildBlocker.UnknownWrites, "OutDir: " + current.Id + " expected=" + output + " observed=" + AbsoluteDirectory(outDir, projectDirectory));
                if (evaluated.OutputPath is { Length: > 0 } outputPath &&
                    !Workspace.PathComparer.Equals(output, AbsoluteDirectory(outputPath, projectDirectory)))
                    return TestBuildPreparation.Blocked(TestBuildBlocker.UnknownWrites, "OutputPath: " + current.Id + " expected=" + output + " observed=" + AbsoluteDirectory(outputPath, projectDirectory));
                foreach (var directory in new[] { projectDirectory, output, intermediate, baseIntermediate })
                    Workspace.CheckDirectory(directory, allowMissing: true);
                var projectBlocker = InspectProject(projectPath);
                if (projectBlocker is { } problem)
                    return TestBuildPreparation.Blocked(problem, current.ProjectPath + ": unsupported project build logic");
                if (!SafeImports(evaluated.Imports, sdkDirectory, baseIntermediate, projectPath))
                    return TestBuildPreparation.Blocked(TestBuildBlocker.UnsupportedImports,
                        current.Id + ": unrecognized import " + string.Join(";", evaluated.Imports));
            }
            catch (XmlException ex)
            {
                return TestBuildPreparation.Blocked(TestBuildBlocker.UnsupportedImports, current.Id + ": " + ex.Message);
            }
            catch (Exception ex) when (ex is IOException or ArgumentException or UnauthorizedAccessException or NotSupportedException)
            {
                return TestBuildPreparation.Blocked(TestBuildBlocker.MissingOutputMetadata, current.Id + ": " + ex.Message);
            }

            var (asset, assetError) = ReadAssets(assets, projectPath, workspace.Root, baseIntermediate, current, index);
            if (assetError is not null)
                return assetError;
            expectedAssets.Add(asset!);
            outputs.Add(new PlannedBuildTarget(current.Id, projectPath, current.Framework, current.Configuration,
                current.Platform, current.RuntimeIdentifier, output, intermediate, baseIntermediate, assembly));
            roots.AddRange([output, intermediate, baseIntermediate]);
        }

        // A build command invokes all references inside one worker lease. Two different
        // projects must never own the same or nested build tree; distinct TFMs of one
        // project may share the base intermediate directory but not an assembly path.
        for (var i = 0; i < outputs.Count; i++)
        {
            var left = outputs[i];
            if (Overlaps(resultsRoot, left.OutputDirectory) || Overlaps(resultsRoot, left.BaseIntermediateDirectory))
                return TestBuildPreparation.Blocked(TestBuildBlocker.OutputCollision, "results overlap " + left.TargetId);
            for (var j = 0; j < i; j++)
            {
                var right = outputs[j];
                if (Workspace.PathComparer.Equals(left.TargetAssembly, right.TargetAssembly) ||
                    !Workspace.PathComparer.Equals(left.ProjectPath, right.ProjectPath) &&
                    (Overlaps(left.OutputDirectory, right.OutputDirectory) ||
                     Overlaps(left.BaseIntermediateDirectory, right.BaseIntermediateDirectory) ||
                     Overlaps(left.OutputDirectory, right.BaseIntermediateDirectory) ||
                     Overlaps(left.BaseIntermediateDirectory, right.OutputDirectory)))
                    return TestBuildPreparation.Blocked(TestBuildBlocker.OutputCollision, left.TargetId + " / " + right.TargetId);
            }
        }

        WorkerRequirements requirements;
        try
        {
            requirements = new WorkerRequirements(ExecutionOperation.Build | ExecutionOperation.CompilerExtensions,
                UsesProcesses: true, ExecutionProfile.NormalizeRoots(workspace, roots));
            grant.Require(requirements);
        }
        catch (Exception ex) when (ex is UnauthorizedAccessException or IOException or ArgumentException or NotSupportedException)
        {
            return TestBuildPreparation.Blocked(TestBuildBlocker.DeniedRoots, ex.Message);
        }

        var command = new TestBuildCommand(snapshot.DotnetPath, Path.GetDirectoryName(outputs[^1].ProjectPath)!,
        [
            "exec", Path.Combine(sdkDirectory, "MSBuild.dll"), outputs[^1].ProjectPath, "-t:Build",
            "-p:TargetFramework=" + selected.Framework,
            "-p:Configuration=" + selected.Configuration, "-p:Platform=" + selected.Platform,
            .. (selected.RuntimeIdentifier.Length == 0 ? [] : new[] { "-p:RuntimeIdentifier=" + selected.RuntimeIdentifier }),
            "-p:UseSharedCompilation=false", "-nologo", "-noAutoResponse", "-nodeReuse:false"
        ]);
        return new TestBuildPreparation(new TestBuildPlan(selected.Id, outputs, expectedAssets, resultsRoot,
            requirements, command), null, null);
    }

    private static bool Overlaps(string a, string b) =>
        ExecutionProfile.Contains(a, b) || ExecutionProfile.Contains(b, a);

    private static bool SafeProperty(string value) =>
        !string.IsNullOrWhiteSpace(value) && value.Length <= 64 &&
        value.All(ch => char.IsAsciiLetterOrDigit(ch) || ch is '.' or '-' or '_' or ' ');

    private static string AbsoluteFile(string path, string baseDirectory)
    {
        if (string.IsNullOrWhiteSpace(path) || path.Contains("$(", StringComparison.Ordinal) ||
            path.Contains("@(", StringComparison.Ordinal) || path.Contains('*') || path.Contains('?'))
            throw new IOException("An evaluated path is missing or not concrete.");
        var normalized = path.Replace('\\', Path.DirectorySeparatorChar);
        return Path.GetFullPath(Path.IsPathFullyQualified(normalized) ? normalized : Path.Combine(baseDirectory, normalized));
    }

    private static string AbsoluteDirectory(string path, string baseDirectory) =>
        Path.TrimEndingDirectorySeparator(AbsoluteFile(path, baseDirectory));

    private static bool TrySdk(string dotnet, string version, out string sdkDirectory)
    {
        sdkDirectory = string.Empty;
        if (string.IsNullOrWhiteSpace(dotnet) || !Path.IsPathFullyQualified(dotnet) ||
            !Version.TryParse(version, out var parsed) || parsed.Major != 10 || version.Contains(Path.DirectorySeparatorChar) ||
            version.Contains(Path.AltDirectorySeparatorChar) || !File.Exists(dotnet) ||
            !Path.GetFileName(dotnet).Equals(OperatingSystem.IsWindows() ? "dotnet.exe" : "dotnet", StringComparison.Ordinal) ||
            (File.GetAttributes(dotnet) & FileAttributes.ReparsePoint) != 0)
            return false;
        sdkDirectory = Path.GetFullPath(Path.Combine(Path.GetDirectoryName(dotnet)!, "sdk", version));
        try
        {
            Workspace.CheckDirectory(sdkDirectory);
            return File.Exists(Path.Combine(sdkDirectory, "MSBuild.dll"));
        }
        catch (Exception ex) when (ex is IOException or UnauthorizedAccessException)
        {
            return false;
        }
    }

    private static bool ExpectedSdkHook(string? value, string sdkDirectory, string fileName) =>
        string.IsNullOrEmpty(value) || Workspace.PathComparer.Equals(
            Path.GetFullPath(value), Path.Combine(sdkDirectory, "vCurrent", fileName));

    private static bool SafeImports(IEnumerable<string> imports, string sdkDirectory, string obj, string projectPath)
    {
        // MSBuildAllProjects may omit an early imported NuGet .props file even
        // when it reports the corresponding generated .targets file. Inspect
        // both physical manifests rather than assuming the reported list is exhaustive.
        foreach (var suffix in new[] { ".nuget.g.props", ".nuget.g.targets" })
        {
            var manifest = Path.Combine(obj, Path.GetFileName(projectPath) + suffix);
            if (File.Exists(manifest) && !SafeGeneratedImport(manifest))
                return false;
        }
        foreach (var item in imports)
        {
            if (string.IsNullOrWhiteSpace(item))
                continue;
            if (!Path.IsPathFullyQualified(item) || item.Contains("$(", StringComparison.Ordinal))
                return false;
            var path = Path.GetFullPath(item);
            var generated = ExecutionProfile.Contains(obj, path) &&
                (path.EndsWith(".nuget.g.props", StringComparison.OrdinalIgnoreCase) ||
                 path.EndsWith(".nuget.g.targets", StringComparison.OrdinalIgnoreCase));
            if (!ExecutionProfile.Contains(sdkDirectory, path) && !generated &&
                !Workspace.PathComparer.Equals(projectPath, path))
                return false;
            if (!File.Exists(path) || (File.GetAttributes(path) & FileAttributes.ReparsePoint) != 0)
                return false;
            Workspace.CheckDirectory(Path.GetDirectoryName(path)!);
            if (generated && !SafeGeneratedImport(path))
                return false;
        }
        return true;
    }

    private static bool SafeGeneratedImport(string import)
    {
        if (new FileInfo(import).Length > 2 * 1024 * 1024)
            return false;
        var settings = new XmlReaderSettings { DtdProcessing = DtdProcessing.Prohibit, XmlResolver = null,
            MaxCharactersInDocument = 2 * 1024 * 1024 };
        using var reader = XmlReader.Create(import, settings);
        if (!reader.ReadToFollowing("Project"))
            return false;
        while (reader.Read())
        {
            if (reader.NodeType == XmlNodeType.Element &&
                reader.LocalName is "Import" or "ImportGroup" or "UsingTask" or "Target" or "OnError" or "Sdk")
                return false;
        }
        return true;
    }

    private static TestBuildBlocker? InspectProject(string projectFile)
    {
        if ((File.GetAttributes(projectFile) & (FileAttributes.Directory | FileAttributes.ReparsePoint)) != 0 ||
            new FileInfo(projectFile).Length > 2 * 1024 * 1024)
            return TestBuildBlocker.UnsupportedImports;
        var settings = new XmlReaderSettings { DtdProcessing = DtdProcessing.Prohibit, XmlResolver = null, MaxCharactersInDocument = 2 * 1024 * 1024 };
        using var reader = XmlReader.Create(projectFile, settings);
        if (!reader.ReadToFollowing("Project") || reader.GetAttribute("Sdk") != "Microsoft.NET.Sdk")
            return TestBuildBlocker.UnsupportedImports;
        while (reader.Read())
        {
            if (reader.NodeType != XmlNodeType.Element)
                continue;
            if (reader.LocalName is "Import" or "ImportGroup" or "UsingTask" or "Target" or "OnError" or "Sdk")
                return TestBuildBlocker.UnsupportedImports;
            if (reader.LocalName is "Content" or "None" or "EmbeddedResource" or "Resource" or "Copy" or
                "Move" or "Exec" or "WriteLinesToFile")
                return TestBuildBlocker.UnknownWrites;
        }
        return null;
    }

    private static (ExpectedBuildAssets? Asset, TestBuildPreparation? Error) ReadAssets(string path, string projectPath,
        string workspaceRoot, string baseIntermediate, ProjectTarget target,
        IReadOnlyDictionary<string, ProjectTarget> graph)
    {
        if (!File.Exists(path))
            return (null, TestBuildPreparation.Blocked(TestBuildBlocker.MissingAssets, path));
        try
        {
            Workspace.CheckDirectory(Path.GetDirectoryName(path)!);
            if ((File.GetAttributes(path) & FileAttributes.ReparsePoint) != 0)
                return (null, TestBuildPreparation.Blocked(TestBuildBlocker.IncompatibleAssets, path));
            using var handle = PhysicalArtifactFile.OpenRead(path);
            if (!PhysicalArtifactFile.TryHandleIdentity(handle, expectDirectory: false, out var before) ||
                !PhysicalArtifactFile.TryPathIdentity(path, expectDirectory: false, out var observed) || before != observed)
                return (null, TestBuildPreparation.Blocked(TestBuildBlocker.IncompatibleAssets, path));
            using var stream = new FileStream(handle, FileAccess.Read);
            var length = stream.Length;
            if (length is <= 0 or > MaxAssetsBytes)
                return (null, TestBuildPreparation.Blocked(TestBuildBlocker.IncompatibleAssets, path));
            var modified = File.GetLastWriteTimeUtc(handle);
            var bytes = new byte[(int)length];
            stream.ReadExactly(bytes);
            if (stream.ReadByte() != -1 || File.GetLastWriteTimeUtc(handle) != modified ||
                !PhysicalArtifactFile.TryPathIdentity(path, expectDirectory: false, out var after) || before != after)
                return (null, TestBuildPreparation.Blocked(TestBuildBlocker.IncompatibleAssets, path + " changed"));
            using var document = JsonDocument.Parse(bytes);
            var root = document.RootElement;
            if (root.GetProperty("version").GetInt32() is not (3 or 4))
                return (null, TestBuildPreparation.Blocked(TestBuildBlocker.IncompatibleAssets, path + " version"));
            var project = root.GetProperty("project");
            var restore = project.GetProperty("restore");
            var restoredPath = restore.GetProperty("projectPath").GetString();
            if (!Workspace.PathComparer.Equals(Path.GetFullPath(restoredPath ?? ""), projectPath))
                return (null, TestBuildPreparation.Blocked(TestBuildBlocker.IncompatibleAssets, path + " project"));
            var restoredOutput = restore.GetProperty("outputPath").GetString();
            if (!Workspace.PathComparer.Equals(AbsoluteDirectory(restoredOutput ?? "", Path.GetDirectoryName(projectPath)!),
                    baseIntermediate))
                return (null, TestBuildPreparation.Blocked(TestBuildBlocker.IncompatibleAssets, path + " intermediate output"));
            if (!project.GetProperty("frameworks").TryGetProperty(target.Framework, out _))
                return (null, TestBuildPreparation.Blocked(TestBuildBlocker.IncompatibleAssets, path + " project frameworks"));
            var frameworks = restore.GetProperty("frameworks");
            if (!frameworks.TryGetProperty(target.Framework, out var framework))
                return (null, TestBuildPreparation.Blocked(TestBuildBlocker.IncompatibleAssets, path + " framework"));
            var assetTarget = target.Framework + (target.RuntimeIdentifier.Length == 0 ? "" : "/" + target.RuntimeIdentifier);
            if (!root.GetProperty("targets").TryGetProperty(assetTarget, out _))
                return (null, TestBuildPreparation.Blocked(TestBuildBlocker.IncompatibleAssets, path + " target"));
            var referenced = new HashSet<string>(Workspace.PathComparer);
            foreach (var id in target.References)
                referenced.Add(Path.GetFullPath(Path.Combine(workspaceRoot,
                    graph[id].ProjectPath.Replace('/', Path.DirectorySeparatorChar))));
            // Assets project references are absolute project paths. The graph reference
            // path is relative to workspace root, not the current project directory.
            var actual = framework.GetProperty("projectReferences").EnumerateObject()
                .Select(item => Path.GetFullPath(item.Name)).ToHashSet(Workspace.PathComparer);
            if (!referenced.SetEquals(actual))
                return (null, TestBuildPreparation.Blocked(TestBuildBlocker.IncompatibleAssets, path + " references"));
            var hash = Convert.ToHexString(SHA256.HashData(bytes));
            var data = new ExpectedBuildAssets(target.Id, path, hash, assetTarget);
            return (data, null);
        }
        catch (Exception ex) when (ex is IOException or JsonException or InvalidOperationException or ArgumentException or
                                   UnauthorizedAccessException or KeyNotFoundException or NotSupportedException or
                                   EntryPointNotFoundException or DllNotFoundException)
        {
            return (null, TestBuildPreparation.Blocked(TestBuildBlocker.IncompatibleAssets, path + ": " + ex.Message));
        }
    }
}
