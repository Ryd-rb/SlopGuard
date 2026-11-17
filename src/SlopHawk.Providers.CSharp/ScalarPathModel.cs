using Microsoft.CodeAnalysis;
using Microsoft.CodeAnalysis.FlowAnalysis;
using Microsoft.CodeAnalysis.Operations;

namespace SlopHawk.Providers.CSharp;

public static partial class ScalarPaths
{
    private sealed record Comparison(BinaryOperatorKind Operator, Scalar Left, Scalar Right);

    private readonly record struct Scalar(int Value, SpecialType Type,
        IReadOnlyDictionary<int, int>? Coefficients = null, IReadOnlyList<Comparison>? Comparisons = null)
    {
        public bool Boolean => Value != 0;
    }

    private sealed record ReachedGuard(string Taken, string Alternate, IReadOnlyList<Comparison> Comparisons);

    private sealed record Simulation(IReadOnlyList<ReachedGuard> Guards, string? Unknown);

    private sealed class ScalarInterpreter(ControlFlowGraph flow, IMethodSymbol method, long[] arguments,
        IReadOnlySet<CaptureId> assignmentCaptures, CancellationToken cancellationToken)
    {
        private readonly Dictionary<ISymbol, Scalar> variables = new(SymbolEqualityComparer.Default);
        private readonly Dictionary<CaptureId, Scalar> captures = [];
        private readonly Dictionary<CaptureId, ISymbol> slots = [];
        private readonly List<ReachedGuard> guards = [];

        public Simulation Run()
        {
            foreach (var parameter in method.Parameters)
            {
                var value = (int)arguments[parameter.Ordinal];
                var coefficients = new Dictionary<int, int> { [parameter.Ordinal] = 1 };
                variables[parameter] = parameter.Type.SpecialType == SpecialType.System_Int32
                    ? new(value, SpecialType.System_Int32, coefficients)
                    : new(value, SpecialType.System_Boolean, Comparisons:
                        [new(BinaryOperatorKind.NotEquals, new(value, SpecialType.System_Int32, coefficients),
                            new(0, SpecialType.System_Int32))]);
            }

            var block = flow.Blocks[0];
            var visited = new HashSet<int>();
            try
            {
                while (block.Kind != BasicBlockKind.Exit)
                {
                    cancellationToken.ThrowIfCancellationRequested();
                    if (!visited.Add(block.Ordinal))
                        return new(guards, $"Loop frontier at block {block.Ordinal}.");

                    foreach (var statement in block.Operations)
                        Evaluate(statement);

                    var branchValue = block.BranchValue is { } operation ? Evaluate(operation) : (Scalar?)null;
                    var takeConditional = block.ConditionKind switch
                    {
                        ControlFlowConditionKind.None => false,
                        ControlFlowConditionKind.WhenTrue when branchValue?.Type == SpecialType.System_Boolean =>
                            branchValue.Value.Boolean,
                        ControlFlowConditionKind.WhenFalse when branchValue?.Type == SpecialType.System_Boolean =>
                            !branchValue.Value.Boolean,
                        _ => throw new NotModeledException($"Unsupported branch condition at block {block.Ordinal}.")
                    };
                    var edge = takeConditional ? block.ConditionalSuccessor : block.FallThroughSuccessor;
                    if (edge is null || edge.FinallyRegions.Length != 0 || edge.Semantics is not
                        (ControlFlowBranchSemantics.Regular or ControlFlowBranchSemantics.Return) ||
                        edge.Destination is null)
                        return new(guards, $"Unsupported transition on {EdgeId(block, takeConditional)} " +
                            $"({edge?.Semantics.ToString() ?? "missing"}).");

                    if (block.ConditionKind != ControlFlowConditionKind.None)
                        guards.Add(new(EdgeId(block, takeConditional), EdgeId(block, !takeConditional),
                            branchValue?.Comparisons ?? []));

                    foreach (var region in edge.LeavingRegions)
                    {
                        foreach (var local in region.Locals)
                            variables.Remove(local);
                        foreach (var id in region.CaptureIds)
                        {
                            captures.Remove(id);
                            slots.Remove(id);
                        }
                    }

                    block = edge.Destination;
                }

                return new(guards, null);
            }
            catch (NotModeledException exception)
            {
                return new(guards, exception.Message);
            }
        }

        private Scalar Evaluate(IOperation operation)
        {
            cancellationToken.ThrowIfCancellationRequested();
            switch (operation)
            {
                case ILiteralOperation literal:
                    return Constant(literal);
                case IDefaultValueOperation value when IsScalar(value.Type):
                    return new(0, value.Type!.SpecialType);
                case IFieldReferenceOperation field when field.Field.IsConst && field.Instance is null:
                    return Constant(field);
                case IParameterReferenceOperation parameter when variables.TryGetValue(parameter.Parameter, out var value):
                    return value;
                case ILocalReferenceOperation local when variables.TryGetValue(local.Local, out var value):
                    return value;
                case IFlowCaptureReferenceOperation capture when captures.TryGetValue(capture.Id, out var value):
                    return value;
                case IFlowCaptureReferenceOperation capture when slots.TryGetValue(capture.Id, out var slot) &&
                    variables.TryGetValue(slot, out var value):
                    return value;
                case IFlowCaptureOperation capture when assignmentCaptures.Contains(capture.Id):
                    slots[capture.Id] = Slot(capture.Value);
                    return default;
                case IFlowCaptureOperation capture:
                    return captures[capture.Id] = Evaluate(capture.Value);
                case IExpressionStatementOperation expression:
                    return Evaluate(expression.Operation);
                case IParenthesizedOperation parenthesized:
                    return Evaluate(parenthesized.Operand);
                case IConversionOperation conversion when !conversion.IsChecked && conversion.Conversion.IsIdentity:
                    return Evaluate(conversion.Operand);
                case ISimpleAssignmentOperation assignment when !assignment.IsRef:
                    return Assign(assignment.Target, Evaluate(assignment.Value));
                case ICompoundAssignmentOperation assignment when !assignment.IsChecked && !assignment.IsLifted &&
                    assignment.Target is not IFlowCaptureReferenceOperation && assignment.OperatorMethod is null &&
                    assignment.InConversion.IsIdentity && assignment.OutConversion.IsIdentity:
                    return Assign(assignment.Target, Binary(assignment, assignment.OperatorKind,
                        Evaluate(assignment.Target), Evaluate(assignment.Value)));
                case IIncrementOrDecrementOperation step when !step.IsChecked && !step.IsLifted &&
                    step.Target is not IFlowCaptureReferenceOperation && step.OperatorMethod is null:
                    var current = Evaluate(step.Target);
                    var change = step.Kind == OperationKind.Increment ? BinaryOperatorKind.Add : BinaryOperatorKind.Subtract;
                    var updated = Assign(step.Target, Binary(step, change, current, new(1, SpecialType.System_Int32)));
                    return step.IsPostfix ? current : updated;
                case IUnaryOperation unary when !unary.IsChecked && !unary.IsLifted && unary.OperatorMethod is null:
                    return Unary(unary, Evaluate(unary.Operand));
                case IBinaryOperation binary when !binary.IsChecked && !binary.IsLifted && binary.OperatorMethod is null:
                    var left = Evaluate(binary.LeftOperand);
                    if (left.Type == SpecialType.System_Boolean &&
                        ((binary.OperatorKind == BinaryOperatorKind.ConditionalAnd && !left.Boolean) ||
                        (binary.OperatorKind == BinaryOperatorKind.ConditionalOr && left.Boolean)))
                        return left;
                    return Binary(binary, binary.OperatorKind, left, Evaluate(binary.RightOperand));
                default:
                    throw Unsupported(operation);
            }
        }

        private Scalar Assign(IOperation target, Scalar value)
        {
            if (target.Type?.SpecialType != value.Type)
                throw Unsupported(target);

            return variables[Slot(target)] = value;
        }

        private ISymbol Slot(IOperation operation) => operation switch
        {
            ILocalReferenceOperation { Local.RefKind: RefKind.None } local => local.Local,
            IParameterReferenceOperation parameter when variables.ContainsKey(parameter.Parameter) => parameter.Parameter,
            IFlowCaptureReferenceOperation reference when slots.TryGetValue(reference.Id, out var slot) => slot,
            _ => throw Unsupported(operation)
        };

        private static Scalar Constant(IOperation operation) => operation.ConstantValue switch
        {
            { HasValue: true, Value: int number } => new(number, SpecialType.System_Int32),
            { HasValue: true, Value: bool boolean } => new(boolean ? 1 : 0, SpecialType.System_Boolean),
            _ => throw Unsupported(operation)
        };

        private static Scalar Unary(IUnaryOperation operation, Scalar operand) => operation.OperatorKind switch
        {
            UnaryOperatorKind.Not when operand.Type == SpecialType.System_Boolean =>
                operand with { Value = operand.Boolean ? 0 : 1 },
            UnaryOperatorKind.Plus when operand.Type == SpecialType.System_Int32 => operand,
            UnaryOperatorKind.Minus when operand.Type == SpecialType.System_Int32 =>
                new(unchecked(-operand.Value), SpecialType.System_Int32, Combine(null, operand.Coefficients, subtract: true)),
            _ => throw Unsupported(operation)
        };

        private static Scalar Binary(IOperation operation, BinaryOperatorKind kind, Scalar left, Scalar right)
        {
            if (left.Type != right.Type)
                throw Unsupported(operation);

            if (left.Type == SpecialType.System_Int32)
            {
                if (kind is BinaryOperatorKind.Add or BinaryOperatorKind.Subtract)
                {
                    var subtract = kind == BinaryOperatorKind.Subtract;
                    return new(subtract ? unchecked(left.Value - right.Value) : unchecked(left.Value + right.Value),
                        SpecialType.System_Int32, Combine(left.Coefficients, right.Coefficients, subtract));
                }

                if (kind is BinaryOperatorKind.Equals or BinaryOperatorKind.NotEquals or BinaryOperatorKind.LessThan or
                    BinaryOperatorKind.LessThanOrEqual or BinaryOperatorKind.GreaterThan or BinaryOperatorKind.GreaterThanOrEqual)
                    return new(Compare(kind, left.Value, right.Value) ? 1 : 0, SpecialType.System_Boolean,
                        Comparisons: [new(kind, left, right)]);
            }

            if (left.Type == SpecialType.System_Boolean)
            {
                var result = kind switch
                {
                    BinaryOperatorKind.Equals => left.Boolean == right.Boolean,
                    BinaryOperatorKind.NotEquals => left.Boolean != right.Boolean,
                    BinaryOperatorKind.ConditionalAnd or BinaryOperatorKind.And => left.Boolean && right.Boolean,
                    BinaryOperatorKind.ConditionalOr or BinaryOperatorKind.Or => left.Boolean || right.Boolean,
                    _ => throw Unsupported(operation)
                };
                return new(result ? 1 : 0, SpecialType.System_Boolean,
                    Comparisons: (left.Comparisons ?? []).Concat(right.Comparisons ?? []).Distinct().ToArray());
            }

            throw Unsupported(operation);
        }

        private static NotModeledException Unsupported(IOperation operation)
        {
            var detail = operation switch
            {
                IBinaryOperation binary => $" {binary.OperatorKind}{(binary.IsChecked ? " checked" : "")}",
                IConversionOperation conversion => $" from {conversion.Operand.Type?.ToDisplayString() ?? "unknown type"}",
                _ => ""
            };
            return new($"Unsupported {operation.Kind}{detail} ({operation.Type?.ToDisplayString() ?? "no scalar type"}) " +
                $"at source offset {operation.Syntax.SpanStart}.");
        }
    }

    private static HashSet<CaptureId> AssignmentCaptures(ControlFlowGraph flow, CancellationToken cancellationToken)
    {
        var captures = new HashSet<CaptureId>();
        foreach (var block in flow.Blocks)
        {
            cancellationToken.ThrowIfCancellationRequested();
            foreach (var operation in block.Operations)
                Add(operation);
            if (block.BranchValue is { } branch)
                Add(branch);
        }

        return captures;

        void Add(IOperation root)
        {
            foreach (var operation in root.DescendantsAndSelf())
            {
                cancellationToken.ThrowIfCancellationRequested();
                var target = operation switch
                {
                    IAssignmentOperation assignment => assignment.Target,
                    IIncrementOrDecrementOperation step => step.Target,
                    _ => null
                };
                if (target is IFlowCaptureReferenceOperation capture)
                    captures.Add(capture.Id);
            }
        }
    }

    private static bool Compare(BinaryOperatorKind kind, int left, int right) => kind switch
    {
        BinaryOperatorKind.Equals => left == right,
        BinaryOperatorKind.NotEquals => left != right,
        BinaryOperatorKind.LessThan => left < right,
        BinaryOperatorKind.LessThanOrEqual => left <= right,
        BinaryOperatorKind.GreaterThan => left > right,
        BinaryOperatorKind.GreaterThanOrEqual => left >= right,
        _ => throw new ArgumentOutOfRangeException(nameof(kind))
    };

    private static IReadOnlyDictionary<int, int>? Combine(IReadOnlyDictionary<int, int>? left,
        IReadOnlyDictionary<int, int>? right, bool subtract)
    {
        if (right is null)
            return left;
        if (left is null && !subtract)
            return right;

        var result = left is null ? [] : new Dictionary<int, int>(left);
        foreach (var (index, factor) in right)
        {
            var combined = unchecked(result.GetValueOrDefault(index) + (subtract ? -factor : factor));
            if (combined == 0)
                result.Remove(index);
            else
                result[index] = combined;
        }

        return result;
    }

    private static bool IsScalar(ITypeSymbol? type) => type?.SpecialType is
        SpecialType.System_Int32 or SpecialType.System_Boolean;

    private sealed class NotModeledException(string message) : Exception(message);
}
