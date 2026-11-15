using ModelContextProtocol.Protocol;
using ModelContextProtocol.Server;
using SlopGuard.Daemon;
using SlopGuard.Providers.CSharp;

namespace SlopGuard.Cli;

internal static class McpBridge
{
    internal static async Task RunAsync(DaemonConnection connection)
    {
        var tools = new McpServerPrimitiveCollection<McpServerTool>();
        tools.Add(Create("workspace_status", "Read the workspace revision and syntax checker state.",
            DaemonCommand.Status, readOnly: true, connection));
        tools.Add(Create("get_findings", "Read validated findings for the current workspace revision.",
            DaemonCommand.Results, readOnly: true, connection));
        tools.Add(Create("run_checks", "Queue a full recheck with the C# syntax checker.",
            DaemonCommand.Recheck, readOnly: false, connection));
        tools.Add(McpServerTool.Create((Func<object>)(() =>
        {
            var checker = new SyntaxChecker();
            return new { checker.Id, checker.Language, coverage = "syntax" };
        }), new McpServerToolCreateOptions
        {
            Name = "list_checkers",
            Description = "List the checker available in this version of SlopGuard.",
            ReadOnly = true,
        }));
        var options = new McpServerOptions
        {
            ServerInfo = new Implementation { Name = "SlopGuard", Version = "0.1.0" },
            ToolCollection = tools,
        };
        await using var server = McpServer.Create(new StdioServerTransport(options), options);
        await server.RunAsync();
    }

    private static McpServerTool Create(
        string name, string description, DaemonCommand command, bool readOnly, DaemonConnection connection)
    {
        return McpServerTool.Create(
            (Func<CancellationToken, Task<DaemonReply>>)(async token =>
                await connection.SendAsync(command, token)
                ?? new DaemonReply(false, null, "No workspace daemon is listening.")),
            new McpServerToolCreateOptions { Name = name, Description = description, ReadOnly = readOnly });
    }
}
