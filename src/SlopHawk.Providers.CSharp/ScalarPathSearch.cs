using System.Globalization;
using System.Numerics;
using Microsoft.CodeAnalysis;
using Microsoft.CodeAnalysis.FlowAnalysis;
using Microsoft.CodeAnalysis.Operations;

namespace SlopHawk.Providers.CSharp;

public static partial class ScalarPaths
{
    private sealed class ScalarSearch(ControlFlowGraph flow, IMethodSymbol method, IReadOnlyList<long[]> seeds,
        int workLimit, int caseLimit, CancellationToken cancellationToken)
    {
        private readonly HashSet<string> seedKeys = new(StringComparer.Ordinal);
        private readonly HashSet<string> simulated = new(StringComparer.Ordinal);
        private readonly HashSet<string> covered = new(StringComparer.Ordinal);
        private readonly SortedSet<string> unknown = new(StringComparer.Ordinal);
        private readonly Dictionary<string, int> expansions = new(StringComparer.Ordinal);
        private readonly Dictionary<string, int> pending = new(StringComparer.Ordinal);
        private readonly PriorityQueue<SearchInput, (int Depth, long Order)> queue = new();
        private readonly List<SearchOutput> outputs = [];
        private IReadOnlySet<CaptureId> assignmentCaptures = new HashSet<CaptureId>();
        private long proposalOrder;
        private long proposalWork;
        private int work;
        private long ProposalLimit => 64L * workLimit;

        public ScalarPathResult Run(PathGraph graph, bool eligible)
        {
            if (caseLimit == 0)
                return new(true, null, graph, [], 0, ["Scalar search was not requested."]);
            if (!eligible)
                return new(true, null, graph, [], 0,
                    ["Scalar search requires a synchronous method with by-value int or bool parameters and return."]);

            if (workLimit == 0)
                return new(true, null, graph, [], 0, ["Scalar work limit reached before search."]);

            assignmentCaptures = AssignmentCaptures(flow, cancellationToken);
            var initial = new List<SearchInput>();
            var seedLimit = Math.Max(1, workLimit / 4);
            foreach (var seed in seeds)
            {
                cancellationToken.ThrowIfCancellationRequested();
                if (!Compatible(seed))
                {
                    unknown.Add("A seed is incompatible with the bound int and bool parameters.");
                    continue;
                }

                var key = Key(seed);
                if (seedKeys.Add(key) && initial.Count < seedLimit)
                    initial.Add(new((long[])seed.Clone(), key, null, true));
            }

            if (seedKeys.Count > initial.Count)
                unknown.Add("Some seed paths were skipped to reserve work for generated inputs.");
            if (initial.Count == 0)
            {
                var defaults = new long[method.Parameters.Length];
                initial.Add(new(defaults, Key(defaults), null, false));
            }

            foreach (var input in initial)
                Simulate(input);

            while (work < workLimit && queue.TryDequeue(out var input, out var priority))
            {
                cancellationToken.ThrowIfCancellationRequested();
                if (!pending.TryGetValue(input.Key, out var depth) || depth != priority.Depth)
                    continue;
                pending.Remove(input.Key);
                Simulate(input);
            }

            if (pending.Count != 0)
                unknown.Add($"Scalar work limit {workLimit} reached.");
            if (outputs.Count > caseLimit)
                unknown.Add($"Scalar candidate case limit {caseLimit} reached.");
            if (proposalWork >= ProposalLimit)
                unknown.Add($"Scalar proposal limit {ProposalLimit} reached.");

            foreach (var edge in graph.Edges.Where(edge => edge.Polarity is not null && !covered.Contains(edge.Id)))
                unknown.Add($"No supported input explored {edge.Id}.");

            var results = outputs.OrderByDescending(output => output.Novel)
                .ThenByDescending(output => output.Targeted)
                .ThenByDescending(output => output.Depth)
                .ThenBy(output => output.Order)
                .Take(caseLimit).Select(output => output.Candidate).ToArray();
            return new(true, null, graph, results, work, unknown.ToArray());
        }

        private bool Compatible(long[]? arguments)
        {
            if (arguments is null || arguments.Length != method.Parameters.Length)
                return false;
            foreach (var parameter in method.Parameters)
            {
                var argument = arguments[parameter.Ordinal];
                if (argument is < int.MinValue or > int.MaxValue ||
                    parameter.Type.SpecialType == SpecialType.System_Boolean && argument is not (0 or 1))
                    return false;
            }

            return true;
        }

        private void Simulate(SearchInput input)
        {
            if (!simulated.Add(input.Key))
                return;
            work++;

            var simulation = new ScalarInterpreter(flow, method, input.Arguments, assignmentCaptures, cancellationToken).Run();
            if (simulation.Unknown is { } reason)
                unknown.Add(reason);

            string? novel = null;
            var targeted = false;
            var prefix = string.Empty;
            var preserved = new List<Comparison>();
            for (var index = 0; index < simulation.Guards.Count; index++)
            {
                var guard = simulation.Guards[index];
                if (covered.Add(guard.Taken))
                    novel = guard.Taken;
                targeted |= input.Goal == guard.Taken;

                prefix += "/" + guard.Taken;
                var attempts = expansions.GetValueOrDefault(prefix);
                if (attempts < 4)
                {
                    expansions[prefix] = attempts + 1;
                    foreach (var comparison in guard.Comparisons)
                        Propose(input.Arguments, comparison, guard.Alternate, guard.Taken, index, preserved);
                }

                if (guard.Comparisons.Count == 1 && guard.Comparisons[0] is { } predicate &&
                    predicate.Operator is BinaryOperatorKind.Equals or BinaryOperatorKind.NotEquals &&
                    predicate.Left.Value == predicate.Right.Value)
                    preserved.Add(predicate);
            }

            if (input.Seed || simulation.Guards.Count == 0)
                return;

            var goal = novel ?? (targeted ? input.Goal : simulation.Guards[^1].Taken)!;
            outputs.Add(new(new(input.Arguments, goal), novel is not null, targeted, simulation.Guards.Count, work));
        }

        private void Propose(long[] arguments, Comparison comparison, string alternate, string taken, int depth,
            IReadOnlyList<Comparison> preserved)
        {
            var left = comparison.Left;
            var right = comparison.Right;
            var visited = new HashSet<int>();
            foreach (var parameter in (left.Coefficients?.Keys ?? []).Concat(right.Coefficients?.Keys ?? []))
            {
                cancellationToken.ThrowIfCancellationRequested();
                if (!visited.Add(parameter))
                    continue;

                var leftFactor = Factor(left, parameter);
                var rightFactor = Factor(right, parameter);
                var difference = unchecked(left.Value - right.Value);
                var differenceFactor = unchecked(leftFactor - rightFactor);
                foreach (var target in new[] { 0, -1, 1 })
                {
                    foreach (var change in Changes(differenceFactor, unchecked(target - difference)))
                        Consider(parameter, change);
                    foreach (var prior in preserved)
                        ProposePair(prior, parameter, differenceFactor, unchecked(target - difference), left, right);
                }

                foreach (var boundary in new[] { int.MinValue, int.MaxValue })
                {
                    foreach (var change in Changes(leftFactor, unchecked(boundary - left.Value)))
                        Consider(parameter, change);
                    foreach (var change in Changes(rightFactor, unchecked(boundary - right.Value)))
                        Consider(parameter, change);
                    foreach (var prior in preserved)
                    {
                        ProposePair(prior, parameter, leftFactor, unchecked(boundary - left.Value), left);
                        ProposePair(prior, parameter, rightFactor, unchecked(boundary - right.Value), right);
                    }
                }
            }

            void ProposePair(Comparison prior, int parameter, int targetFactor, int delta, Scalar firstOperand,
                Scalar? secondOperand = null)
            {
                cancellationToken.ThrowIfCancellationRequested();
                var priorFactor = unchecked(Factor(prior.Left, parameter) - Factor(prior.Right, parameter));
                if (priorFactor == 0 || proposalWork >= ProposalLimit)
                    return;

                var partners = (prior.Left.Coefficients?.Keys ?? []).Concat(prior.Right.Coefficients?.Keys ?? []).Distinct();
                foreach (var partner in partners)
                {
                    if (partner == parameter)
                        continue;
                    var partnerPriorFactor = unchecked(Factor(prior.Left, partner) - Factor(prior.Right, partner));
                    if (partnerPriorFactor == 0)
                        continue;

                    var partnerTargetFactor = Factor(firstOperand, partner);
                    if (secondOperand is { } operand)
                        partnerTargetFactor = unchecked(partnerTargetFactor - Factor(operand, partner));
                    var determinant = unchecked(targetFactor * partnerPriorFactor - partnerTargetFactor * priorFactor);
                    foreach (var first in Changes(determinant, unchecked(delta * partnerPriorFactor)))
                    foreach (var second in Changes(partnerPriorFactor, unchecked(-priorFactor * first)))
                    {
                        if (unchecked(targetFactor * first + partnerTargetFactor * second) == delta)
                            Consider(parameter, first, partner, second);
                    }
                }
            }

            void Consider(int parameter, int change, int? partner = null, int partnerChange = 0)
            {
                if (proposalWork >= ProposalLimit)
                    return;
                proposalWork++;
                var newArgument = unchecked((int)arguments[parameter] + change);
                if (change == 0 && partnerChange == 0 || !ValidParameter(parameter, newArgument))
                    return;
                var copy = (long[])arguments.Clone();
                copy[parameter] = newArgument;
                var changedLeft = unchecked(left.Value + Factor(left, parameter) * change);
                var changedRight = unchecked(right.Value + Factor(right, parameter) * change);
                if (partner is { } other)
                {
                    var newPartner = unchecked((int)arguments[other] + partnerChange);
                    if (!ValidParameter(other, newPartner))
                        return;
                    copy[other] = newPartner;
                    changedLeft = unchecked(changedLeft + Factor(left, other) * partnerChange);
                    changedRight = unchecked(changedRight + Factor(right, other) * partnerChange);
                }

                var flips = Compare(comparison.Operator, left.Value, right.Value) !=
                    Compare(comparison.Operator, changedLeft, changedRight);
                Enqueue(copy, flips ? alternate : taken, -2 * depth - (flips ? 1 : 0));
            }
        }

        private bool ValidParameter(int index, int value) =>
            method.Parameters[index].Type.SpecialType != SpecialType.System_Boolean || value is 0 or 1;

        private void Enqueue(long[] arguments, string goal, int depth)
        {
            var key = Key(arguments);
            if (seedKeys.Contains(key) || simulated.Contains(key))
                return;
            if (pending.TryGetValue(key, out var existing) && existing <= depth)
                return;
            pending[key] = depth;
            queue.Enqueue(new(arguments, key, goal, false), (depth, proposalOrder++));
        }
    }

    private static string Key(long[] arguments) => string.Join(',', arguments.Select(argument =>
        argument.ToString(CultureInfo.InvariantCulture)));

    private static int Factor(Scalar value, int index) => value.Coefficients?.GetValueOrDefault(index) ?? 0;

    // Solve factor * change == delta modulo 2^32. Return at most two representatives.
    private static IEnumerable<int> Changes(int factor, int delta)
    {
        if (factor == 0)
            yield break;
        var shift = BitOperations.TrailingZeroCount(unchecked((uint)factor));
        var divisor = 1U << shift;
        if ((unchecked((uint)delta) & (divisor - 1)) != 0)
            yield break;

        var odd = unchecked((uint)factor) >> shift;
        var inverse = odd;
        for (var iteration = 0; iteration < 5; iteration++)
            inverse = unchecked(inverse * (2 - odd * inverse));

        var period = 1UL << (32 - shift);
        var residue = unchecked(((uint)delta >> shift) * inverse) & (period - 1);
        var nearest = (long)residue;
        if (residue >= period / 2)
            nearest -= (long)period;
        yield return unchecked((int)nearest);
        if (shift != 0)
            yield return unchecked((int)(nearest + (nearest < 0 ? (long)period : -(long)period)));
    }

    private sealed record SearchInput(long[] Arguments, string Key, string? Goal, bool Seed);

    private sealed record SearchOutput(ScalarPathCandidate Candidate, bool Novel, bool Targeted, int Depth, int Order);
}
