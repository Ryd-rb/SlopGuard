using System.Text;
using System.Text.Json;
using System.Text.Json.Serialization;

namespace SlopHawk.Daemon;

internal sealed record AdditionalFileEvidenceExport(string Status, JsonElement? Witness);

internal sealed record AdditionalFileEvidenceBinding(
    string RootIdentity, string Epoch, long Revision, string OperationId, string CaptureId,
    string TargetId, string MappingIdentity, BuildCollectionMode CollectionMode, string BuildSha256);

internal sealed record AdditionalFileSettingsEntry(string Path, string Sha256, long Length);

internal sealed record AdditionalFileCaptureWitness(
    string Schema, string Provider, AdditionalFileEvidenceBinding Binding,
    BuildIdentityInputs IdentityInputs, BuildIdentityStamp Stamp,
    IReadOnlyList<AdditionalFileSettingsEntry> SettingsFiles, string ObservationStatus);

/// <summary>
/// Optional transport of the exact v1 identity inputs and complete SG-13 settings
/// list retained by one admitted capture. This is not a producer of evaluated
/// AdditionalFiles observations. ProjectEvaluator does not collect that item class.
/// No JSON flag attests authentication, stable whole-capture coherence, aliases,
/// generator consumption or permission to reuse. The Rust adapter requires those
/// applicable facts independently from its trusted caller.
/// </summary>
internal static class AdditionalFileEvidence
{
    internal const string Schema = "slophawk.additional-files-evidence.v1";
    internal const string Provider = "slophawk.managed-build-capture/1";
    internal const int MaxWireBytes = 256 * 1024;
    private const int MaxRecords = 4096;
    private const int MaxFieldBytes = 4096;
    private const int MaxTextBytes = 1024 * 1024;
    private static readonly UTF8Encoding StrictUtf8 = new(false, true);
    private static readonly JsonSerializerOptions Wire = new()
    {
        PropertyNamingPolicy = JsonNamingPolicy.SnakeCaseLower,
        Converters = { new JsonStringEnumConverter(JsonNamingPolicy.SnakeCaseLower, allowIntegerValues: false) }
    };

    // This snapshot is used by v1 Capture as well as retained for export. Cloning
    // only after accounting prevents caller-owned target/list mutation after the
    // capture from silently changing what the sidecar says was hashed.
    internal static BuildIdentityInputs? CopyInputsForWitness(BuildIdentityInputs input,
        IReadOnlyList<BuildArtifactFileEvidence>? settings)
    {
        var budget = new Budget();
        if (!AccountInput(input, budget) || !AccountSettings(settings, budget))
            return null;
        return input with
        {
            Target = input.Target with
            {
                Compile = Copy(input.Target.Compile)!,
                References = Copy(input.Target.References)!
            },
            OrderedCompileInputs = Copy(input.OrderedCompileInputs),
            ReferenceAssemblies = Copy(input.ReferenceAssemblies),
            RuntimeAssets = Copy(input.RuntimeAssets),
            DeclaredOutputs = Copy(input.DeclaredOutputs)
        };
    }

    private static IReadOnlyList<T>? Copy<T>(IReadOnlyList<T>? values) =>
        values is null ? null : Array.AsReadOnly(values.ToArray());

    internal static AdditionalFileEvidenceExport Export(BuildArtifactCaptureResult capture,
        NativeExecutionIdentity identity, string captureId)
    {
        ArgumentNullException.ThrowIfNull(capture);
        ArgumentNullException.ThrowIfNull(identity);
        var input = capture.IdentityInputs;
        var settings = capture.SettingsFiles;
        if (input is null || settings is null)
            return new("capture_witness_unavailable", null);
        var budget = new Budget();
        if (!AccountInput(input, budget) || !AccountSettings(settings, budget) ||
            !AccountStamp(capture.Stamp, budget) ||
            !new[] { identity.RootIdentity, identity.JobId, identity.Context, captureId,
                identity.Epoch.ToString("D"), capture.TargetId, capture.SdkVersion }
                .All(budget.Text))
            return new("budget_exceeded", null);
        if (identity.Epoch == Guid.Empty || identity.CommittedRevision < 0 ||
            !Field(identity.RootIdentity) || !Field(identity.JobId) || !Field(captureId) ||
            identity.Context != input.Target.Id || capture.TargetId != input.Target.Id ||
            capture.SdkVersion != input.Target.SdkVersion)
            return new("binding_mismatch", null);
        if (!capture.Stamp.Complete)
            return new("identity_incomplete", null);
        var recomputed = BuildOutputIdentity.Capture(input);
        if (!recomputed.Complete)
            return new("identity_incomplete", null);
        if (!SameStamp(recomputed, capture.Stamp))
            return new("binding_mismatch", null);
        if (settings.Any(file => file.Status != BuildArtifactObservationStatus.ByteVerified ||
            !CanonicalPath(file.LogicalPath) || !Digest(file.Sha256) || file.Length is null or < 0))
            return new("capture_witness_unavailable", null);
        var digest = BuildArtifactCapture.SettingsDigest(settings);
        if (digest is null || !string.Equals(digest, input.SettingsSha256, StringComparison.OrdinalIgnoreCase))
            return new("binding_mismatch", null);

        // No physical paths, timestamps, partial lists or inferred observations.
        // StringComparer.Ordinal is the UTF-16 ordering of the existing aggregate.
        var files = settings.OrderBy(file => file.LogicalPath, StringComparer.Ordinal)
            .Select(file => new AdditionalFileSettingsEntry(file.LogicalPath, file.Sha256!, file.Length!.Value))
            .ToArray();
        var binding = new AdditionalFileEvidenceBinding(identity.RootIdentity, identity.Epoch.ToString("D"),
            identity.CommittedRevision, identity.JobId, captureId, input.Target.Id,
            input.PathMappingIdentity!, input.CollectionMode, capture.Stamp.Sha256);
        var witness = new AdditionalFileCaptureWitness(Schema, Provider, binding, input, capture.Stamp, files,
            "evaluated_additional_files_unavailable");
        // Preflight bounds all vectors/text before copying, sorting or serialization.
        // Escaped JSON may exceed the smaller transport cap; omit it without changing
        // the complete build fact or its identity/reuse decisions.
        var bytes = JsonSerializer.SerializeToUtf8Bytes(witness, Wire);
        if (bytes.Length > MaxWireBytes)
            return new("budget_exceeded", null);
        using var document = JsonDocument.Parse(bytes);
        return new("captured_settings_only", document.RootElement.Clone());
    }

    internal static JsonElement Attach(JsonElement buildData, BuildArtifactCaptureResult capture,
        NativeExecutionIdentity identity, string captureId)
    {
        // Leave ample room for the worker frame and its existing identity wrapper.
        // Omitting the optional property is itself a missing-evidence disposition.
        if (buildData.ValueKind != JsonValueKind.Object ||
            Encoding.UTF8.GetByteCount(buildData.GetRawText()) > NativeExecutionBridge.MaxFrameBytes / 2)
            return buildData;
        var exported = Export(capture, identity, captureId);
        using var stream = new MemoryStream();
        using (var writer = new Utf8JsonWriter(stream))
        {
            writer.WriteStartObject();
            foreach (var property in buildData.EnumerateObject())
                property.WriteTo(writer);
            writer.WriteString("additionalFileEvidenceStatus", exported.Status);
            writer.WritePropertyName("additionalFileEvidence");
            if (exported.Witness is { } witness)
                witness.WriteTo(writer);
            else
                writer.WriteNullValue();
            writer.WriteEndObject();
        }
        using var document = JsonDocument.Parse(stream.ToArray());
        return document.RootElement.Clone();
    }

    private static bool SameStamp(BuildIdentityStamp left, BuildIdentityStamp right) =>
        left.DomainVersion == right.DomainVersion && left.Sha256 == right.Sha256 &&
        left.Complete == right.Complete && left.Sections.SequenceEqual(right.Sections) &&
        left.Problems.SequenceEqual(right.Problems);

    private static bool Field(string? value) =>
        !string.IsNullOrWhiteSpace(value) && !value.Any(char.IsControl);

    private static bool Digest(string? value) => value is { Length: 64 } &&
        value.All(c => c is >= '0' and <= '9' or >= 'a' and <= 'f' or >= 'A' and <= 'F');

    private static bool CanonicalPath(string path) => Field(path) &&
        !path.Contains('\\') && !path.Contains(':') && !path.StartsWith('/') && !path.EndsWith('/') &&
        path.Split('/').All(part => part.Length > 0 && part is not "." and not ".." &&
            !part.EndsWith(' ') && !part.EndsWith('.'));

    private static bool AccountInput(BuildIdentityInputs input, Budget budget)
    {
        var target = input.Target;
        if (target is null)
            return false;
        foreach (var count in new[] { target.Compile?.Count ?? 0, target.References?.Count ?? 0,
            input.OrderedCompileInputs?.Count ?? 0, input.ReferenceAssemblies?.Count ?? 0,
            input.RuntimeAssets?.Count ?? 0, input.DeclaredOutputs?.Count ?? 0 })
            if (!budget.Records(count))
                return false;
        foreach (var text in new[] { target.Id, target.ProjectPath, target.Framework, target.Configuration,
            target.Platform, target.RuntimeIdentifier, target.SdkVersion, target.EvaluationFingerprint,
            input.PathMappingIdentity, input.SettingsSha256, input.RunnerIdentity, input.AdapterIdentity })
            if (!budget.Text(text))
                return false;
        foreach (var member in target.Compile ?? [])
            if (member is null || !budget.Text(member.Path) || !budget.Text(member.Link))
                return false;
        foreach (var reference in target.References ?? [])
            if (!budget.Text(reference))
                return false;
        foreach (var file in input.OrderedCompileInputs ?? [])
            if (file is null || !budget.Text(file.Path) || !budget.Text(file.Link) || !budget.Text(file.Sha256))
                return false;
        foreach (var files in new[] { input.ReferenceAssemblies, input.RuntimeAssets })
            foreach (var file in files ?? [])
                if (file is null || !budget.Text(file.Path) || !budget.Text(file.Sha256))
                    return false;
        foreach (var file in input.DeclaredOutputs ?? [])
            if (file is null || !budget.Text(file.Path) || !budget.Text(file.Sha256))
                return false;
        return true;
    }

    private static bool AccountSettings(IReadOnlyList<BuildArtifactFileEvidence>? files, Budget budget)
    {
        if (!budget.Records(files?.Count ?? 0))
            return false;
        foreach (var file in files ?? [])
            if (file is null || !budget.Text(file.LogicalPath) || !budget.Text(file.Sha256))
                return false;
        return true;
    }

    private static bool AccountStamp(BuildIdentityStamp stamp, Budget budget)
    {
        if (stamp.Sections is null || stamp.Problems is null ||
            !budget.Records(stamp.Sections.Count) || !budget.Records(stamp.Problems.Count) ||
            !budget.Text(stamp.Sha256))
            return false;
        foreach (var section in stamp.Sections)
            if (section is null || !budget.Text(section.Name) || !budget.Text(section.Sha256))
                return false;
        foreach (var problem in stamp.Problems)
            if (problem is null || !budget.Text(problem.Field))
                return false;
        return true;
    }

    private sealed class Budget
    {
        private int records;
        private int textBytes;

        internal bool Records(int count)
        {
            if (count < 0 || count > MaxRecords - records)
                return false;
            records += count;
            return true;
        }

        internal bool Text(string? text)
        {
            if (text is { Length: > MaxFieldBytes })
                return false;
            int bytes;
            try { bytes = text is null ? 0 : StrictUtf8.GetByteCount(text); }
            catch (EncoderFallbackException) { return false; }
            if (bytes > MaxFieldBytes || bytes > MaxTextBytes - textBytes)
                return false;
            textBytes += bytes;
            return true;
        }
    }
}
