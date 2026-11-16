using System.Buffers;
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
        var serving = ServeAsync(workspace, monitor, stop);
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

    private static async Task ServeAsync(Workspace workspace, WorkspaceMonitor monitor, CancellationTokenSource stop)
    {
        // Keep the server instances alive across requests. On Unix, disposing
        // the last instance closes the shared listening socket and its queued connections.
        var listeners = new List<NamedPipeServerStream>(DaemonWire.MaximumConnections);
        try
        {
            for (var index = 0; index < DaemonWire.MaximumConnections; index++)
            {
                var options = PipeOptions.Asynchronous | PipeOptions.CurrentUserOnly;
                if (index == 0 && OperatingSystem.IsWindows())
                    options |= PipeOptions.FirstPipeInstance;
                listeners.Add(new NamedPipeServerStream(workspace.InstanceName, PipeDirection.InOut,
                    DaemonWire.MaximumConnections, PipeTransmissionMode.Byte, options));
            }

            await Task.WhenAll(listeners.Select(pipe => ServeConnectionsAsync(pipe, monitor, stop)));
        }
        finally
        {
            foreach (var listener in listeners)
                await listener.DisposeAsync();
        }
    }

    private static async Task ServeConnectionsAsync(
        NamedPipeServerStream pipe, WorkspaceMonitor monitor, CancellationTokenSource stop)
    {
        try
        {
            while (!stop.IsCancellationRequested)
            {
                var accepted = false;
                try
                {
                    await pipe.WaitForConnectionAsync(stop.Token);
                    accepted = true;
                    await HandleRequestAsync(pipe, monitor, stop);
                }
                catch (UnauthorizedAccessException) when (!accepted)
                {
                    // The runtime can reject a peer's credentials during Unix accept.
                }
                finally
                {
                    if (accepted)
                        pipe.Disconnect();
                }
            }
        }
        catch (OperationCanceledException) when (stop.IsCancellationRequested)
        {
            // Shutdown cancels pending accepts.
        }
        catch
        {
            stop.Cancel();
            throw;
        }
    }

    private static async Task HandleRequestAsync(
        NamedPipeServerStream pipe, WorkspaceMonitor monitor, CancellationTokenSource stop)
    {
        using (var deadline = CancellationTokenSource.CreateLinkedTokenSource(stop.Token))
        {
            deadline.CancelAfter(DaemonWire.OperationDeadline);
            using var read = CancellationTokenSource.CreateLinkedTokenSource(deadline.Token);
            read.CancelAfter(DaemonWire.ReadDeadline);
            try
            {
                var first = new byte[1];
                await pipe.ReadExactlyAsync(first, read.Token);
                byte command = first[0];
                ServerReply reply;
                if (command == DaemonWire.Hello)
                {
                    reply = Success(new DaemonHandshake(DaemonWire.Version, ReportFile.SchemaVersion,
                        ["status", "results", "recheck", "stop", "list_checkers"]));
                }
                else
                {
                    if (command == DaemonWire.VersionedRequest)
                    {
                        var envelope = new byte[2];
                        await pipe.ReadExactlyAsync(envelope, read.Token);
                        if (envelope[0] != DaemonWire.Version)
                        {
                            reply = new ServerReply(false,
                                new { supportedWireVersion = DaemonWire.Version },
                                "Unsupported local IPC version. Restart with a compatible SlopGuard build.",
                                "unsupported_wire_version");
                            await WriteReplyAndAcknowledgeAsync(pipe, reply, deadline.Token);
                            return;
                        }
                        command = envelope[1];
                    }
                    reply = await DispatchAsync(command, monitor, deadline.Token);
                }

                if (await WriteReplyAndAcknowledgeAsync(pipe, reply, deadline.Token) &&
                    command == (byte)DaemonCommand.Stop && reply.Ok)
                    stop.Cancel(); // Only an acknowledged stop may end this daemon.
            }
            catch (IOException)
            {
                // A disconnected or malformed local peer cannot interrupt the watcher.
            }
            catch (OperationCanceledException)
            {
                // Timed-out reads, operations, handoffs or shutdown release this slot.
            }
        }
    }

    private static async Task<ServerReply> DispatchAsync(
        byte command, WorkspaceMonitor monitor, CancellationToken cancellationToken)
    {
        return (DaemonCommand)command switch
        {
            DaemonCommand.Status => Success(monitor.Status()),
            DaemonCommand.Results => await ReadResultsAsync(monitor, cancellationToken),
            DaemonCommand.Recheck => Success(new { revision = monitor.Invalidate() }),
            DaemonCommand.Stop => Success(new { stopping = true }),
            _ => new ServerReply(false, null, "Unknown daemon command.", "unknown_command")
        };
    }

    private static async Task<ServerReply> ReadResultsAsync(WorkspaceMonitor monitor, CancellationToken token)
    {
        var report = await monitor.ReadCurrentAsync(token);
        return report is null
            ? new ServerReply(false, null, "No current inventory report. Check workspace status.", "no_current_report")
            : Success(report);
    }

    private static async Task<bool> WriteReplyAndAcknowledgeAsync(
        NamedPipeServerStream pipe, ServerReply reply, CancellationToken token)
    {
        ReadOnlyMemory<byte> bytes;
        try
        {
            bytes = SerializeReply(reply);
        }
        catch (ResponseTooLargeException)
        {
            bytes = SerializeReply(new ServerReply(false, null,
                "The daemon response exceeds the local IPC budget.", "response_too_large"));
        }
        var header = new byte[sizeof(int)];
        BinaryPrimitives.WriteInt32LittleEndian(header, bytes.Length);
        await pipe.WriteAsync(header, token);
        await pipe.WriteAsync(bytes, token);
        // Preserve the legacy Windows ACK before disposing this single-use instance.
        using var acknowledgement = CancellationTokenSource.CreateLinkedTokenSource(token);
        acknowledgement.CancelAfter(DaemonWire.AcknowledgementDeadline);
        var ack = new byte[1];
        await pipe.ReadExactlyAsync(ack, acknowledgement.Token);
        return ack[0] == 1;
    }

    private static ReadOnlyMemory<byte> SerializeReply(ServerReply reply)
    {
        var buffer = new ReplyBuffer();
        using var writer = new Utf8JsonWriter(buffer);
        JsonSerializer.Serialize(writer, reply, ReportFile.Json);
        writer.Flush();
        return buffer.Written;
    }

    private static ServerReply Success(object payload) => new(true, payload, null);

    private sealed record ServerReply(bool Ok, object? Payload, string? Error, string? Code = null);
    private sealed class ResponseTooLargeException : IOException;

    private sealed class ReplyBuffer : IBufferWriter<byte>
    {
        private byte[] bytes = new byte[256];
        private int written;

        internal ReadOnlyMemory<byte> Written => bytes.AsMemory(0, written);

        public void Advance(int count) => written += count;
        public Memory<byte> GetMemory(int sizeHint = 0)
        {
            var required = written + (long)Math.Max(1, sizeHint);
            if (required > DaemonWire.MaximumResponseBytes)
                throw new ResponseTooLargeException();
            if (required > bytes.Length)
                Array.Resize(ref bytes, Math.Min(DaemonWire.MaximumResponseBytes,
                    Math.Max((int)required, bytes.Length * 2)));
            return bytes.AsMemory(written);
        }

        public Span<byte> GetSpan(int sizeHint = 0) => GetMemory(sizeHint).Span;
    }
}
