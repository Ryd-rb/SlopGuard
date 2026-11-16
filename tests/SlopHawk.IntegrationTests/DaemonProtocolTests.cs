using System.Buffers.Binary;
using System.Diagnostics;
using System.IO.Pipes;
using System.Text.Json;
using SlopHawk.Daemon;
using Xunit;

namespace SlopHawk.IntegrationTests;

public sealed class DaemonProtocolTests
{
    [Fact]
    public async Task NewClientNegotiatesAndLegacyByteCommandsRemainAvailable()
    {
        await using var fixture = new Fixture();
        await fixture.StartAsync();

        var negotiated = await fixture.Connection.NegotiateAsync();
        Assert.Null(negotiated.Failure);
        Assert.Equal(1, negotiated.Handshake?.WireVersion);
        Assert.Equal(2, negotiated.Handshake?.ReportSchemaVersion);
        Assert.Equal(new[] { "status", "results", "recheck", "stop", "list_checkers" },
            Assert.IsType<string[]>(negotiated.Handshake?.Capabilities));

        var legacy = await SendRawAsync(fixture.Workspace, [(byte)DaemonCommand.Status]);
        Assert.True(legacy.Ok, legacy.Error);
        Assert.Equal(2, legacy.Payload?.GetProperty("reportSchemaVersion").GetInt32());
        var versioned = await fixture.Connection.SendAsync(DaemonCommand.Status);
        Assert.True(versioned?.Ok == true, versioned?.Error);
        Assert.Equal(2, versioned?.Payload?.GetProperty("reportSchemaVersion").GetInt32());

        var recheck = await fixture.Connection.SendAsync(DaemonCommand.Recheck);
        Assert.True(recheck?.Ok == true, recheck?.Error);
        Assert.True(recheck?.Payload?.GetProperty("revision").GetInt64() > 0);
        var results = await fixture.Connection.SendAsync(DaemonCommand.Results);
        Assert.NotNull(results); // Either a current report or a typed pending-report error is valid.
        if (results is { Ok: false })
            Assert.Equal("no_current_report", results.Code);
        var legacyRecheck = await SendRawAsync(fixture.Workspace, [(byte)DaemonCommand.Recheck]);
        Assert.True(legacyRecheck.Ok, legacyRecheck.Error);
        var legacyResults = await SendRawAsync(fixture.Workspace, [(byte)DaemonCommand.Results]);
        Assert.NotNull(legacyResults);
        if (!legacyResults.Ok)
            Assert.Equal("no_current_report", legacyResults.Code);
        var legacyStop = await SendRawAsync(fixture.Workspace, [(byte)DaemonCommand.Stop]);
        Assert.True(legacyStop.Ok, legacyStop.Error);
        await fixture.Daemon!.WaitAsync(TimeSpan.FromSeconds(5));
    }

    [Fact]
    public async Task AStalledReaderDoesNotBlockOtherStatusRequests()
    {
        await using var fixture = new Fixture();
        await fixture.StartAsync();
        await using var stalled = CreateClient(fixture.Workspace);
        using var timeout = new CancellationTokenSource(TimeSpan.FromSeconds(4));
        await stalled.ConnectAsync(timeout.Token);
        await stalled.WriteAsync(new byte[] { DaemonWire.VersionedRequest }, timeout.Token);
        await Task.Delay(100, timeout.Token); // Let this client enter its bounded header read.

        var watch = Stopwatch.StartNew();
        var answer = await fixture.Connection.SendAsync(DaemonCommand.Status, timeout.Token);
        Assert.True(answer?.Ok == true, answer?.Error);
        Assert.True(watch.Elapsed < TimeSpan.FromSeconds(2), $"Status took {watch.Elapsed} behind a stalled peer.");
        await fixture.StopAsync();
    }

    [Fact]
    public async Task AnUnacknowledgedResponseCannotHoldTheListener()
    {
        await using var fixture = new Fixture();
        await fixture.StartAsync();
        await using var withoutAck = CreateClient(fixture.Workspace);
        using var timeout = new CancellationTokenSource(TimeSpan.FromSeconds(4));
        await withoutAck.ConnectAsync(timeout.Token);
        await withoutAck.WriteAsync(new byte[] { (byte)DaemonCommand.Status }, timeout.Token);
        var length = new byte[4];
        await withoutAck.ReadExactlyAsync(length, timeout.Token);
        var payload = new byte[BinaryPrimitives.ReadInt32LittleEndian(length)];
        await withoutAck.ReadExactlyAsync(payload, timeout.Token);
        // Deliberately leave the old Windows handoff ACK unsent.
        var reply = await fixture.Connection.SendAsync(DaemonCommand.Status, timeout.Token);
        Assert.True(reply?.Ok == true, reply?.Error);
        await fixture.StopAsync();
    }

    [Fact]
    public async Task UnknownCommandsAndVersionsAreTypedAndDoNotRunOperations()
    {
        await using var fixture = new Fixture();
        await fixture.StartAsync();

        var future = await SendRawAsync(fixture.Workspace, [DaemonWire.VersionedRequest, 99, (byte)DaemonCommand.Recheck]);
        Assert.False(future.Ok);
        Assert.Equal("unsupported_wire_version", future.Code);
        Assert.Equal(1, future.Payload?.GetProperty("supportedWireVersion").GetInt32());

        var unknown = await SendRawAsync(fixture.Workspace, [DaemonWire.VersionedRequest, DaemonWire.Version, 255]);
        Assert.False(unknown.Ok);
        Assert.Equal("unknown_command", unknown.Code);
        var oldUnknown = await SendRawAsync(fixture.Workspace, [255]);
        Assert.False(oldUnknown.Ok);
        Assert.Equal("unknown_command", oldUnknown.Code);

        await using (var disconnect = CreateClient(fixture.Workspace))
        {
            using var timeout = new CancellationTokenSource(TimeSpan.FromSeconds(3));
            await disconnect.ConnectAsync(timeout.Token);
            await disconnect.WriteAsync(new byte[] { DaemonWire.VersionedRequest }, timeout.Token);
        }
        Assert.True((await fixture.Connection.SendAsync(DaemonCommand.Status))?.Ok);
        await fixture.StopAsync();
    }

    [Fact]
    public async Task LegacyDaemonWithoutHelloReturnsAnUpgradeErrorNotASpawnHint()
    {
        await using var fixture = new Fixture();
        await using var server = CreateServer(fixture.Workspace);
        var serving = RespondOnceAsync(server, new
        {
            ok = false,
            payload = (object?)null,
            error = "Unknown daemon command."
        });

        var result = await fixture.Connection.SendAsync(DaemonCommand.Status);
        Assert.False(result?.Ok);
        Assert.Equal("upgrade_required", result?.Code);
        Assert.Contains("matching SlopHawk build", Assert.IsType<string>(result?.Error));
        Assert.Equal(DaemonWire.Hello, await serving.WaitAsync(TimeSpan.FromSeconds(3)));
    }

    [Fact]
    public async Task ClientRejectsFutureWireVersionBeforeSendingAnOperation()
    {
        await using var fixture = new Fixture();
        await using var server = CreateServer(fixture.Workspace);
        var serving = RespondOnceAsync(server, new DaemonReply(true,
            JsonSerializer.SerializeToElement(new DaemonHandshake(88, 2, ["status"]), ReportFile.Json), null));
        var result = await fixture.Connection.SendAsync(DaemonCommand.Status);
        Assert.False(result?.Ok);
        Assert.Equal("unsupported_wire_version", result?.Code);
        Assert.Equal(DaemonWire.Hello, await serving.WaitAsync(TimeSpan.FromSeconds(3)));
    }

    [Fact]
    public async Task AdvertisedCapabilitiesControlWhichCommandsTheClientSends()
    {
        await using var fixture = new Fixture();
        await using var server = CreateServer(fixture.Workspace);
        var serving = RespondOnceAsync(server, new DaemonReply(true,
            JsonSerializer.SerializeToElement(new DaemonHandshake(1, 2, ["status"]), ReportFile.Json), null));
        var result = await fixture.Connection.SendAsync(DaemonCommand.Results);
        Assert.False(result?.Ok);
        Assert.Equal("unsupported_feature", result?.Code);
        Assert.Equal(DaemonWire.Hello, await serving.WaitAsync(TimeSpan.FromSeconds(3)));
    }

    [Fact]
    public async Task OversizedResponseLengthIsRejectedBeforeAllocatingPayload()
    {
        await using var fixture = new Fixture();
        await using var server = CreateServer(fixture.Workspace);
        var serving = Task.Run(async () =>
        {
            using var timeout = new CancellationTokenSource(TimeSpan.FromSeconds(4));
            await server.WaitForConnectionAsync(timeout.Token);
            var incoming = new byte[1];
            await server.ReadExactlyAsync(incoming, timeout.Token);
            Assert.Equal(DaemonWire.Hello, incoming[0]);
            var length = new byte[4];
            BinaryPrimitives.WriteInt32LittleEndian(length, DaemonWire.MaximumResponseBytes + 1);
            await server.WriteAsync(length, timeout.Token);
        });
        var failure = await Assert.ThrowsAsync<IOException>(() => fixture.Connection.NegotiateAsync());
        Assert.Contains("response length", failure.Message);
        await serving.WaitAsync(TimeSpan.FromSeconds(4));
    }

    [Fact]
    public async Task AcknowledgedStopCancelsActiveRequestsAndFinishesShutdown()
    {
        await using var fixture = new Fixture();
        await fixture.StartAsync();
        await using var stalled = CreateClient(fixture.Workspace);
        using var timeout = new CancellationTokenSource(TimeSpan.FromSeconds(5));
        await stalled.ConnectAsync(timeout.Token);
        await stalled.WriteAsync(new byte[] { DaemonWire.VersionedRequest }, timeout.Token);

        await fixture.StopAsync();
        // The server must not wait for the unrelated stalled reader's five-second operation budget.
        Assert.True(fixture.Daemon?.IsCompleted);
    }

    private static NamedPipeClientStream CreateClient(Workspace workspace) =>
        new(".", workspace.InstanceName, PipeDirection.InOut,
            PipeOptions.Asynchronous | PipeOptions.CurrentUserOnly);

    private static NamedPipeServerStream CreateServer(Workspace workspace) =>
        new(workspace.InstanceName, PipeDirection.InOut, 1, PipeTransmissionMode.Byte,
            PipeOptions.Asynchronous | PipeOptions.CurrentUserOnly);

    private static async Task<DaemonReply> SendRawAsync(Workspace workspace, byte[] request)
    {
        await using var pipe = CreateClient(workspace);
        using var timeout = new CancellationTokenSource(TimeSpan.FromSeconds(5));
        await pipe.ConnectAsync(timeout.Token);
        await pipe.WriteAsync(request, timeout.Token);
        var header = new byte[4];
        await pipe.ReadExactlyAsync(header, timeout.Token);
        var length = BinaryPrimitives.ReadInt32LittleEndian(header);
        Assert.InRange(length, 1, DaemonWire.MaximumResponseBytes);
        var payload = new byte[length];
        await pipe.ReadExactlyAsync(payload, timeout.Token);
        await pipe.WriteAsync(new byte[] { 1 }, timeout.Token);
        return JsonSerializer.Deserialize<DaemonReply>(payload, ReportFile.Json)!;
    }

    private static async Task<byte> RespondOnceAsync(NamedPipeServerStream pipe, object reply)
    {
        using var timeout = new CancellationTokenSource(TimeSpan.FromSeconds(5));
        await pipe.WaitForConnectionAsync(timeout.Token);
        var request = new byte[1];
        await pipe.ReadExactlyAsync(request, timeout.Token);
        var payload = JsonSerializer.SerializeToUtf8Bytes(reply, ReportFile.Json);
        var header = new byte[4];
        BinaryPrimitives.WriteInt32LittleEndian(header, payload.Length);
        await pipe.WriteAsync(header, timeout.Token);
        await pipe.WriteAsync(payload, timeout.Token);
        var ack = new byte[1];
        await pipe.ReadExactlyAsync(ack, timeout.Token);
        Assert.Equal(1, ack[0]);
        return request[0];
    }

    private sealed class Fixture : IAsyncDisposable
    {
        private readonly CancellationTokenSource cancellation = new();
        private readonly string container = Directory.CreateTempSubdirectory("slophawk-ipc-").FullName;

        internal Fixture()
        {
            Root = Directory.CreateDirectory(Path.Combine(container, "workspace")).FullName;
            Workspace = new Workspace(Root, Path.Combine(container, "state"));
        }

        internal string Root { get; }
        internal Workspace Workspace { get; }
        internal DaemonConnection Connection => new(Workspace);
        internal Task? Daemon { get; private set; }

        internal async Task StartAsync()
        {
            Daemon = Task.Factory.StartNew(() => DaemonHost.Run(Workspace, null, cancellation.Token),
                CancellationToken.None, TaskCreationOptions.LongRunning, TaskScheduler.Default);
            using var timeout = new CancellationTokenSource(TimeSpan.FromSeconds(10));
            while (true)
            {
                if (Daemon.IsCompleted)
                    await Daemon;
                var negotiation = await Connection.NegotiateAsync(timeout.Token);
                if (negotiation.Handshake is not null)
                    return;
                await Task.Delay(50, timeout.Token);
            }
        }

        internal async Task StopAsync()
        {
            var answer = await Connection.SendAsync(DaemonCommand.Stop);
            Assert.True(answer?.Ok == true, answer?.Error);
            await Daemon!.WaitAsync(TimeSpan.FromSeconds(5));
        }

        public async ValueTask DisposeAsync()
        {
            try
            {
                await cancellation.CancelAsync();
                if (Daemon is not null)
                    await Daemon.WaitAsync(TimeSpan.FromSeconds(5));
            }
            finally
            {
                cancellation.Dispose();
                Directory.Delete(container, recursive: true);
            }
        }
    }
}
