using System.Security.Cryptography;
using System.Text;
using Microsoft.CodeAnalysis;
using Microsoft.CodeAnalysis.CSharp;
using Microsoft.CodeAnalysis.CSharp.Syntax;
using Microsoft.CodeAnalysis.Operations;
using Microsoft.CodeAnalysis.Text;

namespace SlopGuard.Providers.CSharp;

// The caller has already established these identities and selected a compilation. No path is opened here.
// ContextKey includes the evaluated target; InputFingerprint/Revision identify the version, not the symbol.
public sealed record RoslynDocumentIdentity(
    string ContextKey, string DocumentKey, string PhysicalPath, string LogicalPath,
    string Revision, string InputFingerprint, bool ReferenceClosureComplete = false);

public sealed record RoslynFactLimits(int MaxRecords = 20_000, int MaxFrontiers = 256,
    int MaxSyntaxNodes = 500_000);

public sealed record DocumentSpan(int Start, int Length, int StartLine, int StartColumn,
    int EndLine, int EndColumn);

public enum ReferenceUse { Read, Write, ReadWrite, TypeUse, Attribute, Invocation, Construction }

public enum StaticCallBinding { Direct, PotentialDispatch, Unresolved, Dynamic, Delegate }

public enum IncompleteReason
{
    MissingCompilationReferences,
    UnresolvedBinding,
    UnsupportedDynamicDispatch,
    UnsupportedDelegateDispatch,
    PotentialRuntimeDispatch,
    RecordLimit,
    SyntaxNodeLimit,
    FrontierLimit,
    Cancelled
}

public sealed record DeclarationFact(string SymbolId, string Name, string SymbolKind, DocumentSpan Span);

public sealed record ReferenceFact(string? SymbolId, string? OwnerId, ReferenceUse Use,
    DocumentSpan Span, string? CandidateReason);

// Direct is a compiler-bound static call edge, not proof that the call executes at runtime.
// PotentialDispatch retains the static method and marks an unknown override/implementation.
public sealed record CallFact(string? CallerId, string? CalleeId, StaticCallBinding Binding,
    DocumentSpan Span, string? ReceiverTypeId, string? CandidateReason);

public sealed record IncompleteScope(IncompleteReason Reason, DocumentSpan? Span, string Detail);

public sealed record RoslynDocumentFacts(
    int SchemaVersion, RoslynDocumentIdentity Identity, string CompilationAssembly,
    string RoslynCommonAssemblyVersion, string RoslynCSharpAssemblyVersion,
    IReadOnlyList<DeclarationFact> Declarations, IReadOnlyList<ReferenceFact> References,
    IReadOnlyList<CallFact> Calls, IReadOnlyList<IncompleteScope> Frontier)
{
    public bool Complete => Frontier.Count == 0;
}

/// <summary>
/// Extracts declarations, typed name references and statically bound explicit calls for one C# tree.
/// The caller owns compilation creation, trusted identity, MSBuild/generator policy and publication.
/// The routine performs no disk, network, process, generator or diagnostic operations.
/// </summary>
public static class RoslynDocumentFactExtractor
{
    public const int SchemaVersion = 1;

    public static RoslynDocumentFacts Extract(
        CSharpCompilation compilation, SyntaxTree tree, RoslynDocumentIdentity identity,
        RoslynFactLimits? limits = null, CancellationToken cancellationToken = default)
    {
        ArgumentNullException.ThrowIfNull(compilation);
        ArgumentNullException.ThrowIfNull(tree);
        ArgumentNullException.ThrowIfNull(identity);
        limits ??= new RoslynFactLimits();
        if (limits.MaxRecords is < 1 or > 100_000 || limits.MaxFrontiers is < 3 or > 10_000 ||
            limits.MaxSyntaxNodes is < 1 or > 2_000_000)
            throw new ArgumentOutOfRangeException(nameof(limits), "Record and frontier limits must be bounded and positive.");
        if (new[] { identity.ContextKey, identity.DocumentKey, identity.PhysicalPath,
                    identity.LogicalPath, identity.Revision, identity.InputFingerprint }
            .Any(part => string.IsNullOrWhiteSpace(part) || part.Length > 4096 || part.Contains('\0')))
            throw new ArgumentException("Trusted context, document and version identities are required.", nameof(identity));
        if (!compilation.SyntaxTrees.Any(item => ReferenceEquals(item, tree)))
            throw new ArgumentException("The supplied document must belong to the supplied compilation.", nameof(tree));

        var writer = new Collector(compilation, tree, identity, limits, cancellationToken);
        writer.Collect();
        return writer.Snapshot();
    }

    private sealed class Collector(
        CSharpCompilation compilation, SyntaxTree tree, RoslynDocumentIdentity identity,
        RoslynFactLimits limits, CancellationToken cancellationToken)
    {
        private readonly List<DeclarationFact> declarations = [];
        private readonly List<ReferenceFact> references = [];
        private readonly List<CallFact> calls = [];
        private readonly List<IncompleteScope> frontier = [];
        private readonly Dictionary<ISymbol, string> ids = new(SymbolEqualityComparer.Default);
        private bool stopped;
        private bool frontierLimited;
        private SemanticModel? model;

        public void Collect()
        {
            if (!identity.ReferenceClosureComplete)
                Mark(IncompleteReason.MissingCompilationReferences, null,
                    "The caller did not certify the compilation reference closure.");

            try
            {
                cancellationToken.ThrowIfCancellationRequested();
                var semanticModel = compilation.GetSemanticModel(tree);
                model = semanticModel;
                var root = tree.GetRoot(cancellationToken);
                var visited = 0;
                foreach (var node in root.DescendantNodesAndSelf())
                {
                    cancellationToken.ThrowIfCancellationRequested();
                    if (++visited > limits.MaxSyntaxNodes)
                    {
                        Mark(IncompleteReason.SyntaxNodeLimit, node.Span,
                            "The syntax-node traversal limit was reached.");
                        break;
                    }
                    if (TryDeclarationSpan(node, out var declarationSpan))
                    {
                        var symbol = semanticModel.GetDeclaredSymbol(node, cancellationToken);
                        if (symbol is not null && !Add(new DeclarationFact(
                                SymbolId(symbol), symbol.Name, symbol.Kind.ToString(), Span(declarationSpan))))
                            break;
                    }

                    if (node is SimpleNameSyntax name && !IsDeclarationOnlyName(name) && !IsContextualTypeKeyword(name))
                    {
                        var info = semanticModel.GetSymbolInfo(name, cancellationToken);
                        var target = info.Symbol;
                        if (target is null && InvocationForCallee(name) is { } enclosing)
                            target = semanticModel.GetSymbolInfo(enclosing, cancellationToken).Symbol;
                        var use = Usage(name, target);
                        if (!Add(new ReferenceFact(target is null ? null : SymbolId(target),
                                EnclosingSymbolId(name.SpanStart), use, Span(name.Span),
                                target is null ? info.CandidateReason.ToString() : null)))
                            break;
                        if (target is null)
                            Mark(IncompleteReason.UnresolvedBinding, name.Span,
                                "Name did not bind to one symbol: " + info.CandidateReason);
                    }

                    if (node is InvocationExpressionSyntax invocation)
                        VisitInvocation(invocation);
                    else if (node is ObjectCreationExpressionSyntax or ImplicitObjectCreationExpressionSyntax)
                        VisitCreation(node);
                    else if (node is ConstructorInitializerSyntax initializer)
                        VisitInitializer(initializer);

                    if (stopped)
                        break;
                }
            }
            catch (OperationCanceledException) when (cancellationToken.IsCancellationRequested)
            {
                Mark(IncompleteReason.Cancelled, null, "Document extraction was cancelled.");
            }
        }

        public RoslynDocumentFacts Snapshot() => new(
            SchemaVersion, identity, compilation.Assembly.Identity.ToString(),
            typeof(Compilation).Assembly.GetName().Version?.ToString() ?? "unknown",
            typeof(CSharpCompilation).Assembly.GetName().Version?.ToString() ?? "unknown",
            declarations.ToArray(), references.ToArray(), calls.ToArray(), frontier.ToArray());

        private void VisitInvocation(InvocationExpressionSyntax node)
        {
            var operation = model!.GetOperation(node, cancellationToken);
            if (operation is IDynamicInvocationOperation)
            {
                Add(new CallFact(EnclosingSymbolId(node.SpanStart), null,
                    StaticCallBinding.Dynamic, Span(node.Span), null, "runtime_dynamic_binder"));
                Mark(IncompleteReason.UnsupportedDynamicDispatch, node.Span,
                    "The dynamic binder chooses a target at runtime.");
            }
            else if (operation is IInvocationOperation invocation)
            {
                var method = invocation.TargetMethod;
                var receiver = invocation.Instance?.Type;
                if (method.MethodKind == MethodKind.DelegateInvoke)
                {
                    Add(new CallFact(EnclosingSymbolId(node.SpanStart), null,
                        StaticCallBinding.Delegate, Span(node.Span),
                        receiver is null ? null : SymbolId(receiver), "delegate_invoke"));
                    Mark(IncompleteReason.UnsupportedDelegateDispatch, node.Span,
                        "A delegate invocation does not identify the subscribed runtime target.");
                }
                else
                {
                    var possible = method.IsVirtual || method.IsAbstract || method.IsOverride ||
                        method.ContainingType.TypeKind == TypeKind.Interface;
                    Add(new CallFact(EnclosingSymbolId(node.SpanStart), SymbolId(method),
                        possible ? StaticCallBinding.PotentialDispatch : StaticCallBinding.Direct,
                        Span(node.Span), receiver is null ? null : SymbolId(receiver), null));
                    if (possible)
                        Mark(IncompleteReason.PotentialRuntimeDispatch, node.Span,
                            "An override or interface implementation may receive this call.");
                }
            }
            else if (operation is not INameOfOperation)
            {
                var info = model!.GetSymbolInfo(node, cancellationToken);
                Add(new CallFact(EnclosingSymbolId(node.SpanStart), null,
                    StaticCallBinding.Unresolved, Span(node.Span), null, info.CandidateReason.ToString()));
                Mark(IncompleteReason.UnresolvedBinding, node.Span,
                    "The invocation has no statically bound method.");
            }
        }

        private void VisitCreation(SyntaxNode node)
        {
            var operation = model!.GetOperation(node, cancellationToken);
            if (operation is IObjectCreationOperation creation && creation.Constructor is { } constructor)
            {
                Add(new CallFact(EnclosingSymbolId(node.SpanStart), SymbolId(constructor),
                    StaticCallBinding.Direct, Span(node.Span), null, null));
            }
            else
            {
                var info = model!.GetSymbolInfo(node, cancellationToken);
                Add(new CallFact(EnclosingSymbolId(node.SpanStart), null,
                    StaticCallBinding.Unresolved, Span(node.Span), null, info.CandidateReason.ToString()));
                Mark(IncompleteReason.UnresolvedBinding, node.Span,
                    "The constructor could not be bound.");
            }
        }

        private void VisitInitializer(ConstructorInitializerSyntax node)
        {
            var info = model!.GetSymbolInfo(node, cancellationToken);
            if (info.Symbol is IMethodSymbol ctor)
                Add(new CallFact(EnclosingSymbolId(node.SpanStart), SymbolId(ctor),
                    StaticCallBinding.Direct, Span(node.Span), null, null));
            else
            {
                Add(new CallFact(EnclosingSymbolId(node.SpanStart), null,
                    StaticCallBinding.Unresolved, Span(node.Span), null, info.CandidateReason.ToString()));
                Mark(IncompleteReason.UnresolvedBinding, node.Span,
                    "The constructor initializer did not bind.");
            }
        }

        private bool Add(DeclarationFact fact)
        {
            if (!Reserve(fact.Span)) return false;
            declarations.Add(fact);
            return true;
        }

        private bool Add(ReferenceFact fact)
        {
            if (!Reserve(fact.Span)) return false;
            references.Add(fact);
            return true;
        }

        private bool Add(CallFact fact)
        {
            if (!Reserve(fact.Span)) return false;
            calls.Add(fact);
            return true;
        }

        private bool Reserve(DocumentSpan span)
        {
            if (stopped) return false;
            if (declarations.Count + references.Count + calls.Count < limits.MaxRecords)
                return true;
            stopped = true;
            Mark(IncompleteReason.RecordLimit, new TextSpan(span.Start, span.Length),
                "The per-document fact record limit was reached.");
            return false;
        }

        private void Mark(IncompleteReason reason, TextSpan? span, string detail)
        {
            if (frontierLimited)
            {
                // Reserve one final slot so limits/cancellation cannot disappear behind noisy binding failures.
                if ((reason is IncompleteReason.RecordLimit or IncompleteReason.SyntaxNodeLimit or IncompleteReason.Cancelled) &&
                    frontier.Count < limits.MaxFrontiers)
                    frontier.Add(new IncompleteScope(reason, span is null ? null : Span(span.Value), detail));
                return;
            }
            if (frontier.Count >= limits.MaxFrontiers - 2)
            {
                frontier.Add(new IncompleteScope(IncompleteReason.FrontierLimit,
                    span is null ? null : Span(span.Value),
                    "Further incomplete scopes were omitted by the frontier limit."));
                frontierLimited = true;
                return;
            }
            frontier.Add(new IncompleteScope(reason, span is null ? null : Span(span.Value), detail));
        }

        private DocumentSpan Span(TextSpan span)
        {
            var line = tree.GetLineSpan(span);
            return new DocumentSpan(span.Start, span.Length,
                line.StartLinePosition.Line, line.StartLinePosition.Character,
                line.EndLinePosition.Line, line.EndLinePosition.Character);
        }

        private string? EnclosingSymbolId(int position)
        {
            var symbol = model!.GetEnclosingSymbol(position, cancellationToken);
            return symbol is null ? null : SymbolId(symbol);
        }

        private string SymbolId(ISymbol symbol)
        {
            // OriginalDefinition merges constructed generic references with their declaration.
            // Documentation IDs distinguish overloads, parameter types and explicit interface members.
            // A reduced extension invocation must point back to the declared extension method.
            if (symbol is IMethodSymbol { ReducedFrom: { } reduced })
                symbol = reduced;
            var original = symbol.OriginalDefinition;
            if (ids.TryGetValue(original, out var existing))
                return existing;
            var declaration = DocumentationCommentId.CreateDeclarationId(original);
            if (declaration is null)
            {
                var location = original.Locations.FirstOrDefault(candidate => candidate.IsInSource);
                var sourceIndex = location?.SourceTree is { } sourceTree
                    ? compilation.SyntaxTrees.ToList().FindIndex(item => ReferenceEquals(item, sourceTree))
                    : -1;
                var documentAnchor = sourceIndex >= 0
                    ? (ReferenceEquals(location!.SourceTree, tree) ? identity.DocumentKey : "tree:" + sourceIndex)
                    : "metadata";
                declaration = original.Kind + ":" + original.ToDisplayString(SymbolDisplayFormat.FullyQualifiedFormat) +
                              ":" + documentAnchor + ":" + (location?.SourceSpan.Start ?? -1);
            }
            var assembly = original.ContainingAssembly?.Identity.ToString() ??
                           compilation.Assembly.Identity.ToString();
            var text = "sg08:1\0" + identity.ContextKey + "\0" + assembly + "\0" + declaration;
            var id = Convert.ToHexString(SHA256.HashData(Encoding.UTF8.GetBytes(text)));
            ids.Add(original, id);
            return id;
        }

        private static InvocationExpressionSyntax? InvocationForCallee(SimpleNameSyntax name) =>
            name.Parent switch
            {
                InvocationExpressionSyntax invocation when ReferenceEquals(invocation.Expression, name) => invocation,
                MemberAccessExpressionSyntax access when ReferenceEquals(access.Name, name) &&
                    access.Parent is InvocationExpressionSyntax invocation &&
                    ReferenceEquals(invocation.Expression, access) => invocation,
                MemberBindingExpressionSyntax binding when ReferenceEquals(binding.Name, name) &&
                    binding.Parent is InvocationExpressionSyntax invocation => invocation,
                _ => null
            };

        private static ReferenceUse Usage(SimpleNameSyntax node, ISymbol? symbol)
        {
            if (node.Ancestors().OfType<AttributeSyntax>().Any(attribute => attribute.Name.Span.Contains(node.Span)))
                return ReferenceUse.Attribute;
            if (node.Ancestors().OfType<ObjectCreationExpressionSyntax>()
                .Any(creation => creation.Type.Span.Contains(node.Span)))
                return ReferenceUse.Construction;
            if (InvocationForCallee(node) is not null)
                return ReferenceUse.Invocation;
            if (symbol is INamedTypeSymbol or ITypeParameterSymbol or INamespaceSymbol ||
                node.Parent is TypeArgumentListSyntax or BaseTypeSyntax or QualifiedNameSyntax)
                return ReferenceUse.TypeUse;
            if (node.Ancestors().OfType<AssignmentExpressionSyntax>().FirstOrDefault(candidate =>
                candidate.Left.Span.End == node.Span.End) is { } assignment)
                return assignment.IsKind(Microsoft.CodeAnalysis.CSharp.SyntaxKind.SimpleAssignmentExpression)
                    ? ReferenceUse.Write : ReferenceUse.ReadWrite;
            if (node.Parent?.Kind() is SyntaxKind.PreIncrementExpression or SyntaxKind.PreDecrementExpression
                or SyntaxKind.PostIncrementExpression or SyntaxKind.PostDecrementExpression)
                return ReferenceUse.ReadWrite;
            return ReferenceUse.Read;
        }

        private static bool IsDeclarationOnlyName(SimpleNameSyntax name) =>
            name.Ancestors().OfType<BaseNamespaceDeclarationSyntax>()
                .Any(ns => ns.Name.Span.Contains(name.Span));

        // Contextual type keywords are not unresolved references. A user-declared type named
        // 'var' is still a real reference and must not be discarded.
        private bool IsContextualTypeKeyword(SimpleNameSyntax name)
        {
            if (name.Identifier.ValueText == "var")
            {
                var isInferredType = name.Parent switch
                {
                    VariableDeclarationSyntax declaration => ReferenceEquals(declaration.Type, name),
                    ForEachStatementSyntax each => ReferenceEquals(each.Type, name),
                    DeclarationExpressionSyntax expression => ReferenceEquals(expression.Type, name),
                    _ => false
                };
                if (isInferredType && model!.GetSymbolInfo(name, cancellationToken).Symbol is null)
                    return true;
            }
            return name.Identifier.ValueText == "dynamic" &&
                   model!.GetTypeInfo(name, cancellationToken).Type?.TypeKind == TypeKind.Dynamic;
        }

        private static bool TryDeclarationSpan(SyntaxNode node, out TextSpan span)
        {
            switch (node)
            {
                case BaseNamespaceDeclarationSyntax ns: span = ns.Name.Span; return true;
                case BaseTypeDeclarationSyntax type: span = type.Identifier.Span; return true;
                case DelegateDeclarationSyntax del: span = del.Identifier.Span; return true;
                case MethodDeclarationSyntax method: span = method.Identifier.Span; return true;
                case ConstructorDeclarationSyntax ctor: span = ctor.Identifier.Span; return true;
                case DestructorDeclarationSyntax destructor: span = destructor.Identifier.Span; return true;
                case PropertyDeclarationSyntax property: span = property.Identifier.Span; return true;
                case EventDeclarationSyntax ev: span = ev.Identifier.Span; return true;
                case IndexerDeclarationSyntax indexer: span = indexer.ThisKeyword.Span; return true;
                case OperatorDeclarationSyntax op: span = op.OperatorToken.Span; return true;
                case ConversionOperatorDeclarationSyntax conv: span = conv.ImplicitOrExplicitKeyword.Span; return true;
                case VariableDeclaratorSyntax variable: span = variable.Identifier.Span; return true;
                case EnumMemberDeclarationSyntax member: span = member.Identifier.Span; return true;
                case LocalFunctionStatementSyntax local: span = local.Identifier.Span; return true;
                case ParameterSyntax parameter: span = parameter.Identifier.Span; return true;
                case TypeParameterSyntax typeParameter: span = typeParameter.Identifier.Span; return true;
                case SingleVariableDesignationSyntax designation: span = designation.Identifier.Span; return true;
                case ForEachStatementSyntax each: span = each.Identifier.Span; return true;
                case CatchDeclarationSyntax catchClause: span = catchClause.Identifier.Span; return true;
                case AccessorDeclarationSyntax accessor: span = accessor.Keyword.Span; return true;
                default: span = default; return false;
            }
        }
    }
}
