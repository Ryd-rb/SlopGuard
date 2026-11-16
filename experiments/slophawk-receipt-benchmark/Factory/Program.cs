using System.Security.Cryptography;
using System.Text;
using System.Xml.Linq;
using SlopHawk.Daemon;

if (args is not [var root, var dotnet, var sdk, var passing, var failing] || !Path.IsPathFullyQualified(root))
{
    Console.Error.WriteLine("Usage: factory NEW_ABSOLUTE_ROOT DOTNET_PATH SDK_VERSION PASS_DLL FAIL_DLL");
    return 2;
}

if (Directory.Exists(root) || File.Exists(root))
    throw new IOException("The experiment requires a fresh destination.");
Directory.CreateDirectory(root);

var pass = await Record(root, dotnet, sdk, passing, TestOutcome.Passed, 411);
var fail = await Record(root, dotnet, sdk, failing, TestOutcome.Failed, 438);
var names = new[] { "Arc", "Basin", "Cairn", "Dune", "Elmwood", "Ford" };
var oracleRoot = Directory.CreateDirectory(Path.Combine(root, "oracles")).FullName;
for (var index = 0; index < names.Length; index++)
{
    var dossier = Directory.CreateDirectory(Path.Combine(root, "dossiers", names[index])).FullName;
    var cases = new[]
    {
        "build-bytes", "build-closure", "build-control",
        "run-binding", "run-artifact", "run-control",
        "coverage-provenance", "coverage-scope", "coverage-control"
    };
    new Random(88177 + index).Shuffle(cases);

    var items = new List<AuditItem>();
    var oracle = new List<Oracle>();
    Directory.CreateDirectory(Path.Combine(dossier, "packets"));
    for (var number = 0; number < cases.Length; number++)
    {
        var id = $"Q{number + 1:D2}";
        var packet = Directory.CreateDirectory(Path.Combine(dossier, "packets", id)).FullName;
        var kind = cases[number];
        var (claim, expected) = kind.Split('-')[0] switch
        {
            "build" => MakeBuild(packet, kind, index, sdk, passing),
            "run" => MakeRun(packet, kind, index, pass, fail),
            "coverage" => MakeCoverage(packet, kind, index),
            _ => throw new InvalidOperationException("Unknown fixture kind.")
        };
        var item = new AuditItem(id, kind.Split('-')[0], claim);
        var observed = Evidence.Observe(packet, item.Component);
        if (observed.State != expected.Observation ||
            (expected.Diagnostic.Length != 0 && !observed.Facts.Any(fact => fact.Contains(expected.Diagnostic, StringComparison.Ordinal))))
            throw new InvalidOperationException($"{names[index]}/{id}: expected {expected.Observation}/{expected.Diagnostic}; " +
                $"observed {observed.State} / {string.Join(" | ", observed.Facts)}");

        items.Add(item);
        oracle.Add(expected with { Id = id, Component = item.Component });
        Console.WriteLine($"{names[index]}/{id}: {observed.State} ({expected.Cause})");
    }

    Evidence.Write(dossier, "index.json", new AuditIndex(items.ToArray()));
    Evidence.Write(oracleRoot, names[index] + ".json", oracle.ToArray());
    File.WriteAllText(Path.Combine(dossier, "README.md"), Handbook(names[index], items));
}
Console.WriteLine("Qualified six dossiers and two genuine VSTest outcomes.");
return 0;

static (string, Oracle) MakeBuild(string packet, string kind, int ordinal, string sdk, string assembly)
{
    var paths = new[] { "inputs/Window.cs", "inputs/Queue.cs", "rules/runtime.json", "settings/verify.runsettings", "out/GateSpecimen.dll" };
    var labels = new[] { "compile", "compile", "runtime-assets", "settings" };
    for (var i = 0; i < paths.Length; i++)
        Directory.CreateDirectory(Path.GetDirectoryName(Path.Combine(packet, "current", paths[i]))!);
    File.WriteAllText(Path.Combine(packet, "current", paths[0]), "internal static class Window { internal const int Days = 16; }\n");
    File.WriteAllText(Path.Combine(packet, "current", paths[1]), "internal static class Queue { internal const int Limit = 16; }\n");
    File.WriteAllText(Path.Combine(packet, "current", paths[2]), "{\"days\":16}\n");
    File.WriteAllText(Path.Combine(packet, "current", paths[3]),
        "<RunSettings><RunConfiguration><MaxCpuCount>1</MaxCpuCount></RunConfiguration></RunSettings>\n");
    File.Copy(assembly, Path.Combine(packet, "current", paths[4]));
    var target = Target(sdk) with
    {
        Compile = [new(paths[0], null), new(paths[1], "Shared/Queue.cs")]
    };
    var baseline = new BuildArtifactManifest(target, true, "archived-dossier/v1", "vstest/sdk", "xunit",
        [new(paths[0], null), new(paths[1], "Shared/Queue.cs")],
        [], [paths[2]], [paths[3]], [], [paths[4]]);
    var prior = Evidence.Capture(packet, new BuildInputs(baseline, paths));
    if (!prior.Stamp.Complete)
        throw new InvalidOperationException("Incomplete reference capture: " + string.Join(", ", prior.Stamp.Problems));

    Evidence.Write(packet, "prior-stamp.json", prior.Stamp);
    Evidence.Write(packet, "prior-manifest.json", baseline);
    Evidence.Write(packet, "prior-file-observations.json", prior.Evidence.Select(file => new
    {
        file.LogicalPath, file.Sha256, file.Length, file.LastWriteUtc
    }).ToArray());

    var current = baseline;
    var expected = kind switch
    {
        "build-bytes" => ChangeBytes(),
        "build-closure" => ChangeManifest(),
        "build-control" => Retimestamp(),
        _ => throw new InvalidOperationException("Unknown build fixture.")
    };
    Evidence.Write(packet, "current.json", new BuildInputs(current, paths));
    return ("The stored build is reusable for the current closed target. Matching lengths and times may support the handoff.", expected);

    Oracle ChangeBytes()
    {
        var selected = new[] { 3, 2, 1, 0, 3, 2 }[ordinal];
        var path = Path.Combine(packet, "current", paths[selected]);
        var original = File.ReadAllBytes(path);
        var timestamp = File.GetLastWriteTimeUtc(path);
        var replacement = Encoding.UTF8.GetString(original).Replace(selected == 3 ? ">1<" : "16",
            selected == 3 ? ">2<" : "17", StringComparison.Ordinal);
        File.WriteAllText(path, replacement);
        File.SetLastWriteTimeUtc(path, timestamp);
        var revised = File.ReadAllBytes(path);
        if (original.SequenceEqual(revised) || original.Length != revised.Length || File.GetLastWriteTimeUtc(path) != timestamp)
            throw new IOException("The synthetic former-stamp control did not preserve length and timestamp.");

        return new Oracle("", "", true, $"{paths[selected]} bytes differ despite equal length and timestamp",
            "Mismatch", $"Mismatch/{labels[selected]}");
    }

    Oracle ChangeManifest()
    {
        switch (ordinal % 3)
        {
            case 0:
                current = baseline with { OrderedCompileInputs = baseline.OrderedCompileInputs!.Reverse().ToArray() };
                return new Oracle("", "", true, "the ordered Compile list was reversed with unchanged paths and bytes",
                    "Mismatch", "Mismatch/compile");
            case 1:
                current = baseline with { AdapterFiles = null };
                return new Oracle("", "", true, "the current adapter closure is null/unknown, while prior was known-empty",
                    "Incomplete", "Incomplete/adapter");
            default:
                current = baseline with
                {
                    Target = target with { Compile = [new(paths[0], null), new(paths[1], "Services/Queue.cs")] },
                    OrderedCompileInputs = [new(paths[0], null), new(paths[1], "Services/Queue.cs")]
                };
                return new Oracle("", "", true, "the linked Queue.cs logical identity moved from Shared to Services",
                    "Mismatch", "Mismatch/compile");
        }
    }

    Oracle Retimestamp()
    {
        var path = Path.Combine(packet, "current", paths[0]);
        File.SetLastWriteTimeUtc(path, File.GetLastWriteTimeUtc(path).AddHours(2));
        var expectedHash = prior.Evidence.Single(file => file.LogicalPath == paths[0]).Sha256;
        if (Digest(path) != expectedHash)
            throw new IOException("Timestamp-only change unexpectedly modified bytes.");
        File.WriteAllText(Path.Combine(packet, "current", "operator-note.txt"), "Regenerated by the archive transport.");
        return new Oracle("", "", false, "timestamp and unbound notes changed while the verified closure and bytes matched",
            "Reusable", "");
    }
}

static (string, Oracle) MakeRun(string packet, string kind, int ordinal, SeedRun pass, SeedRun fail)
{
    var seed = kind == "run-control" && ordinal is 1 or 3 or 5 ? fail : pass;
    var input = seed.Input;
    var receipt = seed.Receipt;
    var expected = kind switch
    {
        "run-binding" => InvalidateBinding(),
        "run-artifact" => BreakArchive(),
        "run-control" => new Oracle("", "", false, "the recorded outcome agrees with the actual TRX and the admitted attempt",
            "ReceiptMatches", $"replayed={receipt.Platform}"),
        _ => throw new InvalidOperationException("Unknown run fixture.")
    };
    var path = Path.Combine(packet, "run.trx");
    File.Copy(seed.Trx, path);
    File.SetLastWriteTimeUtc(path, File.GetLastWriteTimeUtc(seed.Trx));
    Evidence.Write(packet, "admission.json", input);
    Evidence.Write(packet, "receipt.json", receipt);

    var wording = seed.Input.Outcome == TestOutcome.Passed ? "pass" : "completed test failure";
    return ($"The current admission may retain this durable receipt as a verified {wording} for its archived run.", expected);

    Oracle InvalidateBinding()
    {
        var fields = new[] { "fence sequence", "profile revision", "Git identity", "input fingerprint", "epoch", "lease identity" };
        receipt = ordinal switch
        {
            0 => receipt with { FenceSequence = receipt.FenceSequence + 1 },
            1 => receipt with { ProfileRevision = "profile-previous" },
            2 => receipt with { GitFingerprint = new string('E', 64) },
            3 => receipt with { InputFingerprint = new string('F', 64) },
            4 => receipt with { Epoch = Guid.NewGuid() },
            _ => receipt with { LeaseId = "legacy-lease" }
        };
        return new Oracle("", "", true, $"archived receipt has a different {fields[ordinal]} from the admitted attempt",
            "ReceiptRejected", "provenance");
    }

    Oracle BreakArchive()
    {
        if (ordinal % 3 == 0)
        {
            receipt = receipt with { ArtifactSha256 = new string('F', 64) };
            return new Oracle("", "", true, "the archived TRX bytes have a different SHA-256 from the stored receipt",
                "ArchiveDisagrees", "stored receipt disagrees");
        }
        if (ordinal % 3 == 1)
        {
            var time = input.Process.StartedUtc.AddMinutes(5);
            input = input with { Process = input.Process with { StartedUtc = time, FinishedUtc = time.AddSeconds(4) } };
            return new Oracle("", "", true, "the archived TRX time window predates the recorded process attempt",
                "ArchiveDisagrees", "artifact=Stale");
        }

        receipt = receipt with { RunId = Guid.NewGuid() };
        return new Oracle("", "", true, "the actual TRX run id differs from the durable receipt run id",
            "ArchiveDisagrees", "stored receipt disagrees");
    }
}

static (string, Oracle) MakeCoverage(string packet, string kind, int ordinal)
{
    const string document = "src/Renewal.cs";
    const string mapped = "maps/src/Renewal.cs";
    const string relative = "collector/coverage.cobertura.xml";
    const string build = "config-17";
    const string binary = "binary-17";
    var root = Directory.CreateDirectory(Path.Combine(packet, "run", "collector")).Parent!.FullName;
    var now = DateTimeOffset.UtcNow;
    var epoch = Guid.NewGuid();
    var outcome = kind == "coverage-control" && ordinal % 2 == 0 ? TestOutcome.Failed : TestOutcome.Passed;
    var branch = kind == "coverage-scope";
    var line = kind == "coverage-provenance" && ordinal == 4 ? 13 : 12;
    var hits = kind == "coverage-control" && ordinal % 2 == 1 ? 0 : 7;
    Directory.CreateDirectory(Path.Combine(packet, "src"));
    var source = Path.Combine(packet, document);
    File.WriteAllText(source, """
        namespace Renewal;

        internal static class Rule
        {
            internal static int Window(bool extended)
            {
                // A frozen logical document from the collector's mapped build.
                const int normal = 16;
                const int exceptional = 24;

                var limit = normal;
                if (extended) limit = exceptional;
                return limit;
            }
        }
        """);
    var attempt = new CoverageSupervisedAttempt(epoch, 265 + ordinal, "unit-net10", build, binary,
        "coverlet", "collected-1", "mapping-17", now.AddMinutes(-1), now.AddSeconds(1), now.AddSeconds(2),
        outcome, true, true, [new CoverageDocumentAlias(mapped, "unit-net10", document, Digest(source))]);
    var lease = new CoverageResultsLease(epoch, attempt.AttemptId, attempt.Target, build, binary, attempt.Collector,
        attempt.CollectorVersion, attempt.SourceIdentity, "run", now.AddSeconds(-30), true);
    var xmlPath = mapped;
    var stale = false;
    string claim;
    Oracle expected;

    if (kind == "coverage-provenance")
    {
        claim = $"The run supplies an attributable current coverage observation for {document}:{line}.";
        switch (ordinal)
        {
            case 0:
                stale = true;
                expected = new Oracle("", "", true, "report modification time predates the supervised attempt",
                    "Expired", "coverage_artifact_not_fresh");
                break;
            case 1:
                lease = lease with { BinaryIdentity = "binary-16" };
                expected = new Oracle("", "", true, "output lease belongs to a different binary than the admitted attempt",
                    "Mismatched", "attempt_results_lease_mismatch");
                break;
            case 2:
                xmlPath = "reports/duplicate/Renewal.cs";
                expected = new Oracle("", "", true, "the collector filename lacks an independently verified exact alias",
                    "UnmappedScope", "Renewal.cs:12=Unknown");
                break;
            case 3:
                lease = lease with { OwnedAndDrained = false };
                expected = new Oracle("", "", true, "the results lease is not proven drained",
                    "Missing", "results_lease_not_drained");
                break;
            case 4:
                expected = new Oracle("", "", true, "the verified Cobertura file has a counter only for line 12, so line 13 is unknown",
                    "Imported", "Renewal.cs:13=Unknown");
                break;
            default:
                attempt = attempt with { CollectorAvailable = false };
                expected = new Oracle("", "", true, "collector availability for the admitted attempt is false",
                    "Missing", "collector_unavailable");
                break;
        }
    }
    else if (branch)
    {
        claim = $"The Cobertura entry specifically proves that the extended branch ran at {document}:12.";
        expected = new Oracle("", "", true, "the mapped aggregate of 1 covered out of 2 branches does not identify either arm",
            "Imported", "branches=1/2 (aggregate)");
    }
    else if (hits == 0)
    {
        claim = $"An attributable zero-hit counter marks {document}:12 uncovered for this run.";
        expected = new Oracle("", "", false, "a mapped counter of zero is uncovered, rather than unknown",
            "Imported", "Renewal.cs:12=Uncovered");
    }
    else
    {
        claim = $"The completed failed test run supplies a verified line hit at {document}:12.";
        expected = new Oracle("", "", false, "matching failed attempt still supplies its observed line without asserting pass",
            "Imported", "run outcome=Failed");
    }

    var artifact = Path.Combine(root, relative);
    var branchAttributes = branch ? " branch=\"true\" condition-coverage=\"50% (1/2)\"" : "";
    File.WriteAllText(artifact, $"""
        <coverage lines-covered="{(hits > 0 ? 1 : 0)}" lines-valid="1" branches-covered="{(branch ? 1 : 0)}"
          branches-valid="{(branch ? 2 : 0)}" version="1.9">
          <sources><source>/workspace/legacy-source</source></sources>
          <packages><package name="unit"><classes><class name="Rule" filename="{xmlPath}">
            <lines><line number="12" hits="{hits}"{branchAttributes}/></lines>
          </class></classes></package></packages>
        </coverage>
        """);
    if (stale)
        File.SetLastWriteTimeUtc(artifact, attempt.StartedAt.AddMinutes(-5).UtcDateTime);
    Evidence.Write(packet, "capture.json", new CoverageInputs(attempt, lease, relative, document, line));
    return (claim, expected);
}

static async Task<SeedRun> Record(string root, string hostPath, string sdk, string assemblyPath, TestOutcome outcome, long id)
{
    var target = Target(sdk);
    var leaseRoot = Directory.CreateDirectory(Path.Combine(root, "native", id.ToString(), "results")).FullName;
    var currentWorkspace = Directory.CreateDirectory(Path.Combine(root, "native", id.ToString(), "workspace")).FullName;
    var workspace = new Workspace(currentWorkspace, Path.Combine(root, "native", "state"));
    var rootIdentity = RootIdentity.Read(workspace.Root);
    var attempt = new TestAttempt(id, 1, new ScheduleFence(Guid.NewGuid(), 23, 9, rootIdentity,
        new GitRevision("refs/heads/frozen-release", new string('b', 40))),
        new SelectedTarget(target.Id, new string('A', 64), ["release-gate"]), "frozen-release");
    var grant = new ExecutionGrant(attempt.ProfileRevision, rootIdentity, ExecutionOperation.Test, [leaseRoot], FromProfile: false);
    var (worker, waiting) = new WorkerAdmission(workspace).TryAcquire(grant,
        new WorkerRequirements(ExecutionOperation.Test, true, [leaseRoot]));
    if (worker is null)
        throw new IOException($"Cannot acquire the owned VSTest worker: {waiting}");
    await using var ownedWorker = worker;

    var assembly = Verified(assemblyPath);
    var companions = Directory.EnumerateFiles(Path.GetDirectoryName(assemblyPath)!)
        .Where(path => !string.Equals(path, assemblyPath, StringComparison.OrdinalIgnoreCase)).Select(Verified).ToArray();
    var mappings = companions.Prepend(assembly).Select(file =>
        new BuildArtifactPathMapping("out/" + Path.GetFileName(file.Path), file.Path)).ToArray();
    var manifest = new BuildArtifactManifest(target, true, "frozen-runner/v1", "VSTest", "xunit", [], [], [], [], [],
        mappings.Select(mapping => mapping.LogicalPath).ToArray());
    var build = BuildArtifactCapture.Capture(manifest,
        new BuildArtifactReadScope([Path.GetDirectoryName(assemblyPath)!], mappings, true));
    var host = Verified(hostPath);
    var runner = Verified(Path.Combine(Path.GetDirectoryName(hostPath)!, "sdk", sdk, "vstest.console.dll"));
    var request = new VstestAttemptRequest(attempt, host, runner, sdk, build.Stamp, build, assembly, companions,
        leaseRoot, TimeSpan.FromSeconds(45));
    var result = await VstestAttemptRunner.RunAsync(request, worker, CancellationToken.None);
    var typed = result.Classification ?? throw new IOException($"No verified native result: {result.Gate} {result.Reason}");
    var expected = outcome == TestOutcome.Passed ? VstestDisposition.Passed : VstestDisposition.Failed;
    if (result.Gate != VstestAttemptGate.Launched || typed.Disposition != expected ||
        typed.Artifact.Health != VstestArtifactHealth.Complete || result.Process is not { Exited: true })
        throw new IOException($"Prebuilt test outcome {typed.Disposition}, wanted {expected}. {typed.Reason} {result.StandardError}");

    var trx = Path.Combine(result.ResultsDirectory!, "results.trx");
    XNamespace ns = "http://microsoft.com/schemas/VisualStudio/TeamTest/2010";
    var counters = XDocument.Load(trx).Descendants(ns + "Counters").Single();
    var independentlyFailed = (int?)counters.Attribute("failed");
    if (independentlyFailed != (outcome == TestOutcome.Passed ? 0 : 1) || (int?)counters.Attribute("executed") != 2)
        throw new IOException("Independent XML counts disagree with the requested specimen outcome.");
    var lease = "journal-test-attempt";
    var receipt = new TestRunReceiptDetails(attempt.Id, attempt.Fence.Revision, attempt.Fence.Epoch, attempt.Fence.Sequence,
        TestRunReceiptDetails.GitIdentity(attempt.Fence.Git), attempt.Fence.RootIdentity, target.Id,
        attempt.Target.InputFingerprint, attempt.ProfileRevision, lease, build.Stamp.DomainVersion, build.Stamp.Sha256,
        runner.Sha256, typed.Artifact.Sha256, typed.Artifact.ByteLength, typed.Artifact.RunId, typed.Disposition,
        typed.Runner, typed.Artifact.Health, typed.Execution, typed.Artifact.Counters, "archived_vstest_result");
    receipt.Validate(attempt, outcome, lease);
    Console.WriteLine($"Native Windows VSTest attempt {id}: {typed.Disposition}; executed 2, failed {independentlyFailed}.");
    return new SeedRun(new RunInputs(attempt, outcome, lease, result.Process), receipt, trx);
}

static ProjectTarget Target(string sdk) =>
    new("tests/GateSpecimen.csproj|net10.0", "tests/GateSpecimen.csproj", "net10.0", "Debug", "AnyCPU",
        "", sdk, true, [], [], new string('A', 64));

static VstestVerifiedFile Verified(string path) => new(Path.GetFullPath(path), Digest(path));
static string Digest(string path) => Convert.ToHexString(SHA256.HashData(File.ReadAllBytes(path)));

static string Handbook(string name, List<AuditItem> items)
{
    var builder = new StringBuilder($"""
        # {name} release-evidence audit

        The rollout board needs an independent audit of nine handoff claims.
        Treat the archive as read-only and examine the exact build or attempt for
        each row. The packet folder and index are the raw records. The build
        and coverage descriptors are the archived supervisor's scope declarations.
        They are not derived from the claims, and their absence or unknown fields
        can invalidate the evidence. Coverage is evaluated as of the archived time.

        | Claim | Evidence family | Statement to audit |
        | --- | --- | --- |

        """);
    foreach (var item in items)
        builder.AppendLine($"| {item.Id} | {item.Component} | {item.Claim} |");
    builder.Append("""

        ## Release evidence rules

        - Build reuse requires complete matching manifests and bytes. Compile
          order and linked identities are significant. A null closure is
          unknown, while an empty list is a known-empty closure. A timestamp
          is an aid to inspection and is not part of byte identity.
        - A durable test receipt must bind to the admitted target and exact
          attempt, including its fence and lease, and agree with the raw
          TRX from that attempt. Check archived process and artifact timing
          as well as exit code, outcome, counters, run ID and content digest.
          A correctly recorded completed failure is valid failure evidence.
          The Git fingerprint is uppercase SHA-256 of UTF-8 text formatted
          as "branch-length:branch:commit-length:commit".
        - Coverage depends on the matching supervised output lease, the
          actual collector artifact and independently verified source aliases.
          XML source guesses do not grant mapping authority. A missing exact
          counter is unknown. An exact zero counter is uncovered. A reported
          condition percentage is aggregate and cannot identify a branch arm.
          Valid line hits from a completed failed run remain line evidence.
        """);
    return builder.ToString();
}

internal sealed record SeedRun(RunInputs Input, TestRunReceiptDetails Receipt, string Trx);
internal sealed record Oracle(string Id, string Component, bool Challenge, string Cause, string Observation, string Diagnostic);
