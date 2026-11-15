using System.Buffers.Binary;
using System.IO.Pipes;
using System.Text.Json;

namespace SlopGuard.Daemon;

public enum DaemonCommand : byte
{
    Status = 1,
    Results = 2,
    Recheck = 3,
    Stop = 4
}

public sealed record DaemonReply(bool Ok, JsonElement? Payload, string? Error);

public sealed class DaemonConnection(Workspace workspace)
{
    public async Task<DaemonReply?> SendAsync(DaemonCommand command, CancellationToken cancellationToken = default)
    {
        await using var pipe = new NamedPipeClientStream(".", workspace.InstanceName, PipeDirection.InOut,
            PipeOptions.Asynchronous | PipeOptions.CurrentUserOnly);
        using var connectionTimeout = CancellationTokenSource.CreateLinkedTokenSource(cancellationToken);
        connectionTimeout.CancelAfter(250);
        try
        {
            await pipe.ConnectAsync(connectionTimeout.Token);
        }
        catch (OperationCanceledException) when (!cancellationToken.IsCancellationRequested)
        {
            return null;
        }

        using var commandTimeout = CancellationTokenSource.CreateLinkedTokenSource(cancellationToken);
        commandTimeout.CancelAfter(TimeSpan.FromSeconds(6));
        var token = commandTimeout.Token;
        try
        {
            var buffer = new byte[sizeof(int)];
            await pipe.WriteAsync(new byte[] { (byte)command }, token);
            await pipe.ReadExactlyAsync(buffer, token);
            var length = BinaryPrimitives.ReadInt32LittleEndian(buffer);
            if (length is <= 0 or > ReportFile.MaximumBytes + 4_096)
                throw new IOException("Invalid daemon response length.");
            var bytes = new byte[length];
            await pipe.ReadExactlyAsync(bytes, token);
            await pipe.WriteAsync(new byte[] { 1 }, token);
            return JsonSerializer.Deserialize<DaemonReply>(bytes, ReportFile.Json)
                ?? throw new IOException("Invalid daemon response.");
        }
        catch (OperationCanceledException) when (!cancellationToken.IsCancellationRequested)
        {
            throw new IOException("The daemon request timed out.");
        }
    }
}
