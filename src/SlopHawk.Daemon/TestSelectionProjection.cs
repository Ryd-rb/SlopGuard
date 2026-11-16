namespace SlopHawk.Daemon;

// A diagnostic view of the *evaluated* selection, not a second source of admission decisions.
// The scheduler still consumes TestSelection.Targets and TestSelection.Complete alone.
internal enum TestSelectionReasonKind
{
    CurrentMembership,
    CurrentDependency,
    PredecessorMembership,
    PredecessorDependency,
    FirstRevisionFallback,
    MissingPredecessorFallback
}

internal enum TestSelectionTargetScope { Selected, BudgetDeferred, Unapproved }

internal sealed record TestSelectionReason(string CauseId, TestSelectionReasonKind Kind, string? SourcePath,
    IReadOnlyList<string> TargetReferenceChain, bool ChainTruncated);

internal sealed record TestTargetExplanation(string TargetId, TestSelectionTargetScope Scope,
    IReadOnlyList<TestSelectionReason> Reasons, int OmittedReasons);

internal sealed record TestSelectionFrontier(string? Predecessor, string Path, string Reason);

internal sealed record TestSelectionProjection(
    IReadOnlyList<TestTargetExplanation> Targets,
    IReadOnlyList<TestSelectionFrontier> UnresolvedFrontiers,
    int OmittedTargetExplanations,
    int OmittedFrontiers,
    int ApprovedAffectedTargets,
    int BudgetDeferredTargets,
    int UnapprovedAffectedTargets,
    bool ConservativeFallback);
