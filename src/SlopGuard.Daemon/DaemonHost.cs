using System.Buffers.Binary;
using System.IO.Pipes;
using System.Text.Json;

namespace SlopGuard.Daemon;

public static class DaemonHost
{
    public static void Run(Workspace workspace, CancellationToken cancellationToken = default) =>
        Run(workspace, null, cancellationToken);

    internal static void Run(Workspace workspace, ITestAdapter? adapter, CancellationToken cancellationToken)
    {
        using var singleton = new Mutex(false, workspace.InstanceName);
        try
        {
            if (!singleton.WaitOne(0))
                return;
        }
        catch (AbandonedMutexException)
        {
            // The terminated owner released the guard.
        }

        try
        {
            RunOwnedAsync(workspace, adapter, cancellationToken).GetAwaiter().GetResult();
        }
        finally
        {
            singleton.ReleaseMutex();
        }
    }

    private static async Task RunOwnedAsync(Workspace workspace, ITestAdapter? adapter, CancellationToken cancellationToken)
    {
        var git = await GitContext.DiscoverAsync(workspace.Root, cancellationToken);
        using var stop = CancellationTokenSource.CreateLinkedTokenSource(cancellationToken);
        using var monitor = new WorkspaceMonitor(workspace, git, adapter);
        monitor.Start();
        var indexing = monitor.RunAsync(stop.Token);
        var serving = ServeAsync(workspace, monitor, stop.Token);
        var scheduling = monitor.RunSchedulerAsync(stop.Token);
        var tasks = scheduling is null ? new[] { indexing, serving } : [indexing, serving, scheduling];
        await Task.WhenAny(tasks);
        await stop.CancelAsync();
        try
        {
            await Task.WhenAll(tasks);
        }
        catch (OperationCanceledException) when (stop.IsCancellationRequested)
        {
            // The caller requested shutdown or the local listener finished.
        }
    }

    private static async Task ServeAsync(Workspace workspace, WorkspaceMonitor monitor, CancellationToken cancellationToken)
    {
        await using var pipe = new NamedPipeServerStream(workspace.InstanceName, PipeDirection.InOut, 1,
            PipeTransmissionMode.Byte, PipeOptions.Asynchronous | PipeOptions.CurrentUserOnly);

        while (!cancellationToken.IsCancellationRequested)
        {
            await pipe.WaitForConnectionAsync(cancellationToken);
            using var deadline = CancellationTokenSource.CreateLinkedTokenSource(cancellationToken);
            deadline.CancelAfter(TimeSpan.FromSeconds(5));
            var token = deadline.Token;
            try
            {
                var buffer = new byte[sizeof(int)];
                await pipe.ReadExactlyAsync(buffer.AsMemory(0, 1), token);
                var command = (DaemonCommand)buffer[0];
                DaemonReply reply;
                switch (command)
                {
                    case DaemonCommand.Status:
                        reply = Success(monitor.Status());
                        break;
                    case DaemonCommand.Results:
                        var report = await monitor.ReadCurrentAsync(token);
                        reply = report is null
                            ? new DaemonReply(false, null, "No current inventory report. Check workspace status.")
                            : Success(report);
                        break;
                    case DaemonCommand.Recheck:
                        reply = Success(new { revision = monitor.Invalidate() });
                        break;
                    case DaemonCommand.Stop:
                        reply = Success(new { stopping = true });
                        break;
                    default:
                        reply = new DaemonReply(false, null, "Unknown daemon command.");
                        break;
                }

                var payload = JsonSerializer.SerializeToUtf8Bytes(reply, ReportFile.Json);
                BinaryPrimitives.WriteInt32LittleEndian(buffer, payload.Length);
                await pipe.WriteAsync(buffer, token);
                await pipe.WriteAsync(payload, token);
                // Let the client consume the entire frame before disconnecting the Windows pipe.
                await pipe.ReadExactlyAsync(buffer.AsMemory(0, 1), token);
                if (command == DaemonCommand.Stop)
                    return;
            }
            catch (IOException)
            {
                // A disconnected local client cannot interrupt the workspace watcher.
            }
            catch (OperationCanceledException) when (!cancellationToken.IsCancellationRequested)
            {
                // Release the single connection slot when an individual client stalls.
            }
            finally
            {
                if (pipe.IsConnected)
                    pipe.Disconnect();
            }
        }
    }

    private static DaemonReply Success<T>(T payload) =>
        new(true, JsonSerializer.SerializeToElement(payload, ReportFile.Json), null);
}
