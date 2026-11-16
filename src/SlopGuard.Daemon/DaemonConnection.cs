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

// Code is optional so legacy peers can still decode the original three-field reply.
public sealed record DaemonReply(bool Ok, JsonElement? Payload, string? Error, string? Code = null);

public sealed record DaemonHandshake(int WireVersion, int ReportSchemaVersion, string[] Capabilities);

public sealed record DaemonNegotiation(DaemonHandshake? Handshake, DaemonReply? Failure);

public sealed class DaemonConnection(Workspace workspace)
{
    public async Task<DaemonReply?> SendAsync(DaemonCommand command, CancellationToken cancellationToken = default)
    {
        try
        {
            return await SendNegotiatedAsync(command, cancellationToken);
        }
        catch (IOException ex) when (command == DaemonCommand.Status && IsDisconnected(ex))
        {
            // A probe can connect to the previous listener as its process exits.
            await Task.Delay(50, cancellationToken);
            try
            {
                return await SendNegotiatedAsync(command, cancellationToken);
            }
            catch (IOException retry) when (IsDisconnected(retry))
            {
                return null;
            }
        }
    }

    public async Task<DaemonNegotiation> NegotiateAsync(CancellationToken cancellationToken = default)
    {
        var hello = await SendOnceAsync([DaemonWire.Hello], cancellationToken);
        if (hello is null)
            return new(null, null);
        if (!hello.Ok)
        {
            if (hello.Code != "unknown_command" &&
                !(hello.Code is null && hello.Error == "Unknown daemon command."))
                return new(null, hello);
            return new(null, Failure("upgrade_required",
                "The running workspace daemon does not support this local IPC handshake. " +
                "Stop it with its matching SlopGuard build, then restart this build."));
        }

        DaemonHandshake? handshake;
        try
        {
            handshake = hello.Payload?.Deserialize<DaemonHandshake>(ReportFile.Json);
        }
        catch (JsonException)
        {
            handshake = null;
        }
        if (handshake is null || handshake.Capabilities is null || handshake.Capabilities.Length > 16 ||
            handshake.Capabilities.Any(value => string.IsNullOrWhiteSpace(value) || value.Length > 32))
            return new(null, Failure("invalid_handshake", "The running daemon returned an invalid IPC handshake. Restart it."));
        if (handshake.WireVersion != DaemonWire.Version)
            return new(null, Failure("unsupported_wire_version",
                $"Local IPC version {handshake.WireVersion} is incompatible with client version {DaemonWire.Version}. " +
                "Restart with a matching SlopGuard build."));
        if (handshake.ReportSchemaVersion != ReportFile.SchemaVersion)
            return new(null, Failure("unsupported_report_schema",
                $"Daemon report schema {handshake.ReportSchemaVersion} is incompatible with client schema " +
                $"{ReportFile.SchemaVersion}. Restart with a matching SlopGuard build."));
        return new(handshake, null);
    }

    private async Task<DaemonReply?> SendNegotiatedAsync(DaemonCommand command, CancellationToken cancellationToken)
    {
        var capability = DaemonWire.Capability(command);
        if (capability is null)
            return Failure("unknown_command", "Unknown local daemon command.");

        using var deadline = CancellationTokenSource.CreateLinkedTokenSource(cancellationToken);
        deadline.CancelAfter(TimeSpan.FromSeconds(6));
        try
        {
            var negotiation = await NegotiateAsync(deadline.Token);
            if (negotiation.Failure is { } failure)
                return failure;
            if (negotiation.Handshake is not { } handshake)
                return null;
            if (!handshake.Capabilities.Contains(capability, StringComparer.Ordinal))
                return Failure("unsupported_feature", $"The running daemon does not advertise '{capability}'. Restart with a compatible SlopGuard build.");
            return await SendOnceAsync([DaemonWire.VersionedRequest, DaemonWire.Version, (byte)command], deadline.Token);
        }
        catch (OperationCanceledException) when (!cancellationToken.IsCancellationRequested)
        {
            throw new IOException("The daemon request timed out.");
        }
    }

    private static DaemonReply Failure(string code, string message) => new(false, null, message, code);

    private static bool IsDisconnected(IOException error) => OperatingSystem.IsWindows() &&
        (error.HResult & 0xffff) is 109 or 232 or 233;

    private async Task<DaemonReply?> SendOnceAsync(byte[] request, CancellationToken cancellationToken)
    {
        await using var pipe = new NamedPipeClientStream(".", workspace.InstanceName, PipeDirection.InOut,
            PipeOptions.Asynchronous | PipeOptions.CurrentUserOnly);
        using var connectionTimeout = CancellationTokenSource.CreateLinkedTokenSource(cancellationToken);
        connectionTimeout.CancelAfter(TimeSpan.FromMilliseconds(1_500));
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
            var header = new byte[sizeof(int)];
            await pipe.WriteAsync(request, token);
            await pipe.ReadExactlyAsync(header, token);
            var length = BinaryPrimitives.ReadInt32LittleEndian(header);
            if (length is <= 0 or > DaemonWire.MaximumResponseBytes)
                throw new IOException("Invalid daemon response length.");
            var bytes = new byte[length];
            await pipe.ReadExactlyAsync(bytes, token);
            // Windows needs the final reader acknowledgement before the server closes its pipe.
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
