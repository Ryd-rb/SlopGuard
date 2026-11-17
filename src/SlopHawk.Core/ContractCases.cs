using System.Globalization;
using System.Numerics;
using System.Text.Json;
using System.Text.RegularExpressions;

namespace SlopHawk.Core;

public sealed record IntegerContract(string Operation, string[] Parameters, string Domain, string Expected, string Source,
    Dictionary<string, string>? Let = null, Dictionary<string, long[]>? Basis = null);

public sealed record ContractQualification(string Operation, string? Source, string? Error);

public sealed class ContractCases
{
    private readonly ContractRule[] rules;
    public IReadOnlyList<ContractQualification> Qualification { get; }
    public IReadOnlyDictionary<string, int> ParameterCounts { get; }

    public ContractCases(IReadOnlyList<IntegerContract> contracts, IReadOnlyList<ProbeExpectation> ordinary)
    {
        ProbeReplay.InputsForCheck(ordinary);
        if (contracts.Any(rule => rule is null || string.IsNullOrWhiteSpace(rule.Operation)) ||
            contracts.Select(rule => rule.Operation).Distinct(StringComparer.Ordinal).Count() != contracts.Count)
            throw new ArgumentException("Contract operation names must be present and unique.");
        var definitions = contracts.ToDictionary(rule => rule.Operation, StringComparer.Ordinal);
        var rulesToRun = new List<ContractRule>();
        var qualifications = new List<ContractQualification>();
        foreach (var group in ordinary.GroupBy(item => item.Operation, StringComparer.Ordinal))
        {
            if (!definitions.TryGetValue(group.Key, out var definition))
            {
                qualifications.Add(new(group.Key, null, "No executable public rule."));
                continue;
            }
            var examples = group.ToArray();
            try
            {
                var rule = new ContractRule(definition, examples);
                foreach (var example in examples)
                {
                    if (!example.Expected.TryGetInt64(out var known) ||
                        !rule.TryExpected(example.Args, out var calculated) || calculated != known)
                        throw new ArgumentException("The rule does not reproduce a published example or its legal domain.");
                }
                qualifications.Add(new(group.Key, definition.Source, null));
                rulesToRun.Add(rule);
            }
            catch (Exception ex) when (ex is ArgumentException or ArithmeticException or InvalidOperationException)
            {
                qualifications.Add(new(group.Key, definition.Source, ex.Message));
            }
        }
        foreach (var definition in contracts.Where(definition => ordinary.All(example => example.Operation != definition.Operation)))
            qualifications.Add(new(definition.Operation, definition.Source, "No published example to qualify the rule."));
        Qualification = qualifications.AsReadOnly();
        rules = [.. rulesToRun];
        ParameterCounts = rules.ToDictionary(rule => rule.Definition.Operation, rule => rule.Definition.Parameters.Length,
            StringComparer.Ordinal).AsReadOnly();
    }

    public IEnumerable<ProbeExpectation> Generate(int perOperation, int seed, ISet<string>? operations = null,
        bool guideWithBoundaries = false, IReadOnlyDictionary<string, long[]>? implementationLiterals = null)
    {
        ArgumentOutOfRangeException.ThrowIfLessThan(perOperation, 1);
        ArgumentOutOfRangeException.ThrowIfGreaterThan(perOperation, 8192);
        foreach (var rule in rules)
        {
            if (operations is not null && !operations.Contains(rule.Definition.Operation))
                continue;
            var emitted = 0;
            var seen = new HashSet<string>(rule.Ordinary.Select(example => Key(example.Args)), StringComparer.Ordinal);
            var random = new Random(seed);
            var anchors = Math.Min(32, rule.Ordinary.Length);
            var legal = rule.Ordinary.Take(anchors).Select(example => example.Args).ToList();
            using var probes = ProbeReplay.Plan(rule.Ordinary.Select(example => new ProbeInput(example.Operation, example.Args)),
                Math.Max(32, perOperation), seed).GetEnumerator();
            using var boundaries = rule.BoundaryPoints().GetEnumerator();
            using var equalities = rule.EqualityPoints().GetEnumerator();
            using var implementation = rule.ImplementationPoints(
                implementationLiterals?.GetValueOrDefault(rule.Definition.Operation) ?? []).GetEnumerator();
            using var combined = rule.CombinedPoints().GetEnumerator();
            for (var attempt = 0; attempt < perOperation * 48 && emitted < perOperation; attempt++)
            {
                var point = attempt % 4 == 1 && implementation.MoveNext() ? implementation.Current
                    : guideWithBoundaries && attempt % 4 == 0 && boundaries.MoveNext() ? boundaries.Current
                    : guideWithBoundaries && attempt % 4 == 2 && equalities.MoveNext() ? equalities.Current
                    : guideWithBoundaries && attempt % 8 == 3 && combined.MoveNext() ? combined.Current
                    : attempt % 3 == 0 && probes.MoveNext() ? probes.Current.Args
                    : rule.Mutate(legal, random, guideWithBoundaries);
                if (!seen.Add(Key(point)) || !rule.TryExpected(point, out var value))
                    continue;
                if (legal.Count < 64)
                    legal.Add(point);
                else
                    legal[random.Next(anchors, legal.Count)] = point;
                emitted++;
                yield return new(rule.Definition.Operation, point, JsonSerializer.SerializeToElement(value));
            }
        }
    }

    public ProbeExpectation? Evaluate(string operation, long[] args)
    {
        var rule = rules.FirstOrDefault(candidate => candidate.Definition.Operation == operation);
        return rule is not null && args.Length == rule.Definition.Parameters.Length &&
            rule.TryExpected(args, out var value)
            ? new(operation, args, JsonSerializer.SerializeToElement(value))
            : null;
    }

    private static string Key(long[] args) => string.Join(',', args);

    private sealed class ContractRule
    {
        private readonly Func<BigInteger[], BigInteger> domain;
        private readonly Func<BigInteger[], BigInteger> expected;
        private readonly Func<BigInteger[], BigInteger>[] locals;
        private readonly long[][] atoms;
        private readonly long[][] landmarks;
        private readonly long[][] guidedAtoms;
        private readonly (int First, int Second)[] equalityPairs;
        private readonly ContractComparison[][] combined;
        public IntegerContract Definition { get; }
        public ProbeExpectation[] Ordinary { get; }

        public ContractRule(IntegerContract definition, ProbeExpectation[] ordinary)
        {
            Definition = definition;
            Ordinary = ordinary;
            var parameters = definition.Parameters;
            if (string.IsNullOrWhiteSpace(definition.Source) || parameters is not { Length: > 0 and <= 32 } ||
                parameters.Any(name => !ContractExpression.IsName(name)) ||
                parameters.Distinct(StringComparer.Ordinal).Count() != parameters.Length ||
                ordinary.Any(example => example.Args.Length != parameters.Length) ||
                definition.Let?.Count > 64 || definition.Basis?.Any(pair =>
                    !parameters.Contains(pair.Key, StringComparer.Ordinal) || pair.Value is not { Length: <= 32 }) == true)
                throw new ArgumentException("Invalid contract source, parameter names, arity, locals, or input basis.");

            var names = parameters.Select((name, index) => (name, index)).ToDictionary(item => item.name, item => item.index,
                StringComparer.Ordinal);
            var inputs = Enumerable.Range(0, parameters.Length).Select(index => 1u << index).ToList();
            var domainGroups = new List<ContractComparison[]>();
            var resultGroups = new List<ContractComparison[]>();
            domain = new ContractExpression(definition.Domain, names, inputs, domainGroups).Compile().Evaluate;
            var compiled = new List<Func<BigInteger[], BigInteger>>();
            foreach (var pair in definition.Let ?? [])
            {
                if (!ContractExpression.IsName(pair.Key) || names.ContainsKey(pair.Key))
                    throw new ArgumentException($"Invalid or repeated local '{pair.Key}'.");
                var binding = new ContractExpression(pair.Value, names, inputs, resultGroups).Compile();
                compiled.Add(binding.Evaluate);
                inputs.Add(binding.Inputs);
                names.Add(pair.Key, names.Count);
            }
            locals = [.. compiled];
            expected = new ContractExpression(definition.Expected, names, inputs, resultGroups).Compile().Evaluate;
            combined = resultGroups.OrderByDescending(group => group.Length)
                .Concat(domainGroups.OrderByDescending(group => group.Length))
                .Where(group => BitOperations.PopCount(group.Aggregate(0u, (mask, relation) => mask | relation.Inputs)) > 1)
                .Take(24).ToArray();
            var expressions = string.Join('\n', new[] { definition.Domain, definition.Expected }
                .Concat(definition.Let?.Values ?? Enumerable.Empty<string>()));
            var bounds = LiteralBoundaries(expressions);
            equalityPairs = EqualityPairs(expressions, parameters);
            landmarks = parameters.Select(name => bounds[name].Concat(definition.Basis?.GetValueOrDefault(name) ?? [])
                .SelectMany(value => new[] { value, Offset(value, -1), Offset(value, 1) }).Distinct().Take(64).ToArray()).ToArray();
            atoms = parameters.Select((name, index) =>
            {
                long[] standard = [-1, 0, 1, 2, 3, 4, 5, 7, 8, 10, 15, 16, 20];
                var authored = definition.Basis?.GetValueOrDefault(name) ?? [];
                var anchors = ordinary.Select(item => item.Args[index]).Concat(authored);
                return anchors.SelectMany(value => new[] { value, Offset(value, -1), Offset(value, 1) })
                    .Concat(standard).Distinct().ToArray();
            }).ToArray();
            guidedAtoms = atoms.Select((values, index) => values.Concat(landmarks[index]).Distinct().ToArray()).ToArray();
        }

        public IEnumerable<long[]> CombinedPoints() => ContractConditionCases.Generate(combined, Ordinary, locals);

        public IEnumerable<long[]> BoundaryPoints()
        {
            foreach (var example in Ordinary)
                for (var field = 0; field < landmarks.Length; field++)
                    foreach (var value in landmarks[field])
                    {
                        var point = example.Args.ToArray();
                        point[field] = value;
                        yield return point;
                    }
        }

        public IEnumerable<long[]> EqualityPoints()
        {
            foreach (var example in Ordinary)
                for (var first = 0; first < equalityPairs.Length; first++)
                    for (var second = first; second < equalityPairs.Length; second++)
                        for (var direction = 0; direction < (first == second ? 2 : 4); direction++)
                        {
                            var point = example.Args.ToArray();
                            Apply(equalityPairs[first], (direction & 1) != 0);
                            if (second != first)
                                Apply(equalityPairs[second], (direction & 2) != 0);
                            yield return point;

                            void Apply((int First, int Second) pair, bool reverse)
                            {
                                if (reverse)
                                    point[pair.First] = point[pair.Second];
                                else
                                    point[pair.Second] = point[pair.First];
                            }
                        }
        }

        public IEnumerable<long[]> ImplementationPoints(IEnumerable<long> literals)
        {
            var scales = literals.Where(value => value is > 1 or < -1 && value != long.MinValue)
                .Select(Math.Abs).Distinct().OrderDescending().Take(16).ToArray();
            foreach (var scale in scales)
                foreach (var example in Ordinary)
                {
                    var baseline = example.Args;
                    for (var field = 0; field < baseline.Length; field++)
                        foreach (var delta in new[] { 0, -1, 1 })
                        {
                            var point = baseline.ToArray();
                            point[field] = Offset(scale, delta);
                            yield return point;
                        }

                    // Probe tuples that can collide in an encoding such as major * scale + minor.
                    for (var stride = 1; stride < baseline.Length; stride++)
                        for (var source = 0; source + stride < baseline.Length; source++)
                            for (var target = 0; target + stride < baseline.Length; target++)
                            {
                                if (source == target || source + stride == target || target + stride == source)
                                    continue;
                                foreach (var direction in new[] { -1, 1 })
                                {
                                    var point = baseline.ToArray();
                                    point[target] = Offset(baseline[source], direction);
                                    point[target + stride] = Offset(baseline[source + stride], -direction * scale);
                                    yield return point;
                                }
                            }
                }
        }

        public bool TryExpected(long[] input, out long result)
        {
            var values = new BigInteger[input.Length + locals.Length];
            for (var i = 0; i < input.Length; i++)
                values[i] = input[i];
            result = default;
            try
            {
                if (domain(values).IsZero)
                    return false;
                for (var i = 0; i < locals.Length; i++)
                    values[input.Length + i] = locals[i](values);
                result = (long)expected(values);
                return true;
            }
            catch (ArithmeticException)
            {
                return false;
            }
        }

        public long[] Mutate(IReadOnlyList<long[]> legal, Random random, bool guided)
        {
            var point = legal[random.Next(legal.Count)].ToArray();
            var palette = guided ? guidedAtoms : atoms;
            var changes = random.Next(5) switch
            {
                0 => 1, 1 => 2, 2 => Math.Min(3, point.Length), 3 => point.Length, _ => random.Next(1, point.Length + 1)
            };
            var target = point[random.Next(point.Length)];
            for (var n = 0; n < changes; n++)
            {
                var index = changes == point.Length ? n : random.Next(point.Length);
                point[index] = random.Next(5) switch
                {
                    0 => Offset(target, random.Next(-1, 2)),
                    1 => Offset(point[index], random.Next(-3, 4)),
                    2 => Offset(point[random.Next(point.Length)], random.Next(-1, 2)),
                    _ => palette[index][random.Next(palette[index].Length)]
                };
            }
            return point;
        }
    }

    private static ILookup<string, long> LiteralBoundaries(string expressions)
    {
        const string forward = @"\b(?<name>[A-Za-z_]\w*)\s*(?:<=|>=|==|!=|<|>)\s*(?<number>-?[0-9]+)\b";
        const string reverse = @"(?<![\w.])(?<number>-?[0-9]+)\s*(?:<=|>=|==|!=|<|>)\s*(?<name>[A-Za-z_]\w*)\b";
        return Regex.Matches(expressions, forward, RegexOptions.CultureInvariant)
            .Concat(Regex.Matches(expressions, reverse, RegexOptions.CultureInvariant))
            .Select(match => (Name: match.Groups["name"].Value,
                Value: long.TryParse(match.Groups["number"].Value, NumberStyles.Integer, CultureInfo.InvariantCulture, out var value)
                    ? (long?)value : null))
            .Where(item => item.Value.HasValue)
            .ToLookup(item => item.Name, item => item.Value.GetValueOrDefault(), StringComparer.Ordinal);
    }

    private static (int First, int Second)[] EqualityPairs(string expressions, string[] parameters)
    {
        const string equality = @"\b(?<left>[A-Za-z_]\w*)\s*(?:==|!=)\s*(?<right>[A-Za-z_]\w*)\b";
        return Regex.Matches(expressions, equality, RegexOptions.CultureInvariant).Select(match =>
            (Left: Array.IndexOf(parameters, match.Groups["left"].Value),
                Right: Array.IndexOf(parameters, match.Groups["right"].Value)))
            .Where(pair => pair.Left >= 0 && pair.Right >= 0 && pair.Left != pair.Right)
            .Select(pair => (Math.Min(pair.Left, pair.Right), Math.Max(pair.Left, pair.Right)))
            .Distinct().Take(32).ToArray();
    }

    private static long Offset(long value, long delta) => (long)Int128.Clamp((Int128)value + delta, long.MinValue, long.MaxValue);
}
