using System.Diagnostics;
using System.Security.Cryptography;

namespace SlopHawk.Daemon;

// This is one prebuilt VSTest process boundary, not a scheduler, builder, or collector.
// The process lease covers LeasedResultsRoot. Build outputs are bound to a fresh
// collector observation and checked again before runner launch and receipt acceptance.
internal sealed record VstestVerifiedFile(string Path, string Sha256);

internal sealed record VstestAttemptRequest(
    TestAttempt Attempt,
    VstestVerifiedFile DotnetHost,
    VstestVerifiedFile VstestConsole,
    string SdkVersion,
    BuildIdentityStamp? BuiltIdentity,
    BuildArtifactCaptureResult? CurrentBuild,
    VstestVerifiedFile Assembly,
    IReadOnlyList<VstestVerifiedFile> Companions,
    string LeasedResultsRoot,
    TimeSpan Timeout,
    IReadOnlyDictionary<string, string>? Environment = null);

internal enum VstestAttemptGate
{
    Launched, MissingBuildProvenance, DeniedAuthority, InvalidInputs, Cancelled
}

internal sealed record VstestAttemptResult(
    VstestAttemptGate Gate,
    long AttemptId,
    int AttemptNumber,
    string? ResultsDirectory,
    VstestProcessFacts? Process,
    VstestClassification? Classification,
    string Reason,
    string StandardOutput,
    string StandardError);

// A test-only narrow seam substitutes the process scope; production always enters
// through WorkerLease.RunAsync, which owns Windows Job Object enrollment and draining.
internal interface IVstestAttemptExecution
{
    string ProfileRevision { get; }
    void Revalidate();
    void RequireResultsRoot(string path);
    Task<WorkerResult> RunAsync(ProcessStartInfo command, int maxOutput, CancellationToken cancellationToken);
}

internal static class VstestAttemptRunner
{
    internal const int MaxCompanions = 128;
    internal const int MaxOutputCharacters = 1024;
    internal const int MaxEnvironmentEntries = 24;
    private const int MaxWorkerOutputCharacters = 16_384;
    private const long MaxVerifiedFileBytes = 128L * 1024 * 1024;
    private const long MaxVerifiedTotalBytes = 512L * 1024 * 1024;
    private static readonly StringComparison FileComparison = OperatingSystem.IsWindows()
        ? StringComparison.OrdinalIgnoreCase : StringComparison.Ordinal;
    // Explicit process environment, never the daemon's unrestricted environment.
    // No startup hooks, additional dependency roots, or profiler injection.
    private static readonly HashSet<string> AllowedEnvironment = new(StringComparer.OrdinalIgnoreCase)
    {
        "PATH", "SystemRoot", "WINDIR", "TMP", "TEMP", "TMPDIR", "HOME", "USERPROFILE",
        "LANG", "LC_ALL", "TZ", "DOTNET_CLI_HOME"
    };


    internal static Task<VstestAttemptResult> RunAsync(VstestAttemptRequest request,
        WorkerLease worker, CancellationToken cancellationToken) =>
        RunCoreAsync(request, new LeaseExecution(worker), cancellationToken);

    internal static Task<VstestAttemptResult> RunForTestsAsync(VstestAttemptRequest request,
        IVstestAttemptExecution execution, CancellationToken cancellationToken) =>
        RunCoreAsync(request, execution, cancellationToken);

    private static async Task<VstestAttemptResult> RunCoreAsync(VstestAttemptRequest request,
        IVstestAttemptExecution execution, CancellationToken cancellationToken)
    {
        ArgumentNullException.ThrowIfNull(request);
        ArgumentNullException.ThrowIfNull(execution);
        var attempt = request.Attempt;
        ArgumentNullException.ThrowIfNull(attempt);
        VstestAttemptResult Rejected(VstestAttemptGate gate, string reason) =>
            new(gate, attempt.Id, attempt.Number, null, null, null, Excerpt(reason), "", "");

        if (cancellationToken.IsCancellationRequested)
            return Rejected(VstestAttemptGate.Cancelled, "Attempt cancelled before launch.");
        try
        {
            execution.Revalidate();
            if (attempt.ProfileRevision != execution.ProfileRevision)
                throw new UnauthorizedAccessException("The admitted attempt has a different profile revision.");
        }
        catch (Exception ex) when (ex is UnauthorizedAccessException or ObjectDisposedException)
        {
            return Rejected(VstestAttemptGate.DeniedAuthority, "Worker authority was denied before launch.");
        }

        // The SG-03 stamp must be complete, structurally valid, and equal to its
        // independently refreshed observation. A digest string alone cannot grant reuse.
        if (request.BuiltIdentity is null || request.CurrentBuild is null ||
            request.CurrentBuild.TargetId != attempt.Target.Id || request.CurrentBuild.SdkVersion != request.SdkVersion ||
            !BuildOutputIdentity.Compare(request.BuiltIdentity, request.CurrentBuild.Stamp).Reusable)
            return Rejected(VstestAttemptGate.MissingBuildProvenance,
                "No complete, matching byte-verified build provenance was supplied.");

        string root;
        ProcessStartInfo command;
        try
        {
            command = PrepareCommand(request, out root);
            execution.RequireResultsRoot(root);
            if (!MatchesObservedOutputs(request))
                return Rejected(VstestAttemptGate.MissingBuildProvenance,
                    "A test assembly or companion is outside the observed build outputs.");
            VerifyInputs(request, cancellationToken);
        }
        catch (OperationCanceledException) when (cancellationToken.IsCancellationRequested)
        {
            return Rejected(VstestAttemptGate.Cancelled, "Attempt cancelled before launch.");
        }
        catch (Exception ex) when (ex is IOException or ArgumentException or NotSupportedException or
                                   EntryPointNotFoundException or DllNotFoundException)
        {
            return Rejected(VstestAttemptGate.InvalidInputs, "Verified runner, build, or result inputs are invalid: " + ex.Message);
        }
        catch (UnauthorizedAccessException)
        {
            return Rejected(VstestAttemptGate.DeniedAuthority, "Access to a requested input path was denied.");
        }

        // The trusted attempt starts before creation of its fresh results directory.
        // Filesystem modification timestamps can have coarser precision than UtcNow.
        var started = DateTimeOffset.UtcNow;
        string directory;
        try
        {
            // Never reuse an old attempt directory (including one from the same lease).
            directory = Path.Combine(root, $"vstest-{attempt.Id}-{attempt.Number}-{Guid.NewGuid():N}");
            if (Directory.Exists(directory) || File.Exists(directory))
                throw new IOException("The results directory was already allocated.");
            Directory.CreateDirectory(directory);
            VerifyPhysicalDirectory(directory);
            if (Directory.EnumerateFileSystemEntries(directory).Any())
                throw new IOException("A newly allocated attempt directory was not empty.");
            command.ArgumentList.Add("/ResultsDirectory:" + directory);
            execution.RequireResultsRoot(directory);
        }
        catch (Exception ex) when (ex is UnauthorizedAccessException or ObjectDisposedException)
        {
            return Rejected(VstestAttemptGate.DeniedAuthority, "Worker authority was denied before process launch.");
        }
        catch (Exception ex) when (ex is IOException or ArgumentException)
        {
            return Rejected(VstestAttemptGate.InvalidInputs, "Could not reserve a private results directory: " + ex.Message);
        }

        WorkerResult? completed = null;
        var timedOut = false;
        var cancelled = false;
        var startupOrBuildFailed = false;
        using (var deadline = CancellationTokenSource.CreateLinkedTokenSource(cancellationToken))
        {
            deadline.CancelAfter(request.Timeout);
            try
            {
                // This call is the only production process-launch path. A returned
                // WorkerResult attests a confirmed exit AND drained process scope.
                completed = await execution.RunAsync(command, MaxWorkerOutputCharacters, deadline.Token);
            }
            catch (Exception ex) when (ex is UnauthorizedAccessException or ObjectDisposedException)
            {
                return new VstestAttemptResult(VstestAttemptGate.DeniedAuthority, attempt.Id, attempt.Number,
                    directory, null, null, "Worker authority was denied during process launch.", "", "");
            }
            catch (OperationCanceledException) when (cancellationToken.IsCancellationRequested)
            {
                cancelled = true;
            }
            catch (OperationCanceledException) when (deadline.IsCancellationRequested)
            {
                timedOut = true;
            }
            catch (Exception ex) when (ex is IOException or InvalidOperationException or System.ComponentModel.Win32Exception)
            {
                // A failed launch, failed drain, or failed output read does NOT
                // attest an exited process. Never accept even a plausible TRX.
                startupOrBuildFailed = ex is System.ComponentModel.Win32Exception;
            }
        }
        var finished = DateTimeOffset.UtcNow;
        var process = new VstestProcessFacts(attempt.Id, started, finished, false, null,
            completed is not null, completed?.ExitCode, TimedOut: timedOut,
            Cancelled: cancelled, StartupOrBuildFailed: startupOrBuildFailed);

        try
        {
            execution.RequireResultsRoot(directory);
            if (completed is null)
            {
                var missing = VstestTrxClassifier.Parse(null);
                var unconfirmed = timedOut || cancelled || startupOrBuildFailed
                    ? VstestTrxClassifier.Classify(process, missing)
                    : new VstestClassification(VstestDisposition.Incomplete, VstestRunnerHealth.Incomplete,
                        missing, VstestExecution.Unknown, "The worker did not confirm process exit and drainage.");
                return Result(unconfirmed);
            }
            // Output identity and the admitted physical directories must still
            // agree after test code has executed, before evidence is accepted.
            VerifyInputs(request, cancellationToken);
            VerifyPhysicalDirectory(directory);
            var artifact = ReadOneTrx(directory, out var lastWrite);
            process = process with { OwnedResultsDirectory = true, ArtifactLastWriteUtc = lastWrite };
            execution.RequireResultsRoot(directory);
            cancellationToken.ThrowIfCancellationRequested();
            var classification = VstestTrxClassifier.Classify(process, artifact);
            return Result(classification);
        }
        catch (OperationCanceledException) when (cancellationToken.IsCancellationRequested)
        {
            process = process with { Cancelled = true };
            return Result(VstestTrxClassifier.Classify(process, VstestTrxClassifier.Parse(null)));
        }
        catch (Exception ex) when (ex is UnauthorizedAccessException or ObjectDisposedException)
        {
            // Do not publish even a failed test result after authority revocation.
            return new VstestAttemptResult(VstestAttemptGate.DeniedAuthority, attempt.Id, attempt.Number,
                directory, process, null, "Worker authority or verified file access was revoked.", "", "");
        }
        catch (Exception ex) when (ex is IOException or ArgumentException or NotSupportedException or
                                   EntryPointNotFoundException or DllNotFoundException)
        {
            // A raced or changed build input is not a test pass. The metadata
            // remains bounded and does not include the untrusted TRX contents.
            var artifact = Invalid(VstestArtifactHealth.Inconsistent, "Result or input verification failed: " + ex.Message);
            return Result(VstestTrxClassifier.Classify(process, artifact));
        }

        VstestAttemptResult Result(VstestClassification classification) =>
            new(VstestAttemptGate.Launched, attempt.Id, attempt.Number,
                directory, process, classification, Excerpt(classification.Reason),
                Excerpt(completed?.StandardOutput), Excerpt(completed?.StandardError));
    }

    private static ProcessStartInfo PrepareCommand(VstestAttemptRequest request, out string resultsRoot)
    {
        if (request.Attempt.Id <= 0 || request.Attempt.Number <= 0 ||
            string.IsNullOrWhiteSpace(request.Attempt.Target.Id) ||
            string.IsNullOrWhiteSpace(request.Attempt.Target.InputFingerprint) ||
            request.Companions is null || request.Companions.Count is < 2 or > MaxCompanions ||
            request.Timeout < TimeSpan.FromSeconds(1) || request.Timeout > TimeSpan.FromMinutes(30))
            throw new IOException("Attempt, companion, or timeout bounds are invalid.");
        if (!Version.TryParse(request.SdkVersion, out var sdk) || sdk.Major != 10 || sdk.Minor != 0 ||
            sdk.Build < 0 || sdk.Revision >= 0)
            throw new IOException("This slice requires an approved stable .NET 10 SDK.");

        VerifyPhysicalFile(request.DotnetHost.Path);
        VerifyPhysicalFile(request.VstestConsole.Path);
        VerifyPhysicalFile(request.Assembly.Path);
        var host = Path.GetFullPath(request.DotnetHost.Path);
        var runner = Path.GetFullPath(request.VstestConsole.Path);
        var expectedRunner = Path.Combine(Path.GetDirectoryName(host)!, "sdk", request.SdkVersion,
            "vstest.console.dll");
        if (!Path.GetFileName(host).Equals(OperatingSystem.IsWindows() ? "dotnet.exe" : "dotnet", FileComparison) ||
            !runner.Equals(Path.GetFullPath(expectedRunner), FileComparison) ||
            !Path.GetExtension(request.Assembly.Path).Equals(".dll", FileComparison))
            throw new IOException("Runner SDK layout or test assembly is not the approved VSTest target.");

        var assemblyDirectory = Path.GetDirectoryName(Path.GetFullPath(request.Assembly.Path))!;
        VerifyPhysicalDirectory(assemblyDirectory);
        var baseName = Path.GetFileNameWithoutExtension(request.Assembly.Path);
        var companions = request.Companions.Select(file => Path.GetFullPath(file.Path)).ToArray();
        if (companions.Distinct(Workspace.PathComparer).Count() != companions.Length ||
            companions.Any(path => !Path.GetDirectoryName(path)!.Equals(assemblyDirectory, FileComparison)) ||
            companions.Any(path => path.Equals(Path.GetFullPath(request.Assembly.Path), FileComparison)) ||
            !companions.Any(path => Path.GetFileName(path).Equals(baseName + ".deps.json", FileComparison)) ||
            !companions.Any(path => Path.GetFileName(path).Equals(baseName + ".runtimeconfig.json", FileComparison)))
            throw new IOException("An SDK test assembly needs its verified, colocated runtime and dependency outputs.");

        resultsRoot = Path.GetFullPath(request.LeasedResultsRoot);
        VerifyPhysicalDirectory(resultsRoot);
        if (ExecutionProfile.Contains(resultsRoot, assemblyDirectory) ||
            ExecutionProfile.Contains(assemblyDirectory, resultsRoot) ||
            ExecutionProfile.Contains(resultsRoot, Path.GetDirectoryName(host)!) ||
            ExecutionProfile.Contains(Path.GetDirectoryName(host)!, resultsRoot))
            throw new IOException("The results lease must not overlap the SDK or prebuilt test outputs.");

        var command = new ProcessStartInfo
        {
            FileName = host,
            WorkingDirectory = assemblyDirectory,
            UseShellExecute = false,
            CreateNoWindow = true
        };
        command.ArgumentList.Add("exec");
        command.ArgumentList.Add(runner);
        command.ArgumentList.Add(Path.GetFullPath(request.Assembly.Path));
        command.ArgumentList.Add("/Logger:trx;LogFileName=results.trx");
        command.Environment.Clear();
        if (request.Environment is { } environment)
        {
            if (environment.Count > MaxEnvironmentEntries)
                throw new IOException("Too many process environment entries were supplied.");
            foreach (var (key, value) in environment)
            {
                if (string.IsNullOrWhiteSpace(key) || key.Length > 80 || !AllowedEnvironment.Contains(key) ||
                    key.Contains('\0') || value is null || value.Length > 4096 || value.Contains('\0'))
                    throw new IOException("Invalid process environment entry.");
                command.Environment[key] = value;
            }
        }
        // Pin the SDK host's root regardless of caller-provided environment.
        command.Environment["DOTNET_ROOT"] = Path.GetDirectoryName(host)!;
        command.Environment["DOTNET_MULTILEVEL_LOOKUP"] = "0";
        command.Environment["DOTNET_CLI_TELEMETRY_OPTOUT"] = "1";
        if (OperatingSystem.IsWindows())
        {
            // VSTest uses a loopback socket to communicate with the test host.
            // Winsock provider paths contain %SystemRoot%.
            var windows = Environment.GetFolderPath(Environment.SpecialFolder.Windows);
            command.Environment["SystemRoot"] = windows;
            command.Environment["WINDIR"] = windows;
        }
        return command;
    }

    private static bool MatchesObservedOutputs(VstestAttemptRequest request) =>
        request.Companions.Prepend(request.Assembly).All(file => request.CurrentBuild!.Outputs.Any(output =>
            output.Status == BuildArtifactObservationStatus.ByteVerified &&
            Workspace.PathComparer.Equals(output.PhysicalPath, Path.GetFullPath(file.Path)) &&
            string.Equals(output.Sha256, file.Sha256, StringComparison.OrdinalIgnoreCase)));

    private static void VerifyInputs(VstestAttemptRequest request, CancellationToken cancellationToken)
    {
        long bytes = 0;
        var unique = new HashSet<string>(Workspace.PathComparer);
        Span<byte> buffer = stackalloc byte[8192];
        foreach (var file in new[] { request.DotnetHost, request.VstestConsole, request.Assembly }
            .Concat(request.Companions))
        {
            cancellationToken.ThrowIfCancellationRequested();
            if (file is null || !unique.Add(Path.GetFullPath(file.Path)))
                throw new IOException("Verified input paths must be present and unique.");
            VerifyPhysicalFile(file.Path);
            if (file.Sha256 is not { Length: 64 } || file.Sha256.Any(character =>
                character is not (>= '0' and <= '9' or >= 'a' and <= 'f' or >= 'A' and <= 'F')))
                throw new IOException("A required input SHA-256 is missing or malformed.");
            if (!PhysicalArtifactFile.TryPathIdentity(file.Path, expectDirectory: false, out var before))
                throw new IOException("A required runner or build output is not a regular file.");
            using var handle = PhysicalArtifactFile.OpenRead(file.Path);
            if (!PhysicalArtifactFile.TryHandleIdentity(handle, expectDirectory: false, out var opened) || opened != before)
                throw new IOException("A verified input changed while it was opened.");
            using var stream = new FileStream(handle, FileAccess.Read);
            var remaining = stream.Length;
            if (remaining <= 0 || remaining > MaxVerifiedFileBytes ||
                (bytes += remaining) > MaxVerifiedTotalBytes)
                throw new IOException("Verified input outputs exceed the byte budget.");
            using var hasher = IncrementalHash.CreateHash(HashAlgorithmName.SHA256);
            while (remaining > 0)
            {
                cancellationToken.ThrowIfCancellationRequested();
                var chunk = buffer[..(int)Math.Min(buffer.Length, remaining)];
                stream.ReadExactly(chunk);
                hasher.AppendData(chunk);
                remaining -= chunk.Length;
            }
            var hash = Convert.ToHexString(hasher.GetHashAndReset());
            if (stream.ReadByte() != -1 ||
                !PhysicalArtifactFile.TryPathIdentity(file.Path, expectDirectory: false, out var after) || after != opened)
                throw new IOException("A verified input changed during hashing.");
            if (!hash.Equals(file.Sha256, StringComparison.OrdinalIgnoreCase))
                throw new IOException("A verified runner or build output changed on disk.");
        }
    }

    private static VstestTrxArtifact ReadOneTrx(string directory, out DateTimeOffset? lastWrite)
    {
        lastWrite = null;
        VerifyPhysicalDirectory(directory);
        var entries = Directory.EnumerateFileSystemEntries(directory).Take(2).ToArray();
        if (entries.Length == 0)
            return VstestTrxClassifier.Parse(null);
        if (entries.Length != 1 || !Path.GetFileName(entries[0]).Equals("results.trx", FileComparison))
            return Invalid(VstestArtifactHealth.Inconsistent, "Expected exactly one owned results.trx payload.");
        var path = entries[0];
        try
        {
            VerifyPhysicalFile(path);
            if (!PhysicalArtifactFile.TryPathIdentity(path, expectDirectory: false, out var before))
                throw new IOException("The attempt TRX is not a regular file.");
            using var handle = PhysicalArtifactFile.OpenRead(path);
            if (!PhysicalArtifactFile.TryHandleIdentity(handle, expectDirectory: false, out var opened) || before != opened)
                throw new IOException("The attempt TRX changed before reading.");
            var modified = File.GetLastWriteTimeUtc(handle);
            using var stream = new FileStream(handle, FileAccess.Read);
            var length = stream.Length;
            if (length > VstestTrxClassifier.MaxArtifactBytes)
                return Invalid(VstestArtifactHealth.Oversized, "The attempt TRX exceeds the byte budget.", length);
            var bytes = new byte[(int)length];
            stream.ReadExactly(bytes);
            if (stream.ReadByte() != -1)
                throw new IOException("TRX grew during the bounded read.");
            if (stream.Length != length || File.GetLastWriteTimeUtc(handle) != modified ||
                !PhysicalArtifactFile.TryPathIdentity(path, expectDirectory: false, out var after) || opened != after)
                throw new IOException("TRX changed during the bounded read.");
            VerifyPhysicalFile(path);
            lastWrite = new DateTimeOffset(modified, TimeSpan.Zero);
            return VstestTrxClassifier.Parse(bytes); // Hash and classify these same bytes.
        }
        catch (Exception ex) when (ex is IOException or ArgumentException)
        {
            return Invalid(VstestArtifactHealth.Inconsistent, "The owned TRX is unsafe or changed during reading.");
        }
    }

    private static VstestTrxArtifact Invalid(VstestArtifactHealth health, string reason, long length = 0) =>
        new(health, length, null, null, null, null, null, null, false, [], Excerpt(reason));

    private static void VerifyPhysicalDirectory(string directory)
    {
        VerifyComponents(directory, finalDirectory: true);
    }

    private static void VerifyPhysicalFile(string path)
    {
        VerifyComponents(path, finalDirectory: false);
    }

    private static void VerifyComponents(string path, bool finalDirectory)
    {
        if (string.IsNullOrWhiteSpace(path) || !Path.IsPathFullyQualified(path))
            throw new IOException("All runner and artifact paths must be fully qualified.");
        var full = Path.GetFullPath(path);
        var root = Path.GetPathRoot(full)!;
        var current = root;
        var segments = Path.GetRelativePath(root, full).Split(Path.DirectorySeparatorChar,
            StringSplitOptions.RemoveEmptyEntries);
        if (segments.Length == 0)
            throw new IOException("A filesystem root cannot be a runner input or results directory.");
        for (var i = 0; i < segments.Length; i++)
        {
            current = Path.Combine(current, segments[i]);
            var attributes = File.GetAttributes(current);
            var directory = (attributes & FileAttributes.Directory) != 0;
            if ((attributes & FileAttributes.ReparsePoint) != 0 ||
                directory != (i < segments.Length - 1 || finalDirectory))
                throw new IOException("Input or result paths must be physical files/directories without links.");
        }
    }

    private static string Excerpt(string? source)
    {
        if (string.IsNullOrEmpty(source))
            return "";
        return new string(source.AsSpan(0, Math.Min(source.Length, MaxOutputCharacters))
            .ToArray().Select(ch => char.IsControl(ch) && ch is not ('\n' or '\r' or '\t') ? ' ' : ch).ToArray());
    }

    private sealed class LeaseExecution(WorkerLease lease) : IVstestAttemptExecution
    {
        public string ProfileRevision => lease.ProfileRevision;
        public void Revalidate() => lease.Revalidate();
        public void RequireResultsRoot(string path) => lease.RequireTestOutputRoot(path);
        public Task<WorkerResult> RunAsync(ProcessStartInfo command, int maxOutput, CancellationToken cancellationToken) =>
            lease.RunAsync(command, maxOutput, cancellationToken);
    }
}
