namespace SlopGuard.Core;

public interface ISourceChecker
{
    string Id { get; }
    string Language { get; }
    bool Matches(string path);
    SourceAnalysis Analyze(string source, CancellationToken cancellationToken);
}

public sealed record SourceAnalysis(IReadOnlyList<Diagnostic> Diagnostics, bool Truncated);

public sealed record Diagnostic(string Rule, string Message, int Line, int Column);

public sealed record Finding(
    string Path, string SourceDigest, string Rule, string Message, int Line, int Column);

public sealed record WorkspaceReport(
    int SchemaVersion, string Producer, string Root, Guid Epoch, long Revision,
    string? Branch, string? Commit, string Checker, string Language,
    int FileCount, bool Partial, bool FindingsTruncated, IReadOnlyList<Finding> Findings)
{
    public long CommittedRevision { get; init; }
}
