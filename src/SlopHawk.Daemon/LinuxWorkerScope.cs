using System.ComponentModel;
using System.Diagnostics;
using System.Text.Json;

namespace SlopHawk.Daemon;

// SG-05 is a fixture-only prototype. WorkerAdmission deliberately never calls this class.
// A transient SERVICE, not `systemd-run --scope`, lets the user manager put the
// trusted fixture in the cgroup before exec. No shell or arbitrary worker API exists.
internal enum LinuxScopeDisposition { Unavailable, Drained, Populated, AmbiguousRecovery }
internal enum LinuxCgroupPopulation { Unknown, Missing, Empty, Populated }

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
        if (previous.UnitLoaded)
            throw new IOException("The Linux worker unit name is already in use.");

        // A trusted fixture checks /proc/self/cgroup BEFORE spawning its child.
        // Type=exec and the user manager give enrollment-before-exec semantics.
        await manager.StartFixtureAsync(pending.UnitName, dotnetHost, fixtureAssembly, outputDirectory, token);
        var started = await manager.ObserveAsync(pending.UnitName, null, token);
        if (!started.ManagerAvailable || !started.UnitLoaded ||
            !Guid.TryParseExact(started.InvocationId, "N", out _) ||
            !LinuxSystemdUserManager.IsUnitControlGroup(started.ControlGroup, pending.UnitName) ||
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
        LinuxScopeObservation observed;
        try { observed = await manager.ObserveAsync(identity.UnitName, identity.ControlGroup, token); }
        catch (Exception ex) when (ex is IOException or UnauthorizedAccessException or Win32Exception)
        {
            return LinuxScopeDisposition.Unavailable;
        }
        if (!observed.ManagerAvailable)
            return LinuxScopeDisposition.Unavailable;
        if (!observed.UnitLoaded || observed.InvocationId != identity.InvocationId ||
            // systemd can clear ControlGroup after draining. A missing property
            // is NOT the same as a manager-confirmed empty property.
            observed.ControlGroup is null || (observed.ControlGroup.Length != 0 &&
                observed.ControlGroup != identity.ControlGroup))
            return LinuxScopeDisposition.AmbiguousRecovery;
        if (observed.Population == LinuxCgroupPopulation.Populated)
            return LinuxScopeDisposition.Populated;
        if ((observed.Population is LinuxCgroupPopulation.Missing or LinuxCgroupPopulation.Empty) &&
            (observed.ActiveState is "inactive" or "failed" ||
             observed.ActiveState == "active" && observed.SubState == "exited"))
            return LinuxScopeDisposition.Drained;
        return LinuxScopeDisposition.AmbiguousRecovery;
    }

    internal async Task StopAndDrainAsync(Guid lease, TimeSpan deadline)
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
            // Never signal an unverified or reused name.
            var before = await InspectAsync(lease, token);
            if (before == LinuxScopeDisposition.Drained) return;
            if (before != LinuxScopeDisposition.Populated)
                throw new IOException($"The Linux worker scope cannot be cancelled: {before}.");
            token.ThrowIfCancellationRequested();
            await manager.KillScopeAsync(identity.UnitName, token);
            while (true)
            {
                token.ThrowIfCancellationRequested();
                var disposition = await InspectAsync(lease, token);
                if (disposition == LinuxScopeDisposition.Drained) return;
                if (disposition != LinuxScopeDisposition.Populated)
                    throw new IOException($"The Linux worker scope did not prove drainage: {disposition}.");
                await Task.Delay(30, token);
            }
        }
        catch (OperationCanceledException) when (token.IsCancellationRequested)
        {
            throw new IOException("The Linux worker scope did not prove drainage before its deadline.");
        }
    }
}

// Uses the existing user manager; never modifies host cgroups, starts a manager,
// requests privilege, or moves an already running process into a group.
internal sealed class LinuxSystemdUserManager : ILinuxUserScopeManager
{
    private const string CgroupRoot = "/sys/fs/cgroup";

    internal static bool IsUnitControlGroup(string? cgroup, string unit) =>
        cgroup is not null && cgroup.StartsWith("/", StringComparison.Ordinal) &&
        cgroup.EndsWith("/" + unit, StringComparison.Ordinal) &&
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
        if (result.ExitCode != 0)
            return missing;
        var values = new Dictionary<string, string>(StringComparer.Ordinal);
        foreach (var line in result.Output.Split('\n', StringSplitOptions.RemoveEmptyEntries))
        {
            var separator = line.IndexOf('=');
            if (separator <= 0 || !values.TryAdd(line[..separator], line[(separator + 1)..].TrimEnd('\r')))
                return missing;
        }
        if (!values.TryGetValue("LoadState", out var load) || load is not ("loaded" or "not-found"))
            return missing;
        values.TryGetValue("InvocationID", out var invocation);
        values.TryGetValue("ControlGroup", out var group);
        values.TryGetValue("ActiveState", out var active);
        values.TryGetValue("SubState", out var sub);
        var loaded = load == "loaded";
        var population = loaded ? ReadPopulation(IsUnitControlGroup(group, unitName) ? group : knownControlGroup, unitName)
            : LinuxCgroupPopulation.Unknown;
        return new LinuxScopeObservation(true, loaded, invocation, group, active, sub, population);
    }

    public async Task StartFixtureAsync(string unitName, string dotnetHost, string fixtureAssembly,
        string outputDirectory, CancellationToken token)
    {
        if (!File.Exists(dotnetHost) || Path.GetFileName(dotnetHost) != "dotnet" ||
            Path.GetFileName(fixtureAssembly) != "SlopHawk.WorkerFixture.dll" ||
            !IsSafeUnit(unitName))
            throw new UnauthorizedAccessException("Only the trusted fixture is accepted.");
        var args = new[] { "--user", $"--unit={unitName}", "--expand-environment=no", "--property=Type=exec",
            "--property=KillMode=control-group", "--property=SendSIGKILL=yes", "--property=Restart=no",
            "--property=RuntimeMaxSec=45s",
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

    internal async Task RemoveDrainedFixtureAsync(LinuxScopeIdentity identity, CancellationToken token)
    {
        identity.Validate();
        var observed = await ObserveAsync(identity.UnitName, identity.ControlGroup, token);
        if (observed.ManagerAvailable && !observed.UnitLoaded)
            return;
        if (!observed.ManagerAvailable || identity.InvocationId is null ||
            observed.InvocationId != identity.InvocationId ||
            observed.ControlGroup is null || (observed.ControlGroup != "" && observed.ControlGroup != identity.ControlGroup) ||
            observed.Population is not (LinuxCgroupPopulation.Empty or LinuxCgroupPopulation.Missing) ||
            !(observed.ActiveState is "inactive" or "failed" ||
                observed.ActiveState == "active" && observed.SubState == "exited"))
            throw new IOException("The drained Linux fixture unit could not be verified for removal.");

        var result = await RunAsync("/usr/bin/systemctl", ["--user", "stop", identity.UnitName], token);
        if (result.ExitCode != 0)
            throw new IOException("The user manager could not remove the drained fixture unit: " + result.Error);
    }

    private static bool IsSafeUnit(string unit) =>
        unit.StartsWith("slopguard-fixture-", StringComparison.Ordinal) && unit.EndsWith(".service", StringComparison.Ordinal) &&
        Guid.TryParseExact(unit["slopguard-fixture-".Length..^8], "N", out _);

    private static LinuxCgroupPopulation ReadPopulation(string? group, string unit)
    {
        if (!IsUnitControlGroup(group, unit)) return LinuxCgroupPopulation.Unknown;
        var absolute = Path.GetFullPath(Path.Combine(CgroupRoot, group![1..]));
        if (!absolute.StartsWith(CgroupRoot + "/", StringComparison.Ordinal))
            return LinuxCgroupPopulation.Unknown;
        try
        {
            var attributes = File.GetAttributes(absolute);
            if ((attributes & (FileAttributes.Directory | FileAttributes.ReparsePoint)) != FileAttributes.Directory)
                return LinuxCgroupPopulation.Unknown;
            var lines = File.ReadAllLines(Path.Combine(absolute, "cgroup.events"));
            var values = lines.Where(line => line.StartsWith("populated ", StringComparison.Ordinal)).ToArray();
            return values.Length == 1 ? values[0] switch
            {
                "populated 0" => LinuxCgroupPopulation.Empty,
                "populated 1" => LinuxCgroupPopulation.Populated,
                _ => LinuxCgroupPopulation.Unknown
            } : LinuxCgroupPopulation.Unknown;
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
            var stdout = process.StandardOutput.ReadToEndAsync(limit.Token);
            var stderr = process.StandardError.ReadToEndAsync(limit.Token);
            await process.WaitForExitAsync(limit.Token);
            var output = await stdout;
            var error = await stderr;
            if (output.Length > 16384 || error.Length > 16384)
                throw new IOException("A systemd manager response exceeded its output budget.");
            return (process.ExitCode, output, error);
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
}
