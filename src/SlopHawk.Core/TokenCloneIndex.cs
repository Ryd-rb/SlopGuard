using System.Buffers.Binary;
using System.Globalization;
using System.Security.Cryptography;
using System.Text;

namespace SlopHawk.Core;

/// <summary>How a caller-classified token participates in SG-10 matching.</summary>
/// <remarks>
/// Only LocalIdentifier is renamed. Bound identifiers (members and types), literals,
/// keywords, operators and punctuation keep their exact text and kind. Callers own
/// tokenization and classification; this index does not infer C# binding.
/// </remarks>
public enum CloneTokenKind
{
    Other,
    LocalIdentifier,
    BoundIdentifier,
    Literal
}

public sealed record CloneToken(CloneTokenKind Kind, string Text);

/// <summary>Zero-based, half-open offsets in the caller's source document.</summary>
public readonly record struct CloneSourceSpan(int StartOffset, int EndOffset);

/// <summary>BlockId is stable and unique within its document, including repeated blocks.</summary>
public sealed record CloneTokenBlock(string BlockId, CloneSourceSpan Span, IReadOnlyList<CloneToken> Tokens);

/// <summary>
/// DocumentId identifies the caller's logical document context (not just a physical path).
/// Version changes when its contents or classification change. Generated and excluded
/// documents are retained as metadata but never indexed by this initial rule version.
/// </summary>
public sealed record CloneTokenDocument(
    string DocumentId, string Version, bool IsGenerated, bool IsExcluded,
    IReadOnlyList<CloneTokenBlock> Blocks);

public sealed record CloneIndexSettings(
    int MinimumTokens = 40,
    int MaximumTokensPerBlock = 4096,
    int MaximumFingerprintPostings = 64);

public sealed record CloneQueryLimits(
    int MaximumCandidatePairs = 20_000,
    int MaximumOutputPairs = 10_000,
    long MaximumComparedTokens = 2_000_000);

public enum CloneMatchClass
{
    Exact,
    RenamedLocalIdentifiers
}

public sealed record CloneMatchLocation(
    string DocumentId, string DocumentVersion, string BlockId,
    CloneSourceSpan Span, int TokenCount);

/// <summary>A canonical unordered pair, with a 1.0 score over its verified full blocks.</summary>
public sealed record CloneTokenPair(
    string PairId, CloneMatchClass MatchClass, string NormalizationRuleVersion,
    CloneMatchLocation Left, CloneMatchLocation Right,
    int ExactMatchedTokens, int NormalizedMatchedTokens, double Score);

public enum CloneScopeGapReason
{
    OversizedBlock,
    BoilerplatePostingLimit,
    CandidatePairLimit,
    ComparedTokenLimit,
    OutputPairLimit
}

/// <summary>
/// A deliberate omission. ScopeKey is a block identity, fingerprint bucket, or the
/// first pair not returned/verified at the corresponding budget frontier.
/// </summary>
public sealed record CloneScopeGap(
    CloneScopeGapReason Reason, string ScopeKey, long AffectedCount);

public sealed record CloneQueryStatistics(
    int DocumentCount, int IndexedBlockCount, int GeneratedDocuments,
    int ExcludedDocuments, int BelowMinimumBlocks, int OversizedBlocks,
    int CandidatePairsExamined, long ComparedTokensCharged);

public sealed record CloneQueryResult(
    IReadOnlyList<CloneTokenPair> Pairs,
    CloneQueryStatistics Statistics,
    IReadOnlyList<CloneScopeGap> Gaps,
    CloneIndexSettings IndexSettings,
    CloneQueryLimits AppliedLimits)
{
    public string NormalizationRuleVersion => TokenCloneIndex.RuleVersion;
    public bool IsTruncated => Gaps.Count != 0;
}

/// <summary>
/// A mutable, in-memory, whole-block Type-1/Type-2 index for pretokenized documents.
/// No near-match search, persistence, source parsing, or daemon integration is implied.
/// Writers and readers must be serialized by the caller. A query derives pairs from
/// current postings (no stale pair cache), so replacement/deletion updates both sides.
/// </summary>
public sealed class TokenCloneIndex
{
    public const string RuleVersion = "sg10.token-clones.v1";

    private static readonly IComparer<BlockKey> KeyComparer = Comparer<BlockKey>.Create(
        static (a, b) =>
        {
            var documents = StringComparer.Ordinal.Compare(a.DocumentId, b.DocumentId);
            return documents != 0 ? documents : StringComparer.Ordinal.Compare(a.BlockId, b.BlockId);
        });

    private readonly CloneIndexSettings _settings;
    private readonly Dictionary<string, StoredDocument> _documents = new(StringComparer.Ordinal);
    private readonly Dictionary<BlockKey, StoredBlock> _blocks = new();
    private readonly Dictionary<string, SortedSet<BlockKey>> _exact = new(StringComparer.Ordinal);
    private readonly Dictionary<string, SortedSet<BlockKey>> _normalized = new(StringComparer.Ordinal);

    public TokenCloneIndex(CloneIndexSettings? settings = null)
    {
        _settings = settings ?? new CloneIndexSettings();
        if (_settings.MinimumTokens < 1 || _settings.MaximumTokensPerBlock < _settings.MinimumTokens ||
            _settings.MaximumFingerprintPostings < 2)
            throw new ArgumentOutOfRangeException(nameof(settings),
                "Require 1 <= minimum <= maximum block tokens and at least two postings.");
    }

    public int DocumentCount => _documents.Count;

    /// <summary>
    /// Validates and snapshots the entire input before replacing any previous version.
    /// Repeating a Version is not a no-op: the supplied snapshot always wins.
    /// </summary>
    public void Upsert(CloneTokenDocument document)
    {
        ArgumentNullException.ThrowIfNull(document);
        RequireId(document.DocumentId, nameof(document.DocumentId));
        RequireId(document.Version, nameof(document.Version));
        ArgumentNullException.ThrowIfNull(document.Blocks);

        var nextBlocks = new List<StoredBlock>(document.Blocks.Count);
        var seen = new HashSet<string>(StringComparer.Ordinal);
        foreach (var block in document.Blocks)
        {
            ArgumentNullException.ThrowIfNull(block);
            RequireId(block.BlockId, nameof(block.BlockId));
            if (!seen.Add(block.BlockId))
                throw new ArgumentException("Block IDs must be unique within a document.", nameof(document));
            if (block.Span.StartOffset < 0 || block.Span.EndOffset <= block.Span.StartOffset)
                throw new ArgumentException("Source spans must be nonempty half-open offsets.", nameof(document));
            ArgumentNullException.ThrowIfNull(block.Tokens);
            var count = block.Tokens.Count;
            foreach (var token in block.Tokens)
            {
                if (token is null || string.IsNullOrEmpty(token.Text) ||
                    !Enum.IsDefined(token.Kind))
                    throw new ArgumentException("Tokens require a known kind and nonempty text.", nameof(document));
            }

            var key = new BlockKey(document.DocumentId, block.BlockId);
            var location = new CloneMatchLocation(document.DocumentId, document.Version,
                block.BlockId, block.Span, count);
            if (document.IsGenerated || document.IsExcluded ||
                count < _settings.MinimumTokens || count > _settings.MaximumTokensPerBlock)
            {
                nextBlocks.Add(new StoredBlock(key, location, null, null, null, null));
                continue;
            }

            var exact = block.Tokens.Select(static t => new CanonicalToken(t.Kind, t.Text)).ToArray();
            var normalized = Normalize(exact);
            nextBlocks.Add(new StoredBlock(key, location, exact, normalized,
                Fingerprint(exact), Fingerprint(normalized)));
        }

        // All validation and fingerprinting above succeeds before existing postings move.
        Delete(document.DocumentId);
        _documents.Add(document.DocumentId,
            new StoredDocument(document.IsGenerated, document.IsExcluded, nextBlocks));
        foreach (var block in nextBlocks)
        {
            if (block.Exact is null)
                continue;
            _blocks.Add(block.Key, block);
            AddPosting(_exact, block.ExactFingerprint!, block.Key);
            AddPosting(_normalized, block.NormalizedFingerprint!, block.Key);
        }
    }

    /// <returns>True if the logical document was present.</returns>
    public bool Delete(string documentId)
    {
        RequireId(documentId, nameof(documentId));
        if (!_documents.Remove(documentId, out var old))
            return false;
        foreach (var block in old.Blocks)
        {
            if (block.Exact is null)
                continue;
            _blocks.Remove(block.Key);
            RemovePosting(_exact, block.ExactFingerprint!, block.Key);
            RemovePosting(_normalized, block.NormalizedFingerprint!, block.Key);
        }
        return true;
    }

    /// <summary>
    /// Scans exact fingerprints before normalized ones; visits keys in ordinal order.
    /// A hash match only nominates a candidate: full token sequences are rechecked.
    /// Budgets charge the sum of both candidate block lengths, conservatively
    /// accounting for exact and normalized verification.
    /// </summary>
    public CloneQueryResult Query(CloneQueryLimits? limits = null)
    {
        limits ??= new CloneQueryLimits();
        if (limits.MaximumCandidatePairs < 1 || limits.MaximumOutputPairs < 1 ||
            limits.MaximumComparedTokens < 1)
            throw new ArgumentOutOfRangeException(nameof(limits), "Query limits must be positive.");

        var gaps = new List<CloneScopeGap>();
        var results = new List<CloneTokenPair>();
        var ordered = _blocks.Values.OrderBy(static b => b.Key, KeyComparer).ToArray();
        var seen = new HashSet<string>(StringComparer.Ordinal);
        var suppressedBuckets = new HashSet<string>(StringComparer.Ordinal);
        var examined = 0;
        long charged = 0;
        var belowMinimum = 0;
        var oversized = 0;
        var generated = 0;
        var excluded = 0;
        foreach (var doc in _documents.Values)
        {
            if (doc.IsExcluded)
                excluded++;
            else if (doc.IsGenerated)
                generated++;
            else
                foreach (var block in doc.Blocks)
                {
                    if (block.Location.TokenCount < _settings.MinimumTokens)
                        belowMinimum++;
                    else if (block.Location.TokenCount > _settings.MaximumTokensPerBlock)
                    {
                        oversized++;
                        gaps.Add(new CloneScopeGap(CloneScopeGapReason.OversizedBlock,
                            BlockIdentity(block.Key), block.Location.TokenCount));
                    }
                }
        }

        foreach (var exactPass in new[] { true, false })
        {
            var postings = exactPass ? _exact : _normalized;
            foreach (var left in ordered)
            {
                var fingerprint = exactPass ? left.ExactFingerprint! : left.NormalizedFingerprint!;
                var bucket = postings[fingerprint];
                if (bucket.Count > _settings.MaximumFingerprintPostings)
                {
                    var bucketId = (exactPass ? "exact:" : "normalized:") + fingerprint;
                    if (suppressedBuckets.Add(bucketId))
                        gaps.Add(new CloneScopeGap(CloneScopeGapReason.BoilerplatePostingLimit,
                            bucketId, ((long)bucket.Count * (bucket.Count - 1)) / 2));
                    continue;
                }

                foreach (var rightKey in bucket)
                {
                    if (KeyComparer.Compare(left.Key, rightKey) >= 0)
                        continue;
                    var pairId = PairIdentity(left.Key, rightKey);
                    if (seen.Contains(pairId))
                        continue;
                    if (examined >= limits.MaximumCandidatePairs)
                    {
                        gaps.Add(new CloneScopeGap(CloneScopeGapReason.CandidatePairLimit, pairId, 1));
                        return Finish();
                    }
                    var right = _blocks[rightKey];
                    var cost = (long)left.Location.TokenCount + right.Location.TokenCount;
                    if (charged > limits.MaximumComparedTokens - cost)
                    {
                        gaps.Add(new CloneScopeGap(CloneScopeGapReason.ComparedTokenLimit, pairId, cost));
                        return Finish();
                    }
                    examined++;
                    charged += cost;
                    if (left.Location.TokenCount != right.Location.TokenCount ||
                        !Equal(left.Normalized!, right.Normalized!))
                        continue;
                    var equalExact = Equal(left.Exact!, right.Exact!);
                    if (exactPass && !equalExact)
                        continue; // A colliding exact hash cannot imply an exact pair.
                    if (results.Count >= limits.MaximumOutputPairs)
                    {
                        gaps.Add(new CloneScopeGap(CloneScopeGapReason.OutputPairLimit, pairId, 1));
                        return Finish();
                    }
                    seen.Add(pairId);
                    var count = left.Location.TokenCount;
                    results.Add(new CloneTokenPair(pairId,
                        equalExact ? CloneMatchClass.Exact : CloneMatchClass.RenamedLocalIdentifiers,
                        RuleVersion, left.Location, right.Location,
                        equalExact ? count : CountEqual(left.Exact!, right.Exact!), count, 1.0));
                }
            }
        }
        return Finish();

        CloneQueryResult Finish()
        {
            results.Sort(static (a, b) =>
            {
                var type = a.MatchClass.CompareTo(b.MatchClass);
                if (type != 0) return type;
                var tokens = b.NormalizedMatchedTokens.CompareTo(a.NormalizedMatchedTokens);
                return tokens != 0 ? tokens : StringComparer.Ordinal.Compare(a.PairId, b.PairId);
            });
            gaps.Sort(static (a, b) =>
            {
                var reason = a.Reason.CompareTo(b.Reason);
                return reason != 0 ? reason : StringComparer.Ordinal.Compare(a.ScopeKey, b.ScopeKey);
            });
            return new CloneQueryResult(results, new CloneQueryStatistics(
                _documents.Count, ordered.Length, generated, excluded, belowMinimum,
                oversized, examined, charged), gaps, _settings, limits);
        }
    }

    private static void RequireId(string? value, string parameter)
    {
        if (string.IsNullOrWhiteSpace(value))
            throw new ArgumentException("Identity must not be empty or whitespace.", parameter);
    }

    private static CanonicalToken[] Normalize(CanonicalToken[] tokens)
    {
        var identifiers = new Dictionary<string, int>(StringComparer.Ordinal);
        var result = new CanonicalToken[tokens.Length];
        for (var i = 0; i < tokens.Length; i++)
        {
            var token = tokens[i];
            if (token.Kind != CloneTokenKind.LocalIdentifier)
                result[i] = token;
            else
            {
                if (!identifiers.TryGetValue(token.Text, out var number))
                {
                    number = identifiers.Count;
                    identifiers.Add(token.Text, number);
                }
                result[i] = new CanonicalToken(token.Kind, number.ToString(CultureInfo.InvariantCulture));
            }
        }
        return result;
    }

    private static string Fingerprint(CanonicalToken[] tokens)
    {
        using var hasher = IncrementalHash.CreateHash(HashAlgorithmName.SHA256);
        Span<byte> header = stackalloc byte[8];
        foreach (var token in tokens)
        {
            var value = Encoding.UTF8.GetBytes(token.Text);
            BinaryPrimitives.WriteInt32LittleEndian(header, (int)token.Kind);
            BinaryPrimitives.WriteInt32LittleEndian(header[4..], value.Length);
            hasher.AppendData(header);
            hasher.AppendData(value);
        }
        return Convert.ToHexString(hasher.GetHashAndReset());
    }

    private static bool Equal(CanonicalToken[] a, CanonicalToken[] b) => a.AsSpan().SequenceEqual(b);

    private static int CountEqual(CanonicalToken[] a, CanonicalToken[] b)
    {
        var count = 0;
        for (var i = 0; i < a.Length; i++)
            if (a[i] == b[i]) count++;
        return count;
    }

    private static string BlockIdentity(BlockKey key) =>
        key.DocumentId.Length.ToString(CultureInfo.InvariantCulture) + ":" + key.DocumentId +
        key.BlockId.Length.ToString(CultureInfo.InvariantCulture) + ":" + key.BlockId;

    private static string PairIdentity(BlockKey a, BlockKey b) =>
        RuleVersion + ":" + BlockIdentity(a) + "/" + BlockIdentity(b);

    private static void AddPosting(Dictionary<string, SortedSet<BlockKey>> map, string key, BlockKey value)
    {
        if (!map.TryGetValue(key, out var postings))
        {
            postings = new SortedSet<BlockKey>(KeyComparer);
            map.Add(key, postings);
        }
        postings.Add(value);
    }

    private static void RemovePosting(Dictionary<string, SortedSet<BlockKey>> map, string key, BlockKey value)
    {
        var postings = map[key];
        postings.Remove(value);
        if (postings.Count == 0)
            map.Remove(key);
    }

    private readonly record struct BlockKey(string DocumentId, string BlockId);
    private readonly record struct CanonicalToken(CloneTokenKind Kind, string Text);
    private sealed record StoredBlock(BlockKey Key, CloneMatchLocation Location,
        CanonicalToken[]? Exact, CanonicalToken[]? Normalized,
        string? ExactFingerprint, string? NormalizedFingerprint);
    private sealed record StoredDocument(bool IsGenerated, bool IsExcluded, List<StoredBlock> Blocks);
}
