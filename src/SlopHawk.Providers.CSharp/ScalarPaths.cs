using Microsoft.CodeAnalysis;
using Microsoft.CodeAnalysis.CSharp;
using Microsoft.CodeAnalysis.CSharp.Syntax;
using Microsoft.CodeAnalysis.FlowAnalysis;
using Microsoft.CodeAnalysis.Operations;
using Microsoft.CodeAnalysis.Text;

namespace SlopHawk.Providers.CSharp;

public sealed record ScalarPathResult(bool Bound, string? Reason, object? Graph,
    IReadOnlyList<ScalarPathCandidate> Candidates, int Work, IReadOnlyList<string> Unknown)
{
    public IReadOnlyList<string>? ParameterTypes { get; init; }
    public bool ScalarEligible { get; init; }
}

public sealed record ScalarPathCandidate(long[] Args, string Goal);

public static partial class ScalarPaths
{
    public static ScalarPathResult Explore(CSharpCompilation compilation, string documentPath,
        string signature, IReadOnlyList<long[]> seeds, int workLimit, int caseLimit,
        CancellationToken cancellationToken = default)
    {
        ArgumentNullException.ThrowIfNull(compilation);
        ArgumentNullException.ThrowIfNull(seeds);
        ArgumentException.ThrowIfNullOrWhiteSpace(documentPath);
        ArgumentException.ThrowIfNullOrWhiteSpace(signature);
        ArgumentOutOfRangeException.ThrowIfNegative(workLimit);
        ArgumentOutOfRangeException.ThrowIfNegative(caseLimit);
        cancellationToken.ThrowIfCancellationRequested();

        var trees = compilation.SyntaxTrees.Where(tree => tree.FilePath == documentPath).Take(2).ToArray();
        if (trees.Length != 1)
            return Unbound("The exact document path must select one compilation tree.");

        var tree = trees[0];
        var model = compilation.GetSemanticModel(tree);
        var matches = new List<(SyntaxNode Syntax, SyntaxNode Body, IMethodSymbol Symbol)>();
        foreach (var node in tree.GetRoot(cancellationToken).DescendantNodes())
        {
            cancellationToken.ThrowIfCancellationRequested();
            var body = node switch
            {
                MethodDeclarationSyntax method => (SyntaxNode?)method.Body ?? method.ExpressionBody?.Expression,
                LocalFunctionStatementSyntax function => (SyntaxNode?)function.Body ?? function.ExpressionBody?.Expression,
                _ => null
            };
            if (body is not null && model.GetDeclaredSymbol(node, cancellationToken) is IMethodSymbol symbol &&
                symbol.ToDisplayString(SymbolDisplayFormat.CSharpErrorMessageFormat) == signature)
                matches.Add((node, body, symbol));
        }

        if (matches.Count != 1)
            return Unbound("The exact method signature must select one declaration with a body in the document.");

        var (declaration, methodBody, methodSymbol) = matches[0];
        var flow = BindFlow(declaration, model, cancellationToken);
        if (flow is null)
            return Unbound("Roslyn could not bind the selected method body to a control-flow graph.");

        var graph = Describe(flow, methodSymbol, signature, tree, declaration.Span, methodBody.Span,
            cancellationToken);
        var eligible = IsScalarEligible(methodSymbol);
        return new ScalarSearch(flow, methodSymbol, seeds, workLimit, caseLimit, cancellationToken).Run(graph, eligible) with
        {
            ParameterTypes = methodSymbol.Parameters.Select(parameter => parameter.Type.SpecialType switch
            {
                SpecialType.System_Int32 => "int",
                SpecialType.System_Boolean => "bool",
                _ => parameter.Type.ToDisplayString()
            }).ToArray(),
            ScalarEligible = eligible
        };
    }

    private static bool IsScalarEligible(IMethodSymbol method) => !method.IsAsync && IsScalar(method.ReturnType) &&
        method.RefKind == RefKind.None && method.Parameters.All(parameter =>
            IsScalar(parameter.Type) && parameter.RefKind == RefKind.None);

    private static ScalarPathResult Unbound(string reason) => new(false, reason, null, [], 0, [reason]);

    private static ControlFlowGraph? BindFlow(SyntaxNode declaration, SemanticModel model,
        CancellationToken cancellationToken)
    {
        try
        {
            if (declaration is not LocalFunctionStatementSyntax)
                return ControlFlowGraph.Create(declaration, model, cancellationToken);

            var containing = declaration.Ancestors().FirstOrDefault(node => node is
                MethodDeclarationSyntax or LocalFunctionStatementSyntax);
            var function = model.GetDeclaredSymbol(declaration, cancellationToken) as IMethodSymbol;
            if (containing is null || function is null)
                return null;

            return BindFlow(containing, model, cancellationToken)?
                .GetLocalFunctionControlFlowGraph(function, cancellationToken);
        }
        catch (ArgumentException)
        {
            return null;
        }
        catch (InvalidOperationException)
        {
            return null;
        }
    }

    private static PathGraph Describe(ControlFlowGraph flow, IMethodSymbol method, string signature,
        SyntaxTree tree, TextSpan declaration, TextSpan body, CancellationToken cancellationToken)
    {
        var regionIds = new Dictionary<ControlFlowRegion, int>();
        var regions = new List<PathRegion>();
        AddRegion(flow.Root, null);

        var blocks = new List<PathBlock>();
        var edges = new List<PathEdge>();
        foreach (var block in flow.Blocks)
        {
            cancellationToken.ThrowIfCancellationRequested();
            blocks.Add(new(block.Ordinal, block.Kind.ToString(), block.IsReachable, regionIds[block.EnclosingRegion],
                block.ConditionKind.ToString(), block.Operations.Select(DescribeOperation).ToArray(),
                block.BranchValue is { } value ? DescribeOperation(value) : null));
            AddEdge(block, block.FallThroughSuccessor, conditional: false);
            AddEdge(block, block.ConditionalSuccessor, conditional: true);
        }

        return new(1, "Structural", tree.FilePath, signature, Span(tree, declaration), Span(tree, body),
            method.ReturnType.ToDisplayString(), method.Parameters.Select(parameter =>
                new PathParameter(parameter.Ordinal, parameter.Name, parameter.Type.ToDisplayString(),
                    parameter.RefKind.ToString())).ToArray(), regions, blocks, edges);

        void AddRegion(ControlFlowRegion region, int? parent)
        {
            cancellationToken.ThrowIfCancellationRequested();
            var id = regions.Count;
            regionIds.Add(region, id);
            regions.Add(new(id, parent, region.Kind.ToString(), region.FirstBlockOrdinal, region.LastBlockOrdinal,
                region.ExceptionType?.ToDisplayString()));
            foreach (var nested in region.NestedRegions)
                AddRegion(nested, id);
        }

        void AddEdge(BasicBlock block, ControlFlowBranch? edge, bool conditional)
        {
            if (edge is null)
                return;
            var role = conditional ? "Conditional" : "FallThrough";
            bool? polarity = block.ConditionKind switch
            {
                ControlFlowConditionKind.WhenTrue => conditional,
                ControlFlowConditionKind.WhenFalse => !conditional,
                _ => null
            };
            var operation = block.BranchValue;
            var span = operation is null ? null : Span(operation.Syntax.SyntaxTree, operation.Syntax.Span);
            edges.Add(new(EdgeId(block, conditional), block.Ordinal, edge.Destination?.Ordinal,
                role, polarity, edge.Semantics.ToString(), edge.EnteringRegions.Select(region => regionIds[region]).ToArray(),
                edge.LeavingRegions.Select(region => regionIds[region]).ToArray(),
                edge.FinallyRegions.Select(region => regionIds[region]).ToArray(), span,
                operation is null ? "unmapped" : SpanKind(operation)));
        }

        PathOperation DescribeOperation(IOperation operation)
        {
            cancellationToken.ThrowIfCancellationRequested();
            (string? Operator, bool? Checked, PathConversion? Input, PathConversion? Output) detail = operation switch
            {
                IBinaryOperation binary => (binary.OperatorKind.ToString(), binary.IsChecked, null, null),
                IUnaryOperation unary => (unary.OperatorKind.ToString(), unary.IsChecked, null, null),
                IIncrementOrDecrementOperation step => (step.Kind.ToString(), step.IsChecked, null, null),
                ICompoundAssignmentOperation assignment => (assignment.OperatorKind.ToString(), assignment.IsChecked,
                    DescribeConversion(assignment.InConversion), DescribeConversion(assignment.OutConversion)),
                IConversionOperation conversion => (null, conversion.IsChecked, DescribeConversion(conversion.Conversion), null),
                _ => (null, null, null, null)
            };
            return new(operation.Kind.ToString(), operation.Type?.ToDisplayString(), operation.IsImplicit,
                Span(operation.Syntax.SyntaxTree, operation.Syntax.Span), SpanKind(operation), detail.Operator,
                detail.Checked, detail.Input, detail.Output, operation.ChildOperations.Select(DescribeOperation).ToArray());
        }
    }

    private static string SpanKind(IOperation operation) => operation.Syntax.Span.IsEmpty ? "synthesized" :
        operation.IsImplicit ? "enclosing" : "exact";

    private static PathConversion DescribeConversion(CommonConversion conversion) =>
        new(conversion.Exists, conversion.IsIdentity, conversion.IsNumeric, conversion.IsReference,
            conversion.IsNullable, conversion.IsUserDefined,
            conversion.MethodSymbol?.ToDisplayString(SymbolDisplayFormat.CSharpErrorMessageFormat));

    private static string EdgeId(BasicBlock block, bool conditional) =>
        $"b{block.Ordinal}:{(conditional ? "conditional" : "fallThrough")}";

    private static DocumentSpan Span(SyntaxTree tree, TextSpan span)
    {
        var lines = tree.GetLineSpan(span);
        return new(span.Start, span.Length, lines.StartLinePosition.Line, lines.StartLinePosition.Character,
            lines.EndLinePosition.Line, lines.EndLinePosition.Character);
    }

    private sealed record PathGraph(int SchemaVersion, string Status, string DocumentPath, string Signature,
        DocumentSpan Declaration, DocumentSpan Body, string ReturnType, IReadOnlyList<PathParameter> Parameters,
        IReadOnlyList<PathRegion> Regions, IReadOnlyList<PathBlock> Blocks, IReadOnlyList<PathEdge> Edges);

    private sealed record PathParameter(int Ordinal, string Name, string Type, string RefKind);

    private sealed record PathRegion(int Id, int? Parent, string Kind, int FirstBlock, int LastBlock,
        string? ExceptionType);

    private sealed record PathBlock(int Id, string Kind, bool RoslynReachable, int Region, string ConditionKind,
        IReadOnlyList<PathOperation> Operations, PathOperation? BranchValue);

    private sealed record PathEdge(string Id, int From, int? To, string Role, bool? Polarity, string Semantics,
        IReadOnlyList<int> EnteringRegions, IReadOnlyList<int> LeavingRegions, IReadOnlyList<int> FinallyRegions,
        DocumentSpan? Span, string SpanKind);

    private sealed record PathOperation(string Kind, string? Type, bool Implicit, DocumentSpan Span,
        string SpanKind, string? Operator, bool? Checked, PathConversion? InputConversion,
        PathConversion? OutputConversion, IReadOnlyList<PathOperation> Children);

    private sealed record PathConversion(bool Exists, bool Identity, bool Numeric, bool Reference,
        bool Nullable, bool UserDefined, string? Method);
}
