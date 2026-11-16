using System.Xml.Linq;
using SlopHawk.Daemon;

internal static class EffortMatrix
{
    private const string Candidate = "release/candidate.json";
    private const string Standing = "release/standing.json";
    private const string Cases = "release/shards.json";
    private const string Namespace = "http://microsoft.com/schemas/VisualStudio/TeamTest/2010";
    private const string GateProject = "app/ReleaseGate.csproj";
    private const string TestProject = "workload/Proof.csproj";
    private const string Report = "receipt.trx";

    internal static async Task Package(string directory, Func<string, string[], Task<string>> dotnet)
    {
        if (Directory.Exists(directory) || File.Exists(directory))
            throw new IOException("Choose a new matrix directory.");
        Directory.CreateDirectory(directory);
        var sdk = (await dotnet(directory, ["--version"])).Trim();
        await Parcel(Path.Combine(directory, "Kestrel"));
        await Ledger(Path.Combine(directory, "Wren"));
        await Museum(Path.Combine(directory, "Oriole"));

        async Task Parcel(string root)
        {
            const string source = "src/ParcelRule.cs";
            const string target = "routing-net10";
            const int line = 14;
            Put(root, source, """
                namespace Routing;

                internal static class ParcelRule
                {
                    internal static int PickLane(string prefix, int kilos)
                    {
                        if (kilos < 0)
                            throw new System.ArgumentOutOfRangeException(nameof(kilos));
                        if (prefix.Length == 0)
                            return 1;
                        if (kilos > 20)
                            return 4;
                        if (prefix.StartsWith("Z", System.StringComparison.Ordinal))
                            return 7;
                        return 3;
                    }
                }
                """);
            const string aliasA = "mapped/ParcelRule.cs";
            const string aliasB = "generated/ParcelRule.cs";
            var sha = EffortInspection.Digest(root, source);
            CoverageDocumentAlias[] aliases = [new(aliasA, target, source, sha), new(aliasB, target, source, sha)];
            var bad = Collect("exports/4904", 14);
            var good = Collect("exports/4912", 15);
            var expected = EffortInspection.Coverage(root, bad);
            var valid = EffortInspection.Coverage(root, good);
            Require(expected.State == CoberturaImportState.Malformed && valid.State == CoberturaImportState.Imported &&
                valid.LineState(target, source, line, sha) == CoverageLineState.Observed, "Canonical coverage replay");
            PilotArchive.Write(root, Candidate, new EvidenceRequest(bad, target, source, line, null));
            PilotArchive.Write(root, Standing, new EvidenceRequest(good, target, source, line, null));
            var program = Gate("""
                var capturePath = request.GetProperty("Capture").GetString()!;
                var capture = Read(capturePath);
                var alias = capture.GetProperty("Attempt").GetProperty("VerifiedAliases").EnumerateArray()
                    .First(entry => entry.GetProperty("SourcePath").GetString() == request.GetProperty("Source").GetString());
                var path = Path.GetDirectoryName(capturePath)!;
                var report = XDocument.Load(Path.Combine(root, path, capture.GetProperty("Report").GetString()!));
                var counted = report.Descendants("class").Where(item => (string?)item.Attribute("filename") == alias.GetProperty("CollectorPath").GetString())
                    .SelectMany(item => item.Descendants("line"))
                    .Any(item => (int?)item.Attribute("number") == request.GetProperty("Line").GetInt32() && (int?)item.Attribute("hits") > 0);
                return counted && (int?)report.Root!.Attribute("lines-covered") >= 2;
                """, "Routing.ParcelRule.PickLane(\"Z4\", 9) == 7 && Routing.ParcelRule.PickLane(\"B2\", 26) == 4");
            var fixedProgram = program.Replace("return counted &&", """
                var paths = capture.GetProperty("Attempt").GetProperty("VerifiedAliases").EnumerateArray().ToDictionary(
                    item => item.GetProperty("CollectorPath").GetString()!, item => item.GetProperty("SourcePath").GetString()!);
                var identity = report.Descendants("class").SelectMany(item => item.Descendants("line").Select(line =>
                    (paths[(string)item.Attribute("filename")!], (int)line.Attribute("number")!))).ToArray();
                return identity.Distinct().Count() == identity.Length && counted &&
                """, StringComparison.Ordinal);
            await Replay(root, program, fixedProgram, "<Compile Include=\"../src/*.cs\" />", """
                # Kestrel parcel release

                The regional routing service is ready for a depot rollout. The current handoff is in `release/candidate.json`.
                `release/standing.json` is the currently accepted handoff. The routing code picks a lane for a postal prefix and parcel weight.
                The depot asks for an attributable hit on the requested rule. The coverage collector gives some files multiple generated names;
                the source map in the exported attempt is authoritative. Count each measured physical source line once within the requested run.
                """);

            string Collect(string folder, int second)
            {
                var now = DateTimeOffset.UtcNow;
                const string xml = "coverage.xml";
                Put(root, $"{folder}/{xml}", $"""
                    <coverage lines-covered="2" lines-valid="2" branches-covered="0" branches-valid="0" version="1.9">
                      <packages><package name="depot"><classes>
                        <class name="Routing.ParcelRule" filename="{aliasA}"><lines><line number="14" hits="3" /></lines></class>
                        <class name="Routing.ParcelRule.Generator" filename="{aliasB}"><lines><line number="{second}" hits="2" /></lines></class>
                      </classes></package></packages>
                    </coverage>
                    """);
                var attempt = new CoverageSupervisedAttempt(Guid.NewGuid(), second == 14 ? 4904 : 4912, target, "routing-046", "binary-046",
                    "cobertura", "1.9", "sources-046", now.AddSeconds(-30), now.AddSeconds(1), now.AddSeconds(2),
                    TestOutcome.Passed, Completed: true, CollectorAvailable: true, aliases);
                var lease = new CoverageResultsLease(attempt.Epoch, attempt.AttemptId, target, attempt.BuildIdentity, attempt.BinaryIdentity,
                    attempt.Collector, attempt.CollectorVersion, attempt.SourceIdentity, "", now.AddSeconds(-25), OwnedAndDrained: true);
                var raw = $"{folder}/supervisor.json";
                PilotArchive.Write(root, raw, new CoveragePacket(attempt, lease, xml));
                return raw;
            }
        }

        async Task Ledger(string root)
        {
            Put(root, "workload/Checks.cs", """
                using System;
                using Xunit;
                namespace Relay;
                public sealed class CodecChecks
                {
                    [Fact] public void Plain() => Assert.Equal("BA32", Uri.UnescapeDataString("BA32"));
                    [Fact] public void Escaped() => Assert.Equal("A B", Uri.UnescapeDataString("A%20B"));
                    [Fact] public void Split() => Assert.Equal("B", "A,B".Split(',')[1]);
                    [Fact] public void Boundary() => Assert.Equal("", "A,B,".Split(',')[2]);
                }
                """);
            var dll = await Proof(root);
            var required = new Dictionary<string, string[]>
            {
                ["ingest"] = ["Relay.CodecChecks.Plain", "Relay.CodecChecks.Escaped"],
                ["replay"] = ["Relay.CodecChecks.Split", "Relay.CodecChecks.Boundary"]
            };
            PilotArchive.Write(root, Cases, required);
            var (first, _) = await Test(root, dll, 2541, "ingest-net10", Filter(required["ingest"]));
            var (retry, _) = await Test(root, dll, 2548, "replay-net10", Filter([required["ingest"][1], required["replay"][0]]));
            var (standing, _) = await Test(root, dll, 2552, "replay-net10", Filter(required["replay"]));
            PilotArchive.Write(root, "exports/2543/close.json", new { Queue = "replay", State = "TimedOut", Attempt = 2543, RetriedBy = 2548 });
            PilotArchive.Write(root, Candidate, new EvidenceRequest(null, null, null, 0, [first, retry]));
            PilotArchive.Write(root, Standing, new EvidenceRequest(null, null, null, 0, [first, standing]));
            Require(Names(root, first).SetEquals(required["ingest"]) &&
                Names(root, retry).SetEquals([required["ingest"][1], required["replay"][0]]) &&
                Names(root, standing).SetEquals(required["replay"]), "Actual native TRX selected cases");
            var program = Gate("""
                var matrix = Read("release/shards.json").EnumerateObject().ToDictionary(item => item.Name,
                    item => item.Value.EnumerateArray().Select(name => name.GetString()!).ToArray());
                var runs = request.GetProperty("Runs").EnumerateArray().Select(name => Read(name.GetString()!)).ToArray();
                foreach (var run in runs)
                {
                    var receipt = run.GetProperty("Receipt").GetProperty("Receipt");
                    var shard = receipt.GetProperty("Target").GetString()!.Replace("-net10", "", StringComparison.Ordinal);
                    var threshold = matrix[shard].Length;
                    if (receipt.GetProperty("Execution").GetString() != "Passed" ||
                        receipt.GetProperty("Counters").GetProperty("Passed").GetInt32() != threshold)
                        return false;
                }
                return runs.Length == matrix.Count;
                """, "Uri.UnescapeDataString(\"A%20B\") == \"A B\" && \"A,B\".Split(',').Length == 2");
            var fixedProgram = program.Replace("var threshold = matrix[shard].Length;", """
                var threshold = matrix[shard].Length;
                var xml = XDocument.Load(Path.Combine(root, run.GetProperty("Report").GetString()!));
                XNamespace ns = "http://microsoft.com/schemas/VisualStudio/TeamTest/2010";
                var definitions = xml.Descendants(ns + "UnitTest").ToDictionary(item => (string)item.Attribute("id")!, item =>
                    (string)item.Element(ns + "TestMethod")!.Attribute("className")! + "." + (string)item.Element(ns + "TestMethod")!.Attribute("name")!);
                var passed = xml.Descendants(ns + "UnitTestResult").Where(item => (string?)item.Attribute("outcome") == "Passed")
                    .Select(item => definitions[(string)item.Attribute("testId")!]).ToHashSet(StringComparer.Ordinal);
                if (!matrix[shard].All(passed.Contains))
                    return false;
                """, StringComparison.Ordinal);
            await Replay(root, program, fixedProgram, null, """
                # Wren ledger relay handoff

                The relay reads a small text envelope in two lanes and retries the replay lane after a timeout. Delivery may go ahead once
                each named case in the release matrix has passed in its assigned lane. A successful retry may replace the timeout.
                `release/candidate.json` names the proposed evidence; `release/standing.json` names a prior accepted handoff. The ordinary
                smoke check exercises the relay's envelope parsing. `workload/` contains the qualification suite that produced native TRX files.
                """);
        }

        async Task Museum(string root)
        {
            Put(root, "workload/Checks.cs", """
                using Xunit;
                namespace Gallery;
                public sealed class Desk
                {
                    [Fact] public void UsesApprovedWording() => Assert.Equal("Iris", "Iris (oil)".Split(' ')[0]);
                }
                public sealed class Accessible
                {
                    [Fact] public void UsesApprovedWording() => Assert.Equal("Iris — oil on canvas", "Iris" + " — " + "oil on canvas");
                }
                """);
            const string smoke = "profiles/front-desk/verify.runsettings";
            const string access = "profiles/accessibility/verify.runsettings";
            foreach (var (path, name) in new[] { (smoke, "Desk"), (access, "Accessible") })
                Put(root, "workload/" + path, $"""
                    <RunSettings><RunConfiguration><TestCaseFilter>FullyQualifiedName=Gallery.{name}.UsesApprovedWording</TestCaseFilter></RunConfiguration></RunSettings>
                    """);
            var dll = await Proof(root);
            const string output = "published/Proof.dll";
            Copy(dll, PilotArchive.Child(root, "workload/" + output));
            var target = new ProjectTarget("Proof.csproj|net10.0", "Proof.csproj", "net10.0", "Debug", "AnyCPU", "", sdk, true,
                [new("Checks.cs", null)], [], EffortInspection.Digest(root, TestProject));
            var manifest = new BuildArtifactManifest(target, true, "gallery-paths-v1", "vstest", "xunit", [new("Checks.cs", null)],
                [], [], [access], [], [output]);
            var profiles = new ProfileEvidence("workload", "accessibility", "front-desk", ["Checks.cs", output, access, smoke],
                new() { ["accessibility"] = manifest, ["front-desk"] = manifest with { SettingsFiles = [smoke] } });
            var selected = EffortInspection.Profile(root, profiles, profiles.Effective);
            var previous = EffortInspection.Profile(root, profiles, profiles.Consent);
            var comparison = BuildOutputIdentity.Compare(previous, selected);
            Require(selected.Complete && previous.Complete && !comparison.Reusable &&
                comparison.Problems is [{ Kind: BuildIdentityProblemKind.Mismatch, Field: "settings" }], "Only effective settings change identity");
            var (desk, _) = await Test(root, dll, 3117, "labels-net10", [], "workload/" + smoke, previous);
            var (accessible, _) = await Test(root, dll, 3118, "labels-net10", [], "workload/" + access, selected);
            Require(Names(root, desk).SetEquals(["Gallery.Desk.UsesApprovedWording"]) &&
                Names(root, accessible).SetEquals(["Gallery.Accessible.UsesApprovedWording"]), "The runsettings actually select different tests");
            PilotArchive.Write(root, "release/proposal-profile.json", profiles);
            PilotArchive.Write(root, "release/standing-profile.json", profiles with { Consent = profiles.Effective });
            PilotArchive.Write(root, "release/deployment.json", new { Target = "labels-net10", Route = profiles.Effective,
                Artifact = "workload/" + output, ArtifactSha256 = EffortInspection.Digest(root, "workload/" + output),
                RequiredInputStamp = selected.Sha256, Format = selected.DomainVersion });
            PilotArchive.Write(root, Candidate, new EvidenceRequest(null, null, null, 0, [desk], "release/proposal-profile.json"));
            PilotArchive.Write(root, Standing, new EvidenceRequest(null, null, null, 0, [accessible], "release/standing-profile.json"));
            var program = Gate("""
                var deployment = Read("release/deployment.json");
                var run = Read(request.GetProperty("Runs")[0].GetString()!);
                var receipt = run.GetProperty("Receipt").GetProperty("Receipt");
                using var executable = File.OpenRead(Path.Combine(root, deployment.GetProperty("Artifact").GetString()!));
                var sha = Convert.ToHexString(SHA256.HashData(executable));
                return receipt.GetProperty("Execution").GetString() == "Passed" &&
                    receipt.GetProperty("Target").GetString() == deployment.GetProperty("Target").GetString() &&
                    sha == deployment.GetProperty("ArtifactSha256").GetString();
                """, "\"Iris (oil)\".Split(' ')[0] == \"Iris\"");
            var fixedProgram = program.Replace("sha == deployment.GetProperty(\"ArtifactSha256\").GetString();", """
                sha == deployment.GetProperty("ArtifactSha256").GetString() &&
                    receipt.GetProperty("BuildDomainVersion").GetInt32() == deployment.GetProperty("Format").GetInt32() &&
                    receipt.GetProperty("BuildSha256").GetString() == deployment.GetProperty("RequiredInputStamp").GetString();
                """, StringComparison.Ordinal);
            await Replay(root, program, fixedProgram, null, """
                # Oriole gallery label handoff

                The museum produces labels from one assembly for multiple desks. The next distribution uses the accessibility route.
                A passing qualification belongs to the settings profile that selected its wording checks. The deployment is described in
                `release/deployment.json`. Operators have selected a proof in `release/candidate.json`; the standing handoff is also supplied.
                The native TRX exports and run commands came from separate runs of the same binary.
                """);
        }

        async Task<string> Proof(string root)
        {
            Put(root, TestProject, """
                <Project Sdk="Microsoft.NET.Sdk">
                  <PropertyGroup><TargetFramework>net10.0</TargetFramework><IsTestProject>true</IsTestProject></PropertyGroup>
                  <ItemGroup>
                    <PackageReference Include="Microsoft.NET.Test.Sdk" Version="17.14.1" />
                    <PackageReference Include="xunit" Version="2.9.3" />
                    <PackageReference Include="xunit.runner.visualstudio" Version="3.1.5" PrivateAssets="all" />
                  </ItemGroup>
                </Project>
                """);
            var build = Path.Combine(directory, "author", Path.GetFileName(root), "proof");
            await dotnet(root, ["build", TestProject, "--artifacts-path", build, "-p:NuGetAudit=false"]);
            return Path.Combine(build, "bin/Proof/debug/Proof.dll");
        }

        async Task<(string Record, ArchivedRun Archive)> Test(string root, string dll, int id, string target,
            string[] options, string? settings = null, BuildIdentityStamp? build = null)
        {
            var path = $"exports/{id}";
            var results = Directory.CreateDirectory(PilotArchive.Child(root, path)).FullName;
            string[] origin = [.. options];
            if (settings is not null)
                origin = [$"/Settings:{PilotArchive.Child(root, settings)}", .. origin];
            var start = DateTimeOffset.UtcNow;
            await dotnet(root, ["vstest", dll, .. origin, $"/ResultsDirectory:{results}", $"/Logger:trx;LogFileName={Report}"]);
            var finish = DateTimeOffset.UtcNow;
            var report = $"{path}/{Report}";
            var trx = VstestTrxClassifier.Parse(File.ReadAllBytes(PilotArchive.Child(root, report)));
            var facts = new VstestProcessFacts(id, start, finish, true, null, true, 0);
            var fence = new ScheduleFence(Guid.NewGuid(), 10, 42, RootIdentity.Read(root), new GitRevision("refs/heads/release", new string('A', 40)));
            var selection = new SelectedTarget(target, EffortInspection.Digest(root, "workload/Checks.cs"), []);
            var attempt = new TestAttempt(id, 1, fence, selection, "release-profile");
            var lease = $"lease-{id}";
            var receipt = new TestRunReceiptDetails(id, fence.Revision, fence.Epoch, fence.Sequence, TestRunReceiptDetails.GitIdentity(fence.Git),
                fence.RootIdentity, target, selection.InputFingerprint, attempt.ProfileRevision, lease, build?.DomainVersion, build?.Sha256, null,
                trx.Sha256, trx.ByteLength, trx.RunId, VstestDisposition.Passed, VstestRunnerHealth.ZeroExit, VstestArtifactHealth.Complete,
                VstestExecution.Passed, trx.Counters, "passed");
            receipt.Validate(attempt, TestOutcome.Passed, lease);
            var archive = new ArchivedRun(report, facts, new RunPacket(attempt, fence, lease, receipt));
            Require(EffortInspection.Classify(root, archive).Disposition == VstestDisposition.Passed, $"Native run {id} replay");
            PilotArchive.Write(root, $"{path}/dispatch.json", new { Target = target, Settings = settings, Options = options,
                BinarySha256 = Convert.ToHexString(System.Security.Cryptography.SHA256.HashData(File.ReadAllBytes(dll))) });
            var record = $"{path}/observation.json";
            PilotArchive.Write(root, record, archive);
            return (record, archive);
        }

        async Task Replay(string root, string program, string fixedProgram, string? include, string docs)
        {
            Require(program != fixedProgram, "The isolated counterfactual must change the gate program.");
            var project = $"""
                <Project Sdk="Microsoft.NET.Sdk">
                  <PropertyGroup><OutputType>Exe</OutputType><TargetFramework>net10.0</TargetFramework><ImplicitUsings>enable</ImplicitUsings></PropertyGroup>
                  <ItemGroup>{include}</ItemGroup>
                </Project>
                """;
            Put(root, GateProject, project);
            Put(root, "app/Program.cs", program);
            Put(root, "README.md", docs + "\n\n" + """
                Run the documented checks from the checkout in PowerShell. Set `$out` to a fresh directory outside the checkout.

                ```powershell
                $root = (Get-Location).Path
                dotnet build app/ReleaseGate.csproj --artifacts-path $out -p:NuGetAudit=false
                $gate = Join-Path $out 'bin/ReleaseGate/debug/ReleaseGate.dll'
                dotnet $gate $root smoke
                dotnet $gate $root release/candidate.json
                dotnet $gate $root release/standing.json
                ```

                Use external build output directories. Keep the original evidence, source and configuration intact for review.
                """);
            var label = Path.GetFileName(root);
            await Exercise(root, Path.Combine(directory, "author", label, "original"), "ready");
            var repaired = Path.Combine(directory, "author", label, "repair");
            Put(repaired, GateProject, project);
            Put(repaired, "app/Program.cs", fixedProgram);
            if (include is not null)
                Copy(PilotArchive.Child(root, "src/ParcelRule.cs"), PilotArchive.Child(repaired, "src/ParcelRule.cs"));
            await Exercise(repaired, Path.Combine(directory, "author", label, "corrected-output"), "hold");

            async Task Exercise(string source, string output, string candidate)
            {
                await dotnet(source, ["build", GateProject, "--artifacts-path", output, "-p:NuGetAudit=false"]);
                var gate = Path.Combine(output, "bin/ReleaseGate/debug/ReleaseGate.dll");
                Require((await dotnet(root, [gate, root, "smoke"])).Trim() == "smoke passed", label + " smoke");
                Require((await dotnet(root, [gate, root, Candidate])).Trim() == candidate, label + " candidate");
                Require((await dotnet(root, [gate, root, Standing])).Trim() == "ready", label + " standing control");
                Console.WriteLine($"{label}: smoke passed; candidate {candidate}; standing ready.");
            }
        }
    }

    private static string Gate(string logic, string smoke) => $$"""
        using System.Security.Cryptography;
        using System.Text.Json;
        using System.Xml.Linq;

        var root = args[0];
        if (args[1] == "smoke")
        {
            if (!({{smoke}}))
                return 1;
            Console.WriteLine("smoke passed");
            return 0;
        }
        Console.WriteLine(Ready(Read(args[1])) ? "ready" : "hold");
        return 0;

        bool Ready(JsonElement request)
        {
        {{string.Join('\n', logic.Split('\n').Select(line => "    " + line))}}
        }

        JsonElement Read(string path) => JsonSerializer.Deserialize<JsonElement>(File.ReadAllText(Path.Combine(root, path)));
        """;

    private static HashSet<string> Names(string root, string record)
    {
        XNamespace ns = Namespace;
        var run = PilotArchive.Read<ArchivedRun>(root, record);
        var xml = XDocument.Load(PilotArchive.Child(root, run.Report));
        var definitions = xml.Descendants(ns + "UnitTest").ToDictionary(test => (string)test.Attribute("id")!, test =>
            (string)test.Element(ns + "TestMethod")!.Attribute("className")! + "." + (string)test.Element(ns + "TestMethod")!.Attribute("name")!);
        return xml.Descendants(ns + "UnitTestResult").Where(result => (string?)result.Attribute("outcome") == "Passed")
            .Select(result => definitions[(string)result.Attribute("testId")!]).ToHashSet(StringComparer.Ordinal);
    }

    private static string[] Filter(string[] names) => ["/TestCaseFilter:" + string.Join('|', names.Select(name => "FullyQualifiedName=" + name))];

    private static void Put(string root, string path, string text)
    {
        var file = PilotArchive.Child(root, path);
        Directory.CreateDirectory(Path.GetDirectoryName(file)!);
        File.WriteAllText(file, text + "\n");
    }

    private static void Copy(string source, string destination)
    {
        Directory.CreateDirectory(Path.GetDirectoryName(destination)!);
        File.Copy(source, destination);
    }

    private static void Require(bool passed, string message)
    {
        if (!passed)
            throw new InvalidDataException(message);
    }
}
