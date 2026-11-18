using System.Security.Cryptography;
using System.Text.Json.Nodes;
using System.Text.Json;
using System.Diagnostics;
using System.Reflection;
using Microsoft.Data.Sqlite;
using SlopHawk.Cli;
using SlopHawk.Daemon;
using Xunit;

namespace SlopHawk.IntegrationTests;

public sealed class NativeEngineSelectionTests
{
    [Fact]
    public void MigrationFrameRejectsOversizedLengthBeforeAllocatingBody()
    {
        using var stream = new MemoryStream(new byte[] { 0x01, 0x00, 0x02, 0x00 });
        Assert.Throws<IOException>(() => MigrationStateBridge.ReadFrame(stream, CancellationToken.None));
    }

    [Fact]
    public void MigrationFrameRejectsTruncatedBodyAndPrefix()
    {
        using var truncatedPrefix = new MemoryStream(new byte[] { 0x10, 0x00 });
        Assert.Throws<EndOfStreamException>(() =>
            MigrationStateBridge.ReadFrame(truncatedPrefix, CancellationToken.None));
        using var truncatedBody = new MemoryStream(new byte[] { 0x0A, 0, 0, 0, (byte)'{' });
        Assert.Throws<EndOfStreamException>(() =>
            MigrationStateBridge.ReadFrame(truncatedBody, CancellationToken.None));
    }

    [Fact]
    public void MigrationFrameRoundTripsBoundedUtf8Json()
    {
        using var stream = new MemoryStream();
        MigrationStateBridge.WriteFrame(stream, new { kind = "request", job = "case-14" }, CancellationToken.None);
        stream.Position = 0;
        using var frame = MigrationStateBridge.ReadFrame(stream, CancellationToken.None);
        Assert.Equal("request", frame!.RootElement.GetProperty("kind").GetString());
        Assert.Equal("case-14", frame.RootElement.GetProperty("job").GetString());
        Assert.Null(MigrationStateBridge.ReadFrame(stream, CancellationToken.None));
    }

    [Fact]
    public async Task ExplicitSelectionRejectsAnUnpinnedProfileBeforeAnyChildIsStarted()
    {
        using var home = new TempWorkspace();
        var file = Path.Combine(home.Parent, "engine-profile.json");
        await File.WriteAllTextAsync(file, "{}");
        await Assert.ThrowsAsync<UnauthorizedAccessException>(() =>
            NativeEngine.RunAsync(["engine", "native", file, new string('A', 64), "--", "status"]));
    }

    [Theory]
    [InlineData("partial")]
    [InlineData("unsupported")]
    [InlineData("missing-terminal")]
    [InlineData("truncated")]
    [InlineData("oversized")]
    [InlineData("echo-mismatch")]
    [InlineData("bad-digest")]
    [InlineData("duplicate-terminal")]
    public void ControlledPeerCannotPublishWriterOnBadOrIncompleteTerminal(string mode)
    {
        // The owner explicitly builds this separate fixture before enabling this
        // suite; it is never included in a release qualification receipt.
        var binary = Environment.GetEnvironmentVariable("SLOPHAWK_NATIVE_TEST_PEER_BINARY");
        if (string.IsNullOrWhiteSpace(binary)) return;
        using var home = new TempWorkspace();
        Seed(home);
        var digest = Convert.ToHexString(SHA256.HashData(File.ReadAllBytes(binary)));
        var previous = Environment.GetEnvironmentVariable("SLOPHAWK_PEER_FIXTURE_MODE");
        try
        {
            Environment.SetEnvironmentVariable("SLOPHAWK_PEER_FIXTURE_MODE", mode);
            Assert.ThrowsAny<IOException>(() => MigrationStateBridge.TransferToNative(
                home.Workspace, binary, digest, new string('A', 64), "controlled-test-only"));
            Assert.Throws<IOException>(() => MigrationStateBridge.RequireLegacyWriter(home.Workspace));
            Assert.Throws<IOException>(() => MigrationStateBridge.RequireNativeWriter(
                home.Workspace, digest, new string('A', 64)));
        }
        finally
        {
            Environment.SetEnvironmentVariable("SLOPHAWK_PEER_FIXTURE_MODE", previous);
        }
    }

    [Fact]
    public void InstalledRustPeerTransfersAndReturnsDurableState()
    {
        var binary = Environment.GetEnvironmentVariable("SLOPHAWK_NATIVE_MIGRATION_BINARY");
        if (string.IsNullOrWhiteSpace(binary)) return;
        using var home = new TempWorkspace();
        Seed(home);
        var digest = Convert.ToHexString(SHA256.HashData(File.ReadAllBytes(binary)));
        var profile = new string('A', 64);

        MigrationStateBridge.TransferToNative(home.Workspace, binary, digest, profile, "native-integration");
        MigrationStateBridge.RequireNativeWriter(home.Workspace, digest, profile);
        Assert.Throws<IOException>(() => MigrationStateBridge.RequireLegacyWriter(home.Workspace));
        Assert.Throws<IOException>(() => DaemonHost.Run(home.Workspace));

        var path = Path.Combine(home.Workspace.StateDirectory, "migration-engine.json");
        var marker = JsonNode.Parse(File.ReadAllText(path))!;
        var epoch = marker["epoch"]!.GetValue<string>();
        var native = Path.Combine(home.Workspace.StateDirectory, $"native-{epoch}.json");
        var state = JsonNode.Parse(File.ReadAllText(native))!;
        Assert.Equal(7L, state["revision"]!.GetValue<long>());
        Assert.Equal("graph:5", state["tables"]![3]!["rows"]![0]![6]!["value"]!.GetValue<string>());

        var edited = Path.Combine(home.Workspace.Root, "Source.cs");
        File.WriteAllText(edited, "class ChangedInRust {}\n");
        var expectedDigest = Convert.ToHexString(SHA256.HashData(File.ReadAllBytes(edited)));
        var updated = JsonNode.Parse(MigrationStateBridge.ReadNativeState(home.Workspace, binary,
            digest, profile, "native-integration", reconcile: true))!;
        Assert.Equal(8L, updated["committedRevision"]!.GetValue<long>());
        Assert.Equal("rust", updated["engine"]!.GetValue<string>());
        var unchanged = JsonNode.Parse(MigrationStateBridge.ReadNativeState(home.Workspace, binary,
            digest, profile, "native-integration", reconcile: true))!;
        Assert.Equal(8L, unchanged["committedRevision"]!.GetValue<long>());

        marker["state"] = "rollback-uncertain";
        File.WriteAllText(path + ".pending", marker.ToJsonString());
        Assert.Throws<IOException>(() => MigrationStateBridge.RequireNativeWriter(home.Workspace, digest, profile));
        MigrationStateBridge.RollbackToLegacy(home.Workspace, binary, digest, profile, "native-integration");
        MigrationStateBridge.RequireLegacyWriter(home.Workspace);
        Assert.Equal(Convert.ToHexString(SHA256.HashData(File.ReadAllBytes(native))),
            File.ReadAllText(Path.Combine(home.Workspace.StateDirectory, $"native-{epoch}.stopped")));
        using var restored = Open(home.Workspace);
        Assert.Equal(8L, Scalar(restored, "SELECT revision FROM workspace"));
        Assert.Equal(expectedDigest, Text(restored, "SELECT digest FROM source"));
        Assert.Equal("graph:5", Text(restored, "SELECT predecessor FROM test_intent"));
        Assert.Equal(3L, Scalar(restored, "SELECT first_revision FROM intent_fallback"));
        Assert.Equal(8L, Scalar(restored, "SELECT last_revision FROM intent_fallback"));
        Assert.Equal("interrupted", Text(restored, "SELECT state FROM test_attempt"));
        Assert.Equal("lease-11", Text(restored, "SELECT lease_id FROM test_attempt"));
        Assert.Equal("receipt-7", Text(restored, "SELECT reason_code FROM test_run_receipt"));
    }

    [Fact]
    public async Task RustResidentServesWorkspaceAndMcpThenReleasesItsWriterOnRollback()
    {
        var binary = Environment.GetEnvironmentVariable("SLOPHAWK_NATIVE_MIGRATION_BINARY");
        if (string.IsNullOrWhiteSpace(binary)) return;
        using var home = new TempWorkspace();
        Seed(home);
        var digest = Convert.ToHexString(SHA256.HashData(File.ReadAllBytes(binary)));
        var profile = new string('D', 64);
        var source = Path.Combine(home.Workspace.Root, "Source.cs");
        File.WriteAllText(source, "class Before {}\n");
        var managedMcp = await ManagedMcpDiscovery();
        MigrationStateBridge.TransferToNative(home.Workspace, binary, digest, profile, "native-resident-test");
        try
        {
            var started = JsonNode.Parse(await Run("start"))!;
            Assert.Equal(2, started["reportSchemaVersion"]!.GetValue<int>());
            var before = JsonNode.Parse(await Run("results"))!;
            Assert.Equal("SlopHawk", before["producer"]!.GetValue<string>());
            var revision = before["committedRevision"]!.GetValue<long>();

            await File.WriteAllTextAsync(source, "class After {}\n");
            await Run("recheck");
            using (var mcp = Launch("mcp", input: true))
            {
                using var deadline = new CancellationTokenSource(TimeSpan.FromSeconds(20));
                await mcp.StandardInput.WriteLineAsync("""{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-11-25","capabilities":{},"clientInfo":{"name":"resident-test","version":"1"}}}""");
                var ready = JsonNode.Parse((await mcp.StandardOutput.ReadLineAsync(deadline.Token))!)!;
                Assert.Equal("2025-11-25", ready["result"]!["protocolVersion"]!.GetValue<string>());
                Assert.True(JsonNode.DeepEquals(managedMcp.Initialize, ready));
                await mcp.StandardInput.WriteLineAsync("""{"jsonrpc":"2.0","method":"notifications/initialized"}""");
                await mcp.StandardInput.WriteLineAsync("""{"jsonrpc":"2.0","id":2,"method":"tools/list"}""");
                var tools = JsonNode.Parse((await mcp.StandardOutput.ReadLineAsync(deadline.Token))!)!;
                Assert.True(JsonNode.DeepEquals(managedMcp.List, tools));
                await mcp.StandardInput.WriteLineAsync("""{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"workspace_status","arguments":{}}}""");
                var status = JsonNode.Parse((await mcp.StandardOutput.ReadLineAsync(deadline.Token))!)!;
                Assert.True(ReadToolPayload(status)["ok"]!.GetValue<bool>());
                await mcp.StandardInput.WriteLineAsync("""{"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"run_checks","arguments":{}}}""");
                var recheck = JsonNode.Parse((await mcp.StandardOutput.ReadLineAsync(deadline.Token))!)!;
                var response = ReadToolPayload(recheck);
                Assert.True(response["ok"]!.GetValue<bool>());
                var requested = response["payload"]!["revision"]!.GetValue<long>();
                while (true)
                {
                    var current = JsonNode.Parse(await Run("status"))!;
                    if (current["state"]!.GetValue<string>() == "ready" && current["revision"]!.GetValue<long>() >= requested)
                        break;
                    await Task.Delay(35, deadline.Token);
                }
                await mcp.StandardInput.WriteLineAsync("""{"jsonrpc":"2.0","id":5,"method":"tools/call","params":{"name":"get_findings","arguments":{}}}""");
                var findings = JsonNode.Parse((await mcp.StandardOutput.ReadLineAsync(deadline.Token))!)!;
                var result = ReadToolPayload(findings);
                Assert.True(result["ok"]!.GetValue<bool>());
                Assert.Equal("SlopHawk", result["payload"]!["producer"]!.GetValue<string>());
                mcp.StandardInput.Close();
                await mcp.WaitForExitAsync(deadline.Token);
                Assert.Equal(0, mcp.ExitCode);
            }
            using var reconciled = new CancellationTokenSource(TimeSpan.FromSeconds(8));
            while (JsonNode.Parse(await Run("status"))!["state"]!.GetValue<string>() != "ready")
                await Task.Delay(35, reconciled.Token);
            var after = JsonNode.Parse(await Run("results"))!;
            Assert.True(after["committedRevision"]!.GetValue<long>() > revision);
            Assert.True(JsonNode.Parse(await Run("stop"))!["stopping"]!.GetValue<bool>());
            using var stopped = new CancellationTokenSource(TimeSpan.FromSeconds(8));
            while (File.Exists(Path.Combine(home.Workspace.StateDirectory, "native-daemon.json")) ||
                   File.Exists(Path.Combine(home.Workspace.Root, ".slophawk", "results.json")))
                await Task.Delay(30, stopped.Token);
            var restarted = JsonNode.Parse(await Run("start"))!;
            Assert.Equal(after["committedRevision"]!.GetValue<long>(), restarted["committedRevision"]!.GetValue<long>());
            Assert.NotEqual(after["epoch"]!.GetValue<string>(), restarted["epoch"]!.GetValue<string>());
        }
        finally
        {
            MigrationStateBridge.RollbackToLegacy(home.Workspace, binary, digest, profile, "native-resident-test");
        }
        MigrationStateBridge.RequireLegacyWriter(home.Workspace);
        Assert.False(File.Exists(Path.Combine(home.Workspace.Root, ".slophawk", "results.json")));
        using var restored = Open(home.Workspace);
        Assert.Equal(Convert.ToHexString(SHA256.HashData(File.ReadAllBytes(source))), Text(restored, "SELECT digest FROM source"));

        static JsonNode ReadToolPayload(JsonNode response) =>
            JsonNode.Parse(response["result"]!["content"]![0]!["text"]!.GetValue<string>())!;

        async Task<(JsonNode Initialize, JsonNode List)> ManagedMcpDiscovery()
        {
            using var stop = new CancellationTokenSource(TimeSpan.FromSeconds(20));
            var daemon = Task.Factory.StartNew(() => DaemonHost.Run(home.Workspace, stop.Token),
                CancellationToken.None, TaskCreationOptions.LongRunning, TaskScheduler.Default);
            try
            {
                var connection = new DaemonConnection(home.Workspace);
                while ((await connection.NegotiateAsync(stop.Token)).Handshake is null)
                    await Task.Delay(50, stop.Token);
                var apphost = Path.Combine(AppContext.BaseDirectory, OperatingSystem.IsWindows() ? "slophawk.exe" : "slophawk");
                var info = new ProcessStartInfo(apphost) { UseShellExecute = false, CreateNoWindow = true,
                    RedirectStandardInput = true, RedirectStandardOutput = true, RedirectStandardError = true };
                foreach (var arg in new[] { "mcp", "--root", home.Workspace.Root }) info.ArgumentList.Add(arg);
                using var mcp = Process.Start(info)!;
                var errors = mcp.StandardError.ReadToEndAsync(stop.Token);
                await mcp.StandardInput.WriteLineAsync("""{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-11-25","capabilities":{},"clientInfo":{"name":"resident-test","version":"1"}}}""");
                var initialize = JsonNode.Parse((await mcp.StandardOutput.ReadLineAsync(stop.Token))!)!;
                await mcp.StandardInput.WriteLineAsync("""{"jsonrpc":"2.0","method":"notifications/initialized"}""");
                await mcp.StandardInput.WriteLineAsync("""{"jsonrpc":"2.0","id":2,"method":"tools/list"}""");
                var list = JsonNode.Parse((await mcp.StandardOutput.ReadLineAsync(stop.Token))!)!;
                mcp.StandardInput.Close();
                await mcp.WaitForExitAsync(stop.Token);
                Assert.True(mcp.ExitCode == 0, await errors);
                return (initialize, list);
            }
            finally
            {
                await stop.CancelAsync();
                await daemon.WaitAsync(TimeSpan.FromSeconds(8));
            }
        }

        Process Launch(string verb, bool input = false)
        {
            var info = new ProcessStartInfo(binary)
            {
                UseShellExecute = false, CreateNoWindow = true, RedirectStandardInput = input,
                RedirectStandardError = true, RedirectStandardOutput = true,
            };
            foreach (var arg in new[] { "--native-workspace-v1", home.Workspace.Root, home.Workspace.StateDirectory, profile, verb })
                info.ArgumentList.Add(arg);
            return Process.Start(info)!;
        }

        async Task<string> Run(string verb)
        {
            using var child = Launch(verb);
            using var deadline = new CancellationTokenSource(TimeSpan.FromSeconds(20));
            var output = child.StandardOutput.ReadToEndAsync(deadline.Token);
            var error = child.StandardError.ReadToEndAsync(deadline.Token);
            await child.WaitForExitAsync(deadline.Token);
            Assert.True(child.ExitCode == 0, await error);
            return await output;
        }
    }

    [Fact]
    public async Task RustResidentSchedulesAllFourSourceCheckersAndRecoversDurableAttempts()
    {
        var binary = Environment.GetEnvironmentVariable("SLOPHAWK_NATIVE_MIGRATION_BINARY");
        var host = Environment.GetEnvironmentVariable("SLOPHAWK_PROJECT_TEST_DOTNET");
        var pack = Environment.GetEnvironmentVariable("SLOPHAWK_CHECKER_TEST_REFERENCE_PACK");
        if (string.IsNullOrWhiteSpace(binary) || string.IsNullOrWhiteSpace(host) || string.IsNullOrWhiteSpace(pack)) return;
        using var home = new TempWorkspace();
        Seed(home);
        var digest = Digest(binary);
        var profile = new string('E', 64);
        var workspace = home.Workspace;
        var samples = new Dictionary<string, string>
        {
            [".cs"] = "partial class Source { static int Value(int x) => x + 1; }\n",
            [".py"] = "def total():\n    local_value = 1\n    return local_value + 2\n",
            [".rs"] = "fn value() { let local_value = 1; local_value + 2; }\n",
            [".fluxr"] = "fluxr 1\npub run 3 x int factor int offset int int ret add mul x factor offset\n",
        };
        foreach (var (extension, source) in samples)
        {
            await File.WriteAllTextAsync(Path.Combine(workspace.Root, "A" + extension), source);
            await File.WriteAllTextAsync(Path.Combine(workspace.Root, "B" + extension), source);
        }
        MigrationStateBridge.TransferToNative(workspace, binary, digest, profile, "native-check-scheduler-test");
        var marker = JsonNode.Parse(File.ReadAllText(Path.Combine(workspace.StateDirectory, "migration-engine.json")))!;
        var epoch = marker["epoch"]!.GetValue<string>();
        var receipt = Path.Combine(workspace.StateDirectory, $"native-{epoch}.checks.json");
        try
        {
            var assembly = Path.Combine(AppContext.BaseDirectory, "SlopHawk.Providers.CSharp.Worker.dll");
            var config = Path.Combine(workspace.StateDirectory, "native-checkers.json");
            await File.WriteAllTextAsync(config, JsonSerializer.Serialize(new
            {
                version = 1, rootIdentity = marker["rootIdentity"]!.GetValue<string>(),
                context = new { key = "native-scheduler-fixture", targetFramework = "net10.0", languageVersion = "CSharp14",
                    assemblyName = "Fixture", referencePack = pack, fluxrModule = "fixture@checked-v1",
                    referenceClosureComplete = true, sourceClosureComplete = true },
                worker = new { dotnet = new { path = host, sha256 = Digest(host) }, assembly = new { path = assembly, sha256 = Digest(assembly) } },
            }));
            if (!OperatingSystem.IsWindows()) File.SetUnixFileMode(config, UnixFileMode.UserRead | UnixFileMode.UserWrite);

            await Run("start");
            var first = await WaitForReceipt();
            var pairs = first["report"]!["pairs"]!.AsArray();
            Assert.True(pairs.Select(pair => pair!["language"]!.GetValue<string>()).Order()
                .SequenceEqual(new[] { "csharp", "fluxr", "python", "rust" }), first["report"]!.ToJsonString());
            var firstAttempt = first["attempt"]!.GetValue<string>();
            await Stop();
            await Run("start");
            Assert.Equal(firstAttempt, (await WaitForReceipt())["attempt"]!.GetValue<string>());
            await Stop();

            var interrupted = JsonNode.Parse(await File.ReadAllTextAsync(receipt))!;
            interrupted["outcome"] = "running";
            interrupted["report"] = null;
            await File.WriteAllTextAsync(receipt, interrupted.ToJsonString());
            await Run("start");
            var recovered = await WaitForReceipt(firstAttempt);
            Assert.Equal(first["scopeSha256"]!.GetValue<string>(), recovered["scopeSha256"]!.GetValue<string>());

            await File.WriteAllTextAsync(Path.Combine(workspace.Root, "B.py"), samples[".py"].Replace("+ 2", "+ 3", StringComparison.Ordinal));
            await Run("recheck");
            var changed = await WaitForReceipt(recovered["attempt"]!.GetValue<string>());
            Assert.NotEqual(recovered["scopeSha256"]!.GetValue<string>(), changed["scopeSha256"]!.GetValue<string>());
            Assert.DoesNotContain(changed["report"]!["pairs"]!.AsArray(), pair => pair!["language"]!.GetValue<string>() == "python");
        }
        finally
        {
            MigrationStateBridge.RollbackToLegacy(workspace, binary, digest, profile, "native-check-scheduler-test");
        }
        MigrationStateBridge.RequireLegacyWriter(workspace);
        Assert.True(File.Exists(receipt));

        static string Digest(string path) => Convert.ToHexString(SHA256.HashData(File.ReadAllBytes(path)));

        async Task<JsonNode> Run(string verb)
        {
            var start = new ProcessStartInfo(binary) { UseShellExecute = false, RedirectStandardOutput = true, RedirectStandardError = true };
            foreach (var argument in new[] { "--native-workspace-v1", workspace.Root, workspace.StateDirectory, profile, verb })
                start.ArgumentList.Add(argument);
            using var process = Process.Start(start)!;
            using var deadline = new CancellationTokenSource(TimeSpan.FromSeconds(10));
            var output = process.StandardOutput.ReadToEndAsync(deadline.Token);
            var error = process.StandardError.ReadToEndAsync(deadline.Token);
            await process.WaitForExitAsync(deadline.Token);
            Assert.True(process.ExitCode == 0, await error);
            return JsonNode.Parse(await output)!;
        }

        async Task Stop()
        {
            Assert.True((await Run("stop"))["stopping"]!.GetValue<bool>());
            using var deadline = new CancellationTokenSource(TimeSpan.FromSeconds(8));
            while (File.Exists(Path.Combine(workspace.StateDirectory, "native-daemon.json")) ||
                   File.Exists(Path.Combine(workspace.Root, ".slophawk", "results.json")))
                await Task.Delay(50, deadline.Token);
        }

        async Task<JsonNode> WaitForReceipt(string? previous = null)
        {
            using var deadline = new CancellationTokenSource(TimeSpan.FromSeconds(24));
            while (true)
            {
                var scheduler = (await Run("status"))["checkerScheduler"];
                var state = scheduler?["state"]?.GetValue<string>();
                Assert.False(state is "failed" or "blocked", scheduler?.ToJsonString());
                if (state is "complete" or "partial" && scheduler!["attempt"]!.GetValue<string>() != previous)
                    return scheduler;
                await Task.Delay(75, deadline.Token);
            }
        }
    }

    [Fact]
    public async Task RustProjectsRoutesPinnedMsbuildFactsWithTheManagedPublicShape()
    {
        var binary = Environment.GetEnvironmentVariable("SLOPHAWK_NATIVE_MIGRATION_BINARY");
        var host = Environment.GetEnvironmentVariable("SLOPHAWK_PROJECT_TEST_DOTNET");
        if (string.IsNullOrWhiteSpace(binary) || string.IsNullOrWhiteSpace(host)) return;
        using var home = new TempWorkspace();
        var root = home.Workspace.Root;
        var sdk = typeof(Providers.CSharp.Worker.Program).Assembly.GetCustomAttributes<AssemblyMetadataAttribute>()
            .Single(attribute => attribute.Key == "SlopHawk.BuildSdk").Value;
        await File.WriteAllTextAsync(Path.Combine(root, "global.json"),
            JsonSerializer.Serialize(new { sdk = new { version = sdk, rollForward = "disable", allowPrerelease = false } }));
        await File.WriteAllTextAsync(Path.Combine(root, "App.csproj"), """
            <Project Sdk="Microsoft.NET.Sdk"><PropertyGroup><TargetFramework>net10.0</TargetFramework></PropertyGroup></Project>
            """);
        await File.WriteAllTextAsync(Path.Combine(root, "Source.cs"), "internal class Source {}\n");
        var worker = Path.Combine(AppContext.BaseDirectory, "SlopHawk.Providers.CSharp.Worker.dll");
        using var deadline = new CancellationTokenSource(TimeSpan.FromSeconds(60));
        var expected = await new ProjectEvaluator(home.Workspace).EvaluateAsync(deadline.Token);
        Assert.True(expected.Complete, JsonSerializer.Serialize(expected.Issues));
        var start = new ProcessStartInfo(binary) { UseShellExecute = false, CreateNoWindow = true,
            RedirectStandardOutput = true, RedirectStandardError = true };
        foreach (var arg in new[] { "projects", "--root", root }) start.ArgumentList.Add(arg);
        start.Environment["SLOPHAWK_PROJECT_WORKER"] = worker;
        start.Environment["SLOPHAWK_PROJECT_WORKER_SHA256"] = Convert.ToHexString(SHA256.HashData(await File.ReadAllBytesAsync(worker)));
        start.Environment["SLOPHAWK_NATIVE_DOTNET"] = host;
        start.Environment["SLOPHAWK_NATIVE_DOTNET_SHA256"] = Convert.ToHexString(SHA256.HashData(await File.ReadAllBytesAsync(host)));
        using var child = Process.Start(start)!;
        var output = child.StandardOutput.ReadToEndAsync(deadline.Token);
        var error = child.StandardError.ReadToEndAsync(deadline.Token);
        await child.WaitForExitAsync(deadline.Token);
        var actual = await output;
        Assert.True(child.ExitCode == 0, $"Exit {child.ExitCode}\n{await error}\n{actual}");
        var graph = JsonNode.Parse(actual);
        var managed = JsonSerializer.SerializeToNode(expected, new JsonSerializerOptions(JsonSerializerDefaults.Web));
        Assert.True(JsonNode.DeepEquals(managed, graph), $"Managed {managed}\nNative {graph}");
        Assert.Single(graph!["targets"]!.AsArray());
    }

    [Fact]
    public void InstalledRustPeerAbortsAnUnconfirmedImport()
    {
        var binary = Environment.GetEnvironmentVariable("SLOPHAWK_NATIVE_MIGRATION_BINARY");
        if (string.IsNullOrWhiteSpace(binary)) return;
        using var home = new TempWorkspace();
        Seed(home);
        var digest = Convert.ToHexString(SHA256.HashData(File.ReadAllBytes(binary)));
        var profile = new string('A', 64);
        MigrationStateBridge.TransferToNative(home.Workspace, binary, digest, profile, "native-integration");

        // Model the crash interval between native import and the managed commit.
        var path = Path.Combine(home.Workspace.StateDirectory, "migration-engine.json");
        var marker = JsonNode.Parse(File.ReadAllText(path))!;
        var epoch = marker["epoch"]!.GetValue<string>();
        marker["state"] = "transition-uncertain";
        File.WriteAllText(path, marker.ToJsonString());
        Assert.Throws<IOException>(() => MigrationStateBridge.RequireLegacyWriter(home.Workspace));
        Assert.Throws<IOException>(() => MigrationStateBridge.RequireNativeWriter(home.Workspace, digest, profile));

        MigrationStateBridge.RollbackToLegacy(home.Workspace, binary, digest, profile, "native-integration");
        MigrationStateBridge.RequireLegacyWriter(home.Workspace);
        Assert.Equal("aborted", File.ReadAllText(Path.Combine(home.Workspace.StateDirectory, $"native-{epoch}.stopped")));
        using var restored = Open(home.Workspace);
        Assert.Equal(7L, Scalar(restored, "SELECT revision FROM workspace"));
        Assert.Equal("receipt-7", Text(restored, "SELECT reason_code FROM test_run_receipt"));
    }

    [Fact]
    public void SemanticExportPreservesPredecessorPendingReceiptsAndInterruptedLeaseIdentity()
    {
        using var home = new TempWorkspace();
        Seed(home);
        var root = MigrationStateBridge.ReadRootIdentity(home.Workspace);
        var inventory = new SourceInventory(home.Workspace);
        var snapshot = inventory.ExportMigration(root);

        Assert.Equal(SourceInventory.MigrationFormat, snapshot.Format);
        Assert.Equal(7L, snapshot.Revision);
        Assert.Equal("graph:5", Assert.Single(snapshot.Tables[3].Rows)[6].Value);
        Assert.Equal("interrupted", Assert.Single(snapshot.Tables[5].Rows)[8].Value);
        Assert.Equal("lease-11", Assert.Single(snapshot.Tables[5].Rows)[11].Value);
        Assert.Equal("receipt-7", Assert.Single(snapshot.Tables[8].Rows)[27].Value);
        Assert.Equal("old-fingerprint", Assert.Single(snapshot.Tables[3].Rows)[3].Value);
    }

    [Fact]
    public void RoundTripAfterDrainCarriesDetailedAndFallbackCausesAcrossFreshHandles()
    {
        using var home = new TempWorkspace();
        Seed(home);
        var root = MigrationStateBridge.ReadRootIdentity(home.Workspace);
        var inventory = new SourceInventory(home.Workspace);
        var export = inventory.ExportMigration(root);
        inventory.ImportMigration(export, root, 7);
        using var connection = Open(home.Workspace);
        Assert.Equal(7L, Scalar(connection, "SELECT revision FROM workspace"));
        Assert.Equal("graph:5", Text(connection, "SELECT predecessor FROM test_intent"));
        Assert.Equal(3L, Scalar(connection, "SELECT first_revision FROM intent_fallback"));
        Assert.Equal(7L, Scalar(connection, "SELECT last_revision FROM intent_fallback"));
        Assert.Equal("interrupted", Text(connection, "SELECT state FROM test_attempt"));
        Assert.Equal("lease-11", Text(connection, "SELECT lease_id FROM test_attempt"));
        Assert.Equal("profile-11", Text(connection, "SELECT profile_revision FROM test_attempt"));
        Assert.Equal("receipt-7", Text(connection, "SELECT reason_code FROM test_run_receipt"));
        Assert.Equal("old-fingerprint", Text(connection, "SELECT old_digest FROM test_intent"));
        Assert.Equal("changed-fingerprint", Text(connection, "SELECT new_digest FROM test_intent"));
        Assert.Equal(1L, Scalar(connection, "SELECT count(*) FROM test_evidence"));
        Assert.Equal(1L, Scalar(connection, "SELECT count(*) FROM project_graph"));
    }

    [Fact]
    public void UnsupportedInterchangeIsRejectedBeforeAnyMutation()
    {
        using var home = new TempWorkspace();
        Seed(home);
        var root = MigrationStateBridge.ReadRootIdentity(home.Workspace);
        var inventory = new SourceInventory(home.Workspace);
        var snapshot = inventory.ExportMigration(root);
        Assert.Throws<IOException>(() => inventory.ImportMigration(snapshot with { Format = 99 }, root, 7));
        using var verify = Open(home.Workspace);
        Assert.Equal("running", Text(verify, "SELECT state FROM test_attempt"));
        Assert.Equal("graph:5", Text(verify, "SELECT predecessor FROM test_intent"));
        Assert.Equal(1L, Scalar(verify, "SELECT count(*) FROM test_run_receipt"));
    }

    [Fact]
    public void UnsupportedDurableSchemaBlocksExportWithoutUpgradingOrDeletingPendingWork()
    {
        using var home = new TempWorkspace();
        Seed(home);
        using (var connection = Open(home.Workspace))
            Exec(connection, "PRAGMA user_version = 6;");
        var root = MigrationStateBridge.ReadRootIdentity(home.Workspace);
        Assert.Throws<IOException>(() => new SourceInventory(home.Workspace).ExportMigration(root));
        using var verify = Open(home.Workspace);
        Assert.Equal(6L, Scalar(verify, "PRAGMA user_version"));
        Assert.Equal(1L, Scalar(verify, "SELECT count(*) FROM test_intent"));
        Assert.Equal(1L, Scalar(verify, "SELECT count(*) FROM test_run_receipt"));
    }

    [Fact]
    public void RevisionFencePreventsOldRollbackFromReplacingLaterCommit()
    {
        using var home = new TempWorkspace();
        Seed(home);
        var root = MigrationStateBridge.ReadRootIdentity(home.Workspace);
        var inventory = new SourceInventory(home.Workspace);
        var snapshot = inventory.ExportMigration(root);
        using (var connection = Open(home.Workspace))
            Exec(connection, "UPDATE workspace SET revision = 8 WHERE id = 1;");
        Assert.Throws<IOException>(() => inventory.ImportMigration(snapshot, root, 7));
        using var verify = Open(home.Workspace);
        Assert.Equal(8L, Scalar(verify, "SELECT revision FROM workspace"));
        Assert.Equal("graph:5", Text(verify, "SELECT predecessor FROM test_intent"));
    }

    private static void Seed(TempWorkspace home)
    {
        var inventory = new SourceInventory(home.Workspace);
        _ = inventory.Load(); // create pinned schema-v5 tables in the test's isolated user state
        var root = MigrationStateBridge.ReadRootIdentity(home.Workspace);
        using var connection = Open(home.Workspace);
        using var transaction = connection.BeginTransaction();
        using var command = connection.CreateCommand();
        command.Transaction = transaction;
        command.CommandText = "UPDATE workspace SET revision = 7, root_identity = $root, " +
            "policy = 'csharp.syntax:inventory-v1' WHERE id = 1";
        command.Parameters.AddWithValue("$root", root);
        command.ExecuteNonQuery();
        command.Parameters.Clear();
        command.CommandText = "INSERT INTO source VALUES ('SOURCE.CS','Source.cs','changed-fingerprint',1)";
        command.ExecuteNonQuery();
        command.CommandText = "INSERT INTO test_intent VALUES " +
            "(6,'SOURCE.CS','Source.cs','old-fingerprint','Source.cs','changed-fingerprint','graph:5')";
        command.ExecuteNonQuery();
        command.CommandText = "INSERT INTO intent_fallback VALUES (1,3,7)";
        command.ExecuteNonQuery();
        command.CommandText = "INSERT INTO test_attempt " +
            "(id,number,revision,epoch,target,fingerprint,causes,plan_key,state,lease_id,profile_revision) " +
            "VALUES (1,1,7,'00000000-0000-0000-0000-000000000001','unit','fp-1','[\"cause-6\"]','plan-7'," +
            "'running','lease-11','profile-11')";
        command.ExecuteNonQuery();
        command.CommandText = "INSERT INTO test_evidence VALUES ('cause-6','unit','fp-1')";
        command.ExecuteNonQuery();
        command.Parameters.AddWithValue("$root", root);
        command.CommandText = "INSERT INTO project_graph VALUES (7,$root,1,x'7B7D')";
        command.ExecuteNonQuery();
        command.CommandText = "INSERT INTO test_run_receipt " +
            "(attempt_id,format,revision,epoch,fence_sequence,git_fingerprint,root_identity,target," +
            "fingerprint,profile_revision,platform,runner,artifact,execution,reason_code) VALUES " +
            "(1,1,7,'00000000-0000-0000-0000-000000000001',5,'" + new string('A', 64) +
            "',$root,'unit','fp-1','profile-11','Incomplete','Incomplete','Incomplete','Unknown','receipt-7')";
        command.ExecuteNonQuery();
        transaction.Commit();
    }

    private static SqliteConnection Open(Workspace workspace)
    {
        var connection = new SqliteConnection(new SqliteConnectionStringBuilder
        {
            DataSource = workspace.InventoryPath, Pooling = false, Mode = SqliteOpenMode.ReadWrite
        }.ToString());
        connection.Open();
        return connection;
    }
    private static void Exec(SqliteConnection connection, string sql)
    {
        using var command = connection.CreateCommand();
        command.CommandText = sql;
        command.ExecuteNonQuery();
    }
    private static long Scalar(SqliteConnection connection, string sql)
    {
        using var command = connection.CreateCommand();
        command.CommandText = sql;
        return (long)command.ExecuteScalar()!;
    }
    private static string Text(SqliteConnection connection, string sql)
    {
        using var command = connection.CreateCommand();
        command.CommandText = sql;
        return (string)command.ExecuteScalar()!;
    }

    private sealed class TempWorkspace : IDisposable
    {
        internal string Parent { get; } = Path.Combine(Path.GetTempPath(), "slophawk-engine-" + Guid.NewGuid().ToString("N"));
        internal Workspace Workspace { get; }
        internal TempWorkspace()
        {
            var root = Directory.CreateDirectory(Path.Combine(Parent, "checkout")).FullName;
            Workspace = new Workspace(root, Directory.CreateDirectory(Path.Combine(Parent, "state")).FullName);
        }
        public void Dispose() => Directory.Delete(Parent, recursive: true);
    }
}
