using System.Globalization;
using System.Security.Cryptography;
using System.Xml;

namespace SlopHawk.Daemon;

// The supervisor supplies both the current context and its receipt. Neither is inferred from
// the collector XML. The caller must verify physical source/build inputs and capture artifact
// bytes under the attempt's owned output root before invoking this pure boundary.
internal sealed record CoverageDocumentAlias(string CollectorPath, string Target, string SourcePath,
    string SourceSha256);

internal sealed record CoverageExpectedRun(Guid Epoch, long AttemptId, string Target, string BuildIdentity,
    string BinaryIdentity, string Collector, string CollectorVersion, string SourceIdentity,
    DateTimeOffset StartedAt, DateTimeOffset Now, TestOutcome Outcome,
    IReadOnlyList<CoverageDocumentAlias> Aliases);

internal sealed record CoberturaReceipt(Guid Epoch, long AttemptId, string Target, string BuildIdentity,
    string BinaryIdentity, string Collector, string CollectorVersion, string SourceIdentity,
    DateTimeOffset CollectedAt, string ArtifactSha256, IReadOnlyList<CoverageDocumentAlias> Aliases);

internal enum CoberturaImportState { Imported, Missing, Malformed, Expired, Mismatched, UnmappedScope }
internal enum CoverageLineState { Unknown, Uncovered, Observed }

internal sealed record CoberturaLine(string Target, string SourcePath, string SourceSha256, int Number, int Hits)
{
    internal CoverageLineState State => Hits == 0 ? CoverageLineState.Uncovered : CoverageLineState.Observed;
}

// The Cobertura condition percentage is an aggregate, not a stable identity for either arm.
internal sealed record CoberturaBranchAggregate(string Target, string SourcePath, int Line,
    int Covered, int Valid);

internal sealed record CoberturaUnknownScope(string CollectorPath, string Reason);

// Root totals describe the whole artifact/run. RunTarget is an execution identity, not
// proof that every aggregated count belongs to a mapped source in that target.
internal sealed record CoberturaRunTotals(Guid Epoch, long AttemptId, string RunTarget, string BuildIdentity,
    string BinaryIdentity, string Collector, string CollectorVersion, string SourceIdentity,
    DateTimeOffset CollectedAt, string ArtifactSha256, TestOutcome Outcome,
    int LinesCovered, int LinesValid, int BranchesCovered, int BranchesValid);

internal sealed record CoberturaImportResult(CoberturaImportState State, string Reason,
    CoberturaRunTotals? Totals, IReadOnlyList<CoberturaLine> Lines,
    IReadOnlyList<CoberturaBranchAggregate> BranchAggregates,
    IReadOnlyList<CoberturaUnknownScope> UnknownScope)
{
    // No counter for an exact target/document/line means unknown, never uncovered.
    internal CoverageLineState LineState(string target, string sourcePath, int line) =>
        Lines.FirstOrDefault(measured => measured.Target == target &&
            measured.SourcePath == sourcePath && measured.Number == line)?.State ?? CoverageLineState.Unknown;
}

internal static class CoberturaCoverageImporter
{
    internal const int MaxArtifactBytes = 1024 * 1024;
    internal const int MaxAliases = 4096;
    internal const int MaxElements = 200_000;
    internal const int MaxClasses = 10_000;
    internal const int MaxLines = 20_000;
    internal const int MaxDepth = 12;
    private const int MaxCount = 1_000_000_000;
    private const int MaxLineNumber = 1_000_000;
    private static readonly TimeSpan MaxReceiptAge = TimeSpan.FromDays(1);
    private static readonly TimeSpan ClockSkew = TimeSpan.FromMinutes(2);

    // A nullable byte payload represents an absent artifact. Empty bytes are malformed.
    // One private snapshot is hashed and then parsed, avoiding a mutation race between them.
    internal static CoberturaImportResult Import(ReadOnlyMemory<byte>? artifact,
        CoberturaReceipt? receipt, CoverageExpectedRun expected)
    {
        ArgumentNullException.ThrowIfNull(expected);
        if (receipt is null || artifact is null)
            return Failure(CoberturaImportState.Missing, "artifact_or_receipt_missing");
        if (artifact.Value.Length is 0 or > MaxArtifactBytes)
            return Failure(CoberturaImportState.Malformed, "artifact_size_invalid");
        if (!ValidContext(expected) || !ValidReceipt(receipt))
            return Failure(CoberturaImportState.Malformed, "receipt_or_context_invalid");

        if (receipt.Epoch != expected.Epoch || receipt.AttemptId != expected.AttemptId ||
            receipt.Target != expected.Target || receipt.BuildIdentity != expected.BuildIdentity ||
            receipt.BinaryIdentity != expected.BinaryIdentity || receipt.Collector != expected.Collector ||
            receipt.CollectorVersion != expected.CollectorVersion || receipt.SourceIdentity != expected.SourceIdentity)
            return Failure(CoberturaImportState.Mismatched, "receipt_identity_mismatch");

        if (receipt.CollectedAt < expected.StartedAt || receipt.CollectedAt > expected.Now + ClockSkew ||
            expected.Now - receipt.CollectedAt > MaxReceiptAge)
            return Failure(CoberturaImportState.Expired, "receipt_outside_current_run");

        // Never adopt a collector-supplied alias or digest. Both copies must match the
        // trusted mappings before any XML line can acquire a source/target identity.
        var trusted = expected.Aliases.ToArray();
        var signed = receipt.Aliases.ToArray();
        if (!MatchingAliases(trusted, signed))
            return Failure(CoberturaImportState.Mismatched, "source_alias_or_digest_mismatch");

        var snapshot = artifact.Value.ToArray();
        if (!CryptographicOperations.FixedTimeEquals(SHA256.HashData(snapshot),
                Convert.FromHexString(receipt.ArtifactSha256)))
            return Failure(CoberturaImportState.Mismatched, "artifact_digest_mismatch");

        try
        {
            return Parse(snapshot, receipt, expected, trusted);
        }
        catch (XmlException)
        {
            return Failure(CoberturaImportState.Malformed, "invalid_cobertura_xml");
        }
        catch (InvalidDataException)
        {
            return Failure(CoberturaImportState.Malformed, "invalid_cobertura_structure_or_counter");
        }
    }

    private static CoberturaImportResult Parse(byte[] snapshot, CoberturaReceipt receipt,
        CoverageExpectedRun expected, CoverageDocumentAlias[] aliases)
    {
        var aliasGroups = aliases.GroupBy(alias => alias.CollectorPath, StringComparer.Ordinal)
            .ToDictionary(group => group.Key, group => group.ToArray(), StringComparer.Ordinal);
        var lines = new List<CoberturaLine>();
        var branches = new List<CoberturaBranchAggregate>();
        var unknown = new Dictionary<string, CoberturaUnknownScope>(StringComparer.Ordinal);
        var classKeys = new HashSet<(string Path, string Class)>();
        var lineKeys = new HashSet<(string Path, int Line)>();
        var measuredKeys = new HashSet<(string Target, string SourcePath, int Line)>();
        var stack = new List<string>();
        var elements = 0;
        var classes = 0;
        var lineNodes = 0;
        var seenPackages = false;
        var seenSources = false;
        string? classAlias = null;
        CoverageDocumentAlias? mapped = null;
        CoberturaRunTotals? totals = null;

        using var stream = new MemoryStream(snapshot, writable: false);
        using var reader = XmlReader.Create(stream, new XmlReaderSettings
        {
            DtdProcessing = DtdProcessing.Prohibit,
            XmlResolver = null,
            ConformanceLevel = ConformanceLevel.Document,
            MaxCharactersInDocument = MaxArtifactBytes,
            MaxCharactersFromEntities = MaxArtifactBytes,
            IgnoreWhitespace = true,
            IgnoreComments = true,
            IgnoreProcessingInstructions = true
        });

        while (reader.Read())
        {
            if (reader.NodeType == XmlNodeType.Element)
            {
                var name = reader.LocalName;
                if (reader.NamespaceURI.Length != 0 || stack.Count >= MaxDepth ||
                    ++elements > MaxElements || !AllowedChild(stack.Count == 0 ? null : stack[^1], name))
                    throw new InvalidDataException();

                if (name == "coverage")
                {
                    var lineCovered = Counter(reader.GetAttribute("lines-covered"));
                    var lineValid = Counter(reader.GetAttribute("lines-valid"));
                    var branchCovered = Counter(reader.GetAttribute("branches-covered"));
                    var branchValid = Counter(reader.GetAttribute("branches-valid"));
                    if (lineCovered > lineValid || branchCovered > branchValid)
                        throw new InvalidDataException();
                    totals = new CoberturaRunTotals(receipt.Epoch, receipt.AttemptId, expected.Target,
                        receipt.BuildIdentity, receipt.BinaryIdentity, receipt.Collector,
                        receipt.CollectorVersion, receipt.SourceIdentity, receipt.CollectedAt,
                        receipt.ArtifactSha256, expected.Outcome, lineCovered, lineValid,
                        branchCovered, branchValid);
                }
                else if (name == "packages")
                {
                    if (seenPackages)
                        throw new InvalidDataException();
                    seenPackages = true;
                }
                else if (name == "sources")
                {
                    if (seenSources)
                        throw new InvalidDataException();
                    seenSources = true;
                }
                else if (name == "class")
                {
                    if (++classes > MaxClasses)
                        throw new InvalidDataException();
                    var className = reader.GetAttribute("name");
                    classAlias = reader.GetAttribute("filename");
                    if (!ValidField(className, 1024) || !ValidAlias(classAlias) ||
                        !classKeys.Add((classAlias!, className!)))
                        throw new InvalidDataException();
                    mapped = null;
                    if (!aliasGroups.TryGetValue(classAlias!, out var entries))
                        unknown.TryAdd(classAlias!, new CoberturaUnknownScope(classAlias!, "unmapped_alias"));
                    else if (entries.Length != 1)
                        unknown.TryAdd(classAlias!, new CoberturaUnknownScope(classAlias!, "ambiguous_alias"));
                    else if (entries[0].Target != expected.Target)
                        unknown.TryAdd(classAlias!, new CoberturaUnknownScope(classAlias!, "other_target"));
                    else
                        mapped = entries[0];
                }
                else if (name == "line")
                {
                    if (++lineNodes > MaxLines)
                        throw new InvalidDataException();
                    var number = Counter(reader.GetAttribute("number"), MaxLineNumber);
                    var hits = Counter(reader.GetAttribute("hits"));
                    var branchFlag = reader.GetAttribute("branch");
                    var condition = reader.GetAttribute("condition-coverage");
                    if (number == 0 || (branchFlag is not null &&
                        (!bool.TryParse(branchFlag, out var branched) || !branched && condition is not null)))
                        throw new InvalidDataException();
                    (int Covered, int Valid)? branchCounts =
                        condition is null ? null : BranchCounts(condition);
                    // Method lines repeat in class/lines in Coverlet Cobertura output. Only
                    // class-level counters are imported; method repetitions are not hits.
                    if (stack.Count == 6 && stack[4] == "class" && stack[5] == "lines")
                    {
                        if (classAlias is null || !lineKeys.Add((classAlias, number)))
                            throw new InvalidDataException();
                        if (mapped is not null)
                        {
                            // Two different collector aliases cannot manufacture two counters for
                            // the same physical document/target/line.
                            if (!measuredKeys.Add((mapped.Target, mapped.SourcePath, number)))
                                throw new InvalidDataException();
                            lines.Add(new CoberturaLine(mapped.Target, mapped.SourcePath,
                                mapped.SourceSha256, number, hits));
                            if (branchCounts is { } aggregate)
                                branches.Add(new CoberturaBranchAggregate(mapped.Target, mapped.SourcePath,
                                    number, aggregate.Covered, aggregate.Valid));
                        }
                    }
                }
                else if (name == "package" && !ValidField(reader.GetAttribute("name"), 1024))
                    throw new InvalidDataException();
                else if (name == "method" &&
                    (!ValidField(reader.GetAttribute("name"), 1024) ||
                     !ValidField(reader.GetAttribute("signature"), 1024)))
                    throw new InvalidDataException();
                else if (name == "condition")
                {
                    _ = Counter(reader.GetAttribute("number"), MaxLineNumber);
                    if (!ValidField(reader.GetAttribute("type")) ||
                        !ValidPercent(reader.GetAttribute("coverage")))
                        throw new InvalidDataException();
                }

                stack.Add(name);
                if (reader.IsEmptyElement)
                {
                    stack.RemoveAt(stack.Count - 1);
                    if (name == "class")
                    {
                        classAlias = null;
                        mapped = null;
                    }
                }
            }
            else if (reader.NodeType == XmlNodeType.EndElement)
            {
                if (stack.Count == 0 || stack[^1] != reader.LocalName)
                    throw new InvalidDataException();
                if (reader.LocalName == "class")
                {
                    classAlias = null;
                    mapped = null;
                }
                stack.RemoveAt(stack.Count - 1);
            }
            else if (reader.NodeType is XmlNodeType.Text or XmlNodeType.CDATA)
            {
                if ((stack.Count == 0 || stack[^1] != "source") &&
                    !string.IsNullOrWhiteSpace(reader.Value))
                    throw new InvalidDataException();
            }
        }
        if (totals is null || !seenPackages || stack.Count != 0)
            throw new InvalidDataException();

        return new CoberturaImportResult(unknown.Count == 0 ? CoberturaImportState.Imported :
            CoberturaImportState.UnmappedScope, unknown.Count == 0 ? "attributed" : "partial_unmapped_scope",
            totals, lines, branches, unknown.Values.OrderBy(issue => issue.CollectorPath, StringComparer.Ordinal).ToArray());
    }

    private static bool AllowedChild(string? parent, string child) => (parent, child) switch
    {
        (null, "coverage") => true,
        ("coverage", "sources" or "packages") => true,
        ("sources", "source") => true,
        ("packages", "package") => true,
        ("package", "classes") => true,
        ("classes", "class") => true,
        ("class", "methods" or "lines") => true,
        ("methods", "method") => true,
        ("method", "lines") => true,
        ("lines", "line") => true,
        ("line", "conditions") => true,
        ("conditions", "condition") => true,
        _ => false
    };

    private static (int Covered, int Valid) BranchCounts(string value)
    {
        // A percentage alone is insufficient to derive exact counter totals.
        if (value.Length > 80)
            throw new InvalidDataException();
        var percentEnd = value.IndexOf("% (", StringComparison.Ordinal);
        if (percentEnd < 1)
            throw new InvalidDataException();
        var slash = value.IndexOf('/', percentEnd + 3);
        if (slash < 0 || !value.EndsWith(')') ||
            !decimal.TryParse(value.AsSpan(0, percentEnd), NumberStyles.AllowDecimalPoint,
                CultureInfo.InvariantCulture, out var percent) || percent is < 0 or > 100 ||
            !int.TryParse(value.AsSpan(percentEnd + 3, slash - percentEnd - 3),
                NumberStyles.None, CultureInfo.InvariantCulture, out var covered) ||
            !int.TryParse(value.AsSpan(slash + 1, value.Length - slash - 2),
                NumberStyles.None, CultureInfo.InvariantCulture, out var valid) ||
            covered < 0 || valid < 0 || covered > valid || valid > MaxCount)
            throw new InvalidDataException();
        return (covered, valid);
    }

    private static bool ValidPercent(string? value) =>
        value is { Length: > 1 and <= 32 } && value.EndsWith('%') &&
        decimal.TryParse(value.AsSpan(0, value.Length - 1), NumberStyles.AllowDecimalPoint,
            CultureInfo.InvariantCulture, out var percent) && percent is >= 0 and <= 100;

    private static int Counter(string? value, int limit = MaxCount) =>
        int.TryParse(value, NumberStyles.None, CultureInfo.InvariantCulture, out var count) &&
        count >= 0 && count <= limit ? count : throw new InvalidDataException();

    private static bool ValidContext(CoverageExpectedRun run) =>
        run.Epoch != Guid.Empty && run.AttemptId > 0 && ValidField(run.Target) &&
        ValidField(run.BuildIdentity) && ValidField(run.BinaryIdentity) &&
        ValidField(run.Collector) && ValidField(run.CollectorVersion) &&
        ValidField(run.SourceIdentity) && run.StartedAt <= run.Now &&
        ValidAliases(run.Aliases);

    private static bool ValidReceipt(CoberturaReceipt receipt) =>
        receipt.Epoch != Guid.Empty && receipt.AttemptId > 0 && ValidField(receipt.Target) &&
        ValidField(receipt.BuildIdentity) && ValidField(receipt.BinaryIdentity) &&
        ValidField(receipt.Collector) && ValidField(receipt.CollectorVersion) &&
        ValidField(receipt.SourceIdentity) && IsSha256(receipt.ArtifactSha256) &&
        ValidAliases(receipt.Aliases);

    private static bool ValidAliases(IReadOnlyList<CoverageDocumentAlias>? entries)
    {
        if (entries is null || entries.Count > MaxAliases)
            return false;
        var unique = new HashSet<CoverageDocumentAlias>();
        var digests = new Dictionary<(string Target, string SourcePath), string>();
        foreach (var entry in entries)
        {
            if (entry is null || !ValidAlias(entry.CollectorPath) ||
                !ValidField(entry.Target) || !ValidSourcePath(entry.SourcePath) ||
                !IsSha256(entry.SourceSha256) || !unique.Add(entry))
                return false;
            var key = (entry.Target, entry.SourcePath);
            if (digests.TryGetValue(key, out var existing) && existing != entry.SourceSha256)
                return false;
            digests[key] = entry.SourceSha256;
        }
        return true;
    }

    private static bool MatchingAliases(CoverageDocumentAlias[] a, CoverageDocumentAlias[] b) =>
        a.Length == b.Length && a.ToHashSet().SetEquals(b);

    private static bool ValidField(string? value, int maxLength = 256) =>
        value is not null && !string.IsNullOrWhiteSpace(value) && value.Length <= maxLength &&
        !value.Any(char.IsControl);

    private static bool ValidAlias(string? value) =>
        value is not null && ValidField(value, 1024) && value.IndexOf('\\') < 0 &&
        !value.Split('/').Any(part => part is "." or "..");

    private static bool ValidSourcePath(string? value) =>
        value is not null && ValidAlias(value) && !value.StartsWith('/') && !value.Contains(':') &&
        !value.Split('/').Any(string.IsNullOrEmpty);

    private static bool IsSha256(string? value) =>
        value is { Length: 64 } && value.All(character =>
            character is >= '0' and <= '9' or >= 'a' and <= 'f' or >= 'A' and <= 'F');

    private static CoberturaImportResult Failure(CoberturaImportState state, string reason) =>
        new(state, reason, null, [], [], []);
}
