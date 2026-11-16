namespace SlopHawk.Daemon;

public sealed record CompileMember(string Path, string? Link);

public sealed record ProjectTarget(string Id, string ProjectPath, string Framework, string Configuration,
    string Platform, string RuntimeIdentifier, string SdkVersion, bool IsTestProject,
    IReadOnlyList<CompileMember> Compile, IReadOnlyList<string> References, string EvaluationFingerprint);

public sealed record ProjectEvaluationIssue(string Path, string Reason);

public sealed record ProjectGraph(IReadOnlyList<ProjectTarget> Targets, IReadOnlyList<ProjectEvaluationIssue> Issues)
{
    public bool Complete => Issues.Count == 0;
}
