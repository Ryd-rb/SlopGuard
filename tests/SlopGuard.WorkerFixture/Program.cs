using System.Diagnostics;
using System.Runtime.InteropServices;
using System.Text.Json;
using SlopGuard.Daemon;

namespace SlopGuard.WorkerFixture;

internal static class Program
{
    private static async Task<int> Main(string[] args)
    {
        switch (args)
        {
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
            default:
                return 2;
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
