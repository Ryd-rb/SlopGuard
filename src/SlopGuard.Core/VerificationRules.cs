using System.Collections.ObjectModel;
using System.Text.Json.Serialization;

namespace SlopGuard.Core.Verification;

/// <summary>A caller-supplied, already normalized change signal. No source detection is performed here.</summary>
[JsonConverter(typeof(JsonStringEnumConverter<VerificationChangeKind>))]
public enum VerificationChangeKind
{
    [JsonStringEnumMemberName("edited-branch")] EditedBranch,
    [JsonStringEnumMemberName("public-contract")] PublicContract,
    [JsonStringEnumMemberName("serialization")] Serialization
}

[JsonConverter(typeof(JsonStringEnumConverter<VerificationCategory>))]
public enum VerificationCategory
{
    [JsonStringEnumMemberName("coverage-tests")] CoverageTests,
    [JsonStringEnumMemberName("contract-tests")] ContractTests
}
[JsonConverter(typeof(JsonStringEnumConverter<VerificationPolicyMode>))]
public enum VerificationPolicyMode
{
    [JsonStringEnumMemberName("disabled")] Disabled,
    [JsonStringEnumMemberName("advisory")] Advisory,
    [JsonStringEnumMemberName("required")] Required
}
[JsonConverter(typeof(JsonStringEnumConverter<VerificationState>))]
public enum VerificationState
{
    [JsonStringEnumMemberName("observed")] Observed,
    [JsonStringEnumMemberName("uncovered")] Uncovered,
    [JsonStringEnumMemberName("configured")] Configured,
    [JsonStringEnumMemberName("stale")] Stale,
    [JsonStringEnumMemberName("missing")] Missing,
    [JsonStringEnumMemberName("unknown")] Unknown
}
[JsonConverter(typeof(JsonStringEnumConverter<VerificationStrength>))]
public enum VerificationStrength
{
    [JsonStringEnumMemberName("none")] None,
    [JsonStringEnumMemberName("declared")] Declared,
    [JsonStringEnumMemberName("possible-static-path")] PossibleStaticPath,
    [JsonStringEnumMemberName("exact-static-path")] ExactStaticPath,
    [JsonStringEnumMemberName("verified-measurement")] VerifiedMeasurement,
    [JsonStringEnumMemberName("measured-execution")] MeasuredExecution
}
[JsonConverter(typeof(JsonStringEnumConverter<VerificationEvidenceKind>))]
public enum VerificationEvidenceKind
{
    [JsonStringEnumMemberName("configuration")] Configuration,
    [JsonStringEnumMemberName("static-path")] StaticPath,
    [JsonStringEnumMemberName("test-run")] TestRun,
    [JsonStringEnumMemberName("branch-counter")] BranchCounter,
    [JsonStringEnumMemberName("contract-execution")] ContractExecution
}
[JsonConverter(typeof(JsonStringEnumConverter<VerificationBinding>))]
public enum VerificationBinding
{
    [JsonStringEnumMemberName("unresolved")] Unresolved,
    [JsonStringEnumMemberName("name-candidate")] NameCandidate,
    [JsonStringEnumMemberName("possible")] Possible,
    [JsonStringEnumMemberName("exact")] Exact
}
[JsonConverter(typeof(JsonStringEnumConverter<VerificationCounterMapping>))]
public enum VerificationCounterMapping
{
    [JsonStringEnumMemberName("none")] None,
    [JsonStringEnumMemberName("line-only")] LineOnly,
    [JsonStringEnumMemberName("partial-branch")] PartialBranch,
    [JsonStringEnumMemberName("ambiguous")] Ambiguous,
    [JsonStringEnumMemberName("exact-branch")] ExactBranch
}
[JsonConverter(typeof(JsonStringEnumConverter<VerificationRunOutcome>))]
public enum VerificationRunOutcome
{
    [JsonStringEnumMemberName("not-run")] NotRun,
    [JsonStringEnumMemberName("passed")] Passed,
    [JsonStringEnumMemberName("failed")] Failed,
    [JsonStringEnumMemberName("skipped")] Skipped,
    [JsonStringEnumMemberName("incomplete")] Incomplete
}

/// <summary>
/// Identity is contextual: two targets, overloads, branches, or source/build versions are not interchangeable.
/// The behavior ID is a stable branch-arm or contract identity, not display text.
/// </summary>
public sealed record VerificationScope(
    string ContextId, string TargetId, string SymbolId, string BehaviorId,
    string SourceDigest, string BuildId);

public sealed record VerificationChange(
    string Id, VerificationChangeKind Kind, VerificationScope Scope,
    string ChangedRange, IReadOnlyList<string> Reasons);

/// <summary>
/// RequiredDetectors enumerates the *entire* applicable negative-search boundary for the rule.
/// Without this declaration, absence can never be interpreted as missing evidence.
/// </summary>
public sealed record VerificationRuleSetting(
    string RuleId, VerificationPolicyMode Mode, IReadOnlyList<string> RequiredDetectors);

public sealed record VerificationPolicy(
    string Version, VerificationPolicyMode DefaultMode,
    IReadOnlyList<VerificationRuleSetting> Rules);

/// <summary>
/// A detector records explicit inclusion, exclusion, completeness, and unresolved frontiers.
/// Completeness is valid only for the exact current scope and every expected detector.
/// </summary>
public sealed record VerificationSearchFact(
    string Id, string RuleId, string DetectorId, VerificationScope Scope,
    bool Complete, IReadOnlyList<string> SearchedTargets,
    IReadOnlyList<string> ExcludedTargets, IReadOnlyList<string> Frontier,
    bool BudgetExhausted = false);

/// <summary>
/// Evidence is typed and prevalidated by the caller. ValidReceipt means the adapter verified
/// provenance and artifact bytes. ExactTargetMapping means build/target/source mapping was
/// verified against the report. A counter with a line or partial-branch mapping cannot
/// establish coverage of an individual changed branch arm.
/// </summary>
public sealed record VerificationEvidence
{
    public required string Id { get; init; }
    public required string RuleId { get; init; }
    public required string DetectorId { get; init; }
    public required VerificationScope Scope { get; init; }
    public required VerificationEvidenceKind Kind { get; init; }
    public VerificationBinding Binding { get; init; } = VerificationBinding.Unresolved;
    public bool ValidReceipt { get; init; }
    public bool ExactTargetMapping { get; init; }
    public VerificationCounterMapping CounterMapping { get; init; }
    public int? HitCount { get; init; }
    public VerificationRunOutcome RunOutcome { get; init; }
    public bool ExecutedBehavior { get; init; }
    public bool RelevantAssertion { get; init; }
}

public sealed record VerificationRequest(
    VerificationPolicy Policy,
    IReadOnlyList<VerificationChange> Changes,
    IReadOnlyList<VerificationSearchFact> Searches,
    IReadOnlyList<VerificationEvidence> Evidence);

/// <summary>Hard input count caps; oversized input fails closed without input-order sampling.</summary>
public sealed record VerificationLimits(
    int MaxChanges = 128, int MaxSearches = 512, int MaxEvidence = 2048,
    int MaxPolicyRules = 32, int MaxListItems = 128);

public sealed record VerificationDecision(
    string ChangeId, string RuleId, int RuleVersion, VerificationCategory Category,
    string PolicyVersion, VerificationPolicyMode AppliedPolicy,
    VerificationScope Scope, string ChangedRange, IReadOnlyList<string> TriggerReasons,
    VerificationState State, VerificationStrength Strength,
    IReadOnlyList<string> ConsultedDetectors, IReadOnlyList<string> SearchedTargets,
    IReadOnlyList<string> ExcludedTargets, IReadOnlyList<string> EvidenceIds,
    IReadOnlyList<VerificationRunOutcome> RunOutcomes,
    bool RelevantAssertionEstablished, IReadOnlyList<string> Reasons,
    IReadOnlyList<string> Frontier, string NextAction);

public sealed record VerificationBatch(
    IReadOnlyList<VerificationDecision> Decisions,
    bool BudgetExhausted, IReadOnlyList<string> Frontier);

/// <summary>
/// Pure SG-11 evaluator. It consumes no files, subprocesses, network, workspace state or
/// inferred symbol relationships. Producers own normalization and receipt validation.
/// </summary>
public static class VerificationRuleEvaluator
{
    public const string EditedBranchRule = "sg.verification.edited-branch";
    public const string PublicContractRule = "sg.verification.public-contract";
    public const string SerializationRule = "sg.verification.serialization";
    public const int RuleVersion = 1;

    private sealed record Rule(string Id, VerificationCategory Category);
    private static readonly IReadOnlyDictionary<VerificationChangeKind, Rule> Rules =
        new ReadOnlyDictionary<VerificationChangeKind, Rule>(new Dictionary<VerificationChangeKind, Rule>
        {
            [VerificationChangeKind.EditedBranch] = new(EditedBranchRule, VerificationCategory.CoverageTests),
            [VerificationChangeKind.PublicContract] = new(PublicContractRule, VerificationCategory.ContractTests),
            [VerificationChangeKind.Serialization] = new(SerializationRule, VerificationCategory.ContractTests)
        });

    public static VerificationBatch Evaluate(VerificationRequest request, VerificationLimits? limits = null)
    {
        ArgumentNullException.ThrowIfNull(request);
        ArgumentNullException.ThrowIfNull(request.Policy);
        ArgumentNullException.ThrowIfNull(request.Changes);
        ArgumentNullException.ThrowIfNull(request.Searches);
        ArgumentNullException.ThrowIfNull(request.Evidence);
        limits ??= new VerificationLimits();
        if (limits.MaxChanges < 1 || limits.MaxSearches < 1 || limits.MaxEvidence < 1
            || limits.MaxPolicyRules < 1 || limits.MaxListItems < 1)
            throw new ArgumentOutOfRangeException(nameof(limits), "All verification limits must be positive.");

        var limitFrontier = new List<string>();
        if (request.Changes.Count > limits.MaxChanges)
            limitFrontier.Add($"change budget exceeded: {request.Changes.Count} > {limits.MaxChanges}");
        if (request.Searches.Count > limits.MaxSearches)
            limitFrontier.Add($"search-fact budget exceeded: {request.Searches.Count} > {limits.MaxSearches}");
        if (request.Evidence.Count > limits.MaxEvidence)
            limitFrontier.Add($"evidence-fact budget exceeded: {request.Evidence.Count} > {limits.MaxEvidence}");
        if (request.Policy.Rules.Count > limits.MaxPolicyRules)
            limitFrontier.Add($"policy-rule budget exceeded: {request.Policy.Rules.Count} > {limits.MaxPolicyRules}");

        // Bound nested source lists as well as top-level fact counts. Oversized nested
        // input is rejected whole, rather than sampling input-order-dependent prefixes.
        if (request.Changes.Count <= limits.MaxChanges && request.Policy.Rules.Count <= limits.MaxPolicyRules &&
            (request.Changes.Any(c => c.Reasons.Count > limits.MaxListItems) ||
             request.Policy.Rules.Any(p => p.RequiredDetectors.Count > limits.MaxListItems) ||
             (request.Searches.Count <= limits.MaxSearches && request.Searches.Any(s =>
                 s.SearchedTargets.Count > limits.MaxListItems ||
                 s.ExcludedTargets.Count > limits.MaxListItems || s.Frontier.Count > limits.MaxListItems))))
        {
            limitFrontier.Add($"nested scope-list budget exceeded: maximum {limits.MaxListItems} items per list");
            return new VerificationBatch([], true, limitFrontier.ToArray());
        }

        // Never silently truncate a caller-supplied sequence. If changes themselves exceed
        // the cap, the global frontier reports the skipped scope, without an unbounded sort.
        if (request.Changes.Count > limits.MaxChanges || request.Policy.Rules.Count > limits.MaxPolicyRules)
            return new VerificationBatch([], true, limitFrontier.ToArray());

        var decisions = new List<VerificationDecision>(request.Changes.Count);
        foreach (var change in request.Changes
                     .OrderBy(c => c.Scope.ContextId, StringComparer.Ordinal)
                     .ThenBy(c => c.Scope.TargetId, StringComparer.Ordinal)
                     .ThenBy(c => c.Scope.SymbolId, StringComparer.Ordinal)
                     .ThenBy(c => c.Scope.BehaviorId, StringComparer.Ordinal)
                     .ThenBy(c => c.Id, StringComparer.Ordinal))
        {
            if (!Rules.TryGetValue(change.Kind, out var rule))
                throw new ArgumentException($"Unsupported verification change kind: {change.Kind}.");
            var setting = request.Policy.Rules.FirstOrDefault(p => StringComparer.Ordinal.Equals(p.RuleId, rule.Id))
                          ?? new VerificationRuleSetting(rule.Id, request.Policy.DefaultMode, []);
            decisions.Add(limitFrontier.Count > 0
                ? MakeDecision(change, rule, request.Policy.Version, setting.Mode,
                    VerificationState.Unknown, VerificationStrength.None, [], [], [], [], [], false,
                    ["The complete fact set was not evaluated."], limitFrontier,
                    $"Reduce or partition the fact set for target '{change.Scope.TargetId}' and rerun the rule.")
                : EvaluateRule(change, rule, request.Policy.Version, setting, request.Searches, request.Evidence));
        }
        return new VerificationBatch(decisions, limitFrontier.Count > 0 || request.Searches.Any(s => s.BudgetExhausted),
            limitFrontier.ToArray());
    }

    private static VerificationDecision EvaluateRule(
        VerificationChange change, Rule rule, string policyVersion, VerificationRuleSetting setting,
        IReadOnlyList<VerificationSearchFact> searches, IReadOnlyList<VerificationEvidence> evidence)
    {
        if (setting.Mode == VerificationPolicyMode.Disabled)
            return MakeDecision(change, rule, policyVersion, setting.Mode,
                VerificationState.Unknown, VerificationStrength.None, [], [], [], [], [], false,
                ["Rule disabled by explicit policy; no evidence claim was evaluated."], ["policy.disabled"],
                $"Enable rule '{rule.Id}' for target '{change.Scope.TargetId}' to request verification.");

        // Exact contextual matching first; similarly named tests in different targets are not evidence.
        var scopedSearches = searches.Where(f => StringComparer.Ordinal.Equals(f.RuleId, rule.Id)
                                  && SameBehavior(f.Scope, change.Scope))
            .OrderBy(f => f.DetectorId, StringComparer.Ordinal)
            .ThenBy(f => f.Id, StringComparer.Ordinal).ToArray();
        var scopedEvidence = evidence.Where(f => StringComparer.Ordinal.Equals(f.RuleId, rule.Id)
                                  && SameBehavior(f.Scope, change.Scope))
            .OrderBy(f => f.DetectorId, StringComparer.Ordinal)
            .ThenBy(f => f.Id, StringComparer.Ordinal).ToArray();

        var frontier = new List<string>();
        var reasons = new List<string>();
        var expected = setting.RequiredDetectors.Distinct(StringComparer.Ordinal)
            .OrderBy(id => id, StringComparer.Ordinal).ToArray();
        var freshSearches = scopedSearches.Where(f => SameRevision(f.Scope, change.Scope)).ToArray();
        var unreplacedStaleSearch = false;
        foreach (var search in scopedSearches)
        {
            if (!SameRevision(search.Scope, change.Scope))
            {
                // Historical searches do not weaken a complete, current replacement for
                // the same detector. Without a replacement, they remain a stale frontier.
                if (freshSearches.Any(f => StringComparer.Ordinal.Equals(f.DetectorId, search.DetectorId)))
                    reasons.Add($"Historical search '{search.Id}' was superseded by a current search.");
                else
                {
                    unreplacedStaleSearch = true;
                    frontier.Add($"search '{search.Id}' has an obsolete source or build identity");
                }
            }
            else
            {
                if (!search.Complete || search.BudgetExhausted)
                    frontier.Add($"detector '{search.DetectorId}' search '{search.Id}' is incomplete or budget-exhausted");
                if (!search.SearchedTargets.Contains(change.Scope.TargetId, StringComparer.Ordinal)
                    || search.ExcludedTargets.Contains(change.Scope.TargetId, StringComparer.Ordinal))
                    frontier.Add($"detector '{search.DetectorId}' did not include target '{change.Scope.TargetId}'");
                foreach (var item in search.Frontier)
                    frontier.Add($"detector '{search.DetectorId}': {item}");
            }
        }
        if (expected.Length == 0)
            frontier.Add("policy has no exhaustive detector boundary; missing cannot be inferred");
        foreach (var detector in expected)
        {
            if (!freshSearches.Any(f => StringComparer.Ordinal.Equals(f.DetectorId, detector)))
                frontier.Add($"required detector '{detector}' has no current search receipt");
        }

        var matching = scopedEvidence.Where(f => f.Binding is VerificationBinding.Exact or VerificationBinding.Possible)
            .ToArray();
        var fresh = matching.Where(f => SameRevision(f.Scope, change.Scope)).ToArray();
        var stale = matching.Where(f => !SameRevision(f.Scope, change.Scope)).ToArray();
        foreach (var item in stale)
            frontier.Add($"evidence '{item.Id}' has an obsolete source or build identity");

        var observed = new List<VerificationEvidence>();
        var zeros = new List<VerificationEvidence>();
        var configured = new List<VerificationEvidence>();
        var unresolved = new List<VerificationEvidence>();
        foreach (var item in fresh)
        {
            switch (item.Kind)
            {
                case VerificationEvidenceKind.Configuration when item.Binding == VerificationBinding.Exact:
                    configured.Add(item);
                    break;
                case VerificationEvidenceKind.StaticPath:
                    configured.Add(item);
                    if (item.Binding == VerificationBinding.Possible)
                        frontier.Add($"static path '{item.Id}' is only a possible binding");
                    break;
                case VerificationEvidenceKind.BranchCounter when change.Kind == VerificationChangeKind.EditedBranch:
                    if (item.Binding == VerificationBinding.Exact && item.ValidReceipt && item.ExactTargetMapping
                        && item.CounterMapping == VerificationCounterMapping.ExactBranch
                        && item.HitCount >= 0 && IsCompleted(item.RunOutcome))
                    {
                        if (item.HitCount > 0) observed.Add(item);
                        else zeros.Add(item);
                    }
                    else
                    {
                        unresolved.Add(item);
                        frontier.Add($"branch counter '{item.Id}' lacks an exact verified, completed measurement");
                    }
                    break;
                case VerificationEvidenceKind.ContractExecution when change.Kind is
                        VerificationChangeKind.PublicContract or VerificationChangeKind.Serialization:
                    if (item.Binding == VerificationBinding.Exact && item.ValidReceipt && item.ExactTargetMapping
                        && item.ExecutedBehavior && item.RelevantAssertion && IsCompleted(item.RunOutcome))
                        observed.Add(item);
                    else
                    {
                        unresolved.Add(item);
                        frontier.Add($"contract execution '{item.Id}' lacks an attested behavior/assertion or valid receipt");
                    }
                    break;
                default:
                    // A completed test result is not a branch hit, and neither a static path nor
                    // an unrelated counter demonstrates execution or assertion relevance.
                    unresolved.Add(item);
                    frontier.Add($"evidence '{item.Id}' does not measure the required behavior");
                    break;
            }
        }

        var detectors = scopedSearches.Select(s => s.DetectorId)
            .Concat(scopedEvidence.Select(e => e.DetectorId));
        var searchedTargets = freshSearches.SelectMany(s => s.SearchedTargets);
        var excludedTargets = freshSearches.SelectMany(s => s.ExcludedTargets);
        var evidenceIds = matching.Select(e => e.Id);
        var runOutcomes = matching.Where(e => SameRevision(e.Scope, change.Scope) && e.ValidReceipt
                                && e.RunOutcome != VerificationRunOutcome.NotRun)
            .Select(e => e.RunOutcome).Distinct().OrderBy(o => o).ToArray();
        var assertionRelevant = observed.Any(e => e.Kind == VerificationEvidenceKind.ContractExecution);

        VerificationState state;
        VerificationStrength strength;
        string action;
        if (observed.Count > 0)
        {
            state = VerificationState.Observed;
            strength = VerificationStrength.MeasuredExecution;
            reasons.Add($"Verified matching behavior measurement: {string.Join(", ", observed.Select(e => e.Id))}.");
            action = runOutcomes.Contains(VerificationRunOutcome.Failed)
                ? $"Investigate the failed test result for target '{change.Scope.TargetId}', then rerun and import current measurement evidence."
                : $"Retain the verified receipt for target '{change.Scope.TargetId}' and re-evaluate after its inputs change.";
        }
        else if (zeros.Count > 0)
        {
            state = VerificationState.Uncovered;
            strength = VerificationStrength.VerifiedMeasurement;
            reasons.Add($"Exact mapped branch counters recorded zero hits: {string.Join(", ", zeros.Select(e => e.Id))}.");
            action = $"Add a test executing branch '{change.Scope.BehaviorId}' in target '{change.Scope.TargetId}', then collect a new exact branch counter.";
        }
        else if (configured.Count > 0)
        {
            state = VerificationState.Configured;
            strength = configured.Any(e => e.Kind == VerificationEvidenceKind.StaticPath && e.Binding == VerificationBinding.Exact)
                ? VerificationStrength.ExactStaticPath
                : configured.Any(e => e.Kind == VerificationEvidenceKind.StaticPath)
                    ? VerificationStrength.PossibleStaticPath : VerificationStrength.Declared;
            reasons.Add($"Scoped configuration/static path exists: {string.Join(", ", configured.Select(e => e.Id))}; execution is not established.");
            action = $"Run the configured {rule.Category} check for target '{change.Scope.TargetId}' and import a current, exactly mapped result.";
        }
        else if (stale.Length > 0 || unreplacedStaleSearch)
        {
            state = VerificationState.Stale;
            strength = VerificationStrength.None;
            reasons.Add("Only obsolete source/build evidence or search receipts apply to this behavior.");
            action = $"Rerun the {rule.Category} detector for target '{change.Scope.TargetId}' at source '{change.Scope.SourceDigest}' and build '{change.Scope.BuildId}'.";
        }
        else
        {
            var complete = expected.Length > 0 && unresolved.Count == 0 &&
                frontier.Count == 0 &&
                expected.All(d => freshSearches.Any(s => StringComparer.Ordinal.Equals(s.DetectorId, d))) &&
                freshSearches.All(s => s.Complete && !s.BudgetExhausted && s.Frontier.Count == 0 &&
                    s.SearchedTargets.Contains(change.Scope.TargetId, StringComparer.Ordinal) &&
                    !s.ExcludedTargets.Contains(change.Scope.TargetId, StringComparer.Ordinal));
            if (complete)
            {
                state = VerificationState.Missing;
                reasons.Add("Every required detector completed the current applicable scope without linked evidence.");
                action = $"Add a {rule.Category} check for '{change.Scope.SymbolId}/{change.Scope.BehaviorId}' in target '{change.Scope.TargetId}' and repeat the detector search.";
            }
            else
            {
                state = VerificationState.Unknown;
                if (frontier.Count == 0)
                    frontier.Add("no complete applicable evidence search was supplied");
                reasons.Add("No confirmed behavior measurement; the evidence search or mappings are unresolved.");
                action = $"Complete the {rule.Category} detector search and exact behavior mapping for target '{change.Scope.TargetId}', then re-evaluate.";
            }
            strength = VerificationStrength.None;
        }

        return MakeDecision(change, rule, policyVersion, setting.Mode, state, strength,
            SortedDistinct(detectors), SortedDistinct(searchedTargets), SortedDistinct(excludedTargets),
            SortedDistinct(evidenceIds), runOutcomes, assertionRelevant, reasons,
            SortedDistinct(frontier), action);
    }

    private static bool IsCompleted(VerificationRunOutcome outcome) =>
        outcome is VerificationRunOutcome.Passed or VerificationRunOutcome.Failed;

    private static bool SameBehavior(VerificationScope a, VerificationScope b) =>
        StringComparer.Ordinal.Equals(a.ContextId, b.ContextId) &&
        StringComparer.Ordinal.Equals(a.TargetId, b.TargetId) &&
        StringComparer.Ordinal.Equals(a.SymbolId, b.SymbolId) &&
        StringComparer.Ordinal.Equals(a.BehaviorId, b.BehaviorId);

    private static bool SameRevision(VerificationScope a, VerificationScope b) =>
        StringComparer.Ordinal.Equals(a.SourceDigest, b.SourceDigest) &&
        StringComparer.Ordinal.Equals(a.BuildId, b.BuildId);

    private static string[] SortedDistinct(IEnumerable<string> items) =>
        items.Distinct(StringComparer.Ordinal).OrderBy(x => x, StringComparer.Ordinal).ToArray();

    private static VerificationDecision MakeDecision(
        VerificationChange change, Rule rule, string policyVersion, VerificationPolicyMode mode,
        VerificationState state, VerificationStrength strength,
        IReadOnlyList<string> consulted, IReadOnlyList<string> searched, IReadOnlyList<string> excluded,
        IReadOnlyList<string> evidence, IReadOnlyList<VerificationRunOutcome> outcomes, bool assertionRelevant,
        IReadOnlyList<string> reasons, IReadOnlyList<string> frontier, string action) =>
        new(change.Id, rule.Id, RuleVersion, rule.Category, policyVersion, mode, change.Scope,
            change.ChangedRange, SortedDistinct(change.Reasons), state, strength,
            consulted, searched, excluded, evidence, outcomes, assertionRelevant,
            reasons, frontier, action);
}
