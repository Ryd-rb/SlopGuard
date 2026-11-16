namespace SlopHawk.Core;

public sealed record WorkspaceReport(
    int SchemaVersion, string Producer, string Root, Guid Epoch, long Revision,
    string? Branch, string? Commit, int FileCount, bool Partial, long CommittedRevision);
