using System.Diagnostics;
using System.Text.Json;
using SlopGuard.Daemon;
using SlopGuard.Providers.CSharp;

if (args is [] or ["--help"] or ["-h"])
{
    Console.WriteLine("""
        SlopGuard 0.1.0
        Start         slopguard start [--root <checkout>]
        Status        slopguard status [--root <checkout>]
        Read results  slopguard results [--root <checkout>]
        Recheck       slopguard recheck [--root <checkout>]
        MCP server    slopguard mcp [--root <checkout>]
        Stop          slopguard stop [--root <checkout>]
        """);
    return 0;
}

if (args is not [_] and not [_, "--root", _])
{
    Console.Error.WriteLine("Invalid arguments. Run slopguard --help.");
    return 2;
}

try
{
    var workspace = new Workspace(args.Length == 3 ? args[2] : Environment.CurrentDirectory);
    var connection = new DaemonConnection(workspace);
    switch (args[0])
    {
        case "serve":
            DaemonHost.Run(workspace, new SyntaxChecker());
            return 0;
        case "start":
            return Print(await StartAsync(workspace, connection));
        case "mcp":
            if (await StartAsync(workspace, connection) is not { Ok: true })
                throw new IOException("The workspace daemon did not start.");
            await SlopGuard.Cli.McpBridge.RunAsync(connection);
            return 0;
        case "status":
            return Print(await connection.SendAsync(DaemonCommand.Status));
        case "results":
            return Print(await connection.SendAsync(DaemonCommand.Results));
        case "recheck":
            return Print(await connection.SendAsync(DaemonCommand.Recheck));
        case "stop":
            return Print(await connection.SendAsync(DaemonCommand.Stop));
        default:
            Console.Error.WriteLine("Unknown command. Run slopguard --help.");
            return 2;
    }
}
catch (Exception ex) when (ex is IOException or UnauthorizedAccessException or ArgumentException
                           or System.ComponentModel.Win32Exception or JsonException)
{
    Console.Error.WriteLine(ex.Message);
    return 1;
}

static int Print(DaemonReply? reply)
{
    if (reply is not { Ok: true })
    {
        Console.Error.WriteLine(reply?.Error ?? "No daemon is listening in this workspace. Run slopguard start.");
        return 1;
    }
    Console.WriteLine(JsonSerializer.Serialize(reply.Payload, new JsonSerializerOptions { WriteIndented = true }));
    return 0;
}

static async Task<DaemonReply?> StartAsync(Workspace workspace, DaemonConnection connection)
{
    if (await connection.SendAsync(DaemonCommand.Status) is { Ok: true } existing)
        return existing;

    var executable = Environment.ProcessPath ?? throw new IOException("Could not locate the SlopGuard executable.");
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
        launcher.ArgumentList.Add(typeof(SlopGuard.Cli.McpBridge).Assembly.Location);
    launcher.ArgumentList.Add("serve");
    launcher.ArgumentList.Add("--root");
    launcher.ArgumentList.Add(workspace.Root);

    using var child = Process.Start(launcher) ?? throw new IOException("Could not start the workspace daemon.");
    using var deadline = new CancellationTokenSource(TimeSpan.FromSeconds(8));
    try
    {
        while (true)
        {
            if (await connection.SendAsync(DaemonCommand.Status, deadline.Token) is { Ok: true } started)
                return started;
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
