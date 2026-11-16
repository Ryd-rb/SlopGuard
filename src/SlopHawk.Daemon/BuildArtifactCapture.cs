using System.Buffers;
using System.Buffers.Binary;
using System.Security.Cryptography;
using System.Text;

namespace SlopHawk.Daemon;

// A manifest describes one *already evaluated* target. It neither discovers inputs nor
// grants permission to open files. An empty collection is an explicit known-empty set;
// null means that the caller has not supplied that part of the dependency closure.
internal sealed record BuildArtifactCompile(string Path, string? Link);
internal sealed record BuildArtifactManifest(
    ProjectTarget Target,
    bool ProjectGraphComplete,
    string? MappingIdentity,
    string? RunnerIdentity,
    string? AdapterIdentity,
    IReadOnlyList<BuildArtifactCompile>? OrderedCompileInputs,
    IReadOnlyList<string>? ReferenceAssemblies,
    IReadOnlyList<string>? RuntimeAssets,
    IReadOnlyList<string>? SettingsFiles,
    IReadOnlyList<string>? AdapterFiles,
    IReadOnlyList<string>? RequiredOutputs);

// Authority and logical-to-physical mapping come from the caller, not from the manifest.
// Every referenced logical name must have exactly one explicitly established mapping.
internal sealed record BuildArtifactPathMapping(string LogicalPath, string PhysicalPath);
internal sealed record BuildArtifactReadScope(
    IReadOnlyList<string> ReadRoots,
    IReadOnlyList<BuildArtifactPathMapping> Mappings,
    bool MappingTrusted,
    bool ImmutableSnapshotVerified = false);

// Total bytes counts BOTH validation reads; an oversized file cannot bypass the bound
// by supplying a false length. These bounds also apply to local fixture captures.
internal sealed record BuildArtifactCaptureLimits(
    long MaxFileBytes = 64L * 1024 * 1024,
    long MaxTotalBytes = 512L * 1024 * 1024,
    int MaxRecords = 4096);

internal enum BuildArtifactObservationStatus
{
    ByteVerified,
    Missing,
    Unreadable,
    ChangedDuringCapture,
    Denied,
    BudgetExceeded
}

internal sealed record BuildArtifactFileEvidence(
    string Field,
    string LogicalPath,
    string? PhysicalPath,
    BuildArtifactObservationStatus Status,
    string? Sha256,
    long? Length,
    DateTime? LastWriteUtc);

internal sealed record BuildArtifactCaptureResult(
    string TargetId, string SdkVersion, BuildIdentityStamp Stamp,
    IReadOnlyList<BuildArtifactFileEvidence> Evidence,
    IReadOnlyList<BuildArtifactFileEvidence> Outputs);

/// <summary>
/// Bounded, read-only capture of an explicit target closure. It never runs MSBuild, discovers
/// paths, or decides whether to reuse outputs. The returned evidence is from a live checkout
/// unless a separately established immutable snapshot was attested by the caller.
/// </summary>
internal static class BuildArtifactCapture
{
    private const int BufferSize = 64 * 1024;
    private const int MaxPathCharacters = 4096;

    // This seam permits deterministic mutation *between* reads in tests. Production callers
    // leave it null; it cannot extend authority or convert failed evidence into a good stamp.
    internal static BuildArtifactCaptureResult Capture(
        BuildArtifactManifest manifest,
        BuildArtifactReadScope scope,
        BuildArtifactCaptureLimits? limits = null,
        CancellationToken cancellationToken = default,
        Action<string>? betweenValidationReads = null)
    {
        ArgumentNullException.ThrowIfNull(manifest);
        ArgumentNullException.ThrowIfNull(manifest.Target);
        ArgumentNullException.ThrowIfNull(scope);
        limits ??= new BuildArtifactCaptureLimits();
        if (limits.MaxFileBytes is <= 0 or > 64L * 1024 * 1024 ||
            limits.MaxTotalBytes is <= 0 or > 512L * 1024 * 1024 ||
            limits.MaxRecords is <= 0 or > 4096)
            throw new ArgumentOutOfRangeException(nameof(limits), "Capture budgets must be within the artifact limits.");
        cancellationToken.ThrowIfCancellationRequested();

        var problems = new List<BuildIdentityProblem>();
        var evidence = new List<BuildArtifactFileEvidence>();
        var mappings = new Dictionary<string, string>(StringComparer.Ordinal);
        var roots = new List<string>();
        var mappingTrusted = scope.MappingTrusted && scope.ReadRoots is not null &&
                             scope.Mappings is not null && scope.ReadRoots.Count > 0 &&
                             scope.ReadRoots.Count <= limits.MaxRecords &&
                             scope.Mappings.Count <= limits.MaxRecords;
        if (!mappingTrusted)
            Add(BuildIdentityProblemKind.UntrustedPath, "capture.mapping.authority");

        if (mappingTrusted)
        {
            foreach (var rawRoot in scope.ReadRoots!)
            {
                cancellationToken.ThrowIfCancellationRequested();
                if (!TryPhysicalPath(rawRoot, out var root) || !IsPhysicalDirectory(root))
                {
                    mappingTrusted = false;
                    Add(BuildIdentityProblemKind.UntrustedPath, "capture.roots");
                    break;
                }
                if (!roots.Contains(root, Workspace.PathComparer))
                    roots.Add(root);
            }
        }
        if (mappingTrusted)
        {
            foreach (var mapping in scope.Mappings!)
            {
                cancellationToken.ThrowIfCancellationRequested();
                if (mapping is null || !TryLogicalPath(mapping.LogicalPath, out var logical) ||
                    !TryPhysicalPath(mapping.PhysicalPath, out var physical) ||
                    !roots.Any(root => IsStrictChild(root, physical)) ||
                    !mappings.TryAdd(logical, physical))
                {
                    mappingTrusted = false;
                    Add(BuildIdentityProblemKind.UntrustedPath, "capture.mapping.entries");
                    break;
                }
            }
        }

        // The mapping section binds both the caller's logical mapping ID and the actual
        // physical paths used in this observation. Merely changing a mapping cannot reuse
        // a stamp even if its destination currently has identical bytes.
        var mappingIdentity = mappingTrusted && !string.IsNullOrEmpty(manifest.MappingIdentity)
            ? manifest.MappingIdentity + "/" + AggregateMapping(roots, mappings)
            : null;
        long totalRead = 0;
        var records = 0;

        BuildArtifactFileEvidence Read(string logicalPath, string field)
        {
            cancellationToken.ThrowIfCancellationRequested();
            BuildArtifactFileEvidence result;
            if (++records > limits.MaxRecords)
                result = Failed(BuildArtifactObservationStatus.BudgetExceeded, null);
            else if (!mappingTrusted || !TryLogicalPath(logicalPath, out var logical) ||
                     !mappings.TryGetValue(logical, out var physical))
                result = Failed(BuildArtifactObservationStatus.Denied, null);
            else
                result = Observe(physical, logical);
            evidence.Add(result);
            if (result.Status != BuildArtifactObservationStatus.ByteVerified)
                Add(result.Status == BuildArtifactObservationStatus.Denied
                    ? BuildIdentityProblemKind.UntrustedPath : BuildIdentityProblemKind.Incomplete,
                    field + "." + result.Status);
            return result;

            BuildArtifactFileEvidence Failed(BuildArtifactObservationStatus status, string? physical) =>
                new(field, logicalPath ?? "", physical, status, null, null, null);

            BuildArtifactFileEvidence Observe(string physical, string logical)
            {
                try
                {
                    // Do not follow any reparse point in the root, ancestors, or leaf.
                    // Check again around each open to detect ordinary replacements.
                    if (!IsPhysicalFile(physical))
                        return Failed(BuildArtifactObservationStatus.Denied, physical);
                    var before = Metadata(physical);
                    if (!PhysicalArtifactFile.TryPathIdentity(physical, expectDirectory: false, out var identity))
                        return Failed(BuildArtifactObservationStatus.Denied, physical);
                    using var first = Open(physical, identity);
                    if (first.Length > limits.MaxFileBytes)
                        return Failed(BuildArtifactObservationStatus.BudgetExceeded, physical);
                    var firstDigest = ReadHash(first);
                    betweenValidationReads?.Invoke(physical);
                    cancellationToken.ThrowIfCancellationRequested();
                    if (!IsPhysicalFile(physical))
                        return Failed(BuildArtifactObservationStatus.Denied, physical);
                    var middle = Metadata(physical);
                    if (!PhysicalArtifactFile.TryPathIdentity(physical, expectDirectory: false, out var middleIdentity) ||
                        identity != middleIdentity)
                        return Failed(BuildArtifactObservationStatus.ChangedDuringCapture, physical);
                    using var second = Open(physical, identity);
                    var secondDigest = ReadHash(second);
                    if (!IsPhysicalFile(physical))
                        return Failed(BuildArtifactObservationStatus.Denied, physical);
                    var after = Metadata(physical);
                    if (before != middle || middle != after ||
                        !PhysicalArtifactFile.TryPathIdentity(physical, expectDirectory: false, out var afterIdentity) ||
                        identity != afterIdentity ||
                        firstDigest != secondDigest ||
                        firstDigest.Length != before.Length ||
                        secondDigest.Length != after.Length)
                        return Failed(BuildArtifactObservationStatus.ChangedDuringCapture, physical);
                    return new BuildArtifactFileEvidence(field, logical, physical,
                        BuildArtifactObservationStatus.ByteVerified,
                        firstDigest.Sha256, firstDigest.Length, before.LastWriteUtc);
                }
                catch (BudgetException)
                {
                    return Failed(BuildArtifactObservationStatus.BudgetExceeded, physical);
                }
                catch (FileNotFoundException)
                {
                    return Failed(BuildArtifactObservationStatus.Missing, physical);
                }
                catch (DirectoryNotFoundException)
                {
                    return Failed(BuildArtifactObservationStatus.Missing, physical);
                }
                catch (Exception ex) when (ex is IOException or UnauthorizedAccessException or
                                           NotSupportedException or System.Security.SecurityException or
                                           DllNotFoundException or EntryPointNotFoundException)
                {
                    return Failed(BuildArtifactObservationStatus.Unreadable, physical);
                }
            }
        }

        (string Sha256, long Length) ReadHash(Stream stream)
        {
            using var hasher = IncrementalHash.CreateHash(HashAlgorithmName.SHA256);
            var buffer = ArrayPool<byte>.Shared.Rent(BufferSize);
            long length = 0;
            try
            {
                while (true)
                {
                    cancellationToken.ThrowIfCancellationRequested();
                    var count = stream.Read(buffer, 0, buffer.Length);
                    if (count == 0)
                        break;
                    if (count > limits.MaxFileBytes - length || count > limits.MaxTotalBytes - totalRead)
                        throw new BudgetException();
                    length += count;
                    totalRead += count;
                    hasher.AppendData(buffer, 0, count);
                }
                return (Convert.ToHexString(hasher.GetHashAndReset()), length);
            }
            finally
            {
                ArrayPool<byte>.Shared.Return(buffer);
            }
        }

        IReadOnlyList<T>? CaptureList<T>(IReadOnlyList<string>? paths, string section,
            Func<string, BuildArtifactFileEvidence, T> create)
        {
            if (paths is null)
                return null;
            if (paths.Count > limits.MaxRecords || paths.Count > 4096)
            {
                Add(BuildIdentityProblemKind.Incomplete, "capture." + section + ".budget");
                return null;
            }
            var list = new List<T>(paths.Count);
            for (var i = 0; i < paths.Count; i++)
            {
                var path = paths[i];
                var observation = Read(path, $"capture.{section}[{i}]");
                list.Add(create(path, observation));
            }
            return list;
        }

        IReadOnlyList<BuildCompileInput>? compile = null;
        if (manifest.OrderedCompileInputs is { } ordered)
        {
            if (ordered.Count > limits.MaxRecords || ordered.Count > 4096)
                Add(BuildIdentityProblemKind.Incomplete, "capture.compile.budget");
            else
            {
                var items = new List<BuildCompileInput>(ordered.Count);
                for (var i = 0; i < ordered.Count; i++)
                {
                    var entry = ordered[i];
                    var result = Read(entry.Path, $"capture.compile[{i}]");
                    items.Add(new BuildCompileInput(entry.Path, entry.Link, result.Sha256));
                }
                compile = items;
            }
        }
        var references = CaptureList(manifest.ReferenceAssemblies, "references",
            (path, found) => new BuildIdentityFile(path, found.Sha256));
        var runtime = CaptureList(manifest.RuntimeAssets, "runtime",
            (path, found) => new BuildIdentityFile(path, found.Sha256));
        var settings = CaptureList(manifest.SettingsFiles, "settings", (path, found) => found);
        var adapter = CaptureList(manifest.AdapterFiles, "adapter", (path, found) => found);
        var outputEvidence = CaptureList(manifest.RequiredOutputs, "outputs", (_, found) => found);
        var outputs = outputEvidence?.Select(found => new BuildDeclaredOutput(found.LogicalPath, found.Sha256,
                found.Status == BuildArtifactObservationStatus.ByteVerified
                    ? BuildOutputObservation.ByteVerified : BuildOutputObservation.Unavailable));

        var settingsDigest = AggregateFiles(settings, "settings", problems);
        var adapterDigest = AggregateFiles(adapter, "adapter", problems);
        var adapterIdentity = adapterDigest is not null && manifest.AdapterIdentity is not null
            ? manifest.AdapterIdentity + "/" + adapterDigest : null;
        var input = new BuildIdentityInputs(manifest.Target, manifest.ProjectGraphComplete,
            mappingIdentity, mappingTrusted,
            scope.ImmutableSnapshotVerified ? BuildCollectionMode.ImmutableSnapshot : BuildCollectionMode.LiveCheckout,
            settingsDigest, manifest.RunnerIdentity, adapterIdentity, compile, references, runtime, outputs?.ToArray());
        var captured = BuildOutputIdentity.Capture(input);
        var allProblems = problems.Concat(captured.Problems).Distinct().Take(BuildOutputIdentity.MaxProblems).ToArray();
        return new BuildArtifactCaptureResult(manifest.Target.Id, manifest.Target.SdkVersion, captured with
        {
            Complete = allProblems.Length == 0,
            Problems = allProblems
        }, evidence.AsReadOnly(), Array.AsReadOnly(outputEvidence?.ToArray() ?? []));

        void Add(BuildIdentityProblemKind kind, string field)
        {
            if (problems.Count >= BuildOutputIdentity.MaxProblems)
                return;
            var problem = new BuildIdentityProblem(kind, field);
            if (!problems.Contains(problem))
                problems.Add(problem);
        }
    }

    private static string? AggregateFiles(IReadOnlyList<BuildArtifactFileEvidence>? files, string section,
        List<BuildIdentityProblem> problems)
    {
        if (files is null)
            return null;
        var sorted = files.OrderBy(file => file.LogicalPath, StringComparer.Ordinal).ToArray();
        var unique = new HashSet<string>(StringComparer.Ordinal);
        if (sorted.Any(file => file.Status != BuildArtifactObservationStatus.ByteVerified ||
                               !TryLogicalPath(file.LogicalPath, out _) || !unique.Add(file.LogicalPath)))
        {
            if (problems.Count < BuildOutputIdentity.MaxProblems)
                problems.Add(new BuildIdentityProblem(BuildIdentityProblemKind.Incomplete,
                    "capture." + section + ".closure"));
            return null;
        }
        using var writer = new MemoryStream();
        WriteText(writer, "SlopHawk/SG-13/" + section + "/v1");
        WriteInt(writer, sorted.Length);
        foreach (var file in sorted)
        {
            WriteText(writer, file.LogicalPath);
            WriteText(writer, file.Sha256!);
            WriteLong(writer, file.Length!.Value);
        }
        return Convert.ToHexString(SHA256.HashData(writer.ToArray()));
    }

    private static string AggregateMapping(IEnumerable<string> roots, Dictionary<string, string> mappings)
    {
        using var writer = new MemoryStream();
        WriteText(writer, "SlopHawk/SG-13/authorized-mapping/v1");
        var orderedRoots = roots.Order(StringComparer.Ordinal).ToArray();
        WriteInt(writer, orderedRoots.Length);
        foreach (var root in orderedRoots)
            WriteText(writer, root);
        WriteInt(writer, mappings.Count);
        foreach (var pair in mappings.OrderBy(entry => entry.Key, StringComparer.Ordinal))
        {
            WriteText(writer, pair.Key);
            WriteText(writer, pair.Value);
        }
        return Convert.ToHexString(SHA256.HashData(writer.ToArray()));
    }

    private static void WriteText(Stream writer, string value)
    {
        var utf8 = Encoding.UTF8.GetBytes(value);
        WriteInt(writer, utf8.Length);
        writer.Write(utf8);
    }

    private static void WriteInt(Stream writer, int value)
    {
        Span<byte> bytes = stackalloc byte[4];
        BinaryPrimitives.WriteInt32BigEndian(bytes, value);
        writer.Write(bytes);
    }

    private static void WriteLong(Stream writer, long value)
    {
        Span<byte> bytes = stackalloc byte[8];
        BinaryPrimitives.WriteInt64BigEndian(bytes, value);
        writer.Write(bytes);
    }

    private static bool TryLogicalPath(string? path, out string normalized)
    {
        normalized = path?.Replace('\\', '/') ?? "";
        return normalized.Length is > 0 and <= MaxPathCharacters &&
               !normalized.StartsWith('/') && !normalized.EndsWith('/') &&
               !normalized.Contains(':') && !normalized.Any(char.IsControl) &&
               normalized.Split('/').All(part => part.Length > 0 && part is not "." and not ".." &&
                    !part.EndsWith(' ') && !part.EndsWith('.'));
    }

    private static bool TryPhysicalPath(string? path, out string full)
    {
        full = "";
        if (string.IsNullOrWhiteSpace(path) || path.Length > MaxPathCharacters ||
            !Path.IsPathFullyQualified(path) || path.Any(char.IsControl) ||
            path.Split(Path.DirectorySeparatorChar).Any(part => part is "." or ".."))
            return false;
        try
        {
            full = Path.TrimEndingDirectorySeparator(Path.GetFullPath(path));
            return full.Length > 0 && full.Length <= MaxPathCharacters &&
                string.Equals(full, Path.TrimEndingDirectorySeparator(path),
                    OperatingSystem.IsWindows() ? StringComparison.OrdinalIgnoreCase : StringComparison.Ordinal);
        }
        catch (Exception ex) when (ex is ArgumentException or NotSupportedException or PathTooLongException)
        {
            return false;
        }
    }

    private static bool IsStrictChild(string root, string path)
    {
        var relative = Path.GetRelativePath(root, path);
        return relative is not "." and not ".." && !Path.IsPathRooted(relative) &&
               !relative.StartsWith(".." + Path.DirectorySeparatorChar, StringComparison.Ordinal);
    }

    private static bool IsPhysicalDirectory(string path)
    {
        try { return IsUnlinkedPath(path, directory: true); }
        catch (Exception ex) when (ex is IOException or UnauthorizedAccessException or
                                   System.Security.SecurityException or ArgumentException) { return false; }
    }

    private static bool IsPhysicalFile(string path)
    {
        try { return IsUnlinkedPath(path, directory: false); }
        catch (FileNotFoundException) { return true; } // Classified as missing by the open.
        catch (DirectoryNotFoundException) { return true; }
    }

    private static bool IsUnlinkedPath(string path, bool directory)
    {
        // Check every ancestor, including the authorized root and its parents. Re-check
        // before/after both opens because paths may be replaced in a mutable checkout.
        var parent = directory ? path : Path.GetDirectoryName(path);
        for (var p = parent; p is not null; p = Path.GetDirectoryName(p))
        {
            var attrs = File.GetAttributes(p);
            if ((attrs & FileAttributes.ReparsePoint) != 0 || (attrs & FileAttributes.Directory) == 0)
                return false;
            if (Path.GetPathRoot(p) == p)
                break;
        }
        if (directory)
            return true;
        var fileAttrs = File.GetAttributes(path);
        return (fileAttrs & (FileAttributes.Directory | FileAttributes.ReparsePoint)) == 0;
    }

    private static (long Length, DateTime LastWriteUtc, DateTime CreationUtc) Metadata(string path) =>
        (new FileInfo(path).Length, File.GetLastWriteTimeUtc(path), File.GetCreationTimeUtc(path));

    private static FileStream Open(string path, PhysicalArtifactFile.FileIdentity identity)
    {
        var handle = PhysicalArtifactFile.OpenRead(path);
        try
        {
            if (!PhysicalArtifactFile.TryHandleIdentity(handle, expectDirectory: false, out var opened) ||
                opened != identity)
                throw new IOException("The artifact changed before it was opened.");
            return new FileStream(handle, FileAccess.Read, BufferSize);
        }
        catch
        {
            handle.Dispose();
            throw;
        }
    }

    private sealed class BudgetException : Exception { }
}
