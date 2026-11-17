using System.Diagnostics;
using System.Runtime.InteropServices;
using System.Text.Json;
using SlopHawk.Core;
using SlopHawk.Daemon;

namespace SlopHawk.WorkerFixture;

internal static class Program
{
    private static async Task<int> Main(string[] args)
    {
        switch (args)
        {
            case ["probe-batch" or "alternate-probe-batch", var input]:
                var requests = JsonSerializer.Deserialize<ProbeInput[]>(await File.ReadAllTextAsync(input))!;
                Console.WriteLine(JsonSerializer.Serialize(requests.Select(request => request.Operation switch
                {
                    "steady" => 2,
                    "changes" => requests.Length > 4 ? 2 : 3,
                    "disappears" => requests.Length > 4 ? 2 : 1,
                    "new-difference" => requests.Length > 4 ? 1 : 2,
                    _ => request.Args[0]
                })));
                return 0;
            case ["owner", var workspace, var state, var output]:
                DaemonHost.Run(new Workspace(workspace, state), new ProcessFixtureAdapter(output), CancellationToken.None);
                return 0;
            case ["tree", var directory]:
                using (var child = Process.Start(Commands.Create("child", Path.Combine(directory, "child.json"))))
                {
                    if (child is null)
                        throw new InvalidOperationException("The process-tree fixture could not start its child.");
                    RecordProcess(Path.Combine(directory, "parent.json"));
                    await child.WaitForExitAsync();
                    return child.ExitCode;
                }
            case ["child", var marker]:
                RecordProcess(marker);
                await Task.Delay(Timeout.InfiniteTimeSpan);
                return 0;
            case ["linux-scope-owner-pending", var pendingScopeId, var pendingDirectory]:
                // A separate short-lived owner is interrupted after the durable
                // reservation, before a request to the manager. No service is
                // started and a replacement owner must not infer drainage.
                if (!Guid.TryParseExact(pendingScopeId, "N", out var pendingLease))
                    return 43;
                var pendingBoot = File.ReadAllText("/proc/sys/kernel/random/boot_id").Trim();
                new LinuxScopeJournal(pendingDirectory).SavePending(LinuxScopeIdentity.New(pendingLease, pendingBoot));
                return 0;
            case ["linux-scope-owner", var scopeId, var journalDirectory, var scopeOutput]:
                // Test a service which survives the launching owner's process exit.
                if (!Guid.TryParseExact(scopeId, "N", out var lease))
                    return 42;
                var launch = Commands.Create("tree", scopeOutput);
                var boot = File.ReadAllText("/proc/sys/kernel/random/boot_id").Trim();
                using (var acknowledgment = new CancellationTokenSource(TimeSpan.FromSeconds(8)))
                    await new LinuxWorkerScope(new LinuxSystemdUserManager(),
                        new LinuxScopeJournal(journalDirectory), boot).StartFixtureAsync(
                            lease, launch.FileName, typeof(Commands).Assembly.Location,
                            scopeOutput, acknowledgment.Token);
                return 0;
            case ["linux-scoped-tree", var scopedDirectory, var unit]:
                // This is the only command accepted by SG-05's transient service.
                // Fail before starting descendants if systemd did not enroll us.
                if (!TryReadEnrolledCgroup(unit, out var parentGroup))
                    return 40;
                // Record the parent's verified enrollment before it may spawn
                // a descendant; the child performs its own check before work.
                File.WriteAllText(Path.Combine(scopedDirectory, "enrolled.cgroup"), parentGroup);
                RecordProcess(Path.Combine(scopedDirectory, "parent.json"));
                using (var child = Process.Start(Commands.Create("linux-scoped-child",
                    Path.Combine(scopedDirectory, "child.json"), unit)))
                {
                    if (child is null)
                        throw new InvalidOperationException("The scoped fixture could not start its child.");
                    await child.WaitForExitAsync();
                    return child.ExitCode;
                }
            case ["linux-scoped-child", var scopedMarker, var expectedUnit]:
                if (!TryReadEnrolledCgroup(expectedUnit, out var childGroup))
                    return 41;
                File.WriteAllText(scopedMarker + ".cgroup", childGroup);
                RecordProcess(scopedMarker);
                await Task.Delay(Timeout.InfiniteTimeSpan);
                return 0;
            default:
                return 2;
        }
    }

    private static bool TryReadEnrolledCgroup(string unit, out string group)
    {
        group = "";
        if (!OperatingSystem.IsLinux() || !LinuxSystemdUserManager.IsSafeUnit(unit))
            return false;
        try
        {
            var entry = File.ReadLines("/proc/self/cgroup")
                .SingleOrDefault(line => line.StartsWith("0::/", StringComparison.Ordinal));
            if (entry is null || !LinuxSystemdUserManager.IsUnitControlGroup(entry[3..], unit))
                return false;
            group = entry[3..];
            return true;
        }
        catch (IOException)
        {
            return false;
        }
    }

    private static void RecordProcess(string path)
    {
        using var process = Process.GetCurrentProcess();
        File.WriteAllText(path + ".tmp", JsonSerializer.Serialize(
            new FixtureProcess(process.Id, process.StartTime.ToUniversalTime().Ticks)));
        File.Move(path + ".tmp", path, overwrite: true);
    }
}

internal sealed record FixtureProcess(int Id, long StartTicks)
{
    internal bool IsAlive()
    {
        try
        {
            using var process = Process.GetProcessById(Id);
            return !process.HasExited && process.StartTime.ToUniversalTime().Ticks == StartTicks;
        }
        catch (ArgumentException)
        {
            return false;
        }
    }
}

internal static class Commands
{
    internal static ProcessStartInfo Create(params string[] args)
    {
        var name = OperatingSystem.IsWindows() ? "dotnet.exe" : "dotnet";
        var dotnet = Path.GetFullPath(Path.Combine(RuntimeEnvironment.GetRuntimeDirectory(), "..", "..", "..", name));
        var assembly = typeof(Commands).Assembly.Location;
        var command = new ProcessStartInfo(dotnet)
        {
            UseShellExecute = false, CreateNoWindow = true, WorkingDirectory = Path.GetDirectoryName(assembly)!
        };
        command.ArgumentList.Add(assembly);
        foreach (var argument in args)
            command.ArgumentList.Add(argument);
        return command;
    }
}

internal sealed class ProcessFixtureAdapter(string output) : ITestAdapter
{
    public Task<TestSelection> SelectAsync(TestSelectionInput input, CancellationToken cancellationToken) =>
        Task.FromResult(new TestSelection(true,
            [new SelectedTarget("supervised-fixture", "fixture", input.Causes.Select(cause => cause.Id).ToArray())]));

    public WorkerRequirements Requirements(SelectedTarget target) => new(ExecutionOperation.Test, true, [output]);

    public async Task<TestOutcome> RunAsync(TestAttempt attempt, WorkerLease worker, CancellationToken cancellationToken)
    {
        var result = await worker.RunAsync(Commands.Create("tree", output), 4096, cancellationToken);
        return result.ExitCode == 0 ? TestOutcome.Passed : TestOutcome.RetryableFailure;
    }
}
