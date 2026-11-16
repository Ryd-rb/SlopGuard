using System.ComponentModel;
using System.Diagnostics;
using System.Runtime.InteropServices;
using System.Text;
using System.Text.Json;
using Microsoft.Win32.SafeHandles;

namespace SlopHawk.Daemon;

// SG-05 is a fixture-only prototype. WorkerAdmission deliberately never calls this class.
// A transient SERVICE, not `systemd-run --scope`, lets the user manager put the
// trusted fixture in the cgroup before exec. No shell or arbitrary worker API exists.
internal enum LinuxScopeDisposition { Unavailable, Drained, Populated, AmbiguousRecovery }
// Missing is not equivalent to an observed cgroup.events "populated 0".
internal enum LinuxCgroupPopulation { Unknown, Missing, Empty, Populated, Removed }

// A drain proof is issued only after the same invocation and its cgroup were
// checked empty. It is not a reservation release or a production admission API.
internal sealed record LinuxScopeDrainProof(LinuxScopeIdentity Identity);

internal sealed record LinuxScopeIdentity(Guid Lease, string UnitName, string BootId,
    string? InvocationId = null, string? ControlGroup = null)
{
    internal static LinuxScopeIdentity New(Guid lease, string bootId) =>
        new(lease, $"slopguard-fixture-{lease:N}.service", bootId);

    internal void Validate()
    {
        if (Lease == Guid.Empty || UnitName != $"slopguard-fixture-{Lease:N}.service" ||
            !Guid.TryParse(BootId, out _) || (InvocationId is null) != (ControlGroup is null) ||
            (InvocationId is not null && (!Guid.TryParseExact(InvocationId, "N", out _) ||
                ControlGroup!.Length > 2048 ||
                !LinuxSystemdUserManager.IsUnitControlGroup(ControlGroup, UnitName))))
            throw new IOException("The persisted Linux worker scope identity is invalid.");
    }
}

// Stored before asking the manager to start anything. An unacknowledged launch is
// ambiguous after a crash; no daemon may infer drainage from PID disappearance.
internal sealed class LinuxScopeJournal(string directory)
{
    private string FileName(Guid lease) => Path.Combine(directory, $"linux-scope-{lease:N}.json");

    internal void SavePending(LinuxScopeIdentity identity) => Save(identity, createNew: true);

    internal void Save(LinuxScopeIdentity identity, bool createNew = false)
    {
        identity.Validate();
        Workspace.CreatePrivateDirectory(directory);
        var path = FileName(identity.Lease);
        if (File.Exists(path) && (File.GetAttributes(path) & FileAttributes.ReparsePoint) != 0)
            throw new IOException("The worker scope journal is not a physical file.");
        var temporary = path + "." + Guid.NewGuid().ToString("N") + ".tmp";
        try
        {
            using (var stream = new FileStream(temporary, FileMode.CreateNew, FileAccess.Write,
                FileShare.None, 4096, FileOptions.WriteThrough))
            {
                JsonSerializer.Serialize(stream, identity);
                stream.Flush(flushToDisk: true);
            }
            File.Move(temporary, path, overwrite: !createNew);
        }
        finally
        {
            if (File.Exists(temporary)) File.Delete(temporary);
        }
    }

    internal LinuxScopeIdentity Read(Guid lease)
    {
        Workspace.CheckDirectory(directory);
        var path = FileName(lease);
        if ((File.GetAttributes(path) & FileAttributes.ReparsePoint) != 0)
            throw new IOException("The worker scope journal is not a physical file.");
        using var stream = new FileStream(path, FileMode.Open, FileAccess.Read, FileShare.Read);
        if (stream.Length is <= 0 or > 4096)
            throw new IOException("The worker scope journal is invalid or oversized.");
        LinuxScopeIdentity identity;
        try { identity = JsonSerializer.Deserialize<LinuxScopeIdentity>(stream)!; }
        catch (JsonException ex) { throw new IOException("The worker scope journal is unreadable.", ex); }
        if (identity is null || identity.Lease != lease)
            throw new IOException("The worker scope journal has a different lease.");
        identity.Validate();
        return identity;
    }
}

internal sealed record LinuxScopeObservation(bool ManagerAvailable, bool UnitLoaded,
    string? InvocationId, string? ControlGroup, string? ActiveState, string? SubState,
    LinuxCgroupPopulation Population);

// The manager boundary is intentionally small so failed queries and races are
// deterministic in tests. The systemd implementation only talks to --user.
internal interface ILinuxUserScopeManager
{
    Task<LinuxScopeObservation> ObserveAsync(string unitName, string? knownControlGroup, CancellationToken token);
    Task StartFixtureAsync(string unitName, string dotnetHost, string fixtureAssembly,
        string outputDirectory, CancellationToken token);
    Task KillScopeAsync(string unitName, CancellationToken token);
    ILinuxCgroupWitness? PinCgroup(LinuxScopeIdentity identity);
}

internal interface ILinuxCgroupWitness : IDisposable
{
    LinuxCgroupPopulation Read();
}

internal sealed class LinuxWorkerScope(ILinuxUserScopeManager manager, LinuxScopeJournal journal, string bootId)
{
    internal async Task<LinuxScopeIdentity> StartFixtureAsync(Guid lease, string dotnetHost,
        string fixtureAssembly, string outputDirectory, CancellationToken token)
    {
        if (!OperatingSystem.IsLinux() || !Path.IsPathFullyQualified(dotnetHost) ||
            Path.GetFileName(dotnetHost) != "dotnet" || !File.Exists(dotnetHost) ||
            !Path.IsPathFullyQualified(fixtureAssembly) ||
            Path.GetFileName(fixtureAssembly) != "SlopHawk.WorkerFixture.dll" ||
            !File.Exists(fixtureAssembly) || !Path.IsPathFullyQualified(outputDirectory) ||
            !Directory.Exists(outputDirectory))
            throw new UnauthorizedAccessException("SG-05 can run only the installed trusted Linux test fixture.");

        var pending = LinuxScopeIdentity.New(lease, bootId);
        pending.Validate();
        journal.SavePending(pending); // Must precede the first request that could execute a child.
        var previous = await manager.ObserveAsync(pending.UnitName, null, token);
        if (!previous.ManagerAvailable)
            throw new NotSupportedException("The user systemd manager or cgroup delegation is unavailable.");
        if (!LinuxSystemdUserManager.IsAbsent(previous))
            throw new IOException("The Linux worker unit name is already in use or its prior state is ambiguous.");

        // A trusted fixture checks /proc/self/cgroup BEFORE spawning its child.
        // Type=exec and the user manager give enrollment-before-exec semantics.
        await manager.StartFixtureAsync(pending.UnitName, dotnetHost, fixtureAssembly, outputDirectory, token);
        var started = await manager.ObserveAsync(pending.UnitName, null, token);
        if (!started.ManagerAvailable || !started.UnitLoaded ||
            !Guid.TryParseExact(started.InvocationId, "N", out _) ||
            !LinuxSystemdUserManager.IsUnitControlGroup(started.ControlGroup, pending.UnitName) ||
            started.ActiveState != "active" || started.SubState != "running" ||
            started.Population != LinuxCgroupPopulation.Populated)
            throw new IOException("The Linux fixture scope could not be positively identified after enrollment.");

        var identity = pending with { InvocationId = started.InvocationId, ControlGroup = started.ControlGroup };
        journal.Save(identity);
        return identity;
    }

    internal async Task<LinuxScopeDisposition> InspectAsync(Guid lease, CancellationToken token)
    {
        LinuxScopeIdentity identity;
        try { identity = journal.Read(lease); }
        catch (Exception ex) when (ex is IOException or UnauthorizedAccessException)
        {
            return LinuxScopeDisposition.AmbiguousRecovery;
        }
        if (identity.BootId != bootId || identity.InvocationId is null)
            return LinuxScopeDisposition.AmbiguousRecovery;
        var observed = await ObserveAsync(identity, token);
        if (observed is not { ManagerAvailable: true })
            return LinuxScopeDisposition.Unavailable;
        if (!Matches(identity, observed) || observed.ControlGroup != identity.ControlGroup)
            return LinuxScopeDisposition.AmbiguousRecovery;
        if (observed.Population == LinuxCgroupPopulation.Populated)
            return LinuxScopeDisposition.Populated;
        if (observed.Population == LinuxCgroupPopulation.Empty && IsTerminal(observed))
            return LinuxScopeDisposition.Drained;
        return LinuxScopeDisposition.AmbiguousRecovery;
    }

    internal async Task<LinuxScopeDrainProof> StopAndDrainAsync(Guid lease, TimeSpan deadline)
    {
        var identity = journal.Read(lease);
        if (identity.BootId != bootId || identity.InvocationId is null)
            throw new IOException("The Linux worker scope has no verified start identity.");
        if (deadline <= TimeSpan.Zero)
            throw new IOException("The Linux worker scope cancellation deadline has elapsed.");
        using var budget = new CancellationTokenSource(deadline);
        var token = budget.Token;
        try
        {
            var before = await ObserveAsync(identity, token);
            if (!Matches(identity, before) || before.ControlGroup != identity.ControlGroup)
                throw new IOException("The Linux worker scope has no matching manager identity for cancellation.");
            using var witness = manager.PinCgroup(identity);
            if (witness is null)
                throw new IOException("The live worker cgroup could not be pinned before cancellation.");

            // Recheck the manager after opening the cgroup. systemctl still
            // signals by name, so this fixture protocol has no atomic signal fence.
            before = await ObserveAsync(identity, token);
            if (!Matches(identity, before) || before.ControlGroup != identity.ControlGroup)
                throw new IOException("The worker identity changed before cancellation.");
            var population = witness.Read();
            if (ProvesDrainage(identity, before, population))
                return new LinuxScopeDrainProof(identity);
            if (population != LinuxCgroupPopulation.Populated || before.ActiveState != "active" ||
                before.SubState != "running" || before.Population != LinuxCgroupPopulation.Populated)
                throw new IOException("The verified worker scope is not in a cancellable state.");

            token.ThrowIfCancellationRequested();
            await manager.KillScopeAsync(identity.UnitName, token);
            while (true)
            {
                token.ThrowIfCancellationRequested();
                var observed = await ObserveAsync(identity, token);
                if (!Matches(identity, observed))
                    throw new IOException("The worker identity changed or became unavailable during drainage.");
                if (ProvesDrainage(identity, observed, witness.Read()))
                    return new LinuxScopeDrainProof(identity);
                // The kernel and manager can report their transitions separately.
                await Task.Delay(30, token);
            }
        }
        catch (OperationCanceledException) when (token.IsCancellationRequested)
        {
            throw new IOException("The Linux worker scope did not prove drainage before its deadline.");
        }
    }

    private async Task<LinuxScopeObservation?> ObserveAsync(LinuxScopeIdentity identity, CancellationToken token)
    {
        try { return await manager.ObserveAsync(identity.UnitName, identity.ControlGroup, token); }
        catch (Exception ex) when (ex is IOException or UnauthorizedAccessException or Win32Exception)
        {
            return null;
        }
    }

    private static bool Matches(LinuxScopeIdentity identity,
        [System.Diagnostics.CodeAnalysis.NotNullWhen(true)] LinuxScopeObservation? observed) =>
        observed is { ManagerAvailable: true, UnitLoaded: true } &&
        observed.InvocationId == identity.InvocationId &&
        (observed.ControlGroup == identity.ControlGroup || observed.ControlGroup == "");

    internal static bool IsTerminal(LinuxScopeObservation observed) =>
        (observed.ActiveState, observed.SubState) is ("inactive", "dead") or ("failed", "failed") or ("active", "exited");

    private static bool ProvesDrainage(LinuxScopeIdentity identity, LinuxScopeObservation observed,
        LinuxCgroupPopulation witnessed) =>
        IsTerminal(observed) && (witnessed switch
        {
            LinuxCgroupPopulation.Empty => observed.ControlGroup == identity.ControlGroup &&
                observed.Population == LinuxCgroupPopulation.Empty,
            LinuxCgroupPopulation.Removed => observed.Population == LinuxCgroupPopulation.Missing,
            _ => false
        });
}

// Uses the existing user manager; never modifies host cgroups, starts a manager,
// requests privilege, or moves an already running process into a group.
internal sealed class LinuxSystemdUserManager : ILinuxUserScopeManager
{
    private const string CgroupRoot = "/sys/fs/cgroup";

    internal static bool IsUnitControlGroup(string? cgroup, string unit) =>
        cgroup is not null && cgroup.StartsWith("/", StringComparison.Ordinal) &&
        cgroup.EndsWith("/" + unit, StringComparison.Ordinal) &&
        cgroup.Length <= 2048 && !cgroup.Contains("//", StringComparison.Ordinal) &&
        !cgroup.Any(char.IsControl) &&
        !cgroup.Split('/').Any(part => part == ".." || part == ".");

    public async Task<LinuxScopeObservation> ObserveAsync(string unitName, string? knownControlGroup, CancellationToken token)
    {
        var missing = new LinuxScopeObservation(false, false, null, null, null, null,
            LinuxCgroupPopulation.Unknown);
        if (!OperatingSystem.IsLinux() || !File.Exists(Path.Combine(CgroupRoot, "cgroup.controllers")))
            return missing;
        (int ExitCode, string Output, string Error) manager;
        (int ExitCode, string Output, string Error) result;
        try
        {
            manager = await RunAsync("/usr/bin/systemctl", ["--user", "show", "--all", "--no-pager",
                "--property=Version"], token);
            if (manager.ExitCode != 0) return missing;
            result = await RunAsync("/usr/bin/systemctl", ["--user", "show", "--all", "--no-pager",
                "--property=LoadState,ActiveState,SubState,InvocationID,ControlGroup", unitName], token);
        }
        catch (Exception ex) when (ex is IOException or Win32Exception or UnauthorizedAccessException)
        {
            return missing; // A failed manager/helper probe is not evidence of a drained scope.
        }
        if (result.ExitCode != 0 ||
            !manager.Output.Split('\n').Any(line => line.StartsWith("Version=", StringComparison.Ordinal) &&
                !string.IsNullOrWhiteSpace(line["Version=".Length..])))
            return missing;
        return ParseObservation(result.Output, unitName, knownControlGroup);
    }

    // Parsed independently for deterministic malformed/partial manager replies.
    // Only a complete response from a successful --user query is considered.
    internal static LinuxScopeObservation ParseObservation(string output, string unitName, string? knownControlGroup)
    {
        var missing = new LinuxScopeObservation(false, false, null, null, null, null,
            LinuxCgroupPopulation.Unknown);
        var values = new Dictionary<string, string>(StringComparer.Ordinal);
        foreach (var line in output.Split('\n', StringSplitOptions.RemoveEmptyEntries))
        {
            var separator = line.IndexOf('=');
            if (separator <= 0 || !values.TryAdd(line[..separator], line[(separator + 1)..].TrimEnd('\r')))
                return missing;
        }
        if (!values.TryGetValue("LoadState", out var load) || load is not ("loaded" or "not-found") ||
            !values.TryGetValue("InvocationID", out var invocation) ||
            !values.TryGetValue("ControlGroup", out var group) ||
            !values.TryGetValue("ActiveState", out var active) ||
            !values.TryGetValue("SubState", out var sub))
            return missing; // Incomplete manager response is not a negative probe.
        var loaded = load == "loaded";
        if (!loaded && (active != "inactive" || sub != "dead" || invocation.Length != 0 || group.Length != 0))
            return missing;
        if (loaded && (string.IsNullOrWhiteSpace(active) || string.IsNullOrWhiteSpace(sub) ||
            (invocation.Length != 0 && (!Guid.TryParseExact(invocation, "N", out var id) || id == Guid.Empty)) ||
            (group.Length != 0 && !IsUnitControlGroup(group, unitName))))
            return missing;
        var population = loaded ? ReadPopulation(group.Length == 0 ? knownControlGroup : group, unitName)
            : LinuxCgroupPopulation.Unknown;
        return new LinuxScopeObservation(true, loaded, invocation, group, active, sub, population);
    }

    internal static bool IsAbsent(LinuxScopeObservation observed) =>
        observed is { ManagerAvailable: true, UnitLoaded: false, InvocationId: "", ControlGroup: "",
            ActiveState: "inactive", SubState: "dead", Population: LinuxCgroupPopulation.Unknown };

    public async Task StartFixtureAsync(string unitName, string dotnetHost, string fixtureAssembly,
        string outputDirectory, CancellationToken token)
    {
        if (!File.Exists(dotnetHost) || Path.GetFileName(dotnetHost) != "dotnet" ||
            Path.GetFileName(fixtureAssembly) != "SlopHawk.WorkerFixture.dll" ||
            !IsSafeUnit(unitName))
            throw new UnauthorizedAccessException("Only the trusted fixture is accepted.");
        var args = new[] { "--user", $"--unit={unitName}", "--expand-environment=no", "--property=Type=exec",
            "--property=KillMode=control-group", "--property=SendSIGKILL=yes", "--property=Restart=no",
            "--property=RuntimeMaxSec=45s", "--property=TimeoutStopSec=5s",
            "--remain-after-exit", $"--working-directory={Path.GetDirectoryName(fixtureAssembly)}", "--",
            dotnetHost, fixtureAssembly, "linux-scoped-tree", outputDirectory, unitName };
        var result = await RunAsync("/usr/bin/systemd-run", args, token);
        if (result.ExitCode != 0)
            throw new IOException("The user manager could not start the fixture scope: " + result.Error);
    }

    public async Task KillScopeAsync(string unitName, CancellationToken token)
    {
        if (!IsSafeUnit(unitName)) throw new IOException("Invalid Linux worker unit name.");
        var result = await RunAsync("/usr/bin/systemctl", ["--user", "kill", "--kill-whom=all",
            "--signal=SIGKILL", unitName], token);
        if (result.ExitCode != 0)
            throw new IOException("The user manager could not signal the entire worker scope: " + result.Error);
    }

    internal async Task RemoveDrainedFixtureAsync(LinuxScopeDrainProof proof, CancellationToken token)
    {
        var identity = proof.Identity;
        identity.Validate();
        if (identity.InvocationId is null || !OperatingSystem.IsLinux() ||
            !string.Equals(File.ReadAllText("/proc/sys/kernel/random/boot_id").Trim(), identity.BootId,
                StringComparison.OrdinalIgnoreCase))
            throw new IOException("Only a same-boot positively drained fixture can be removed.");
        var observed = await ObserveAsync(identity.UnitName, identity.ControlGroup, token);
        if (!observed.ManagerAvailable)
            throw new IOException("The user manager is unavailable during fixture removal.");
        if (IsAbsent(observed))
            return;
        if (!observed.UnitLoaded || observed.InvocationId != identity.InvocationId ||
            (observed.ControlGroup != "" && observed.ControlGroup != identity.ControlGroup) ||
            observed.Population is not (LinuxCgroupPopulation.Empty or LinuxCgroupPopulation.Missing) ||
            !LinuxWorkerScope.IsTerminal(observed))
            throw new IOException("The drained Linux fixture unit could not be verified for removal.");
        if (observed.ActiveState == "inactive")
            return;

        // A failed unit stays loaded after stop. Reset only this reverified fixture.
        var command = observed.ActiveState == "failed" ? "reset-failed" : "stop";
        var result = await RunAsync("/usr/bin/systemctl", ["--user", command, identity.UnitName], token);
        if (result.ExitCode != 0)
            throw new IOException("The user manager could not remove the drained fixture unit: " + result.Error);
    }

    internal static bool IsSafeUnit(string unit) =>
        unit.StartsWith("slopguard-fixture-", StringComparison.Ordinal) && unit.EndsWith(".service", StringComparison.Ordinal) &&
        Guid.TryParseExact(unit["slopguard-fixture-".Length..^8], "N", out var lease) && lease != Guid.Empty;

    public ILinuxCgroupWitness? PinCgroup(LinuxScopeIdentity identity)
    {
        var directory = CgroupDirectory(identity.ControlGroup, identity.UnitName);
        return directory is null ? null : LinuxPopulationWitness.Open(directory);
    }

    private static string? CgroupDirectory(string? group, string unit)
    {
        if (!IsUnitControlGroup(group, unit)) return null;
        var absolute = Path.GetFullPath(Path.Combine(CgroupRoot, group![1..]));
        return absolute.StartsWith(CgroupRoot + "/", StringComparison.Ordinal) ? absolute : null;
    }

    internal static LinuxCgroupPopulation ParsePopulation(string contents)
    {
        var lines = contents.Split('\n');
        var values = lines.Where(line => line.StartsWith("populated ", StringComparison.Ordinal)).ToArray();
        return values.Length == 1 ? values[0].TrimEnd('\r') switch
        {
            "populated 0" => LinuxCgroupPopulation.Empty,
            "populated 1" => LinuxCgroupPopulation.Populated,
            _ => LinuxCgroupPopulation.Unknown
        } : LinuxCgroupPopulation.Unknown;
    }

    private static LinuxCgroupPopulation ReadPopulation(string? group, string unit)
    {
        var absolute = CgroupDirectory(group, unit);
        if (absolute is null) return LinuxCgroupPopulation.Unknown;
        try
        {
            var attributes = File.GetAttributes(absolute);
            if ((attributes & (FileAttributes.Directory | FileAttributes.ReparsePoint)) != FileAttributes.Directory)
                return LinuxCgroupPopulation.Unknown;
            return ParsePopulation(File.ReadAllText(Path.Combine(absolute, "cgroup.events")));
        }
        catch (Exception ex) when (ex is FileNotFoundException or DirectoryNotFoundException)
        {
            return LinuxCgroupPopulation.Missing;
        }
        catch (Exception ex) when (ex is IOException or UnauthorizedAccessException)
        {
            return LinuxCgroupPopulation.Unknown;
        }
    }

    private static async Task<(int ExitCode, string Output, string Error)> RunAsync(
        string executable, IReadOnlyList<string> arguments, CancellationToken token)
    {
        var command = new ProcessStartInfo(executable)
        {
            UseShellExecute = false, RedirectStandardOutput = true, RedirectStandardError = true,
            RedirectStandardInput = false, CreateNoWindow = true
        };
        foreach (var argument in arguments) command.ArgumentList.Add(argument);
        using var limit = CancellationTokenSource.CreateLinkedTokenSource(token);
        limit.CancelAfter(TimeSpan.FromSeconds(5));
        using var process = Process.Start(command) ?? throw new IOException($"Could not start {executable}.");
        try
        {
            var stdout = ReadBoundedAsync(process.StandardOutput, limit.Token);
            var stderr = ReadBoundedAsync(process.StandardError, limit.Token);
            await process.WaitForExitAsync(limit.Token);
            await Task.WhenAll(stdout, stderr);
            return (process.ExitCode, await stdout, await stderr);
        }
        catch (OperationCanceledException ex) when (!token.IsCancellationRequested)
        {
            throw new IOException("The systemd manager did not respond within five seconds.", ex);
        }
        finally
        {
            if (!process.HasExited)
            {
                try { process.Kill(); } // Only the helper, never a transient service.
                catch (InvalidOperationException) { /* It exited between observation and kill. */ }
                await process.WaitForExitAsync();
            }
        }
    }

    private static async Task<string> ReadBoundedAsync(StreamReader reader, CancellationToken token)
    {
        const int maxCharacters = 16_384;
        var text = new StringBuilder();
        var buffer = new char[1024];
        var oversized = false;
        int count;
        while ((count = await reader.ReadAsync(buffer.AsMemory(), token)) != 0)
        {
            oversized |= text.Length + count > maxCharacters;
            if (!oversized)
                text.Append(buffer, 0, count);
        }
        if (oversized)
            throw new IOException("A systemd manager response exceeded its output budget.");
        return text.ToString();
    }
}

// Pin the cgroup directory, then sample through that descriptor. A fresh
// kernfs events read can fail once systemd deletes the directory. On cgroup v2,
// successful removal itself proves the previously pinned group was unpopulated
// and had no live children (Linux kernel cgroup_destroy_locked).
internal sealed class LinuxPopulationWitness : ILinuxCgroupWitness
{
    private const int CloseOnExec = 0x80000;
    private const int DirectoryOnly = 0x10000;
    private const int NoFollow = 0x20000;
    private const int PathOnly = 0x200000;
    private const long CgroupV2Magic = 0x63677270;
    private readonly SafeFileHandle handle;
    private readonly string directory;
    private readonly string procLink;

    private LinuxPopulationWitness(SafeFileHandle handle, string directory)
    {
        this.handle = handle;
        this.directory = directory;
        procLink = FormattableString.Invariant($"/proc/self/fd/{handle.DangerousGetHandle().ToInt64()}");
    }

    internal static LinuxPopulationWitness? Open(string directory)
    {
        if (!OperatingSystem.IsLinux()) return null;
        var descriptor = OpenPath(directory, PathOnly | DirectoryOnly | NoFollow | CloseOnExec);
        if (descriptor < 0) return null;
        var handle = new SafeFileHandle((IntPtr)descriptor, ownsHandle: true);
        LinuxPopulationWitness? result = null;
        try
        {
            if (StatFs(handle, out var info) != 0 || info.Type != CgroupV2Magic)
                return null;
            var witness = new LinuxPopulationWitness(handle, directory);
            if (new FileInfo(witness.procLink).LinkTarget != directory ||
                witness.Read() is not (LinuxCgroupPopulation.Populated or LinuxCgroupPopulation.Empty))
                return null;
            result = witness;
            return result;
        }
        catch (Exception ex) when (ex is IOException or UnauthorizedAccessException)
        {
            return null;
        }
        finally
        {
            if (result is null) handle.Dispose();
        }
    }

    public LinuxCgroupPopulation Read()
    {
        try
        {
            if (new FileInfo(procLink).LinkTarget == directory)
            {
                try
                {
                    var population = LinuxSystemdUserManager.ParsePopulation(
                        File.ReadAllText(Path.Combine(procLink, "cgroup.events")));
                    if (population != LinuxCgroupPopulation.Unknown)
                        return population;
                }
                catch (IOException)
                {
                    // Removal can race the kernel read of cgroup.events.
                }
            }

            // A lookup also revalidates stale dentries on another cgroup mount.
            // Require the name to be absent and the retained dentry to be unlinked.
            try { File.GetAttributes(directory); }
            catch (Exception ex) when (ex is FileNotFoundException or DirectoryNotFoundException)
            {
                if (new FileInfo(procLink).LinkTarget == directory + " (deleted)")
                    return LinuxCgroupPopulation.Removed;
            }
            return LinuxCgroupPopulation.Unknown;
        }
        catch (Exception ex) when (ex is IOException or UnauthorizedAccessException)
        {
            return LinuxCgroupPopulation.Unknown;
        }
        finally
        {
            GC.KeepAlive(handle);
        }
    }

    public void Dispose() => handle.Dispose();

    // Read only f_type, leaving enough space for the native statfs structure.
    [StructLayout(LayoutKind.Explicit, Size = 256)]
    private struct LinuxStatFs
    {
        [FieldOffset(0)] public nint Type;
    }

    [DllImport("libc", EntryPoint = "open", SetLastError = true)]
    private static extern int OpenPath([MarshalAs(UnmanagedType.LPUTF8Str)] string path, int flags);

    [DllImport("libc", EntryPoint = "fstatfs", SetLastError = true)]
    private static extern int StatFs(SafeFileHandle handle, out LinuxStatFs info);
}
