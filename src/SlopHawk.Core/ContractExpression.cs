using System.Globalization;
using System.Numerics;

namespace SlopHawk.Core;

internal sealed class ContractExpression(string text, IReadOnlyDictionary<string, int> names, IReadOnlyList<uint> inputs,
    List<ContractComparison[]> groups)
{
    private static readonly IReadOnlyDictionary<string, int> Precedence = new Dictionary<string, int>(StringComparer.Ordinal)
    {
        ["||"] = 1, ["&&"] = 2, ["|"] = 3, ["^"] = 4, ["&"] = 5, ["=="] = 6, ["!="] = 6,
        ["<"] = 7, ["<="] = 7, [">"] = 7, [">="] = 7, ["<<"] = 8, [">>"] = 8, ["+"] = 9, ["-"] = 9,
        ["*"] = 10, ["/"] = 10, ["%"] = 10
    };

    private int position;
    private int depth;

    internal readonly record struct Expression(Func<BigInteger[], BigInteger> Evaluate, uint Inputs,
        ContractComparison[]? Conditions = null);

    internal Expression Compile()
    {
        if (string.IsNullOrWhiteSpace(text) || text.Length > 4096 || text.Contains('\0'))
            throw new ArgumentException("Provide a contract expression of at most 4096 characters.");
        var result = Parse(0);
        if (Peek() != '\0')
            throw Error("Unexpected token");
        return result;
    }

    private Expression Parse(int minimum)
    {
        if (++depth > 64)
            throw Error("Expression nesting is too deep");
        var left = Operand();
        while (TryOperator(out var op, out var priority) && priority >= minimum)
        {
            position += op.Length;
            var first = left;
            var second = Parse(priority + 1);
            var operation = op;
            var dependencies = first.Inputs | second.Inputs;
            left = operation switch
            {
                "&&" => new(values => Truth(!first.Evaluate(values).IsZero && !second.Evaluate(values).IsZero), dependencies,
                    first.Conditions is { } a && second.Conditions is { } b && a.Length + b.Length <= 32 ? [.. a, .. b] : null),
                "||" => new(values => Truth(!first.Evaluate(values).IsZero || !second.Evaluate(values).IsZero), dependencies),
                "==" or "!=" or "<" or "<=" or ">" or ">=" => new(values =>
                    Binary(operation, first.Evaluate(values), second.Evaluate(values)), dependencies,
                    [new(operation, dependencies, values => first.Evaluate(values) - second.Evaluate(values))]),
                _ => new(values => Binary(operation, first.Evaluate(values), second.Evaluate(values)), dependencies)
            };
            if (left.Conditions is { } conditions)
                groups.Add(conditions);
        }
        if (minimum == 0 && Take('?'))
        {
            var condition = left;
            var yes = Parse(0);
            Require(':');
            var no = Parse(0);
            left = new(values => condition.Evaluate(values).IsZero ? no.Evaluate(values) : yes.Evaluate(values),
                condition.Inputs | yes.Inputs | no.Inputs);
        }
        depth--;
        return left;
    }

    private Expression Operand()
    {
        var next = Peek();
        if (next is '-' or '!' or '~' or '+')
        {
            position++;
            var value = Parse(11);
            return next switch
            {
                '-' => new(values => -value.Evaluate(values), value.Inputs),
                '!' => new(values => Truth(value.Evaluate(values).IsZero), value.Inputs),
                '~' => new(values => ~value.Evaluate(values), value.Inputs),
                _ => value
            };
        }
        if (Take('('))
        {
            var inner = Parse(0);
            Require(')');
            return inner;
        }
        var start = position;
        if (char.IsAsciiDigit(next))
        {
            while (position < text.Length && char.IsAsciiDigit(text[position]))
                position++;
            if (position - start > 39)
                throw Error("Integer constant is too large");
            var constant = BigInteger.Parse(text.AsSpan(start, position - start), NumberStyles.None, CultureInfo.InvariantCulture);
            return new(_ => constant, 0);
        }
        if (!IsNameStart(next))
            throw Error("Expected a constant, name, or parenthesized expression");
        position++;
        while (position < text.Length && IsNamePart(text[position]))
            position++;
        var name = text[start..position];
        if (Take('('))
            return Function(name);
        if (name is "true" or "false")
            return new(_ => Truth(name == "true"), 0);
        if (!names.TryGetValue(name, out var index))
            throw Error($"Unknown name '{name}'");
        return new(values => values[index], inputs[index]);
    }

    private Expression Function(string name)
    {
        var first = Parse(0);
        if (name is "abs" or "popcount")
        {
            Require(')');
            return name == "abs" ? new(values => BigInteger.Abs(first.Evaluate(values)), first.Inputs)
                : new(values => BitOperations.PopCount((ulong)first.Evaluate(values)), first.Inputs);
        }
        Require(',');
        var second = Parse(0);
        if (name == "clamp")
        {
            Require(',');
            var third = Parse(0);
            Require(')');
            return new(values => BigInteger.Min(BigInteger.Max(first.Evaluate(values), second.Evaluate(values)),
                third.Evaluate(values)), first.Inputs | second.Inputs | third.Inputs);
        }
        Require(')');
        return name switch
        {
            "min" => new(values => BigInteger.Min(first.Evaluate(values), second.Evaluate(values)), first.Inputs | second.Inputs),
            "max" => new(values => BigInteger.Max(first.Evaluate(values), second.Evaluate(values)), first.Inputs | second.Inputs),
            "mod" => new(values => Modulo(first.Evaluate(values), second.Evaluate(values)), first.Inputs | second.Inputs),
            _ => throw Error($"Unknown function '{name}'")
        };
    }

    private bool TryOperator(out string op, out int priority)
    {
        var next = Peek();
        if (next == '\0')
        {
            op = "";
            priority = 0;
            return false;
        }
        if (position + 1 < text.Length && Precedence.TryGetValue(text.Substring(position, 2), out priority))
        {
            op = text.Substring(position, 2);
            return true;
        }
        op = next.ToString();
        return Precedence.TryGetValue(op, out priority);
    }

    private char Peek()
    {
        while (position < text.Length && char.IsWhiteSpace(text[position]))
            position++;
        return position < text.Length ? text[position] : '\0';
    }

    private bool Take(char token)
    {
        if (Peek() != token)
            return false;
        position++;
        return true;
    }

    private void Require(char token)
    {
        if (!Take(token))
            throw Error($"Expected '{token}'");
    }

    internal static bool IsName(string? value) => !string.IsNullOrEmpty(value) && IsNameStart(value[0]) &&
        value.All(IsNamePart) && value is not ("true" or "false");

    private static bool IsNameStart(char value) => char.IsAsciiLetter(value) || value == '_';
    private static bool IsNamePart(char value) => IsNameStart(value) || char.IsAsciiDigit(value);
    private ArgumentException Error(string message) => new($"{message} at column {position + 1}: {text}");
    private static BigInteger Truth(bool value) => value ? BigInteger.One : BigInteger.Zero;

    private static BigInteger Binary(string op, BigInteger left, BigInteger right)
    {
        var result = op switch
        {
            "+" => left + right, "-" => left - right, "*" => left * right, "/" => left / right, "%" => left % right,
            "<<" => left << Shift(right), ">>" => left >> Shift(right),
            "&" => left & right, "|" => left | right, "^" => left ^ right,
            "==" => Truth(left == right), "!=" => Truth(left != right),
            "<" => Truth(left < right), "<=" => Truth(left <= right), ">" => Truth(left > right), ">=" => Truth(left >= right),
            _ => throw new ArgumentException($"Unknown operator '{op}'.")
        };
        if (result.GetBitLength() > 1024)
            throw new OverflowException("A contract intermediate exceeds the expression budget.");
        return result;
    }

    private static int Shift(BigInteger count) => count >= 0 && count <= 127 ? (int)count
        : throw new OverflowException("A contract shift must be between 0 and 127.");

    private static BigInteger Modulo(BigInteger value, BigInteger divisor)
    {
        if (divisor.Sign <= 0)
            throw new DivideByZeroException("Mathematical modulo requires a positive period.");
        var remainder = value % divisor;
        return remainder.Sign < 0 ? remainder + divisor : remainder;
    }
}
