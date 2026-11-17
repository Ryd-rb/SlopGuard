using System.Diagnostics;
using System.Globalization;
using SlopHawk.Core;

namespace SlopHawk.RustCloneParity;

internal static class Program
{
    private static int Main(string[] args)
    {
        if (args.Length > 0 && args[0] == "--bench")
        {
            Bench.Run(args.Skip(1).ToArray());
            return 0;
        }
        var fixture = args.Length > 0 ? args[0] :
            Path.GetFullPath("rust/checkers/clone-index/fixtures/parity.tsv");
        RunFixture(File.ReadAllLines(fixture));
        return 0;
    }

    private static void RunFixture(string[] lines)
    {
        var settings = new CloneIndexSettings();
        var limits = new CloneQueryLimits();
        var index = new TokenCloneIndex(settings);
        var active = new Dictionary<string, CloneTokenDocument>(StringComparer.Ordinal);
        string? documentId = null;
        string? version = null;
        bool generated = false;
        bool excluded = false;
        var blocks = new List<CloneTokenBlock>();
        foreach (var raw in lines)
        {
            var line = raw.TrimEnd('\r');
            if (string.IsNullOrEmpty(line) || line.StartsWith('#')) continue;
            var cells = line.Split('\t');
            switch (cells[0])
            {
                case "SETTINGS":
                    settings = new CloneIndexSettings(Int(cells[1]), Int(cells[2]), Int(cells[3]));
                    index = new TokenCloneIndex(settings);
                    active.Clear();
                    break;
                case "LIMITS":
                    limits = new CloneQueryLimits(Int(cells[1]), Int(cells[2]),
                        long.Parse(cells[3], CultureInfo.InvariantCulture));
                    break;
                case "UPSERT":
                    if (documentId is not null) throw new FormatException("Unclosed UPSERT");
                    documentId = cells[1]; version = cells[2];
                    generated = bool.Parse(cells[3]); excluded = bool.Parse(cells[4]);
                    blocks.Clear();
                    break;
                case "BLOCK":
                    if (documentId is null) throw new FormatException("BLOCK outside UPSERT");
                    blocks.Add(new CloneTokenBlock(cells[1],
                        new CloneSourceSpan(Int(cells[2]), Int(cells[3])),
                        cells.Skip(4).Select(ParseToken).ToArray()));
                    break;
                case "END":
                    if (documentId is null) throw new FormatException("END outside UPSERT");
                    var doc = new CloneTokenDocument(documentId, version!, generated, excluded,
                        blocks.ToArray());
                    index.Upsert(doc);
                    active[doc.DocumentId] = doc;
                    documentId = null;
                    break;
                case "DELETE":
                    index.Delete(cells[1]); active.Remove(cells[1]);
                    break;
                case "REBUILD":
                    index = new TokenCloneIndex(settings);
                    foreach (var entry in active.Values.OrderByDescending(d => d.DocumentId,
                        StringComparer.Ordinal)) index.Upsert(entry);
                    break;
                case "QUERY":
                    Emit(cells[1], index.Query(limits));
                    break;
                default:
                    throw new FormatException($"Unrecognized fixture statement: {cells[0]}");
            }
        }
        if (documentId is not null) throw new FormatException("Unclosed UPSERT");
    }

    private static int Int(string cell) => int.Parse(cell, CultureInfo.InvariantCulture);

    private static CloneToken ParseToken(string entry)
    {
        var separator = entry.IndexOf(':');
        if (separator < 0) throw new FormatException("Expected Kind:Text");
        var kind = entry[..separator] switch
        {
            "O" => CloneTokenKind.Other,
            "L" => CloneTokenKind.LocalIdentifier,
            "B" => CloneTokenKind.BoundIdentifier,
            "T" => CloneTokenKind.Literal,
            _ => throw new FormatException("Unknown Kind")
        };
        return new CloneToken(kind, entry[(separator + 1)..]);
    }

    private static void Emit(string name, CloneQueryResult result)
    {
        var s = result.Statistics;
        var a = result.IndexSettings;
        var l = result.AppliedLimits;
        Console.WriteLine(string.Join('\t', "Q", name,
            s.DocumentCount, s.IndexedBlockCount, s.GeneratedDocuments, s.ExcludedDocuments,
            s.BelowMinimumBlocks, s.OversizedBlocks, s.CandidatePairsExamined,
            s.ComparedTokensCharged, a.MinimumTokens, a.MaximumTokensPerBlock,
            a.MaximumFingerprintPostings, l.MaximumCandidatePairs, l.MaximumOutputPairs,
            l.MaximumComparedTokens, result.NormalizationRuleVersion,
            result.IsTruncated.ToString().ToLowerInvariant(), result.Pairs.Count));
        foreach (var pair in result.Pairs)
        {
            var left = pair.Left;
            var right = pair.Right;
            Console.WriteLine(string.Join('\t', "P", pair.PairId, pair.MatchClass,
                pair.NormalizationRuleVersion,
                left.DocumentId, left.DocumentVersion, left.BlockId,
                left.Span.StartOffset, left.Span.EndOffset, left.TokenCount,
                right.DocumentId, right.DocumentVersion, right.BlockId,
                right.Span.StartOffset, right.Span.EndOffset, right.TokenCount,
                pair.ExactMatchedTokens, pair.NormalizedMatchedTokens,
                pair.Score.ToString("F1", CultureInfo.InvariantCulture)));
        }
        foreach (var gap in result.Gaps)
            Console.WriteLine(string.Join('\t', "G", gap.Reason, gap.ScopeKey, gap.AffectedCount));
    }
}

/// <summary>Engine-only phases, no provider, daemon, file serialization or process launch time.</summary>
internal static class Bench
{
    // Both runtimes use the same arithmetic and document construction as clone-bench.
    private static CloneTokenDocument MakeDoc(int n, int tokens, bool changed = false)
    {
        var text = $"local-{n / 3}";
        var array = new CloneToken[tokens];
        for (int i = 0; i < tokens; i++)
        {
            array[i] = (i % 6) switch
            {
                0 or 4 => new CloneToken(CloneTokenKind.LocalIdentifier, text),
                1 => new CloneToken(CloneTokenKind.Other, "="),
                2 => new CloneToken(CloneTokenKind.Literal, changed ? "9" : (n / 3).ToString(CultureInfo.InvariantCulture)),
                3 => new CloneToken(CloneTokenKind.Other, ";"),
                _ => new CloneToken(CloneTokenKind.BoundIdentifier, "Console")
            };
        }
        return new CloneTokenDocument($"d{n:D6}", "v1", false, false,
            [new CloneTokenBlock("body", new CloneSourceSpan(0, tokens * 2), array)]);
    }

    private static long PssKiB()
    {
        var path = $"/proc/{Environment.ProcessId}/smaps_rollup";
        if (!File.Exists(path)) return -1;
        var line = File.ReadLines(path).FirstOrDefault(x => x.StartsWith("Pss:", StringComparison.Ordinal));
        return line is null ? -1 : long.Parse(line.Split(' ', StringSplitOptions.RemoveEmptyEntries)[1],
            CultureInfo.InvariantCulture);
    }

    public static void Run(string[] args)
    {
        if (args.Length != 4) throw new ArgumentException("--bench DOCS TOKENS REPEATS QUERY-CANDIDATE-CAP");
        var docs = int.Parse(args[0], CultureInfo.InvariantCulture);
        var tokens = int.Parse(args[1], CultureInfo.InvariantCulture);
        var repeats = int.Parse(args[2], CultureInfo.InvariantCulture);
        var candidates = int.Parse(args[3], CultureInfo.InvariantCulture);
        var settings = new CloneIndexSettings(4, Math.Max(4096, tokens), 64);
        var limits = new CloneQueryLimits(candidates, candidates, 2_000_000);
        var fixture = Enumerable.Range(0, docs).Select(n => MakeDoc(n, tokens)).ToArray();
        for (int sample = 0; sample < repeats; sample++)
        {
            var index = new TokenCloneIndex(settings);
            Phase("upsert", sample, () => { foreach (var doc in fixture) index.Upsert(doc); });
            Phase("cold_query", sample, () => index.Query(limits));
            Phase("warm_query", sample, () => index.Query(limits));
            Phase("replacement", sample, () => index.Upsert(MakeDoc(docs / 2, tokens, true)));
            Phase("deletion", sample, () => index.Delete($"d{docs / 3:D6}"));
        }
        void Phase(string phase, int sample, Action work)
        {
            var begin = PssKiB();
            var clock = Stopwatch.StartNew();
            work();
            clock.Stop();
            Console.WriteLine(string.Join('\t', "M", "csharp", docs, tokens, repeats,
                candidates, sample, phase, clock.Elapsed.TotalMicroseconds.ToString("F0",
                CultureInfo.InvariantCulture), begin, PssKiB()));
        }
    }
}
