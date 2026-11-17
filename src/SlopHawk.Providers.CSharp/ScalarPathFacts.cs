using System.Globalization;
using System.Text;
using Microsoft.CodeAnalysis;
using Microsoft.CodeAnalysis.CSharp;
using Microsoft.CodeAnalysis.CSharp.Syntax;
using Microsoft.CodeAnalysis.FlowAnalysis;
using Microsoft.CodeAnalysis.Operations;

namespace SlopHawk.Providers.CSharp;

// Opt-in, data-only boundary: Roslyn continues to own binding, CFG topology and source attribution.
// Neither Explore nor the CLI's default candidate/replay path calls this projection.
public sealed record ScalarPathFactParameter(int Ordinal, string Type);

public sealed record ScalarPathFactNode(int Id, string Kind, string Type, int? Slot, int? Constant,
    string? Operator, bool Checked, bool Lifted, bool Postfix, bool Writable,
    int[] Children, string? Hole, string Diagnostic);

public sealed record ScalarPathFactEdge(int? To, string Semantics, bool HasFinally,
    int[] LeavingLocals, int[] LeavingCaptures);

public sealed record ScalarPathFactBlock(int Ordinal, string Kind, string ConditionKind,
    ScalarPathFactNode[] Nodes, int[] Statements, int? Branch,
    ScalarPathFactEdge? FallThrough, ScalarPathFactEdge? Conditional);

public sealed record ScalarPathFactProjection(int SchemaVersion, bool Bound, bool Eligible, string? Reason,
    object? Graph, ScalarPathFactParameter[] Parameters, ScalarPathFactBlock[] Blocks)
{
    /// <summary>
    /// Deterministic UTF-8, ASCII-tab-delimited v1 wire contract. Text fields are either fixed
    /// enum names or hex-encoded UTF-8; all node/edge references are typed numeric IDs.
    /// Seeds are intentionally separate from facts, so identical graph facts can be replayed.
    /// </summary>
    public string ToWire(IReadOnlyList<long[]> seeds, int workLimit, int caseLimit)
    {
        ArgumentNullException.ThrowIfNull(seeds);
        ArgumentOutOfRangeException.ThrowIfNegative(workLimit);
        ArgumentOutOfRangeException.ThrowIfNegative(caseLimit);

        var result = new StringBuilder();
        result.AppendLine("scalar-path-facts\t1");
        result.Append("limits\t").Append(workLimit).Append('\t').Append(caseLimit).Append('\n');
        result.Append("bound\t").Append(Bound ? '1' : '0').Append('\t').Append(Eligible ? '1' : '0')
            .Append('\t').Append(Hex(Reason)).Append('\n');
        foreach (var parameter in Parameters)
            result.Append("param\t").Append(parameter.Ordinal).Append('\t').Append(parameter.Type).Append('\n');
        foreach (var block in Blocks)
        {
            result.Append("block\t").Append(block.Ordinal).Append('\t').Append(block.Kind)
                .Append('\t').Append(block.ConditionKind).Append('\n');
            foreach (var node in block.Nodes)
            {
                result.Append("node\t").Append(block.Ordinal).Append('\t').Append(node.Id)
                    .Append('\t').Append(node.Kind).Append('\t').Append(node.Type)
                    .Append('\t').Append(Id(node.Slot)).Append('\t').Append(Id(node.Constant))
                    .Append('\t').Append(node.Operator ?? "-").Append('\t').Append(Bool(node.Checked))
                    .Append('\t').Append(Bool(node.Lifted)).Append('\t').Append(Bool(node.Postfix))
                    .Append('\t').Append(Bool(node.Writable)).Append('\t')
                    .Append(string.Join(',', node.Children)).Append('\t').Append(Hex(node.Hole))
                    .Append('\t').Append(Hex(node.Diagnostic)).Append('\n');
            }
            foreach (var statement in block.Statements)
                result.Append("stmt\t").Append(block.Ordinal).Append('\t').Append(statement).Append('\n');
            result.Append("branch\t").Append(block.Ordinal).Append('\t').Append(Id(block.Branch)).Append('\n');
            Edge("fallThrough", block.FallThrough);
            Edge("conditional", block.Conditional);

            void Edge(string role, ScalarPathFactEdge? edge)
            {
                if (edge is null)
                    return;
                result.Append("edge\t").Append(block.Ordinal).Append('\t').Append(role).Append('\t')
                    .Append(Id(edge.To)).Append('\t').Append(edge.Semantics).Append('\t')
                    .Append(Bool(edge.HasFinally)).Append('\t').Append(Csv(edge.LeavingLocals))
                    .Append('\t').Append(Csv(edge.LeavingCaptures)).Append('\n');
            }
        }
        foreach (var seed in seeds)
        {
            if (seed is null)
                result.AppendLine("seed\t-");
            else
                result.Append("seed\t").Append(string.Join(',', seed.Select(value =>
                    value.ToString(CultureInfo.InvariantCulture)))).Append('\n');
        }
        return result.AppendLine("end").ToString();
    }

    private static string Csv(int[] values) => values.Length == 0 ? "-" : string.Join(',', values);
    private static string Id(int? value) => value?.ToString(CultureInfo.InvariantCulture) ?? "-";
    private static char Bool(bool value) => value ? '1' : '0';
    private static string Hex(string? value) => value is null ? "-" : Convert.ToHexString(Encoding.UTF8.GetBytes(value));
}

public static partial class ScalarPaths
{
    /// <summary>Project one exactly selected bound method to typed scalar facts without executing it.</summary>
    public static ScalarPathFactProjection ProjectFacts(CSharpCompilation compilation, string documentPath,
        string signature, CancellationToken cancellationToken = default)
    {
        ArgumentNullException.ThrowIfNull(compilation);
        ArgumentException.ThrowIfNullOrWhiteSpace(documentPath);
        ArgumentException.ThrowIfNullOrWhiteSpace(signature);
        cancellationToken.ThrowIfCancellationRequested();

        var trees = compilation.SyntaxTrees.Where(tree => tree.FilePath == documentPath).Take(2).ToArray();
        if (trees.Length != 1)
            return UnboundFacts("The exact document path must select one compilation tree.");

        var tree = trees[0];
        var model = compilation.GetSemanticModel(tree);
        var matches = new List<(SyntaxNode Declaration, SyntaxNode Body, IMethodSymbol Method)>();
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
            return UnboundFacts("The exact method signature must select one declaration with a body in the document.");

        var (declaration, bodySyntax, methodSymbol) = matches[0];
        var flow = BindFlow(declaration, model, cancellationToken);
        if (flow is null)
            return UnboundFacts("Roslyn could not bind the selected method body to a control-flow graph.");

        var emitter = new FactEmitter(methodSymbol, AssignmentCaptures(flow, cancellationToken), cancellationToken);
        var blocks = flow.Blocks.Select(emitter.Block).ToArray();
        return new(1, true, IsScalarEligible(methodSymbol), null,
            Describe(flow, methodSymbol, signature, tree, declaration.Span, bodySyntax.Span, cancellationToken),
            methodSymbol.Parameters.Select(parameter => new ScalarPathFactParameter(parameter.Ordinal,
                ScalarType(parameter.Type))).ToArray(), blocks);
    }

    private static ScalarPathFactProjection UnboundFacts(string reason) =>
        new(1, false, false, reason, null, [], []);

    private static string ScalarType(ITypeSymbol? type) => type?.SpecialType switch
    {
        SpecialType.System_Int32 => "int",
        SpecialType.System_Boolean => "bool",
        _ => "other"
    };

    private sealed class FactEmitter(IMethodSymbol method, IReadOnlySet<CaptureId> assignmentCaptures,
        CancellationToken cancellationToken)
    {
        private readonly Dictionary<ISymbol, int> locals = new(SymbolEqualityComparer.Default);
        private readonly Dictionary<CaptureId, int> captures = [];
        private List<ScalarPathFactNode> nodes = [];

        private int Slot(ISymbol symbol)
        {
            if (symbol is IParameterSymbol parameter)
                return parameter.Ordinal;
            if (!locals.TryGetValue(symbol, out var id))
                locals.Add(symbol, id = method.Parameters.Length + locals.Count);
            return id;
        }

        private int Capture(CaptureId capture)
        {
            if (!captures.TryGetValue(capture, out var id))
                captures.Add(capture, id = captures.Count);
            return id;
        }

        public ScalarPathFactBlock Block(BasicBlock block)
        {
            cancellationToken.ThrowIfCancellationRequested();
            nodes = [];
            var statements = block.Operations.Select(Add).ToArray();
            int? branch = block.BranchValue is { } value ? Add(value) : null;
            return new(block.Ordinal, block.Kind.ToString(), block.ConditionKind.ToString(), nodes.ToArray(),
                statements, branch, Edge(block.FallThroughSuccessor), Edge(block.ConditionalSuccessor));
        }

        private ScalarPathFactEdge? Edge(ControlFlowBranch? edge)
        {
            if (edge is null)
                return null;
            return new(edge.Destination?.Ordinal, edge.Semantics.ToString(), edge.FinallyRegions.Length != 0,
                edge.LeavingRegions.SelectMany(region => region.Locals).Select(Slot).Distinct().ToArray(),
                edge.LeavingRegions.SelectMany(region => region.CaptureIds).Select(Capture).Distinct().ToArray());
        }

        private int Add(IOperation operation)
        {
            cancellationToken.ThrowIfCancellationRequested();
            var id = nodes.Count;
            nodes.Add(null!);
            var type = ScalarType(operation.Type);
            int? slot = null;
            int? constant = null;
            string? binaryOperator = null;
            string kind;
            var isChecked = operation switch
            {
                IConversionOperation conversion => conversion.IsChecked,
                IBinaryOperation binary => binary.IsChecked,
                IUnaryOperation unary => unary.IsChecked,
                ICompoundAssignmentOperation assignment => assignment.IsChecked,
                IIncrementOrDecrementOperation step => step.IsChecked,
                _ => false
            };
            var lifted = operation switch
            {
                IBinaryOperation binary => binary.IsLifted,
                IUnaryOperation unary => unary.IsLifted,
                ICompoundAssignmentOperation assignment => assignment.IsLifted,
                IIncrementOrDecrementOperation step => step.IsLifted,
                _ => false
            };
            bool postfix = false, writable = true;
            IOperation[] children = [];
            string? hole = null;

            switch (operation)
            {
                case ILiteralOperation or IDefaultValueOperation:
                    kind = "constant";
                    constant = Constant(operation, out hole);
                    break;
                case IFieldReferenceOperation field when field.Field.IsConst && field.Instance is null:
                    kind = "constant";
                    constant = Constant(operation, out hole);
                    break;
                case IParameterReferenceOperation parameter:
                    kind = "slot";
                    slot = Slot(parameter.Parameter);
                    break;
                case ILocalReferenceOperation local:
                    kind = "slot";
                    slot = Slot(local.Local);
                    writable = local.Local.RefKind == RefKind.None;
                    break;
                case IFlowCaptureReferenceOperation reference:
                    kind = "capture-ref";
                    slot = Capture(reference.Id);
                    break;
                case IFlowCaptureOperation capture:
                    kind = assignmentCaptures.Contains(capture.Id) ? "capture-slot" : "capture";
                    slot = Capture(capture.Id);
                    children = [capture.Value];
                    break;
                case IExpressionStatementOperation expression:
                    kind = "identity";
                    children = [expression.Operation];
                    break;
                case IParenthesizedOperation parenthesized:
                    kind = "identity";
                    children = [parenthesized.Operand];
                    break;
                case IConversionOperation conversion when !conversion.IsChecked && conversion.Conversion.IsIdentity:
                    kind = "identity";
                    children = [conversion.Operand];
                    break;
                case ISimpleAssignmentOperation assignment when !assignment.IsRef:
                    kind = "assign";
                    children = [assignment.Target, assignment.Value];
                    break;
                case ICompoundAssignmentOperation assignment when !assignment.IsChecked && !assignment.IsLifted &&
                    assignment.Target is not IFlowCaptureReferenceOperation && assignment.OperatorMethod is null &&
                    assignment.InConversion.IsIdentity && assignment.OutConversion.IsIdentity &&
                    SupportedBinary(assignment.Target.Type, assignment.Value.Type, assignment.OperatorKind):
                    kind = "compound";
                    binaryOperator = assignment.OperatorKind.ToString();
                    children = [assignment.Target, assignment.Value];
                    break;
                case IIncrementOrDecrementOperation step when !step.IsChecked && !step.IsLifted &&
                    step.Target is not IFlowCaptureReferenceOperation && step.OperatorMethod is null:
                    kind = "step";
                    binaryOperator = step.Kind.ToString();
                    postfix = step.IsPostfix;
                    children = [step.Target];
                    break;
                case IUnaryOperation unary when !unary.IsChecked && !unary.IsLifted && unary.OperatorMethod is null &&
                    SupportedUnary(unary):
                    kind = "unary";
                    binaryOperator = unary.OperatorKind.ToString();
                    children = [unary.Operand];
                    break;
                case IBinaryOperation binary when !binary.IsChecked && !binary.IsLifted && binary.OperatorMethod is null &&
                    SupportedBinary(binary.LeftOperand.Type, binary.RightOperand.Type, binary.OperatorKind):
                    kind = "binary";
                    binaryOperator = binary.OperatorKind.ToString();
                    children = [binary.LeftOperand, binary.RightOperand];
                    break;
                default:
                    kind = "hole";
                    hole = Unsupported(operation);
                    break;
            }

            var childIds = children.Select(Add).ToArray();
            nodes[id] = new(id, kind, type, slot, constant, binaryOperator,
                isChecked, lifted, postfix, writable, childIds, hole, Unsupported(operation));
            return id;
        }


        private static bool SupportedUnary(IUnaryOperation unary) =>
            (unary.Operand.Type?.SpecialType, unary.OperatorKind) switch
            {
                (SpecialType.System_Boolean, UnaryOperatorKind.Not) => true,
                (SpecialType.System_Int32, UnaryOperatorKind.Plus or UnaryOperatorKind.Minus) => true,
                _ => false
            };

        private static bool SupportedBinary(ITypeSymbol? left, ITypeSymbol? right, BinaryOperatorKind kind)
        {
            if (left?.SpecialType != right?.SpecialType)
                return false;
            return (left?.SpecialType, kind) switch
            {
                (SpecialType.System_Int32, BinaryOperatorKind.Add or BinaryOperatorKind.Subtract or
                    BinaryOperatorKind.Equals or BinaryOperatorKind.NotEquals or BinaryOperatorKind.LessThan or
                    BinaryOperatorKind.LessThanOrEqual or BinaryOperatorKind.GreaterThan or
                    BinaryOperatorKind.GreaterThanOrEqual) => true,
                (SpecialType.System_Boolean, BinaryOperatorKind.Equals or BinaryOperatorKind.NotEquals or
                    BinaryOperatorKind.ConditionalAnd or BinaryOperatorKind.And or
                    BinaryOperatorKind.ConditionalOr or BinaryOperatorKind.Or) => true,
                _ => false
            };
        }
        private static int? Constant(IOperation operation, out string? hole)
        {
            hole = null;
            if (operation is IDefaultValueOperation && ScalarType(operation.Type) is ("int" or "bool"))
                return 0;
            if (operation.ConstantValue is { HasValue: true, Value: int number })
                return number;
            if (operation.ConstantValue is { HasValue: true, Value: bool boolean })
                return boolean ? 1 : 0;
            hole = Unsupported(operation);
            return null;
        }

        private static string Unsupported(IOperation operation)
        {
            var detail = operation switch
            {
                IBinaryOperation binary => $" {binary.OperatorKind}{(binary.IsChecked ? " checked" : "")}",
                IConversionOperation conversion => $" from {conversion.Operand.Type?.ToDisplayString() ?? "unknown type"}",
                _ => ""
            };
            return $"Unsupported {operation.Kind}{detail} ({operation.Type?.ToDisplayString() ?? "no scalar type"}) " +
                $"at source offset {operation.Syntax.SpanStart}.";
        }
    }
}
