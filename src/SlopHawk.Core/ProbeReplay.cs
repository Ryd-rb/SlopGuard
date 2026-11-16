using System.Text.Json;
using System.Text.Json.Serialization;

namespace SlopHawk.Core;

public sealed record ProbeInput(string Operation, long[] Args);

public sealed record ProbeExpectation([property: JsonRequired] string Operation, [property: JsonRequired] long[] Args,
    [property: JsonRequired] JsonElement Expected);

public sealed record ProbeDifference(int Index, ProbeInput Input, JsonElement Candidate, JsonElement Reference);

public sealed record ProbeOperationResult(
    string Operation, int Compared, int Unsupported, int OneSided, int Different, ProbeDifference? FirstDifference);

public sealed record ProbeCheckResult(int Cases, int Compared, int Unsupported, IReadOnlyList<ProbeDifference> Findings);

public static class ProbeReplay
{
    public static ProbeInput[] InputsForCheck(IReadOnlyList<ProbeExpectation> cases)
    {
        if (cases.Count == 0)
            throw new ArgumentException("Provide at least one check case.", nameof(cases));
        return cases.Select(item => item is not null && !string.IsNullOrWhiteSpace(item.Operation) && item.Args is not null &&
            item.Expected.ValueKind is not (JsonValueKind.Null or JsonValueKind.Undefined)
                ? new ProbeInput(item.Operation, item.Args)
                : throw new ArgumentException("Each check needs an operation, arguments and a non-null expected result.", nameof(cases))).ToArray();
    }

    public static ProbeCheckResult Check(IReadOnlyList<ProbeExpectation> cases, IReadOnlyList<JsonElement> actual)
    {
        if (cases.Count != actual.Count)
            throw new ArgumentException("The adapter must return exactly one result per check, in input order.", nameof(actual));
        var inputs = InputsForCheck(cases);
        var findings = new List<ProbeDifference>();
        var unsupported = 0;
        for (var i = 0; i < inputs.Length; i++)
        {
            if (actual[i].ValueKind is JsonValueKind.Null or JsonValueKind.Undefined)
            {
                unsupported++;
                continue;
            }
            if (!JsonElement.DeepEquals(actual[i], cases[i].Expected))
                findings.Add(new(i, inputs[i], actual[i].Clone(), cases[i].Expected.Clone()));
        }
        return new(cases.Count, cases.Count - unsupported, unsupported, findings);
    }

    public static IEnumerable<ProbeInput> Plan(IEnumerable<ProbeInput> seeds, int limit, int randomSeed = 0,
        IReadOnlyList<long>? scales = null)
    {
        ArgumentOutOfRangeException.ThrowIfNegativeOrZero(limit);
        var units = scales?.Distinct().ToArray() ?? [];
        if (units.Any(unit => unit <= 1))
            throw new ArgumentException("Probe scales must be integers greater than one.", nameof(scales));
        foreach (var group in seeds.GroupBy(seed => seed?.Operation, StringComparer.Ordinal))
        {
            var examples = group.Select(seed => seed?.Args).ToArray();
            if (string.IsNullOrWhiteSpace(group.Key) || examples.Any(args => args is null))
                throw new ArgumentException("Each probe needs an operation name and an argument array.", nameof(seeds));

            var seen = new HashSet<string>(StringComparer.Ordinal);
            var shapes = examples.Select(args => args!).GroupBy(args => args.Length)
                .Select(shape => Candidates(shape.ToArray(), randomSeed, limit, units));
            foreach (var args in Alternate(shapes))
            {
                if (!seen.Add(string.Join(',', args)))
                    continue;
                yield return new ProbeInput(group.Key, args);
                if (seen.Count == limit)
                    break;
            }
        }
    }

    private static IEnumerable<long[]> Alternate(IEnumerable<IEnumerable<long[]>> shapes)
    {
        var pending = new Queue<IEnumerator<long[]>>(shapes.Select(shape => shape.GetEnumerator()));
        try
        {
            while (pending.TryPeek(out var source))
            {
                if (source.MoveNext())
                {
                    pending.Enqueue(pending.Dequeue());
                    yield return source.Current;
                }
                else
                    pending.Dequeue().Dispose();
            }
        }
        finally
        {
            foreach (var source in pending)
                source.Dispose();
        }
    }

    public static IReadOnlyList<ProbeOperationResult> Compare(
        IReadOnlyList<ProbeInput> inputs, IReadOnlyList<JsonElement> candidate, IReadOnlyList<JsonElement> reference)
    {
        if (inputs.Count != candidate.Count || inputs.Count != reference.Count)
            throw new ArgumentException("The two result arrays must have exactly one item per input, in input order.");

        var results = new Dictionary<string, Accumulator>(StringComparer.Ordinal);
        for (var index = 0; index < inputs.Count; index++)
        {
            var input = inputs[index];
            if (input is null || string.IsNullOrWhiteSpace(input.Operation) || input.Args is null)
                throw new ArgumentException("An input is missing its operation or arguments.", nameof(inputs));
            if (!results.TryGetValue(input.Operation, out var state))
            {
                state = new Accumulator();
                results.Add(input.Operation, state);
            }
            var left = candidate[index];
            var right = reference[index];
            var leftMissing = left.ValueKind is JsonValueKind.Null or JsonValueKind.Undefined;
            var rightMissing = right.ValueKind is JsonValueKind.Null or JsonValueKind.Undefined;
            if (leftMissing || rightMissing)
            {
                state.Unsupported++;
                if (leftMissing != rightMissing)
                    state.OneSided++;
                continue;
            }
            state.Compared++;
            if (!JsonElement.DeepEquals(left, right))
            {
                state.Different++;
                state.First ??= new ProbeDifference(index, input, left.Clone(), right.Clone());
            }
        }
        return results.Select(pair => new ProbeOperationResult(pair.Key, pair.Value.Compared, pair.Value.Unsupported,
            pair.Value.OneSided, pair.Value.Different, pair.Value.First)).ToArray();
    }

    private static IEnumerable<long[]> Candidates(long[][] examples, int randomSeed, int limit, long[] scales)
    {
        foreach (var seed in examples)
            yield return seed.ToArray();
        if (examples[0].Length == 0)
            yield break;

        var random = new Random(randomSeed);
        foreach (var seed in examples)
        {
            using var axis = Axes(seed).GetEnumerator();
            using var relation = Relations(seed).GetEnumerator();
            using var scaled = ScaledOffsets(seed, scales).GetEnumerator();
            bool pathsRemain;
            do
            {
                pathsRemain = false;
                if (axis.MoveNext())
                {
                    pathsRemain = true;
                    yield return axis.Current;
                }
                if (relation.MoveNext())
                {
                    pathsRemain = true;
                    yield return relation.Current;
                }
                if (scaled.MoveNext())
                {
                    pathsRemain = true;
                    yield return scaled.Current;
                }
                if (pathsRemain)
                    yield return Scatter(seed, random);
            } while (pathsRemain);
        }
        for (long trial = 0; trial < 8L * limit; trial++)
            yield return Scatter(examples[random.Next(examples.Length)], random);
    }

    // Include compensating changes across two dimensions for a declared scale.
    private static IEnumerable<long[]> ScaledOffsets(long[] seed, long[] scales)
    {
        for (var first = 0; first < seed.Length; first++)
            for (var second = 0; second < seed.Length; second++)
            {
                if (first == second)
                    continue;
                foreach (var scale in scales)
                {
                    var lower = Replace(seed, first, Add(seed[first], -1));
                    lower[second] = Add(seed[second], scale);
                    yield return lower;
                    var upper = Replace(seed, first, Add(seed[first], 1));
                    upper[second] = Add(seed[second], -scale);
                    yield return upper;
                }
            }
    }

    private static IEnumerable<long[]> Axes(long[] seed)
    {
        foreach (var delta in new long[] { -1, 1, -2, 2 })
            for (var field = 0; field < seed.Length; field++)
                yield return Replace(seed, field, Add(seed[field], delta));

        foreach (var value in new long[] { 0, 1, -1, 2, 3, 4, 7, 8, 15, 16 })
            for (var field = 0; field < seed.Length; field++)
                yield return Replace(seed, field, value);

        for (var field = 0; field < seed.Length; field++)
            foreach (var value in seed)
            {
                yield return Replace(seed, field, value);
                yield return Replace(seed, field, Add(value, -1));
                yield return Replace(seed, field, Add(value, 1));
            }
    }

    private static IEnumerable<long[]> Relations(long[] seed)
    {
        foreach (var value in seed.Concat(new long[] { 0, 1, -1, 2, 3 }))
            yield return Enumerable.Repeat(value, seed.Length).ToArray();

        for (var first = 0; first < seed.Length; first++)
            for (var second = first + 1; second < seed.Length; second++)
            {
                var swapped = seed.ToArray();
                (swapped[first], swapped[second]) = (swapped[second], swapped[first]);
                yield return swapped;
                foreach (var tie in new[] { seed[first], seed[second], 0L, 1L })
                    foreach (var delta in new[] { 0L, -1L, 1L })
                    {
                        var point = Replace(seed, first, tie);
                        point[second] = Add(tie, delta);
                        yield return point;
                    }
            }
    }

    private static long[] Scatter(long[] seed, Random random)
    {
        var point = seed.ToArray();
        var shared = random.Next(2) == 0 ? seed[random.Next(seed.Length)] : random.Next(-1, 9);
        var changes = random.Next(5) switch
        {
            0 => 1,
            1 => 2,
            2 => 3,
            3 => seed.Length,
            _ => random.Next(1, seed.Length + 1)
        };
        for (var change = 0; change < changes; change++)
        {
            var index = changes == seed.Length ? change : random.Next(seed.Length);
            point[index] = random.Next(10) switch
            {
                0 => 0,
                1 => 1,
                2 => shared,
                3 => Add(shared, random.Next(-1, 2)),
                4 => Add(seed[index], random.Next(-4, 5)),
                5 => seed[random.Next(seed.Length)],
                6 => random.Next(0, 16),
                7 => seed[index] ^ (1L << random.Next(0, 5)),
                8 => seed[index] / 2,
                _ => Add(seed[index], seed[random.Next(seed.Length)])
            };
        }
        return point;
    }

    private static long[] Replace(long[] seed, int index, long value)
    {
        var point = seed.ToArray();
        point[index] = value;
        return point;
    }

    private static long Add(long value, long delta) => (long)Int128.Clamp((Int128)value + delta, long.MinValue, long.MaxValue);

    private sealed class Accumulator
    {
        internal int Compared;
        internal int Unsupported;
        internal int OneSided;
        internal int Different;
        internal ProbeDifference? First;
    }
}
