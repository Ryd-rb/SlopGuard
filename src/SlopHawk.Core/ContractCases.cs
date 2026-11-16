using System.Numerics;
using System.Text.Json;

namespace SlopHawk.Core;

public sealed record IntegerContract(string Operation, string[] Parameters, string Domain, string Expected, string Source,
    Dictionary<string, string>? Let = null, Dictionary<string, long[]>? Basis = null);

public sealed record ContractQualification(string Operation, string? Source, string? Error);

public sealed class ContractCases
{
    private readonly ContractRule[] rules;
    public IReadOnlyList<ContractQualification> Qualification { get; }

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
    }

    public IEnumerable<ProbeExpectation> Generate(int perOperation, int seed, ISet<string>? operations = null)
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
            var probes = ProbeReplay.Plan(rule.Ordinary.Select(example => new ProbeInput(example.Operation, example.Args)),
                Math.Max(32, perOperation), seed).GetEnumerator();
            using (probes)
            {
                for (var attempt = 0; attempt < perOperation * 48 && emitted < perOperation; attempt++)
                {
                    var point = attempt % 3 == 0 && probes.MoveNext() ? probes.Current.Args : rule.Mutate(legal, random);
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
    }

    private static string Key(long[] args) => string.Join(',', args);

    private sealed class ContractRule
    {
        private readonly Func<BigInteger[], BigInteger> domain;
        private readonly Func<BigInteger[], BigInteger> expected;
        private readonly Func<BigInteger[], BigInteger>[] locals;
        private readonly long[][] atoms;
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
            domain = new ContractExpression(definition.Domain, names).Compile();
            var compiled = new List<Func<BigInteger[], BigInteger>>();
            foreach (var pair in definition.Let ?? [])
            {
                if (!ContractExpression.IsName(pair.Key) || names.ContainsKey(pair.Key))
                    throw new ArgumentException($"Invalid or repeated local '{pair.Key}'.");
                compiled.Add(new ContractExpression(pair.Value, names).Compile());
                names.Add(pair.Key, names.Count);
            }
            locals = [.. compiled];
            expected = new ContractExpression(definition.Expected, names).Compile();
            atoms = parameters.Select((name, index) =>
            {
                long[] standard = [-1, 0, 1, 2, 3, 4, 5, 7, 8, 10, 15, 16, 20];
                var authored = definition.Basis?.GetValueOrDefault(name) ?? [];
                var anchors = ordinary.Select(item => item.Args[index]).Concat(authored);
                return anchors.SelectMany(value => new[] { value, Offset(value, -1), Offset(value, 1) })
                    .Concat(standard).Distinct().ToArray();
            }).ToArray();
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

        public long[] Mutate(IReadOnlyList<long[]> legal, Random random)
        {
            var point = legal[random.Next(legal.Count)].ToArray();
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
                    _ => atoms[index][random.Next(atoms[index].Length)]
                };
            }
            return point;
        }
    }

    private static long Offset(long value, int delta) => (long)Int128.Clamp((Int128)value + delta, long.MinValue, long.MaxValue);
}
