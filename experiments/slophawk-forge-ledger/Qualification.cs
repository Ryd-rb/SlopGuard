using System.Text.RegularExpressions;

partial class Holdout
{
    private static string RenderQualification(Op[] ops, Dictionary<string, int> selected)
    {
        var methods = new List<string>();
        var runs = new List<string>();
        foreach (var op in ops)
        {
            var method = op.Method;
            var domain = RenderMethod(method + "Domain", op.Parameters, "return " + op.Domain + ";", "bool");
            var bindings = string.Join(" ", (op.Let ?? []).Select(pair => $"var {pair.Key} = {pair.Value};"));
            var expected = op.Expected.Replace(">> index", ">> (int)index").Replace(">> (4 - index)", ">> (int)(4 - index)");
            methods.Add(domain);
            methods.Add(RenderMethod(method + "Spec", op.Parameters, bindings + " return " + expected + ";"));
            methods.Add(RenderMethod(method + "Baseline", op.Parameters, op.Baseline));
            methods.Add(RenderMethod(method + "Reference", op.Parameters, op.Reference));
            for (int index = 0; index < op.Variants.Length; index++)
                methods.Add(RenderMethod(method + "V" + index, op.Parameters, op.Variants[index].Body));

            string Args(string array) => string.Join(", ", op.Parameters.Select((_, index) => $"{array}[{index}]"));
            string ArrayExpr(IEnumerable<string> values) => "[" + string.Join(", ", values) + "]";
            var cases = string.Join(", ", op.Examples.Select(ArrayExpr));
            var witnesses = string.Join(", ", op.Variants.Select(variant => ArrayExpr(variant.Witness.Select(value => value.ToString()))));
            var variants = string.Join(", ", op.Variants.Select((_, index) => $"a => {method}V{index}({Args("a")})"));
            var ranges = string.Join(", ", op.Parameters.Select(parameter =>
            {
                var lowMatch = Regex.Match(op.Domain, $@"\b{parameter} >= (-?\d+)(?=\s|\)|$)");
                var highMatch = Regex.Match(op.Domain, $@"\b{parameter} <= (-?\d+)(?=\s|\)|$)");
                var low = lowMatch.Success ? lowMatch.Groups[1].Value : "-100";
                var high = highMatch.Success ? highMatch.Groups[1].Value : "300";
                return $"({low}, {high})";
            }));

            runs.Add($$"""
                    Verify(
                        "{{op.Operation}}", {{selected.GetValueOrDefault(op.Operation, -1)}},
                        [{{cases}}], [{{witnesses}}],
                        a => {{method}}Domain({{Args("a")}}),
                        a => {{method}}Spec({{Args("a")}}),
                        a => {{method}}Baseline({{Args("a")}}),
                        a => {{method}}Reference({{Args("a")}}),
                        [{{variants}}],
                        [{{ranges}}]);
                """);
        }

        return $$"""
            using System.Numerics;
            using System.Text.Json;

            static class Qualification
            {
                private static readonly Random Sampler = new(291317);
                private static int ordinaryCount;
                private static int sampledCount;
                private static int witnessCount;
                private static readonly List<object> WitnessResults = [];

                public static void Main(string[] args)
                {
            {{string.Join("\n", runs)}}
                    File.WriteAllText(args[0], JsonSerializer.Serialize(WitnessResults,
                        new JsonSerializerOptions { WriteIndented = true }));
                    Console.WriteLine($"Passed {ordinaryCount} ordinary examples, {sampledCount} sampled in-domain baseline/reference/spec checks and {witnessCount} distinct variant witnesses.");
                }

                private static void Verify(
                    string operation, int selected, long[][] ordinary, long[][] witnesses,
                    Func<long[], bool> domain, Func<long[], long> spec, Func<long[], long> baseline,
                    Func<long[], long> reference, Func<long[], long>[] variants,
                    (long Min, long Max)[] ranges)
                {
                    long Compare(long[] input)
                    {
                        if (!domain(input))
                            throw new InvalidOperationException($"{operation} is out of domain: [{string.Join(", ", input)}]");
                        var specification = spec(input);
                        var independent = reference(input);
                        var cleanCandidate = baseline(input);
                        if (specification != independent || specification != cleanCandidate)
                            throw new InvalidOperationException($"{operation} has inconsistent correct implementations on [{string.Join(", ", input)}]: {specification}, {independent}, {cleanCandidate}");
                        return specification;
                    }

                    foreach (var example in ordinary)
                    {
                        var expected = Compare(example);
                        for (int index = 0; index < variants.Length; index++)
                            if (variants[index](example) != expected)
                                throw new InvalidOperationException($"{operation} variant {index + 1} fails ordinary example [{string.Join(", ", example)}].");
                        ordinaryCount++;
                    }

                    for (int index = 0; index < witnesses.Length; index++)
                    {
                        var witness = witnesses[index];
                        var expected = Compare(witness);
                        var actual = variants[index](witness);
                        if (actual == expected)
                            throw new InvalidOperationException($"{operation} variant {index + 1} witness does not reproduce.");
                        WitnessResults.Add(new { Operation = operation, Variant = index + 1, Selected = index == selected, Args = witness, Expected = expected, Actual = actual });
                        witnessCount++;
                    }

                    var qualified = 0;
                    for (int draw = 0; draw < 30000 && qualified < 1000; draw++)
                    {
                        var sample = ranges.Select(range => Sampler.NextInt64(range.Min, range.Max + 1)).ToArray();
                        if (!domain(sample))
                            continue;
                        var expected = Compare(sample);
                        if (selected == -1 && baseline(sample) != expected)
                            throw new InvalidOperationException($"{operation} control disagrees with specification.");
                        qualified++;
                    }
                    if (qualified != 1000)
                        throw new InvalidOperationException($"{operation} only admitted {qualified} sampled cases.");
                    sampledCount += qualified;
                }

            {{string.Join("\n\n", methods)}}

            {{Helpers}}
            }
            """ + "\n";
    }
}
