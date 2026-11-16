using System.Buffers.Binary;
using System.Security.Cryptography;
using System.Text;

namespace SlopGuard.Daemon;

// Observations are supplied by an authorized build/artifact collector. This component never
// evaluates projects, reads paths, executes tools, or grants permission to reuse an output.
internal sealed record BuildIdentityFile(string Path, string? Sha256);
internal sealed record BuildCompileInput(string Path, string? Link, string? Sha256);

internal enum BuildCollectionMode { Unknown, LiveCheckout, ImmutableSnapshot }
internal enum BuildOutputObservation { Unavailable, ExpectedOnly, ByteVerified }
internal sealed record BuildDeclaredOutput(string Path, string? Sha256, BuildOutputObservation Observation);

internal sealed record BuildIdentityInputs(
    ProjectTarget Target,
    bool ProjectGraphComplete,
    string? PathMappingIdentity,
    bool PathMappingTrusted,
    BuildCollectionMode CollectionMode,
    string? SettingsSha256,
    string? RunnerIdentity,
    string? AdapterIdentity,
    IReadOnlyList<BuildCompileInput>? OrderedCompileInputs,
    IReadOnlyList<BuildIdentityFile>? ReferenceAssemblies,
    IReadOnlyList<BuildIdentityFile>? RuntimeAssets,
    IReadOnlyList<BuildDeclaredOutput>? DeclaredOutputs);

internal enum BuildIdentityProblemKind { Incomplete, Mismatch, UntrustedPath, UnsupportedIdentity }
internal sealed record BuildIdentityProblem(BuildIdentityProblemKind Kind, string Field);
internal sealed record BuildIdentitySection(string Name, string Sha256);
internal sealed record BuildIdentityStamp(int DomainVersion, string Sha256, bool Complete,
    IReadOnlyList<BuildIdentitySection> Sections, IReadOnlyList<BuildIdentityProblem> Problems);
internal sealed record BuildReuseComparison(bool Reusable, IReadOnlyList<BuildIdentityProblem> Problems);

/// <summary>
/// Versioned, culture-independent identity of one evaluated .NET target and its observed build
/// inputs/outputs. Only a complete stamp with byte-verified declared outputs can be reused.
/// Relative logical paths are normalized by this format; the caller must attest that its path
/// mapping and all digests were established independently under an authorized output scope.
/// </summary>
internal static class BuildOutputIdentity
{
    internal const int DomainVersion = 1;
    internal const int MaxProblems = 24;
    private const int MaxEntries = 4096;
    private const int MaxCharacters = 4096;
    private const string Domain = "SlopGuard/SG-03/BuildOutputIdentity/v1";
    private static readonly string[] SectionNames =
    [
        "target", "mapping", "settings", "runner", "adapter", "membership", "compile",
        "project-references", "reference-assemblies", "runtime-assets", "outputs"
    ];

    internal static BuildIdentityStamp Capture(BuildIdentityInputs input)
    {
        ArgumentNullException.ThrowIfNull(input);
        ArgumentNullException.ThrowIfNull(input.Target);
        var problems = new ProblemSet();
        var sections = new List<BuildIdentitySection>();

        Add("target", writer =>
        {
            var target = input.Target;
            writer.Text(ReferenceId(problems, target.Id, "target.id"));
            writer.Text(Path(problems, target.ProjectPath, "target.project"));
            writer.Text(Text(problems, target.Framework, "target.tfm"));
            writer.Text(Text(problems, target.Configuration, "target.configuration"));
            writer.Text(Text(problems, target.Platform, "target.platform"));
            // Empty RID is a known portable build, not an unknown value.
            writer.Text(Text(problems, target.RuntimeIdentifier, "target.rid", required: false));
            writer.Text(Text(problems, target.SdkVersion, "target.sdk"));
            writer.Bool(target.IsTestProject);
            writer.Bool(input.ProjectGraphComplete);
            if (!input.ProjectGraphComplete)
                problems.Add(BuildIdentityProblemKind.Incomplete, "target.graph");
            writer.Text(Digest(problems, target.EvaluationFingerprint, "target.evaluation"));
        });
        Add("mapping", writer =>
        {
            writer.Text(Text(problems, input.PathMappingIdentity, "mapping.identity"));
            writer.Bool(input.PathMappingTrusted);
            writer.Number((int)input.CollectionMode);
            if (!input.PathMappingTrusted)
                problems.Add(BuildIdentityProblemKind.UntrustedPath, "mapping.trust");
            if (!Enum.IsDefined(input.CollectionMode))
                problems.Add(BuildIdentityProblemKind.UnsupportedIdentity, "mapping.collection_mode");
            else if (input.CollectionMode == BuildCollectionMode.Unknown)
                problems.Add(BuildIdentityProblemKind.Incomplete, "mapping.collection_mode");
        });
        Add("settings", writer => writer.Text(Digest(problems, input.SettingsSha256, "settings.digest")));
        Add("runner", writer => writer.Text(Text(problems, input.RunnerIdentity, "runner.identity")));
        Add("adapter", writer => writer.Text(Text(problems, input.AdapterIdentity, "adapter.identity")));

        List<Member>? members = null;
        Add("membership", writer =>
        {
            var compile = input.Target.Compile;
            if (!Count(problems, writer, compile, "target.compile"))
                return;
            members = compile!.Select((entry, index) => new Member(
                    Path(problems, entry.Path, $"target.compile[{index}].path"),
                    OptionalPath(problems, entry.Link, $"target.compile[{index}].link")))
                .OrderBy(entry => entry.Path, StringComparer.Ordinal)
                .ThenBy(entry => entry.Link, StringComparer.Ordinal).ToList();
            foreach (var member in members)
            {
                writer.Text(member.Path);
                writer.Text(member.Link);
            }
            RejectDuplicateMembers(problems, members, "target.compile");
        });

        List<Member>? observed = null;
        Add("compile", writer =>
        {
            var compile = input.OrderedCompileInputs;
            if (!Count(problems, writer, compile, "compile"))
                return;
            observed = new List<Member>(compile!.Count);
            for (var i = 0; i < compile!.Count; i++)
            {
                var entry = compile![i];
                var path = Path(problems, entry.Path, $"compile[{i}].path");
                var link = OptionalPath(problems, entry.Link, $"compile[{i}].link");
                var digest = Digest(problems, entry.Sha256, $"compile[{i}].digest");
                writer.Text(path);
                writer.Text(link);
                writer.Text(digest);
                observed.Add(new Member(path, link));
            }
            RejectDuplicateMembers(problems, observed, "compile");
        });
        if (members is not null && observed is not null &&
            !members.SequenceEqual(observed.OrderBy(entry => entry.Path, StringComparer.Ordinal)
                .ThenBy(entry => entry.Link, StringComparer.Ordinal)))
            problems.Add(BuildIdentityProblemKind.Mismatch, "compile.membership");

        Add("project-references", writer =>
        {
            var references = input.Target.References;
            if (!Count(problems, writer, references, "target.references"))
                return;
            var ids = references!.Select((id, index) => ReferenceId(problems, id, $"target.references[{index}]"))
                .Order(StringComparer.Ordinal).ToArray();
            foreach (var id in ids)
                writer.Text(id);
            if (ids.Distinct(StringComparer.Ordinal).Count() != ids.Length)
                problems.Add(BuildIdentityProblemKind.UnsupportedIdentity, "target.references.duplicate");
        });
        Add("reference-assemblies", writer => WriteFiles(problems, writer, input.ReferenceAssemblies, "references"));
        Add("runtime-assets", writer => WriteFiles(problems, writer, input.RuntimeAssets, "runtime"));
        Add("outputs", writer => WriteOutputs(problems, writer, input.DeclaredOutputs));

        var fingerprint = new Wire();
        fingerprint.Text(Domain);
        fingerprint.Number(DomainVersion);
        foreach (var section in sections)
        {
            fingerprint.Text(section.Name);
            fingerprint.Text(section.Sha256);
        }
        return new BuildIdentityStamp(DomainVersion, fingerprint.Sha256(), problems.Complete,
            sections.ToArray(), problems.Items.ToArray());

        void Add(string name, Action<Wire> write)
        {
            var writer = new Wire();
            writer.Text(Domain);
            writer.Text(name);
            write(writer);
            sections.Add(new BuildIdentitySection(name, writer.Sha256()));
        }
    }

    internal static BuildReuseComparison Compare(BuildIdentityStamp previous, BuildIdentityStamp current)
    {
        ArgumentNullException.ThrowIfNull(previous);
        ArgumentNullException.ThrowIfNull(current);
        var problems = new ProblemSet();
        var priorValid = ValidStamp(previous);
        var nextValid = ValidStamp(current);
        if (!priorValid || !nextValid)
            problems.Add(BuildIdentityProblemKind.UnsupportedIdentity, "stamp.domain_or_structure");
        if (!previous.Complete)
        {
            problems.Add(BuildIdentityProblemKind.Incomplete, "previous");
            foreach (var problem in previous.Problems)
                problems.Add(problem.Kind, problem.Field);
        }
        if (!current.Complete)
        {
            problems.Add(BuildIdentityProblemKind.Incomplete, "current");
            foreach (var problem in current.Problems)
                problems.Add(problem.Kind, problem.Field);
        }
        if (priorValid && nextValid &&
            !string.Equals(previous.Sha256, current.Sha256, StringComparison.Ordinal))
        {
            var prior = previous.Sections.ToDictionary(section => section.Name, section => section.Sha256,
                StringComparer.Ordinal);
            var next = current.Sections.ToDictionary(section => section.Name, section => section.Sha256,
                StringComparer.Ordinal);
            foreach (var name in prior.Keys.Union(next.Keys, StringComparer.Ordinal).Order(StringComparer.Ordinal))
            {
                if (!prior.TryGetValue(name, out var left) || !next.TryGetValue(name, out var right) ||
                    !string.Equals(left, right, StringComparison.Ordinal))
                    problems.Add(BuildIdentityProblemKind.Mismatch, name);
            }
            if (problems.Complete)
                problems.Add(BuildIdentityProblemKind.Mismatch, "fingerprint");
        }
        return new BuildReuseComparison(problems.Complete, problems.Items.ToArray());
    }

    private static bool ValidStamp(BuildIdentityStamp stamp)
    {
        if (stamp.DomainVersion != DomainVersion || !stamp.Sha256.Equals(stamp.Sha256.ToUpperInvariant(),
                StringComparison.Ordinal) || stamp.Sha256.Length != 64 || !IsHex(stamp.Sha256) ||
            stamp.Sections.Count != SectionNames.Length || stamp.Problems.Count > MaxProblems ||
            stamp.Complete == (stamp.Problems.Count != 0))
            return false;

        var fingerprint = new Wire();
        fingerprint.Text(Domain);
        fingerprint.Number(DomainVersion);
        for (var i = 0; i < SectionNames.Length; i++)
        {
            var section = stamp.Sections[i];
            if (section.Name != SectionNames[i] || section.Sha256.Length != 64 ||
                !IsHex(section.Sha256) || section.Sha256 != section.Sha256.ToUpperInvariant())
                return false;
            fingerprint.Text(section.Name);
            fingerprint.Text(section.Sha256);
        }
        return string.Equals(stamp.Sha256, fingerprint.Sha256(), StringComparison.Ordinal);
    }

    private static bool Count<T>(ProblemSet problems, Wire writer, IReadOnlyList<T>? items, string field)
    {
        if (items is null)
        {
            problems.Add(BuildIdentityProblemKind.Incomplete, field);
            writer.Number(-1);
            return false;
        }
        if (items.Count > MaxEntries)
        {
            problems.Add(BuildIdentityProblemKind.UnsupportedIdentity, field + ".budget");
            writer.Number(-2);
            return false;
        }
        writer.Number(items.Count);
        return true;
    }

    private static void WriteFiles(ProblemSet problems, Wire writer, IReadOnlyList<BuildIdentityFile>? files,
        string field)
    {
        if (!Count(problems, writer, files, field))
            return;
        var normalized = files!.Select((file, index) => new FileEntry(
                Path(problems, file.Path, $"{field}[{index}].path"),
                Digest(problems, file.Sha256, $"{field}[{index}].digest")))
            .OrderBy(file => file.Path, StringComparer.Ordinal)
            .ThenBy(file => file.Digest, StringComparer.Ordinal).ToArray();
        foreach (var file in normalized)
        {
            writer.Text(file.Path);
            writer.Text(file.Digest);
        }
        for (var i = 1; i < normalized.Length; i++)
            if (normalized[i - 1].Path is not null && normalized[i - 1].Path == normalized[i].Path)
                problems.Add(BuildIdentityProblemKind.UnsupportedIdentity, field + ".duplicate");
    }

    private static void WriteOutputs(ProblemSet problems, Wire writer, IReadOnlyList<BuildDeclaredOutput>? outputs)
    {
        if (!Count(problems, writer, outputs, "outputs"))
            return;
        if (outputs!.Count == 0)
            problems.Add(BuildIdentityProblemKind.Incomplete, "outputs.empty");
        var normalized = outputs!.Select((output, index) => new OutputEntry(
                Path(problems, output.Path, $"outputs[{index}].path"),
                Digest(problems, output.Sha256, $"outputs[{index}].digest"),
                output.Observation))
            .OrderBy(output => output.Path, StringComparer.Ordinal)
            .ThenBy(output => output.Digest, StringComparer.Ordinal)
            .ThenBy(output => output.Observation).ToArray();
        foreach (var output in normalized)
        {
            writer.Text(output.Path);
            writer.Text(output.Digest);
            writer.Number((int)output.Observation);
            if (!Enum.IsDefined(output.Observation))
                problems.Add(BuildIdentityProblemKind.UnsupportedIdentity, "outputs.observation");
            else if (output.Observation != BuildOutputObservation.ByteVerified)
                problems.Add(BuildIdentityProblemKind.Incomplete, "outputs.not_byte_verified");
        }
        for (var i = 1; i < normalized.Length; i++)
            if (normalized[i - 1].Path is not null && normalized[i - 1].Path == normalized[i].Path)
                problems.Add(BuildIdentityProblemKind.UnsupportedIdentity, "outputs.duplicate");
    }

    private static void RejectDuplicateMembers(ProblemSet problems, List<Member> members, string field)
    {
        var sorted = members.OrderBy(member => member.Path, StringComparer.Ordinal)
            .ThenBy(member => member.Link, StringComparer.Ordinal).ToArray();
        for (var i = 1; i < sorted.Length; i++)
            if (sorted[i].Path is not null && sorted[i] == sorted[i - 1])
                problems.Add(BuildIdentityProblemKind.UnsupportedIdentity, field + ".duplicate");
    }

    private static string? ReferenceId(ProblemSet problems, string? id, string field)
    {
        if (id is { Length: 64 } && IsHex(id))
            return id.ToUpperInvariant(); // ProjectEvaluator hashes unusually long target IDs.
        var split = id?.LastIndexOf('|') ?? -1;
        if (split <= 0 || split == id!.Length - 1)
        {
            problems.Add(id is null ? BuildIdentityProblemKind.Incomplete : BuildIdentityProblemKind.UnsupportedIdentity,
                field);
            return null;
        }
        var path = Path(problems, id![..split], field + ".path");
        var tfm = Text(problems, id![(split + 1)..], field + ".tfm");
        return path is null || tfm is null ? null : path + "|" + tfm;
    }

    private static string? OptionalPath(ProblemSet problems, string? value, string field) =>
        value is null ? null : Path(problems, value, field);

    private static string? Path(ProblemSet problems, string? value, string field)
    {
        var text = Text(problems, value, field);
        if (text is null)
            return null;
        text = text.Replace('\\', '/');
        if (text.StartsWith('/') || text.EndsWith('/') || text.Contains(':') ||
            text.Split('/').Any(segment => segment is "" or "." or ".." ||
                segment.EndsWith(' ') || segment.EndsWith('.')) ||
            text.Any(char.IsControl))
        {
            problems.Add(BuildIdentityProblemKind.UntrustedPath, field);
            return null;
        }
        return text;
    }

    private static string? Digest(ProblemSet problems, string? value, string field)
    {
        if (string.IsNullOrEmpty(value))
        {
            problems.Add(BuildIdentityProblemKind.Incomplete, field);
            return null;
        }
        if (value.Length != 64 || !IsHex(value))
        {
            problems.Add(BuildIdentityProblemKind.UnsupportedIdentity, field);
            return null;
        }
        return value.ToUpperInvariant();
    }

    private static bool IsHex(string value) => value.All(character =>
        character is >= '0' and <= '9' or >= 'a' and <= 'f' or >= 'A' and <= 'F');

    private static string? Text(ProblemSet problems, string? value, string field, bool required = true)
    {
        if (value is null || required && value.Length == 0)
        {
            problems.Add(BuildIdentityProblemKind.Incomplete, field);
            return null;
        }
        if (value.Length > MaxCharacters || value.Any(char.IsControl) || HasUnpairedSurrogate(value))
        {
            problems.Add(BuildIdentityProblemKind.UnsupportedIdentity, field);
            return null;
        }
        // Logical identities retain their spelling. Composed and decomposed paths
        // can identify different files on a case-sensitive filesystem.
        return value;
    }

    private static bool HasUnpairedSurrogate(string value)
    {
        for (var i = 0; i < value.Length; i++)
        {
            if (char.IsHighSurrogate(value[i]))
            {
                if (i + 1 == value.Length || !char.IsLowSurrogate(value[i + 1]))
                    return true;
                i++;
            }
            else if (char.IsLowSurrogate(value[i]))
                return true;
        }
        return false;
    }

    private sealed record Member(string? Path, string? Link);
    private sealed record FileEntry(string? Path, string? Digest);
    private sealed record OutputEntry(string? Path, string? Digest, BuildOutputObservation Observation);

    private sealed class ProblemSet
    {
        private readonly List<BuildIdentityProblem> items = [];
        internal bool Complete { get; private set; } = true;
        internal IReadOnlyList<BuildIdentityProblem> Items => items;

        internal void Add(BuildIdentityProblemKind kind, string field)
        {
            Complete = false;
            var problem = new BuildIdentityProblem(kind, field);
            if (items.Count < MaxProblems && !items.Contains(problem))
                items.Add(problem);
        }
    }

    // Each string uses an explicit big-endian UTF-8 byte length; null uses -1.
    // Counts and booleans have explicit 32-bit encodings, with no delimiters to escape,
    // culture-dependent formatting, JSON property order, or native-endian ambiguity.
    private sealed class Wire
    {
        private readonly MemoryStream stream = new();

        internal void Text(string? value)
        {
            if (value is null)
            {
                Number(-1);
                return;
            }
            var bytes = Encoding.UTF8.GetBytes(value);
            Number(bytes.Length);
            stream.Write(bytes);
        }

        internal void Bool(bool value) => Number(value ? 1 : 0);

        internal void Number(int value)
        {
            Span<byte> bytes = stackalloc byte[4];
            BinaryPrimitives.WriteInt32BigEndian(bytes, value);
            stream.Write(bytes);
        }

        internal string Sha256() => Convert.ToHexString(SHA256.HashData(stream.ToArray()));
    }
}
