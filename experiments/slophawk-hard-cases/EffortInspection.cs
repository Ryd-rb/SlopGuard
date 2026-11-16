using System.Security.Cryptography;
using SlopHawk.Daemon;

internal sealed record EvidenceRequest(string? Capture, string? Target, string? Source, int Line,
    string[]? Runs, string? Build = null);
internal sealed record ArchivedRun(string Report, VstestProcessFacts Process, RunPacket Receipt);
internal sealed record ProfileEvidence(string Workspace, string Effective, string Consent, string[] Paths,
    Dictionary<string, BuildArtifactManifest> Profiles);

internal static class EffortInspection
{
    internal static void Report(string root, string requestPath, Action<object> emit)
    {
        var request = PilotArchive.Read<EvidenceRequest>(root, requestPath);
        if (request.Capture is not null)
        {
            var result = Coverage(root, request.Capture);
            var source = request.Source ?? throw new InvalidDataException("The request has no source path.");
            var target = request.Target ?? throw new InvalidDataException("The request has no coverage target.");
            var digest = Digest(root, source);
            emit(new { Check = "Coverage", request.Capture, target, source, request.Line, Import = result.State.ToString(),
                result.Reason, CurrentLine = result.LineState(target, source, request.Line, digest).ToString(),
                result.UnknownScope, BranchAggregates = result.BranchAggregates.Where(item => item.Target == target &&
                    item.SourcePath == source && item.Line == request.Line).ToArray() });
        }

        foreach (var path in request.Runs ?? [])
        {
            var archive = PilotArchive.Read<ArchivedRun>(root, path);
            var run = Classify(root, archive);
            string? receiptError = null;
            try
            {
                archive.Receipt.Receipt.Validate(archive.Receipt.Attempt, TestOutcome.Passed, archive.Receipt.AdmittedLease);
                if (archive.Receipt.Receipt.ArtifactSha256 != run.Artifact.Sha256 ||
                    archive.Receipt.Receipt.RunId != run.Artifact.RunId || archive.Receipt.Receipt.Counters != run.Artifact.Counters)
                    receiptError = "The admitted receipt does not match the archived artifact.";
            }
            catch (IOException problem)
            {
                receiptError = problem.Message;
            }
            emit(new { Check = "Run", Record = path, archive.Report, Disposition = run.Disposition.ToString(), run.Reason,
                run.Artifact.Counters, ReceiptValid = receiptError is null, ReceiptError = receiptError });
        }

        if (request.Build is not null)
        {
            var profiles = PilotArchive.Read<ProfileEvidence>(root, request.Build);
            var effective = Profile(root, profiles, profiles.Effective);
            var compared = Profile(root, profiles, profiles.Consent);
            var format = BuildOutputIdentity.Compare(compared, effective);
            var receipt = PilotArchive.Read<ArchivedRun>(root, request.Runs!.Single()).Receipt.Receipt;
            emit(new { Check = "BuildIdentity", profiles.Effective, profiles.Consent, format.Reusable,
                Differences = format.Problems.Select(problem => new { Kind = problem.Kind.ToString(), problem.Field }).ToArray(),
                ReceiptMatchesConsent = receipt.BuildDomainVersion == compared.DomainVersion && receipt.BuildSha256 == compared.Sha256,
                ReceiptMatchesEffective = receipt.BuildDomainVersion == effective.DomainVersion && receipt.BuildSha256 == effective.Sha256 });
        }
    }

    internal static BuildIdentityStamp Profile(string root, ProfileEvidence profiles, string name)
    {
        var checkout = PilotArchive.Child(root, profiles.Workspace);
        var mappings = profiles.Paths.Select(path => new BuildArtifactPathMapping(path, PilotArchive.Child(checkout, path))).ToArray();
        return BuildArtifactCapture.Capture(profiles.Profiles[name], new BuildArtifactReadScope([checkout], mappings, true)).Stamp;
    }

    internal static VstestClassification Classify(string root, ArchivedRun archive)
    {
        var file = new FileInfo(PilotArchive.Child(root, archive.Report));
        if (file.Length > VstestTrxClassifier.MaxArtifactBytes)
            throw new InvalidDataException("TRX exceeds the archive size limit.");
        var facts = archive.Process with { ArtifactLastWriteUtc = file.LastWriteTimeUtc };
        return VstestTrxClassifier.Classify(facts, VstestTrxClassifier.Parse(File.ReadAllBytes(file.FullName)));
    }

    internal static CoberturaImportResult Coverage(string root, string capture)
    {
        var path = PilotArchive.Child(root, capture);
        var packet = PilotArchive.Read<CoveragePacket>(root, capture);
        var lease = packet.Lease with { ResultsDirectory = Path.GetDirectoryName(path)! };
        return CoverageReceiptCapture.Capture(packet.Attempt, lease, new CoverageArtifactDeclaration(packet.Report)).Result;
    }

    internal static string Digest(string root, string relative)
    {
        using var stream = File.OpenRead(PilotArchive.Child(root, relative));
        if (stream.Length > 1024 * 1024)
            throw new InvalidDataException("The input exceeds the archive size limit.");
        return Convert.ToHexString(SHA256.HashData(stream));
    }
}
