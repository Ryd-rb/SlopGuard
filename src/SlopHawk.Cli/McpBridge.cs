using ModelContextProtocol.Protocol;
using ModelContextProtocol.Server;
using SlopHawk.Daemon;

namespace SlopHawk.Cli;

internal static class McpBridge
{
    internal static async Task RunAsync(DaemonConnection connection)
    {
        var negotiation = await connection.NegotiateAsync();
        if (negotiation.Failure is { } failure)
            throw new IOException(failure.Error);
        var capabilities = negotiation.Handshake?.Capabilities
            ?? throw new IOException("No workspace daemon is listening.");

        var tools = new McpServerPrimitiveCollection<McpServerTool>();
        if (capabilities.Contains("status", StringComparer.Ordinal))
            tools.Add(Create("workspace_status", "Read source reconciliation and test scheduling status.",
                DaemonCommand.Status, readOnly: true, connection));
        if (capabilities.Contains("results", StringComparer.Ordinal))
            tools.Add(Create("get_findings", "Read the validated source inventory report.",
                DaemonCommand.Results, readOnly: true, connection));
        if (capabilities.Contains("recheck", StringComparer.Ordinal))
            tools.Add(Create("run_checks", "Queue a full source inventory reconciliation.",
                DaemonCommand.Recheck, readOnly: false, connection));
        if (capabilities.Contains("list_checkers", StringComparer.Ordinal))
            tools.Add(McpServerTool.Create((Func<CancellationToken, Task<string[]>>)(async token =>
            {
                var current = await connection.NegotiateAsync(token);
                if (current.Failure is { } currentFailure)
                    throw new IOException(currentFailure.Error);
                if (current.Handshake?.Capabilities.Contains("list_checkers", StringComparer.Ordinal) != true)
                    throw new IOException("The running workspace daemon does not advertise 'list_checkers'.");
                return [];
            }), new McpServerToolCreateOptions
            {
                Name = "list_checkers",
                Description = "List inspection checkers registered in this version of SlopHawk.",
                ReadOnly = true,
            }));
        var options = new McpServerOptions
        {
            ServerInfo = new Implementation { Name = "SlopHawk", Version = "0.2.0-preview.1" },
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
