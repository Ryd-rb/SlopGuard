using Microsoft.CodeAnalysis;
using Microsoft.CodeAnalysis.CSharp;
using Microsoft.CodeAnalysis.CSharp.Syntax;
using Microsoft.CodeAnalysis.FlowAnalysis;
using Microsoft.CodeAnalysis.Operations;
using Microsoft.CodeAnalysis.Text;

namespace SlopHawk.Providers.CSharp;

public enum ResourceLifetimeSourceKind { Unknown, User, Generated }
public enum ResourceLifetimeStatus { Analyzed, IncompleteInput, Unsupported, BudgetExceeded, Cancelled }
public enum ResourceLifetimeUseState { Live, Disposed, Unknown }
public enum ResourceLifetimeChangeKind { Introduced, Existing, NoRegression, Unknown }

// These are caller certifications for the supplied compilation, not facts inferred from file names.
// The reference pack must implement the .NET 10 MemoryStream contract; no target code is executed.
public sealed record ResourceLifetimeInput(RoslynDocumentIdentity Identity,
    bool SourceClosureComplete, bool CompilerErrorsAbsent, ResourceLifetimeSourceKind SourceKind);

public sealed record ResourceLifetimeLimits(int MaxMethodCharacters = 64_000, int MaxSyntaxNodes = 4096,
    int MaxBlocks = 128, int MaxWork = 16_384, int MaxTracked = 64, int MaxUses = 64,
    int MaxWitnessesPerUse = 8, int MaxDepth = 64)
{
    // A deliberate slice boundary, not a tunable path solver budget.
    public int MaxNonconstantConditions => 1;
}

// All spans are UTF-16, with zero-based line/column coordinates, in this analysis's tree.
// Disposed means every modeled normal path reaching this use has disposed this exact allocation.
// It does not prove that such a path executes. Unknown may retain conditional disposal evidence.
public sealed record ResourceLifetimeUse(DocumentSpan Span, ResourceLifetimeUseState State,
    DocumentSpan? Allocation, IReadOnlyList<DocumentSpan> Disposals, string? Reason);

public sealed record ResourceLifetimeAnalysis(ResourceLifetimeInput Input, string? MethodId,
    DocumentSpan MethodSpan, ResourceLifetimeStatus Status, string? Reason,
    IReadOnlyList<ResourceLifetimeUse> Uses, int Work,
    string RoslynCommonAssemblyVersion, string RoslynCSharpAssemblyVersion, ResourceLifetimeLimits Limits)
{
    public string Contract => CSharpResourceLifetime.Contract;
    public bool PathFeasibilityProved => false;
    // Complete covers only the advertised contract, never all disposal/use families or path feasibility.
    public bool Complete => Status == ResourceLifetimeStatus.Analyzed &&
        Uses.All(use => use.State != ResourceLifetimeUseState.Unknown);
}

// The caller certifies semantic use-site correspondence; offsets alone never infer a match.
public sealed record ResourceLifetimeUseMatch(TextSpan Before, TextSpan After);
public sealed record ResourceLifetimeChange(ResourceLifetimeChangeKind Kind,
    ResourceLifetimeUse? Before, ResourceLifetimeUse After, string? Reason);
public sealed record ResourceLifetimeComparison(IReadOnlyList<ResourceLifetimeChange> Changes, string? Reason)
{
    public bool Complete => Reason is null && Changes.All(change => change.Kind != ResourceLifetimeChangeKind.Unknown);
}

/// <summary>
/// Opt-in, method-local evidence for parameterless new MemoryStream / Dispose / ReadByte.
/// Admits at most one simple Boolean-parameter split; unsupported effects abort the method.
/// No worker, report, analyzer, filesystem, process, generator or default-check registration is changed.
/// The caller supplies already obtained, validated compiler inputs and explicit before/after matching.
/// Checker-owned work is bounded; Roslyn binding/CFG construction still needs caller process admission.
/// </summary>
public static class CSharpResourceLifetime
{
    public const string Contract = "dotnet10-memorystream-readbyte-v1";
    private const int MaximumUses = 128;

    public static ResourceLifetimeAnalysis Analyze(CSharpCompilation compilation, SyntaxTree tree,
        MethodDeclarationSyntax method, ResourceLifetimeInput input, ResourceLifetimeLimits? limits = null,
        CancellationToken cancellationToken = default)
    {
        ArgumentNullException.ThrowIfNull(compilation);
        ArgumentNullException.ThrowIfNull(tree);
        ArgumentNullException.ThrowIfNull(method);
        ArgumentNullException.ThrowIfNull(input);
        ArgumentNullException.ThrowIfNull(input.Identity);
        limits ??= new ResourceLifetimeLimits();
        if (limits.MaxMethodCharacters is < 1 or > 262_144 || limits.MaxSyntaxNodes is < 1 or > 65_536 ||
            limits.MaxBlocks is < 2 or > 512 || limits.MaxWork is < 1 or > 1_000_000 ||
            limits.MaxTracked is < 1 or > 256 || limits.MaxUses is < 1 or > MaximumUses ||
            limits.MaxWitnessesPerUse is < 1 or > 16 || limits.MaxDepth is < 1 or > 128)
            throw new ArgumentOutOfRangeException(nameof(limits), "Resource analysis limits must be bounded and positive.");
        var identity = input.Identity;
        if (new[] { identity.ContextKey, identity.DocumentKey, identity.PhysicalPath, identity.LogicalPath,
                    identity.Revision, identity.InputFingerprint }
            .Any(part => string.IsNullOrWhiteSpace(part) || part.Length > 4096 || part.Contains('\0')))
            throw new ArgumentException("Trusted context, document and version identities are required.", nameof(input));
        if (!ReferenceEquals(method.SyntaxTree, tree) || !compilation.ContainsSyntaxTree(tree))
            throw new ArgumentException("The selected method and tree must belong to the supplied compilation.", nameof(method));

        return new Analysis(compilation, tree, method, input, limits, cancellationToken).Run();
    }

    /// <summary>
    /// Only a certified matched Live -> Disposed read establishes introduction. No diagnostic in
    /// the old version, an unmatched new read, or an incomplete old analysis is not a safe baseline.
    /// Existing is overlap evidence for caller-side deduplication, not another introduced finding.
    /// </summary>
    public static ResourceLifetimeComparison Compare(ResourceLifetimeAnalysis before,
        ResourceLifetimeAnalysis after, IReadOnlyList<ResourceLifetimeUseMatch> matches)
    {
        ArgumentNullException.ThrowIfNull(before);
        ArgumentNullException.ThrowIfNull(after);
        ArgumentNullException.ThrowIfNull(matches);
        if (matches.Count > MaximumUses || before.Uses.Count > MaximumUses || after.Uses.Count > MaximumUses)
            return new([], "comparison_limit");
        if (before.Status != ResourceLifetimeStatus.Analyzed || after.Status != ResourceLifetimeStatus.Analyzed)
            return new([], "incomplete_analysis");
        if (before.RoslynCommonAssemblyVersion != after.RoslynCommonAssemblyVersion ||
            before.RoslynCSharpAssemblyVersion != after.RoslynCSharpAssemblyVersion || before.Limits != after.Limits)
            return new([], "incompatible_compiler_or_limits");
        if (before.RoslynCommonAssemblyVersion == "unknown" || before.RoslynCSharpAssemblyVersion == "unknown")
            return new([], "compiler_identity_unavailable");
        var oldIdentity = before.Input.Identity;
        var newIdentity = after.Input.Identity;
        if (before.MethodId is null || before.MethodId != after.MethodId ||
            oldIdentity.ContextKey != newIdentity.ContextKey || oldIdentity.DocumentKey != newIdentity.DocumentKey ||
            oldIdentity.LogicalPath != newIdentity.LogicalPath || oldIdentity.PhysicalPath != newIdentity.PhysicalPath)
            return new([], "incompatible_scope");
        if (oldIdentity.Revision == newIdentity.Revision || oldIdentity.InputFingerprint == newIdentity.InputFingerprint)
            return new([], "distinct_versions_required");

        var oldUses = before.Uses.ToDictionary(use => Offset(use.Span));
        var newUses = after.Uses.ToDictionary(use => Offset(use.Span));
        var mapping = new Dictionary<TextSpan, ResourceLifetimeUse>();
        var seenOld = new HashSet<TextSpan>();
        foreach (var match in matches)
        {
            if (match is null || !oldUses.TryGetValue(match.Before, out var oldUse) ||
                !newUses.ContainsKey(match.After) || !seenOld.Add(match.Before) ||
                !mapping.TryAdd(match.After, oldUse))
                return new([], "invalid_or_duplicate_use_match");
        }

        var completePair = before.Complete && after.Complete;
        var changes = new List<ResourceLifetimeChange>();
        foreach (var use in after.Uses.OrderBy(use => use.Span.Start).ThenBy(use => use.Span.Length))
        {
            mapping.TryGetValue(Offset(use.Span), out var oldUse);
            var kind = ResourceLifetimeChangeKind.Unknown;
            string? reason = "matched_live_baseline_required";
            if (use.State == ResourceLifetimeUseState.Live)
            {
                kind = ResourceLifetimeChangeKind.NoRegression;
                reason = null;
            }
            else if (use.State == ResourceLifetimeUseState.Disposed && oldUse is not null &&
                completePair)
            {
                if (oldUse.State == ResourceLifetimeUseState.Live)
                    kind = ResourceLifetimeChangeKind.Introduced;
                else if (oldUse.State == ResourceLifetimeUseState.Disposed)
                    kind = ResourceLifetimeChangeKind.Existing;
                if (kind != ResourceLifetimeChangeKind.Unknown) reason = null;
            }
            if (use.State == ResourceLifetimeUseState.Disposed && !completePair)
                reason = "incomplete_lifetime_observations";
            if (use.State == ResourceLifetimeUseState.Unknown) reason = "current_use_unknown";
            changes.Add(new(kind, oldUse, use, reason));
        }
        return new(changes.ToArray(), null);
    }

    private static TextSpan Offset(DocumentSpan span) => new(span.Start, span.Length);

    private static DocumentSpan Span(SyntaxTree tree, TextSpan span)
    {
        var lines = tree.GetLineSpan(span);
        return new(span.Start, span.Length, lines.StartLinePosition.Line, lines.StartLinePosition.Character,
            lines.EndLinePosition.Line, lines.EndLinePosition.Character);
    }

    private sealed class AnalysisStop(ResourceLifetimeStatus status, string reason) : Exception
    {
        public ResourceLifetimeStatus Status { get; } = status;
        public string Reason { get; } = reason;
    }

    private sealed record Lifetime(ResourceLifetimeUseState State, TextSpan[] Disposals);

    private sealed class State
    {
        public Dictionary<ISymbol, int> Locals { get; } = new(SymbolEqualityComparer.Default);
        public Dictionary<CaptureId, int> Captures { get; } = [];
        public Dictionary<int, Lifetime> Resources { get; } = [];
        public bool Terminated { get; set; }
    }

    private sealed class Analysis(CSharpCompilation compilation, SyntaxTree tree,
        MethodDeclarationSyntax method, ResourceLifetimeInput input, ResourceLifetimeLimits limits,
        CancellationToken cancellationToken)
    {
        private readonly List<ResourceLifetimeUse> uses = [];
        private readonly HashSet<TextSpan> useSpans = [];
        private readonly Dictionary<int, TextSpan> allocations = [];
        private INamedTypeSymbol? memoryStream;
        private INamedTypeSymbol? stream;
        private INamedTypeSymbol? disposable;
        private IMethodSymbol? constructor;
        private IMethodSymbol? dispose;
        private IMethodSymbol? interfaceDispose;
        private IMethodSymbol? readByte;
        private IMethodSymbol? streamReadByte;
        private IMethodSymbol? toArray;
        private string? methodId;
        private int work;

        public ResourceLifetimeAnalysis Run()
        {
            try
            {
                cancellationToken.ThrowIfCancellationRequested();
                if (!input.Identity.ReferenceClosureComplete || !input.SourceClosureComplete || !input.CompilerErrorsAbsent)
                    Stop(ResourceLifetimeStatus.IncompleteInput, "uncertified_compilation");
                if (input.SourceKind != ResourceLifetimeSourceKind.User)
                    Stop(ResourceLifetimeStatus.IncompleteInput, input.SourceKind == ResourceLifetimeSourceKind.Generated
                        ? "generated_source_excluded" : "source_provenance_unknown");
                Preflight();
                var model = compilation.GetSemanticModel(tree);
                if (model.GetDeclaredSymbol(method, cancellationToken) is not IMethodSymbol symbol ||
                    symbol.Parameters.Any(parameter => parameter.RefKind != RefKind.None))
                    Stop(ResourceLifetimeStatus.Unsupported, "method_binding_or_ref_parameter");
                else
                    methodId = DocumentationCommentId.CreateDeclarationId(symbol.OriginalDefinition);
                if (methodId is null || methodId.Length > 4096)
                    Stop(ResourceLifetimeStatus.Unsupported, "method_identity_unavailable_or_limit");
                BindContract();
                ControlFlowGraph? graph;
                try
                {
                    graph = ControlFlowGraph.Create(method, model, cancellationToken);
                }
                catch (ArgumentException) { return Result(ResourceLifetimeStatus.Unsupported, "cfg_unavailable"); }
                catch (InvalidOperationException) { return Result(ResourceLifetimeStatus.Unsupported, "cfg_unavailable"); }
                if (graph is null) return Result(ResourceLifetimeStatus.Unsupported, "cfg_unavailable");
                Walk(graph);
                return Result(ResourceLifetimeStatus.Analyzed, null);
            }
            catch (AnalysisStop stop) { return Result(stop.Status, stop.Reason); }
            catch (OperationCanceledException) when (cancellationToken.IsCancellationRequested)
            {
                return Result(ResourceLifetimeStatus.Cancelled, "cancelled");
            }
        }

        private ResourceLifetimeAnalysis Result(ResourceLifetimeStatus status, string? reason) => new(input,
            methodId, Span(tree, method.Span), status, reason,
            status == ResourceLifetimeStatus.Analyzed
                ? uses.OrderBy(use => use.Span.Start).ThenBy(use => use.Span.Length).ToArray() : [], work,
            typeof(Compilation).Assembly.GetName().Version?.ToString() ?? "unknown",
            typeof(CSharpCompilation).Assembly.GetName().Version?.ToString() ?? "unknown", limits);

        private static void Stop(ResourceLifetimeStatus status, string reason) => throw new AnalysisStop(status, reason);

        private void Tick(int amount = 1)
        {
            cancellationToken.ThrowIfCancellationRequested();
            if (amount > limits.MaxWork - work) Stop(ResourceLifetimeStatus.BudgetExceeded, "work_limit");
            work += amount;
        }

        private void Preflight()
        {
            if (method.FullSpan.Length > limits.MaxMethodCharacters)
                Stop(ResourceLifetimeStatus.BudgetExceeded, "method_size_limit");
            if (method.Body is null || method.ContainsDiagnostics ||
                method.Modifiers.Any(SyntaxKind.AsyncKeyword) || method.Modifiers.Any(SyntaxKind.UnsafeKeyword))
                Stop(ResourceLifetimeStatus.Unsupported, "method_shape_or_syntax");
            var nodes = 0;
            foreach (var node in method.DescendantNodesAndSelf())
            {
                cancellationToken.ThrowIfCancellationRequested();
                if (++nodes > limits.MaxSyntaxNodes) Stop(ResourceLifetimeStatus.BudgetExceeded, "syntax_node_limit");
                if (node is UsingStatementSyntax or TryStatementSyntax or AwaitExpressionSyntax or YieldStatementSyntax or
                    AnonymousFunctionExpressionSyntax or LocalFunctionStatementSyntax or LockStatementSyntax or
                    ForStatementSyntax or ForEachStatementSyntax or ForEachVariableStatementSyntax or
                    WhileStatementSyntax or DoStatementSyntax or GotoStatementSyntax or SwitchStatementSyntax or
                    SwitchExpressionSyntax or QueryExpressionSyntax or FixedStatementSyntax or UnsafeStatementSyntax or
                    RefExpressionSyntax or ThrowStatementSyntax or ThrowExpressionSyntax ||
                    node is LocalDeclarationStatementSyntax local && !local.UsingKeyword.IsKind(SyntaxKind.None) ||
                    node is ArgumentSyntax argument && !argument.RefKindKeyword.IsKind(SyntaxKind.None))
                    Stop(ResourceLifetimeStatus.Unsupported, "scope_control_escape_or_deferred_execution");
            }
        }

        private void BindContract()
        {
            memoryStream = compilation.GetTypeByMetadataName("System.IO.MemoryStream");
            stream = compilation.GetTypeByMetadataName("System.IO.Stream");
            disposable = compilation.GetTypeByMetadataName("System.IDisposable");
            var core = compilation.GetSpecialType(SpecialType.System_Object).ContainingAssembly;
            if (new[] { memoryStream, stream, disposable }.Any(type => type is null ||
                type.DeclaringSyntaxReferences.Length != 0 || !Same(type.ContainingAssembly, core)))
                Stop(ResourceLifetimeStatus.IncompleteInput, "framework_contract_unbound");
            constructor = memoryStream!.InstanceConstructors.SingleOrDefault(candidate => candidate.Parameters.Length == 0);
            dispose = Member(stream!, "Dispose", SpecialType.System_Void);
            interfaceDispose = Member(disposable!, "Dispose", SpecialType.System_Void);
            readByte = Member(memoryStream!, "ReadByte", SpecialType.System_Int32);
            streamReadByte = Member(stream!, "ReadByte", SpecialType.System_Int32);
            toArray = memoryStream!.GetMembers("ToArray").OfType<IMethodSymbol>()
                .SingleOrDefault(candidate => !candidate.IsStatic && candidate.Parameters.Length == 0 && candidate.Arity == 0);
            if (constructor is null || dispose is null || interfaceDispose is null || readByte is null ||
                streamReadByte is null || toArray is null ||
                !Same(memoryStream!.FindImplementationForInterfaceMember(interfaceDispose), dispose))
                Stop(ResourceLifetimeStatus.IncompleteInput, "framework_members_unbound");
        }

        private static IMethodSymbol? Member(INamedTypeSymbol type, string name, SpecialType returnType) =>
            type.GetMembers(name).OfType<IMethodSymbol>().SingleOrDefault(candidate =>
                !candidate.IsStatic && candidate.Parameters.Length == 0 && candidate.Arity == 0 &&
                candidate.ReturnType.SpecialType == returnType);

        private static bool Same(ISymbol? left, ISymbol? right) => SymbolEqualityComparer.Default.Equals(left, right);

        private void Walk(ControlFlowGraph graph)
        {
            if (graph.Blocks.Length > limits.MaxBlocks) Stop(ResourceLifetimeStatus.BudgetExceeded, "block_limit");
            var regions = new Stack<ControlFlowRegion>();
            regions.Push(graph.Root);
            while (regions.TryPop(out var region))
            {
                Tick();
                if (region.Kind is not (ControlFlowRegionKind.Root or ControlFlowRegionKind.LocalLifetime))
                    Stop(ResourceLifetimeStatus.Unsupported, "exception_or_unmodeled_region");
                foreach (var nested in region.NestedRegions) { Tick(); regions.Push(nested); }
            }
            // Reject correlations and backward flow before producing observations. Parameters cannot be
            // assigned or passed by ref in this slice. One bool-parameter split admits both valuations.
            var conditions = 0;
            foreach (var block in graph.Blocks.Where(block => block.IsReachable))
            {
                if (block.ConditionKind != ControlFlowConditionKind.None &&
                    block.BranchValue?.ConstantValue is not { HasValue: true, Value: bool })
                {
                    if (++conditions > limits.MaxNonconstantConditions || block.BranchValue is null ||
                        !BooleanParameter(block.BranchValue, 0))
                        Stop(ResourceLifetimeStatus.Unsupported, "predicate_correlation_or_shape");
                }
                foreach (var edge in new[] { block.FallThroughSuccessor, block.ConditionalSuccessor })
                {
                    Tick();
                    if (edge is null) continue;
                    if (edge.FinallyRegions.Length != 0 || edge.Semantics is not
                            (ControlFlowBranchSemantics.Regular or ControlFlowBranchSemantics.Return) ||
                        edge.Destination is null || edge.Destination.Ordinal <= block.Ordinal)
                        Stop(ResourceLifetimeStatus.Unsupported, "non_forward_normal_flow");
                }
            }

            var incoming = new State?[graph.Blocks.Length];
            incoming[0] = new State();
            foreach (var block in graph.Blocks)
            {
                Tick();
                if (!block.IsReachable || incoming[block.Ordinal] is not { } state) continue;
                foreach (var operation in block.Operations)
                {
                    Evaluate(operation, state, 0);
                    if (state.Terminated) break;
                }
                var branchObject = !state.Terminated && block.BranchValue is { } branch
                    ? Evaluate(branch, state, 0) : -1;
                if (state.Terminated) continue;
                if (block.ConditionKind == ControlFlowConditionKind.None && branchObject >= 0)
                    Stop(ResourceLifetimeStatus.Unsupported, "returned_resource_escape");
                bool? conditional = block.BranchValue?.ConstantValue is { HasValue: true, Value: bool value }
                    ? block.ConditionKind switch
                    {
                        ControlFlowConditionKind.WhenTrue => value,
                        ControlFlowConditionKind.WhenFalse => !value,
                        _ => null
                    } : null;
                Transfer(block.FallThroughSuccessor, conditional != true);
                Transfer(block.ConditionalSuccessor, conditional != false);

                void Transfer(ControlFlowBranch? edge, bool feasible)
                {
                    if (!feasible || edge?.Destination is not { IsReachable: true } destination) return;
                    var next = Copy(state);
                    foreach (var leaving in edge.LeavingRegions)
                    {
                        foreach (var local in leaving.Locals) { Tick(); next.Locals.Remove(local); }
                        foreach (var capture in leaving.CaptureIds) { Tick(); next.Captures.Remove(capture); }
                    }
                    if (incoming[destination.Ordinal] is { } previous) Join(previous, next);
                    else incoming[destination.Ordinal] = next;
                }
            }
        }

        // -1 denotes an untracked value, never a fresh identity inferred from a variable's type.
        private int Evaluate(IOperation operation, State state, int depth)
        {
            Tick();
            if (depth >= limits.MaxDepth) Stop(ResourceLifetimeStatus.BudgetExceeded, "operation_depth_limit");
            if (state.Terminated) return -1;
            switch (operation)
            {
                case IExpressionStatementOperation expression:
                    return Evaluate(expression.Operation, state, depth + 1);
                case IParenthesizedOperation parenthesized:
                    return Evaluate(parenthesized.Operand, state, depth + 1);
                case ILocalReferenceOperation local when local.Local.RefKind == RefKind.None:
                    return state.Locals.GetValueOrDefault(local.Local, -1);
                case IParameterReferenceOperation parameter when parameter.Parameter.RefKind == RefKind.None:
                    return -1;
                case ILiteralOperation:
                case IDefaultValueOperation:
                case IDiscardOperation:
                    return -1;
                case IConversionOperation conversion when conversion.OperatorMethod is null &&
                    (conversion.Conversion.IsIdentity || conversion.Conversion.IsReference):
                    var converted = Evaluate(conversion.Operand, state, depth + 1);
                    if (converted < 0 && !conversion.Conversion.IsIdentity && !conversion.IsImplicit)
                        Stop(ResourceLifetimeStatus.Unsupported, "untracked_explicit_reference_cast");
                    if (converted >= 0 && !(Same(conversion.Type, memoryStream) || Same(conversion.Type, stream) ||
                        Same(conversion.Type, disposable) || conversion.Type?.SpecialType == SpecialType.System_Object))
                        Stop(ResourceLifetimeStatus.Unsupported, "unmodeled_reference_conversion");
                    return converted;
                case ISimpleAssignmentOperation assignment when !assignment.IsRef:
                    if (assignment.Target is not ILocalReferenceOperation { Local.RefKind: RefKind.None } &&
                        assignment.Target is not IDiscardOperation)
                        Stop(ResourceLifetimeStatus.Unsupported, "nonlocal_assignment_escape");
                    var assigned = Evaluate(assignment.Value, state, depth + 1);
                    if (assignment.Target is ILocalReferenceOperation target)
                    {
                        if (assigned < 0) state.Locals.Remove(target.Local);
                        else state.Locals[target.Local] = assigned;
                        CheckTracked(state);
                    }
                    return assigned;
                case IObjectCreationOperation creation when Same(creation.Type, memoryStream) &&
                    Same(creation.Constructor, constructor) && creation.Arguments.Length == 0 && creation.Initializer is null:
                    var id = creation.Syntax.SpanStart;
                    allocations[id] = creation.Syntax.Span;
                    state.Resources[id] = new(ResourceLifetimeUseState.Live, []);
                    CheckTracked(state);
                    return id;
                case IFlowCaptureOperation capture:
                    var captured = Evaluate(capture.Value, state, depth + 1);
                    if (captured < 0) state.Captures.Remove(capture.Id);
                    else state.Captures[capture.Id] = captured;
                    CheckTracked(state);
                    return captured;
                case IFlowCaptureReferenceOperation capture:
                    return state.Captures.GetValueOrDefault(capture.Id, -1);
                case IBinaryOperation binary when binary.OperatorMethod is null && !binary.IsLifted && !binary.IsChecked &&
                    binary.OperatorKind is not (BinaryOperatorKind.Divide or BinaryOperatorKind.Remainder) &&
                    Primitive(binary.LeftOperand.Type) && Primitive(binary.RightOperand.Type):
                    Evaluate(binary.LeftOperand, state, depth + 1);
                    Evaluate(binary.RightOperand, state, depth + 1);
                    return -1;
                case IUnaryOperation unary when unary.OperatorMethod is null && !unary.IsLifted && !unary.IsChecked &&
                    Primitive(unary.Operand.Type):
                    Evaluate(unary.Operand, state, depth + 1);
                    return -1;
                case IInvocationOperation invocation when invocation.Arguments.Length == 0:
                    return Invoke(invocation, state, depth);
                default:
                    Stop(ResourceLifetimeStatus.Unsupported, "operation_" + operation.Kind);
                    return -1;
            }
        }

        private static bool Primitive(ITypeSymbol? type) =>
            type?.SpecialType is SpecialType.System_Boolean or SpecialType.System_Int32;

        private bool BooleanParameter(IOperation operation, int depth)
        {
            Tick();
            if (depth >= limits.MaxDepth) Stop(ResourceLifetimeStatus.BudgetExceeded, "condition_depth_limit");
            return operation switch
            {
                IParameterReferenceOperation parameter => parameter.Parameter.RefKind == RefKind.None &&
                    parameter.Parameter.Type.SpecialType == SpecialType.System_Boolean,
                IParenthesizedOperation parenthesized => BooleanParameter(parenthesized.Operand, depth + 1),
                IConversionOperation conversion when conversion.OperatorMethod is null && conversion.Conversion.IsIdentity =>
                    BooleanParameter(conversion.Operand, depth + 1),
                IUnaryOperation unary when unary.OperatorMethod is null && !unary.IsLifted &&
                    unary.OperatorKind == UnaryOperatorKind.Not => BooleanParameter(unary.Operand, depth + 1),
                _ => false
            };
        }

        private int Invoke(IInvocationOperation invocation, State state, int depth)
        {
            var target = invocation.TargetMethod.OriginalDefinition;
            if (!(Same(target, dispose) || Same(target, interfaceDispose) || Same(target, readByte) ||
                  Same(target, streamReadByte) || Same(target, toArray)))
                Stop(ResourceLifetimeStatus.Unsupported, "opaque_call_or_dispatch");
            var id = invocation.Instance is { } instance ? Evaluate(instance, state, depth + 1) : -1;
            if (state.Terminated) return -1;
            if (Same(target, readByte) || Same(target, streamReadByte))
            {
                var life = id >= 0 && state.Resources.TryGetValue(id, out var known)
                    ? known : new Lifetime(ResourceLifetimeUseState.Unknown, []);
                if (uses.Count >= limits.MaxUses) Stop(ResourceLifetimeStatus.BudgetExceeded, "use_limit");
                if (!useSpans.Add(invocation.Syntax.Span)) Stop(ResourceLifetimeStatus.Unsupported, "duplicate_lowered_use");
                uses.Add(new(Span(tree, invocation.Syntax.Span), life.State,
                    id >= 0 && allocations.TryGetValue(id, out var origin) ? Span(tree, origin) : null,
                    life.Disposals.Select(disposal => Span(tree, disposal)).ToArray(),
                    life.State != ResourceLifetimeUseState.Unknown ? null : id < 0
                        ? "object_identity_unknown" : "joined_lifetime_unknown"));
                // Exact MemoryStream.ReadByte throws on a disposed instance; no catch regions are admitted.
                if (life.State == ResourceLifetimeUseState.Disposed) state.Terminated = true;
                return -1;
            }
            if (id < 0) Stop(ResourceLifetimeStatus.Unsupported, "unknown_disposal_or_observer_receiver");
            if (Same(target, dispose) || Same(target, interfaceDispose))
                state.Resources[id] = new(ResourceLifetimeUseState.Disposed, [invocation.Syntax.Span]);
            // ToArray on an exact MemoryStream is legal after close and preserves this lifetime state.
            return -1;
        }

        private void CheckTracked(State state)
        {
            if (allocations.Count > limits.MaxTracked || state.Resources.Count > limits.MaxTracked ||
                state.Locals.Count > limits.MaxTracked || state.Captures.Count > limits.MaxTracked)
                Stop(ResourceLifetimeStatus.BudgetExceeded, "identity_or_alias_limit");
        }

        private State Copy(State state)
        {
            var result = new State();
            foreach (var pair in state.Locals) { Tick(); result.Locals.Add(pair.Key, pair.Value); }
            foreach (var pair in state.Captures) { Tick(); result.Captures.Add(pair.Key, pair.Value); }
            foreach (var pair in state.Resources) { Tick(); result.Resources.Add(pair.Key, pair.Value); }
            return result;
        }

        private void Join(State left, State right)
        {
            foreach (var local in left.Locals.Keys.ToArray())
            {
                Tick();
                if (!right.Locals.TryGetValue(local, out var id) || left.Locals[local] != id) left.Locals.Remove(local);
            }
            foreach (var capture in left.Captures.Keys.ToArray())
            {
                Tick();
                if (!right.Captures.TryGetValue(capture, out var id) || left.Captures[capture] != id) left.Captures.Remove(capture);
            }
            foreach (var id in left.Resources.Keys.ToArray())
            {
                Tick();
                if (!right.Resources.TryGetValue(id, out var other)) { left.Resources.Remove(id); continue; }
                var current = left.Resources[id];
                var disposals = current.Disposals.Concat(other.Disposals).Distinct()
                    .OrderBy(span => span.Start).ThenBy(span => span.Length).ToArray();
                if (disposals.Length > limits.MaxWitnessesPerUse)
                    Stop(ResourceLifetimeStatus.BudgetExceeded, "disposal_witness_limit");
                left.Resources[id] = new(current.State == other.State ? current.State : ResourceLifetimeUseState.Unknown, disposals);
            }
        }
    }
}
