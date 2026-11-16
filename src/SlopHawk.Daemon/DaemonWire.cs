namespace SlopHawk.Daemon;

// Private single-request pipe framing, independent of MCP's public initialize lifecycle.
internal static class DaemonWire
{
    internal const byte Hello = 5;
    internal const byte VersionedRequest = 6;
    internal const byte Version = 1;
    internal const int MaximumResponseBytes = ReportFile.MaximumBytes + 4_096;
    internal const int MaximumConnections = 8;
    internal static readonly TimeSpan ReadDeadline = TimeSpan.FromMilliseconds(750);
    internal static readonly TimeSpan OperationDeadline = TimeSpan.FromSeconds(5);
    internal static readonly TimeSpan AcknowledgementDeadline = TimeSpan.FromSeconds(1);

    internal static string? Capability(DaemonCommand command) => command switch
    {
        DaemonCommand.Status => "status",
        DaemonCommand.Results => "results",
        DaemonCommand.Recheck => "recheck",
        DaemonCommand.Stop => "stop",
        _ => null
    };
}
