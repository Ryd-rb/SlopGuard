using System.Numerics;

namespace SlopHawk.Core;

internal sealed record ContractComparison(string Operator, uint Inputs, Func<BigInteger[], BigInteger> Difference)
{
    internal bool Matches(BigInteger value) => Operator switch
    {
        "==" => value.IsZero, "!=" => !value.IsZero, "<" => value.Sign < 0, "<=" => value.Sign <= 0,
        ">" => value.Sign > 0, ">=" => value.Sign >= 0, _ => throw new InvalidOperationException(Operator)
    };

    internal int Boundary => Operator switch { "<" => -1, ">" or "!=" => 1, _ => 0 };
}

internal static class ContractConditionCases
{
    internal static IEnumerable<long[]> Generate(ContractComparison[][] groups, ProbeExpectation[] examples,
        Func<BigInteger[], BigInteger>[] locals)
    {
        foreach (var example in examples.Take(4))
        {
            var baseline = example.Args;
            var frame = new BigInteger[baseline.Length + locals.Length];
            foreach (var group in groups)
            {
                for (var direction = 0; direction < 2; direction++)
                {
                    var point = (long[])baseline.Clone();
                    for (var field = 0; field < point.Length; field++)
                        frame[field] = point[field];
                    if (!Project(group, point, frame, locals, reverse: direction != 0) || point.SequenceEqual(baseline))
                        continue;

                    yield return point;
                    for (var field = 0; field < point.Length; field++)
                    {
                        if (point[field] == baseline[field])
                            continue;
                        foreach (var offset in new[] { -1, 1 })
                        {
                            var value = (Int128)point[field] + offset;
                            if (value < long.MinValue || value > long.MaxValue)
                                continue;
                            var neighbor = (long[])point.Clone();
                            neighbor[field] = (long)value;
                            yield return neighbor;
                        }
                    }
                }
            }
        }
    }

    private static bool Project(ContractComparison[] group, long[] point, BigInteger[] frame,
        Func<BigInteger[], BigInteger>[] locals, bool reverse)
    {
        try
        {
            for (var index = 0; index < group.Length; index++)
            {
                var comparison = group[index];
                var current = Evaluate(comparison);
                if (comparison.Matches(current))
                    continue;

                var adjusted = false;
                for (var step = 0; step < point.Length && !adjusted; step++)
                {
                    var field = reverse ? point.Length - 1 - step : step;
                    if ((comparison.Inputs & (1u << field)) == 0)
                        continue;
                    adjusted = Adjust(comparison, current, field, group.AsSpan(0, index));
                }
                if (!adjusted)
                    return false;
            }
            return true;
        }
        catch (ArithmeticException)
        {
            return false;
        }

        BigInteger Evaluate(ContractComparison comparison)
        {
            for (var index = 0; index < locals.Length; index++)
                frame[point.Length + index] = locals[index](frame);
            return comparison.Difference(frame);
        }

        bool Adjust(ContractComparison comparison, BigInteger current, int field, ReadOnlySpan<ContractComparison> previous)
        {
            var origin = point[field];
            // Use the local slope to propose a crossing and evaluate the actual expression at that point.
            var step = origin == long.MaxValue ? -1 : 1;
            frame[field] = origin + step;
            var slope = (Evaluate(comparison) - current) * step;
            if (!slope.IsZero)
            {
                var center = origin + (comparison.Boundary - current) / slope;
                foreach (var offset in new[] { 0, -1, 1 })
                {
                    var value = center + offset;
                    if (value < long.MinValue || value > long.MaxValue || value == origin)
                        continue;
                    frame[field] = value;
                    if (!comparison.Matches(Evaluate(comparison)))
                        continue;
                    var keepsPrevious = true;
                    foreach (var earlier in previous)
                    {
                        if (earlier.Matches(Evaluate(earlier)))
                            continue;
                        keepsPrevious = false;
                        break;
                    }
                    if (!keepsPrevious)
                        continue;
                    point[field] = (long)value;
                    return true;
                }
            }
            frame[field] = origin;
            return false;
        }
    }
}
