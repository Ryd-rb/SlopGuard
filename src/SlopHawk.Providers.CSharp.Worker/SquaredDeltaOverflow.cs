using Microsoft.CodeAnalysis;
using Microsoft.CodeAnalysis.CSharp;
using Microsoft.CodeAnalysis.CSharp.Syntax;

namespace SlopHawk.Providers.CSharp.Worker;

internal sealed record SourceAuditFinding(string Rule, int Line, int Column, string Message);

internal static class SquaredDeltaOverflow
{
    private const string Rule = "csharp.squared-delta-overflow";
    private static readonly MetadataReference CoreLibrary =
        MetadataReference.CreateFromFile(typeof(object).Assembly.Location);
    private static readonly SyntaxTree SystemImports = CSharpSyntaxTree.ParseText("global using System;",
        new CSharpParseOptions(LanguageVersion.CSharp14));

    internal static SourceAuditFinding[] Find(SyntaxTree tree)
    {
        var model = new Lazy<SemanticModel>(() => CSharpCompilation.Create("SourceAudit",
            [tree, SystemImports], [CoreLibrary]).GetSemanticModel(tree));
        var findings = new List<SourceAuditFinding>();
        foreach (var block in tree.GetRoot().DescendantNodes().OfType<BlockSyntax>())
        {
            for (int index = 2; index < block.Statements.Count; index++)
            {
                if (!TryLongInitializer(block.Statements[index], out _, out var value) ||
                    value is not BinaryExpressionSyntax sum || !sum.IsKind(SyntaxKind.AddExpression) ||
                    SquaredLocal(sum.Left) is not { } left || SquaredLocal(sum.Right) is not { } right ||
                    left.Identifier.ValueText == right.Identifier.ValueText)
                    continue;
                int before = index - 1;
                var guard = block.Statements[before] as IfStatementSyntax;
                if (guard is not null)
                    before--;
                if (before < 1 ||
                    !TryDelta(block.Statements[before - 1], model, out var first) ||
                    !TryDelta(block.Statements[before], model, out var second))
                    continue;
                var types = model.Value;
                var leftSymbol = types.GetSymbolInfo(left).Symbol;
                var rightSymbol = types.GetSymbolInfo(right).Symbol;
                if (leftSymbol is null || rightSymbol is null ||
                    !(Same(leftSymbol, first) && Same(rightSymbol, second) ||
                      Same(leftSymbol, second) && Same(rightSymbol, first)))
                    continue;
                if (guard is not null && RejectsOutsideBothAxes(guard, leftSymbol, rightSymbol, types))
                    continue;

                var position = sum.GetLocation().GetLineSpan().StartLinePosition;
                findings.Add(new(Rule, position.Line + 1, position.Character + 1,
                    "The sum of squares can overflow signed 64-bit for full-range Int32 coordinate differences."));
            }
        }
        return findings.ToArray();
    }

    private static bool TryLongInitializer(StatementSyntax statement, out VariableDeclaratorSyntax? variable,
        out ExpressionSyntax? value)
    {
        variable = null;
        value = null;
        if (statement is not LocalDeclarationStatementSyntax
            {
                Declaration.Type: PredefinedTypeSyntax { Keyword.RawKind: (int)SyntaxKind.LongKeyword },
                Declaration.Variables: [var local]
            })
            return false;
        variable = local;
        value = local.Initializer?.Value;
        return value is not null;
    }

    private static IdentifierNameSyntax? SquaredLocal(ExpressionSyntax expression) =>
        expression is BinaryExpressionSyntax { Left: IdentifierNameSyntax left, Right: IdentifierNameSyntax right } product
        && product.IsKind(SyntaxKind.MultiplyExpression) && left.Identifier.ValueText == right.Identifier.ValueText
            ? left
            : null;

    private static bool TryDelta(StatementSyntax statement, Lazy<SemanticModel> model, out ISymbol? symbol)
    {
        symbol = null;
        if (!TryLongInitializer(statement, out var variable, out var value) ||
            value is not BinaryExpressionSyntax
            {
                Left: CastExpressionSyntax
                {
                    Type: PredefinedTypeSyntax { Keyword.RawKind: (int)SyntaxKind.LongKeyword },
                    Expression: IdentifierNameSyntax from
                },
                Right: IdentifierNameSyntax to
            } delta || !delta.IsKind(SyntaxKind.SubtractExpression) ||
            from.Identifier.ValueText == to.Identifier.ValueText)
            return false;

        var types = model.Value;
        if (types.GetTypeInfo(from).Type?.SpecialType != SpecialType.System_Int32 ||
            types.GetTypeInfo(to).Type?.SpecialType != SpecialType.System_Int32)
            return false;
        symbol = types.GetDeclaredSymbol(variable!);
        return symbol is not null;
    }

    private static bool RejectsOutsideBothAxes(IfStatementSyntax guard, ISymbol left, ISymbol right,
        SemanticModel model)
    {
        if (guard.Else is not null ||
            (guard.Statement is BlockSyntax body ? body.Statements.LastOrDefault() : guard.Statement)
                is not (ReturnStatementSyntax or ThrowStatementSyntax) ||
            guard.Statement.DescendantNodesAndSelf().Any(node => node is GotoStatementSyntax) ||
            guard.Condition is not BinaryExpressionSyntax condition ||
            !condition.IsKind(SyntaxKind.LogicalOrExpression))
            return false;

        var first = BoundedAxis(condition.Left, model);
        var second = BoundedAxis(condition.Right, model);
        return Same(first, left) && Same(second, right) || Same(first, right) && Same(second, left);
    }

    private static ISymbol? BoundedAxis(ExpressionSyntax condition, SemanticModel model)
    {
        if (condition is not BinaryExpressionSyntax
            {
                Left: InvocationExpressionSyntax { ArgumentList.Arguments: [{ Expression: IdentifierNameSyntax axis }] } call
            } comparison || !comparison.IsKind(SyntaxKind.GreaterThanExpression) ||
            model.GetConstantValue(comparison.Right) is not { HasValue: true, Value: int limit } || limit < 0 ||
            model.GetSymbolInfo(call).Symbol is not IMethodSymbol
            {
                Name: "Abs",
                ContainingType.Name: "Math",
                ContainingNamespace: { Name: "System", ContainingNamespace.IsGlobalNamespace: true },
                ReturnType.SpecialType: SpecialType.System_Int64
            } method || !Same(method.ContainingAssembly, model.Compilation.GetAssemblyOrModuleSymbol(CoreLibrary)))
            return null;
        return model.GetSymbolInfo(axis).Symbol;
    }

    private static bool Same(ISymbol? left, ISymbol? right) =>
        left is not null && right is not null && SymbolEqualityComparer.Default.Equals(left, right);
}
