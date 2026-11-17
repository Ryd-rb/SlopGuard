using System.Security.Cryptography;
using System.Text;
using System.Text.Json;
using System.Text.Json.Nodes;
using System.Xml;
using System.Xml.Linq;

namespace SlopHawk.RustMigrationQualification;

internal sealed record MatrixCase(string Id, string Command, string Provider, string Workflow, string Format,
    string LegacyArtifact, string NativeArtifact, string LegacyStatus, string NativeStatus);
internal sealed record GateEvidence(string Name, string Platform, string BinarySha256,
    bool Complete, string[] Omissions, string[] ProofSha256);
internal sealed record QualificationInput(int SchemaVersion, string Platform, string BinarySha256,
    string LegacyBinarySha256, MatrixCase[] Matrix, SampleCapture[] Samples, GateEvidence Compatibility, GateEvidence Containment,
    GateEvidence Recovery);
internal sealed record ReleaseReceipt(int SchemaVersion, string Platform, string LegacyBinarySha256,
    string BinarySha256, bool MatrixComplete, bool CompatibilityComplete, bool ContainmentComplete,
    bool RecoveryComplete, bool PerformanceComplete, string[] QualifiedCommands,
    string[] QualifiedFormats, string ReceiptId);
internal sealed record QualificationReport(bool Qualified, string[] Reasons, string[] PassedCases,
    string[] MissingCases, Dictionary<string, double> Metrics, string[] NormalizedMetadataPaths);
internal sealed record QualificationResult(ReleaseReceipt Receipt, QualificationReport Report);

internal static class Qualification
{
    // All commands and protocol methods are separate matrix cells; parity of a
    // standalone clone library does not imply coverage for the shipped CLI/MCP.
    private static readonly string[] RequiredCommands =
    [
        "start", "status", "results", "projects", "recheck", "stop", "mcp",
        "probes.plan", "probes.compare", "probes.methods", "contracts.qualify",
        "paths.build", "paths.plan", "paths.check", "paths.review",
        "check", "check-contracts", "check-observe", "check-retest", "check-review",
        "mcp.initialize", "mcp.tools.list", "mcp.get_findings", "mcp.run_checks", "mcp.workspace_status"
    ];
    private static readonly string[] Providers = ["csharp", "python", "rust", "fluxr"];
    private static readonly string[] Workflows =
        ["discovery", "syntax", "clone-spelling", "project-context", "test-selection", "receipt", "scalar-replay", "reporting"];
    private static readonly string[] Formats =
    [
        "results.schema-v2", "mcp.jsonrpc", "checks.report-v1", "contracts.qualify-v1",
        "paths.plan-v1", "paths.review-v1", "trx-v1", "receipts.v1",
        "inventory.v5", "migration-state.v1"
    ];
    // Only ephemeral run metadata is normalized. Durable run IDs, receipt hashes,
    // revisions, ordering, spans, cause IDs and policy identity remain exact.
    private static readonly string[] MetadataPaths =
        ["$.timing.startedUtc", "$.timing.elapsedMs", "$.telemetry.pid", "$.telemetry.sampleUtc"];
    private const int MaxArtifact = 8 * 1024 * 1024;

    internal static QualificationResult Evaluate(QualificationInput input)
    {
        var reasons = new List<string>();
        var passed = new List<string>();
        var missing = new List<string>();
        var metrics = new Dictionary<string, double>();
        if (input.SchemaVersion != 1 || input.Platform is not ("linux" or "windows") ||
            !Digest(input.BinarySha256) || !Digest(input.LegacyBinarySha256) ||
            input.Matrix is null || input.Samples is null)
            throw new IOException("Unsupported qualification input or missing platform/matrix/samples.");

        var entries = new HashSet<string>(StringComparer.Ordinal);
        var commands = new HashSet<string>(StringComparer.Ordinal);
        var allCasesGood = true;
        var formats = new HashSet<string>(StringComparer.Ordinal);
        foreach (var row in input.Matrix)
        {
            if (row is null || string.IsNullOrWhiteSpace(row.Id) || !entries.Add(row.Id) ||
                row.Command is null || row.Provider is null || row.Workflow is null || row.Format is null ||
                (row.Id.StartsWith("provider:", StringComparison.Ordinal) &&
                 (row.Id != $"provider:{row.Provider}:{row.Workflow}" ||
                  !Providers.Contains(row.Provider) || !Workflows.Contains(row.Workflow))))
            {
                reasons.Add("Matrix contains duplicate, blank or malformed cells.");
                allCasesGood = false;
                continue;
            }
            if (row.LegacyStatus != "complete" || row.NativeStatus != "complete")
            {
                reasons.Add($"{row.Id}: incomplete scope");
                allCasesGood = false;
                continue;
            }
            if (!CompareArtifacts(row.LegacyArtifact, row.NativeArtifact, row.Format, out var reason))
            {
                reasons.Add($"{row.Id}: {reason}");
                allCasesGood = false;
                continue;
            }
            passed.Add(row.Id);
            if (row.Command.Length > 0)
                commands.Add(row.Command);
            if (row.Format.Length > 0)
                formats.Add(row.Format);
        }
        foreach (var command in RequiredCommands.Where(cmd => !commands.Contains(cmd)))
            missing.Add("command:" + command);
        foreach (var provider in Providers)
        foreach (var workflow in Workflows)
            if (!passed.Contains($"provider:{provider}:{workflow}", StringComparer.Ordinal))
                missing.Add($"provider:{provider}:{workflow}");
        foreach (var format in Formats.Where(format => !formats.Contains(format)))
            missing.Add("format:" + format);
        if (missing.Count != 0)
            reasons.Add("Compatibility matrix has missing command, provider workflow or format cells.");

        var compatibility = ValidEvidence(input.Compatibility, "compatibility", input, reasons);
        var containment = ValidEvidence(input.Containment, "containment", input, reasons);
        var recovery = ValidEvidence(input.Recovery, "recovery", input, reasons);
        var performance = EvaluatePerformance(input.Samples, input.Platform, input.BinarySha256,
            input.LegacyBinarySha256, metrics, reasons);
        var matrix = missing.Count == 0 && allCasesGood;
        // A compatibility claim must be made for the *same* measured binary, not
        // inferred from another platform, commit, preview or optimization run.
        var qualified = matrix && compatibility && containment && recovery && performance;
        var receiptId = Convert.ToHexString(SHA256.HashData(Encoding.UTF8.GetBytes(
            $"qualification/1\n{input.Platform}\n{input.LegacyBinarySha256}\n{input.BinarySha256}\n{string.Join(',', passed.Order())}\n{string.Join(',', reasons)}")));
        var receipt = new ReleaseReceipt(1, input.Platform, input.LegacyBinarySha256, input.BinarySha256,
            matrix, compatibility, containment, recovery, performance,
            commands.Where(RequiredCommands.Contains).Select(command => command.Split('.')[0])
                .Distinct(StringComparer.Ordinal).Order().ToArray(), formats.Order().ToArray(), receiptId);
        return new QualificationResult(receipt, new QualificationReport(qualified, reasons.ToArray(),
            passed.ToArray(), missing.ToArray(), metrics, MetadataPaths));
    }

    private static bool ValidEvidence(GateEvidence evidence, string name, QualificationInput input,
        List<string> reasons)
    {
        if (evidence is null || evidence.Name != name || evidence.Platform != input.Platform ||
            evidence.BinarySha256 != input.BinarySha256 || !evidence.Complete ||
            evidence.Omissions is null || evidence.Omissions.Length != 0 ||
            evidence.ProofSha256 is null || evidence.ProofSha256.Length == 0 ||
            evidence.ProofSha256.Any(value => !Digest(value)))
        {
            reasons.Add($"{name} proof is absent, incomplete, platform-mismatched or unbound to the exact binary.");
            return false;
        }
        return true;
    }

    private static bool CompareArtifacts(string legacy, string native, string format, out string reason)
    {
        reason = "mismatch after run-metadata normalization";
        try
        {
            var left = ReadBounded(legacy);
            var right = ReadBounded(native);
            if (format == "trx-v1")
            {
                var leftXml = ReadTrx(left);
                var rightXml = ReadTrx(right);
                // Only the TRX run-level clock timestamps are transient metadata.
                // Test identities, per-test durations, outcomes and counters remain exact.
                foreach (var doc in new[] { leftXml, rightXml })
                    foreach (var times in doc.Descendants().Where(e => e.Name.LocalName == "Times"))
                        foreach (var attr in times.Attributes().Where(a =>
                                     a.Name.LocalName is "creation" or "queuing" or "start" or "finish").ToArray())
                            attr.Remove();
                return XNode.DeepEquals(leftXml, rightXml);
            }
            if (format is "mcp.jsonrpc" or "results.schema-v2" or "checks.report-v1" or
                "contracts.qualify-v1" or "paths.plan-v1" or "paths.review-v1" or
                "receipts.v1" or "inventory.v5" or "migration-state.v1")
            {
                var l = JsonNode.Parse(left) ?? throw new JsonException("Missing legacy JSON.");
                var r = JsonNode.Parse(right) ?? throw new JsonException("Missing native JSON.");
                foreach (var path in MetadataPaths)
                {
                    RemovePath(l, path);
                    RemovePath(r, path);
                }
                return JsonNode.DeepEquals(l, r);
            }
            return left.AsSpan().SequenceEqual(right);
        }
        catch (Exception ex) when (ex is IOException or JsonException or XmlException or InvalidOperationException)
        {
            reason = "unsupported or missing artifact: " + ex.Message;
            return false;
        }
    }

    private static XDocument ReadTrx(byte[] payload)
    {
        using var stream = new MemoryStream(payload);
        using var reader = XmlReader.Create(stream, new XmlReaderSettings
        {
            DtdProcessing = DtdProcessing.Prohibit,
            XmlResolver = null,
            MaxCharactersInDocument = MaxArtifact
        });
        return XDocument.Load(reader);
    }

    private static void RemovePath(JsonNode root, string path)
    {
        var parts = path[2..].Split('.');
        JsonNode? parent = root;
        for (var i = 0; i < parts.Length - 1; i++)
            parent = parent is JsonObject obj && obj.TryGetPropertyValue(parts[i], out var node) ? node : null;
        if (parent is JsonObject last)
            last.Remove(parts[^1]);
    }

    private static byte[] ReadBounded(string file)
    {
        using var stream = File.OpenRead(file);
        if (stream.Length < 0 || stream.Length > MaxArtifact)
            throw new IOException("artifact bytes exceed the comparison bound");
        var buffer = new byte[(int)stream.Length];
        stream.ReadExactly(buffer);
        return buffer;
    }

    private static bool EvaluatePerformance(SampleCapture[] captures, string platform, string binarySha256,
        string legacyBinarySha256, Dictionary<string, double> metrics, List<string> reasons)
    {
        var all = true;
        foreach (var scenario in new[] { "cold", "warm", "idle-5m", "edit", "burst-100", "branch-switch", "rollback" })
        foreach (var engine in new[] { "legacy", "native" })
        {
            var matches = captures.Where(c => c.Scenario == scenario && c.Engine == engine && c.Platform == platform).ToArray();
            if (matches.Length != 1 || matches.Any(c => !c.ScopeComplete || c.ScopeOmissions is null ||
                c.ScopeOmissions.Length != 0 || c.Samples is null || c.Samples.Length < 20 ||
                (engine == "native" && c.BinarySha256 != binarySha256) ||
                (engine == "legacy" && c.BinarySha256 != legacyBinarySha256) ||
                (scenario == "idle-5m" && c.ElapsedMilliseconds < 300_000) ||
                c.Samples.Any(s => s is null || s.Processes < 1 || s.LatencyMilliseconds is < 0 ||
                    (platform == "windows" && (s.PrivateWorkingSetBytes is null || s.CommitBytes is null || s.JobPeakBytes is null)) ||
                    (platform == "linux" && (s.PssBytes is null || s.CgroupPeakBytes is null)))) )
            {
                reasons.Add($"Missing complete {platform} {engine}/{scenario} whole-tree sample scope.");
                all = false;
            }
        }
        if (!all) return false;
        foreach (var engine in new[] { "legacy", "native" })
        {
            var latencyCount = captures.Where(c => c.Engine == engine && c.Platform == platform)
                .SelectMany(c => c.Samples).Count(s => s.LatencyMilliseconds is not null);
            var warmCount = captures.Where(c => c.Engine == engine && c.Platform == platform && c.Scenario == "warm")
                .SelectMany(c => c.Samples).Count(s => s.LatencyMilliseconds is not null);
            if (latencyCount < 20 || warmCount < 20)
            {
                reasons.Add($"Missing at least twenty observed {engine} latency/reconciliation events.");
                all = false;
            }
        }
        if (!all) return false;
        // Every comparison retains the same OS and metric family; Windows private
        // working set, commit and Job peak are never substituted for Linux PSS/cgroup.
        double Resident(SamplePoint s) => platform == "windows" ? s.PrivateWorkingSetBytes!.Value : s.PssBytes!.Value;
        foreach (var engine in new[] { "legacy", "native" })
        {
            var idle = captures.Single(c => c.Scenario == "idle-5m" && c.Engine == engine && c.Platform == platform);
            metrics[engine + ".idle.bytes"] = Median(idle.Samples.Select(Resident));
            var checkers = captures.Where(c => c.Engine == engine && c.Platform == platform &&
                (c.Scenario is "cold" or "warm" or "edit" or "burst-100" or "branch-switch"));
            metrics[engine + ".checkerPeak.bytes"] = checkers.Max(c => c.Samples.Max(Resident));
            var events = captures.Where(c => c.Engine == engine && c.Platform == platform).SelectMany(c => c.Samples)
                .Where(s => s.LatencyMilliseconds is not null).Select(s => s.LatencyMilliseconds!.Value).ToArray();
            metrics[engine + ".p95.ms"] = Percentile(events, 0.95);
            var warm = captures.Single(c => c.Scenario == "warm" && c.Engine == engine && c.Platform == platform);
            metrics[engine + ".warmP95.ms"] = Percentile(warm.Samples.Where(s => s.LatencyMilliseconds is not null)
                .Select(s => s.LatencyMilliseconds!.Value).ToArray(), 0.95);
            metrics[engine + ".commitPeak.bytes"] = platform == "windows"
                ? captures.Where(c => c.Engine == engine).SelectMany(c => c.Samples).Max(s => s.CommitBytes!.Value)
                : captures.Where(c => c.Engine == engine).SelectMany(c => c.Samples).Max(s => s.CgroupPeakBytes!.Value);
            metrics[engine + ".scopePeak.bytes"] = platform == "windows"
                ? captures.Where(c => c.Engine == engine).SelectMany(c => c.Samples).Max(s => s.JobPeakBytes!.Value)
                : captures.Where(c => c.Engine == engine).SelectMany(c => c.Samples).Max(s => s.CgroupPeakBytes!.Value);
        }
        if (metrics["native.idle.bytes"] > metrics["legacy.idle.bytes"] * 0.70 ||
            metrics["native.checkerPeak.bytes"] > metrics["legacy.checkerPeak.bytes"] * 0.70)
            reasons.Add("Native warm idle or checker peak resident memory is not at least 30% lower.");
        if (metrics["native.p95.ms"] > metrics["legacy.p95.ms"] * 1.10)
            reasons.Add("Native p95 latency regressed by more than 10%.");
        if (metrics["native.warmP95.ms"] >= 1000)
            reasons.Add("Native warm reconciliation p95 is not below one second.");
        if (platform == "windows")
        {
            var peakIdle = captures.Where(c => c.Engine == "native" && c.Scenario == "idle-5m" && c.Platform == platform)
                .SelectMany(c => c.Samples).Max(s => s.PrivateWorkingSetBytes!.Value);
            metrics["native.daemonPeakIdle.bytes"] = peakIdle;
            if (peakIdle >= 80 * 1024 * 1024)
                reasons.Add("Native Windows daemon resident private working set is not below 80 MiB.");
        }
        return reasons.All(r => !r.StartsWith("Native ", StringComparison.Ordinal));
    }

    private static double Median(IEnumerable<double> values) => Percentile(values.ToArray(), 0.5);
    private static double Percentile(IReadOnlyCollection<double> values, double percentile)
    {
        var ordered = values.Order().ToArray();
        if (ordered.Length < 20)
            throw new IOException("At least twenty observed samples are required for a p95 gate.");
        return ordered[(int)Math.Ceiling(percentile * ordered.Length) - 1];
    }
    private static bool Digest(string value) => value is { Length: 64 } && value.All(c =>
        c is >= '0' and <= '9' or >= 'A' and <= 'F');
}
