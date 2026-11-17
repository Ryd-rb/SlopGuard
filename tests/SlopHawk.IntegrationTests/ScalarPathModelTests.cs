using System.Reflection;
using System.Text.Json;
using Microsoft.CodeAnalysis;
using Microsoft.CodeAnalysis.CSharp;
using SlopHawk.Providers.CSharp;
using Xunit;

namespace SlopHawk.IntegrationTests;

public sealed class ScalarPathModelTests
{
    private const string Document = "scalar-fixture.cs";
    private const string Source = """
        namespace Fixture;

        public static class Planner
        {
            public static int Trace(int x, int y, bool enabled)
            {
                int trace = 0;
                if (enabled)
                {
                    trace += 1;
                    if (x == y && unchecked(x + 1) < y)
                        trace += 2;
                    if (x == y)
                        trace += 4;
                }
                return trace;
            }

            public static int Assignment(int x, int y, bool enabled)
            {
                int value = x;
                value = enabled ? x + 7 : y;
                value -= 5;
                return value == 395 ? 1 : 0;
            }

            public static int Opaque(int x, bool enabled) => enabled && x != 0 && 12 / x > 4 ? 1 : 0;
            public static int Checked(int x) => checked(x + 1) > 0 ? 1 : 0;
            public static int Loop(int x) { while (x > 5) x--; return x; }
            public static int Contradiction(int x) => x == 27 && x != 27 ? 1 : 0;
            public static long UnsupportedSignature(int x, long y, bool enabled) => enabled ? x : y;
        }
        """;

    [Fact]
    public void ConjunctiveCandidatesAgreeWithCompiledGuardTraceAcrossSeededDomain()
    {
        var (compilation, type) = Compile();
        var trace = type.GetMethod("Trace")!;
        var seeds = (from x in new[] { -1, 0, 1 }
                     from y in new[] { -1, 0, 1 }
                     from enabled in new[] { 0, 1 }
                     select new long[] { x, y, enabled }).ToArray();
        var result = Explore(compilation, "Trace(int, int, bool)", seeds);
        var graph = JsonSerializer.SerializeToElement(result.Graph);
        var edges = graph.GetProperty("Edges").EnumerateArray()
            .Where(edge => edge.GetProperty("Polarity").ValueKind != JsonValueKind.Null)
            .ToDictionary(edge => edge.GetProperty("Id").GetString()!);

        Assert.Equal(new[] { 0, 1, 5 }, seeds.Select(Trace).Distinct().Order());
        Assert.DoesNotContain(result.Unknown, reason => reason.StartsWith("No supported input explored", StringComparison.Ordinal));
        Assert.Contains(result.Candidates, candidate => candidate.Args.SequenceEqual(new long[] { int.MaxValue, int.MaxValue, 1 }));
        Assert.Equal(7, Trace([int.MaxValue, int.MaxValue, 1]));
        foreach (var candidate in result.Candidates)
        {
            var edge = edges[candidate.Goal];
            var span = edge.GetProperty("Span");
            var condition = Source.Substring(span.GetProperty("Start").GetInt32(), span.GetProperty("Length").GetInt32());
            var observed = Trace(candidate.Args);
            var (reached, conditionValue) = condition switch
            {
                "enabled" => (true, (observed & 1) != 0),
                "x == y" => ((observed & 1) != 0, (observed & 4) != 0),
                "unchecked(x + 1) < y" => ((observed & 5) == 5, (observed & 2) != 0),
                _ => throw new InvalidOperationException($"Unexpected guard span '{condition}'.")
            };
            Assert.True(reached, $"Compiled execution did not reach {condition} for [{string.Join(", ", candidate.Args)}].");
            Assert.Equal(conditionValue, edge.GetProperty("Polarity").GetBoolean());
        }

        var repeat = Explore(compilation, "Trace(int, int, bool)", seeds);
        Assert.Equal(JsonSerializer.Serialize(result), JsonSerializer.Serialize(repeat));

        int Trace(long[] args) => (int)trace.Invoke(null, [(int)args[0], (int)args[1], args[2] == 1])!;
    }

    [Fact]
    public void TracksAssignmentThroughCapturedLocalAndKeepsUnsupportedFrontiersUnknown()
    {
        var (compilation, type) = Compile();
        var assignment = Explore(compilation, "Assignment(int, int, bool)", [[0, 0, 0]]);
        var compiled = type.GetMethod("Assignment")!;
        Assert.Contains(assignment.Candidates, candidate => (int)compiled.Invoke(null,
            [(int)candidate.Args[0], (int)candidate.Args[1], candidate.Args[2] == 1])! == 1);

        var opaque = Explore(compilation, "Opaque(int, bool)", [[0, 0]]);
        Assert.NotEmpty(opaque.Candidates);
        Assert.Contains(opaque.Unknown, reason => reason.Contains("Divide", StringComparison.Ordinal));
        Assert.Contains(opaque.Unknown, reason => reason.StartsWith("No supported input explored", StringComparison.Ordinal));
        var checkedAddition = Explore(compilation, "Checked(int)", [[0]]);
        Assert.Empty(checkedAddition.Candidates);
        Assert.Contains(checkedAddition.Unknown, reason => reason.Contains("Add checked", StringComparison.Ordinal));
        var loop = Explore(compilation, "Loop(int)", [[0]]);
        Assert.Contains(loop.Unknown, reason => reason.StartsWith("Loop frontier", StringComparison.Ordinal));
        var contradiction = Explore(compilation, "Contradiction(int)", [[0]]);
        Assert.Contains(contradiction.Unknown, reason => reason.StartsWith("No supported input explored", StringComparison.Ordinal));
    }

    [Fact]
    public void GraphOnlySelectionRetainsBoundScalarSignature()
    {
        var (compilation, _) = Compile();
        var graphOnly = ScalarPaths.Explore(compilation, Document, "Fixture.Planner.Trace(int, int, bool)", [], 0, 0);
        Assert.True(graphOnly.Bound);
        Assert.True(graphOnly.ScalarEligible);
        Assert.Equal(new[] { "int", "int", "bool" }, graphOnly.ParameterTypes);
        Assert.NotNull(graphOnly.Graph);
        Assert.Equal(0, graphOnly.Work);

        var unsupported = ScalarPaths.Explore(compilation, Document, "Fixture.Planner.UnsupportedSignature(int, long, bool)", [], 0, 0);
        Assert.True(unsupported.Bound);
        Assert.False(unsupported.ScalarEligible);
        Assert.Equal(new[] { "int", "long", "bool" }, unsupported.ParameterTypes);
        Assert.NotNull(unsupported.Graph);
    }

    private static ScalarPathResult Explore(CSharpCompilation compilation, string method, IReadOnlyList<long[]> seeds)
    {
        var result = ScalarPaths.Explore(compilation, Document, "Fixture.Planner." + method, seeds, 128, 32);
        Assert.True(result.Bound, result.Reason);
        Assert.NotNull(result.Graph);
        Assert.InRange(result.Work, 1, 128);
        Assert.InRange(result.Candidates.Count, 0, 32);
        return result;
    }

    private static (CSharpCompilation Compilation, Type Type) Compile()
    {
        var tree = CSharpSyntaxTree.ParseText(Source, path: Document);
        var compilation = CSharpCompilation.Create("ScalarPathModelFixture", [tree],
            [MetadataReference.CreateFromFile(typeof(object).Assembly.Location)],
            new CSharpCompilationOptions(OutputKind.DynamicallyLinkedLibrary));
        using var output = new MemoryStream();
        var emit = compilation.Emit(output);
        Assert.True(emit.Success, string.Join(Environment.NewLine, emit.Diagnostics));
        return (compilation, Assembly.Load(output.ToArray()).GetType("Fixture.Planner", throwOnError: true)!);
    }
}
