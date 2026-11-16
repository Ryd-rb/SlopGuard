using System.Diagnostics;
using System.Text.Json;
using SlopHawk.Daemon;

if (args is [] or ["--help"] or ["-h"])
{
    Console.WriteLine("""
        SlopHawk 0.2.0-preview.1
        Start         slophawk start [--root <checkout>]
        Status        slophawk status [--root <checkout>]
        Read results  slophawk results [--root <checkout>]
        Projects      slophawk projects [--root <checkout>]
        Recheck       slophawk recheck [--root <checkout>]
        Check         slophawk check <assembly.dll> <cases.json> <workspace> [run-verb [prepare-verb]]
        Observe       slophawk check-observe <assembly.dll> <requests.json> <workspace> [run-verb [prepare-verb]]
        Review checks slophawk check-review <workspace> [acknowledgments.json [scope.json]]
        Plan probes   slophawk probes plan <seeds.json> <per-operation> [seed [scales.json]]
        Compare       slophawk probes compare <probes.json> <candidate.json> <reference.json>
        Compare IL    slophawk probes methods <candidate.dll> <reference.dll>
        MCP server    slophawk mcp [--root <checkout>]
        Stop          slophawk stop [--root <checkout>]
        """);
    return 0;
}

if (args is not ["probes", ..] and not ["check" or "check-observe" or "check-review", ..] and not [_] and not [_, "--root", _])
{
    Console.Error.WriteLine("Invalid arguments. Run slophawk --help.");
    return 2;
}

try
{
    if (args is ["probes", ..])
        return await SlopHawk.Cli.ProbeCommands.RunAsync(args);
    if (args is ["check" or "check-observe", ..])
        return await SlopHawk.Cli.ForegroundCheck.RunAsync(args);
    if (args is ["check-review", ..])
        return await SlopHawk.Cli.CheckReview.RunAsync(args);

    var workspace = new Workspace(args.Length == 3 ? args[2] : Environment.CurrentDirectory);
    var connection = new DaemonConnection(workspace);
    switch (args[0])
    {
        case "serve":
            DaemonHost.Run(workspace);
            return 0;
        case "start":
            return Print(await StartAsync(workspace, connection));
        case "mcp":
            var ready = await StartAsync(workspace, connection);
            if (ready is not { Ok: true })
                throw new IOException(ready?.Error ?? "The workspace daemon did not start.");
            await SlopHawk.Cli.McpBridge.RunAsync(connection);
            return 0;
        case "status":
            return Print(await connection.SendAsync(DaemonCommand.Status));
        case "results":
            return Print(await connection.SendAsync(DaemonCommand.Results));
        case "projects":
            var graph = await new ProjectEvaluator(workspace).EvaluateAsync();
            Console.WriteLine(JsonSerializer.Serialize(graph, new JsonSerializerOptions(JsonSerializerDefaults.Web)
            {
                WriteIndented = true
            }));
            return graph.Complete ? 0 : 1;
        case "recheck":
            return Print(await connection.SendAsync(DaemonCommand.Recheck));
        case "stop":
            return Print(await connection.SendAsync(DaemonCommand.Stop));
        default:
            Console.Error.WriteLine("Unknown command. Run slophawk --help.");
            return 2;
    }
}
catch (Exception ex) when (ex is IOException or UnauthorizedAccessException or ArgumentException
                           or System.ComponentModel.Win32Exception or JsonException or BadImageFormatException or OverflowException)
{
    Console.Error.WriteLine(ex.Message);
    return 1;
}

static int Print(DaemonReply? reply)
{
    if (reply is not { Ok: true })
    {
        Console.Error.WriteLine(reply?.Error ?? "No daemon is listening in this workspace. Run slophawk start.");
        return 1;
    }
    Console.WriteLine(JsonSerializer.Serialize(reply.Payload, new JsonSerializerOptions { WriteIndented = true }));
    return 0;
}

static async Task<DaemonReply?> StartAsync(Workspace workspace, DaemonConnection connection)
{
    var initial = await connection.SendAsync(DaemonCommand.Status);
    if (initial is { Ok: true })
        return RequireCompatibleDaemon(initial);
    if (initial is { Code: not null })
        return initial; // Never replace or spawn over a daemon that reported a mismatch.

    var executable = Environment.ProcessPath ?? throw new IOException("Could not locate the SlopHawk executable.");
    var launcher = new ProcessStartInfo(executable)
    {
        WorkingDirectory = workspace.Root,
        UseShellExecute = false,
        CreateNoWindow = true,
        RedirectStandardInput = true,
        RedirectStandardOutput = true,
        RedirectStandardError = true,
    };
    if (Path.GetFileNameWithoutExtension(executable).Equals("dotnet", StringComparison.OrdinalIgnoreCase))
        launcher.ArgumentList.Add(typeof(SlopHawk.Cli.McpBridge).Assembly.Location);
    launcher.ArgumentList.Add("serve");
    launcher.ArgumentList.Add("--root");
    launcher.ArgumentList.Add(workspace.Root);

    using var child = Process.Start(launcher) ?? throw new IOException("Could not start the workspace daemon.");
    using var deadline = new CancellationTokenSource(TimeSpan.FromSeconds(8));
    try
    {
        while (true)
        {
            var status = await connection.SendAsync(DaemonCommand.Status, deadline.Token);
            if (status is { Ok: true })
                return RequireCompatibleDaemon(status);
            if (status is { Code: not null })
                return status;
            if (child.HasExited && child.ExitCode != 0)
                throw new IOException((await child.StandardError.ReadToEndAsync(deadline.Token)).Trim());
            await Task.Delay(70, deadline.Token);
        }
    }
    catch (OperationCanceledException)
    {
        throw new IOException("The workspace daemon did not become ready within eight seconds.");
    }
}

static DaemonReply RequireCompatibleDaemon(DaemonReply reply)
{
    if (reply.Payload is { } status && status.TryGetProperty("reportSchemaVersion", out var version) &&
        version.ValueKind == JsonValueKind.Number && version.GetInt32() == 2)
        return reply;
    throw new IOException("An older or incompatible SlopHawk daemon is running. Use slophawk stop before starting this build.");
}
