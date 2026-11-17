using System.Diagnostics;
using System.Security.Cryptography;
using System.Text;
using System.Text.Json;
using SlopHawk.Daemon;
using Xunit;

namespace SlopHawk.IntegrationTests;

[CollectionDefinition("NativeExecutionFixtures", DisableParallelization = true)]
public sealed class NativeExecutionFixtureCollection { }

[Collection("NativeExecutionFixtures")]
public sealed class NativeExecutionBridgeTests
{
    private static readonly JsonSerializerOptions Web = new(JsonSerializerDefaults.Web);

    private static string Hash(byte[] bytes) => Convert.ToHexString(SHA256.HashData(bytes));

    private static (NativeExecutionIdentity Id, NativeExecutionWork Work, string Raw) EvaluateRequest(string root)
    {
        var work = new NativeExecutionWork(root, Path.Combine(root, "tool", "dotnet"));
        var raw = JsonSerializer.Serialize(work, Web);
        return (new NativeExecutionIdentity("job-eval", root, "root-physical", "workspace",
            Guid.NewGuid(), 12, 3, Hash(Encoding.UTF8.GetBytes(raw)), new string('A', 64),
            new string('B', 64), new string('B', 64), "job-budget"), work, raw);
    }

    [Fact]
    public void EnvelopeBindsExactRawWorkAndEveryPublicationIdentity()
    {
        using var f = new TempRoot();
        var (id, work, raw) = EvaluateRequest(f.Checkout);
        Assert.True(NativeExecutionBridge.ValidIdentity(id, "evaluate", work, raw, out var reason), reason);
        Assert.False(NativeExecutionBridge.ValidIdentity(id with { Epoch = Guid.NewGuid() }, "unknown",
            work, raw, out _));
        Assert.False(NativeExecutionBridge.ValidIdentity(id, "evaluate",
            work with { DotnetPath = Path.Combine(f.Checkout, "replacement") }, raw, out _));
        Assert.False(NativeExecutionBridge.ValidIdentity(id with { InputSha256 = new string('C', 64) },
            "evaluate", work, raw, out _));
        Assert.False(NativeExecutionBridge.ValidIdentity(id with { Context = "fabricated-target" },
            "evaluate", work, raw, out _));
        Assert.False(NativeExecutionBridge.ValidIdentity(id with { RootIdentity = "" },
            "evaluate", work, raw, out _));
        Assert.False(NativeExecutionBridge.ValidIdentity(id with { BudgetReference = "" },
            "evaluate", work, raw, out _));
    }

    [Fact]
    public void BuildOrTestCannotBorrowTheEvaluateOnlyEnvelope()
    {
        using var f = new TempRoot();
        var (id, work, raw) = EvaluateRequest(f.Checkout);
        Assert.False(NativeExecutionBridge.ValidIdentity(id, "build", work, raw, out _));
        Assert.False(NativeExecutionBridge.ValidIdentity(id, "test", work, raw, out _));
        var wrong = work with { SelectedTargetId = "tests/Unit.csproj|net10.0",
            ResultsRoot = Path.Combine(f.Container, "results"),
            Closure = new NativeExecutionClosure("mapping", "runner", "adapter", [], [], [], [],
                [Path.Combine(f.Checkout, "bin", "Unit.dll")], [], []) };
        Assert.False(NativeExecutionBridge.ValidIdentity(id, "build", wrong,
            JsonSerializer.Serialize(wrong, Web), out _));
    }

    [Fact]
    public void RequiredBuildAndRuntimeClosureMustBeExplicitlyBound()
    {
        using var f = new TempRoot();
        var (evaluation, _, _) = EvaluateRequest(f.Checkout);
        var selectedId = "Tests/Case.csproj|net10.0";
        var output = Path.Combine(f.Checkout, "Tests", "bin", "Case.dll");
        var closure = new NativeExecutionClosure("map/v1", "vstest", "trx", [], [], [], [],
            [output], [], []);
        var work = new NativeExecutionWork(f.Checkout,
            Path.Combine(f.Checkout, "tool", "dotnet"), selectedId,
            Path.Combine(f.Container, "results"), closure);
        NativeExecutionIdentity Id(NativeExecutionWork value) => evaluation with
        {
            Context = selectedId,
            InputSha256 = Hash(Encoding.UTF8.GetBytes(JsonSerializer.Serialize(value, Web)))
        };
        Assert.True(NativeExecutionBridge.ValidIdentity(Id(work), "build", work,
            JsonSerializer.Serialize(work, Web), out _));
        var missingOutput = work with { Closure = closure with { RequiredOutputs = [] } };
        Assert.False(NativeExecutionBridge.ValidIdentity(Id(missingOutput), "build", missingOutput,
            JsonSerializer.Serialize(missingOutput, Web), out _));
        var unboundRuntime = work with { Closure = closure with { RuntimeAssets = null! } };
        Assert.False(NativeExecutionBridge.ValidIdentity(Id(unboundRuntime), "build", unboundRuntime,
            JsonSerializer.Serialize(unboundRuntime, Web), out _));
        var relativeReference = work with { Closure = closure with { ReferenceAssemblies = ["../escape.dll"] } };
        Assert.False(NativeExecutionBridge.ValidIdentity(Id(relativeReference), "build", relativeReference,
            JsonSerializer.Serialize(relativeReference, Web), out _));
    }

    [Fact]
    public void MissingToolchainIsDistinguishedWithoutExecutingAnyBinary()
    {
        using var f = new TempRoot();
        Assert.Null(NativeExecutionBridge.ResolveDotnetForTests(f.Workspace, ""));
        var outside = Path.Combine(f.Container, "fake-dotnet", OperatingSystem.IsWindows() ? "dotnet.exe" : "dotnet");
        Directory.CreateDirectory(Path.GetDirectoryName(outside)!);
        File.WriteAllText(outside, "not an executable");
        Assert.Equal(outside, NativeExecutionBridge.ResolveDotnetForTests(f.Workspace,
            Path.GetDirectoryName(outside)!));
        Assert.Null(NativeExecutionBridge.ResolveDotnetForTests(f.Workspace, f.Checkout));
    }

    [Fact]
    public async Task MissingAndRevokedProfileCannotCreateNewWorkerAuthority()
    {
        using var f = new TempRoot();
        Assert.Throws<UnauthorizedAccessException>(() =>
            ExecutionProfile.Read(f.Workspace, RootIdentity.Read(f.Checkout)));
        f.WriteProfile("evaluate");
        var grant = ExecutionProfile.Read(f.Workspace, RootIdentity.Read(f.Checkout));
        Assert.Throws<UnauthorizedAccessException>(() =>
            grant.Require(new WorkerRequirements(ExecutionOperation.Build, true, [])));
        if (!OperatingSystem.IsWindowsVersionAtLeast(10))
            return;
        var (lease, waiting) = new WorkerAdmission(f.Workspace).TryAcquire(grant,
            new WorkerRequirements(ExecutionOperation.Evaluate, true, []));
        Assert.NotNull(lease);
        Assert.Null(waiting);
        await using var worker = lease!;
        worker.Revalidate();
        f.WriteProfile("evaluate", "test");
        Assert.Throws<UnauthorizedAccessException>(() => worker.Revalidate());
    }

    [Fact]
    public async Task CancellationNeverConvertsAResultIntoAPass()
    {
        using var f = new RunnerFixture();
        var fake = new RecordingExecution((_, _) =>
            Task.FromResult(new WorkerResult(0, "", "")));
        using var stopped = new CancellationTokenSource();
        stopped.Cancel();
        var result = await VstestAttemptRunner.RunForTestsAsync(f.Request, fake, stopped.Token);
        Assert.Equal(VstestAttemptGate.Cancelled, result.Gate);
        Assert.Null(result.Classification);
        Assert.Equal(0, fake.Launches);
    }

    [Theory]
    [InlineData("Completed", 1, 1, 1, 0, 0, (int)VstestDisposition.Passed)]
    [InlineData("Failed", 1, 1, 0, 1, 1, (int)VstestDisposition.Failed)]
    [InlineData("Completed", 0, 0, 0, 0, 0, (int)VstestDisposition.ZeroDiscovered)]
    [InlineData("Completed", 1, 0, 0, 0, 0, (int)VstestDisposition.AllSkipped)]
    public async Task BoundedAdmittedVstestDispositionsRemainDistinct(string outcome,
        int total, int executed, int passed, int failed, int exit, int expected)
    {
        using var f = new RunnerFixture();
        var fake = new RecordingExecution(async (command, token) =>
        {
            Assert.All(command.ArgumentList, argument => Assert.DoesNotContain(" -c ", argument));
            await WriteTrxAsync(ResultsFile(command), outcome, total, executed, passed, failed, token);
            return new WorkerResult(exit, "bounded", "bounded");
        });
        var result = await VstestAttemptRunner.RunForTestsAsync(f.Request, fake, CancellationToken.None);
        Assert.Equal(VstestAttemptGate.Launched, result.Gate);
        Assert.True((VstestDisposition)expected == result.Classification!.Disposition,
            $"Expected {(VstestDisposition)expected}, got {result.Classification.Disposition}: " +
            $"{result.Classification.Reason} Attempt start {result.Process?.StartedUtc:O}, " +
            $"TRX finish {result.Classification.Artifact.FinishedUtc:O}, file timestamp {result.Process?.ArtifactLastWriteUtc:O}.");
        Assert.Equal(VstestArtifactHealth.Complete, result.Classification.Artifact.Health);
        Assert.True(result.Process!.Exited);
        Assert.Equal(1, fake.Launches);
        if ((VstestDisposition)expected is VstestDisposition.ZeroDiscovered or VstestDisposition.AllSkipped)
            Assert.Null(result.Classification.SchedulerOutcome);
    }

    [Fact]
    public async Task RevocationAfterRunnerExitRejectsEvenAValidPassingReceipt()
    {
        using var f = new RunnerFixture();
        RecordingExecution? fake = null;
        fake = new RecordingExecution(async (command, token) =>
        {
            await WriteTrxAsync(ResultsFile(command), "Completed", 1, 1, 1, 0, token);
            fake!.Allowed = false;
            return new WorkerResult(0, "", "");
        });
        var result = await VstestAttemptRunner.RunForTestsAsync(f.Request, fake, CancellationToken.None);
        Assert.Equal(VstestAttemptGate.DeniedAuthority, result.Gate);
        Assert.Null(result.Classification);
        Assert.Equal(1, fake.Launches);
    }

    [Fact]
    public async Task ReusedOutputBytesNeverReuseAnOldAttemptDirectoryOrTrx()
    {
        using var f = new RunnerFixture();
        var fake = new RecordingExecution(async (command, token) =>
        {
            await WriteTrxAsync(ResultsFile(command), "Completed", 1, 1, 1, 0, token);
            return new WorkerResult(0, "", "");
        });
        var first = await VstestAttemptRunner.RunForTestsAsync(f.Request, fake, CancellationToken.None);
        var secondRequest = f.Request with
        {
            Attempt = f.Request.Attempt with { Id = f.Request.Attempt.Id + 1, Number = 2 }
        };
        var second = await VstestAttemptRunner.RunForTestsAsync(secondRequest, fake, CancellationToken.None);
        Assert.Equal(VstestDisposition.Passed, first.Classification!.Disposition);
        Assert.Equal(VstestDisposition.Passed, second.Classification!.Disposition);
        Assert.NotEqual(first.ResultsDirectory, second.ResultsDirectory);
        Assert.Equal(2, fake.Launches);
    }

    [Fact]
    public async Task TamperedBuildBytesCannotBeUsedToLaunchAVerifiedTest()
    {
        using var f = new RunnerFixture();
        var fake = new RecordingExecution((_, _) => Task.FromResult(new WorkerResult(0, "", "")));
        File.AppendAllText(f.AssemblyPath, "mutation");
        var result = await VstestAttemptRunner.RunForTestsAsync(f.Request, fake, CancellationToken.None);
        Assert.Equal(VstestAttemptGate.InvalidInputs, result.Gate);
        Assert.Null(result.Classification);
        Assert.Equal(0, fake.Launches);
    }

    [Fact]
    public async Task EvaluatedMembershipAndSameEvaluatorBuildSnapshotAreBound()
    {
        if (Environment.GetEnvironmentVariable("SLOPHAWK_NATIVE_SDK_TEST") != "1")
            return; // Opt-in .NET 10 SDK fixture; no restore performed by the fixture.
        using var f = new TempRoot();
        var project = Path.Combine(f.Checkout, "Tests", "Unit.csproj");
        Directory.CreateDirectory(Path.GetDirectoryName(project)!);
        File.WriteAllText(project, """
            <Project Sdk="Microsoft.NET.Sdk"><PropertyGroup>
            <TargetFramework>net10.0</TargetFramework><IsTestProject>true</IsTestProject>
            </PropertyGroup></Project>
            """);
        File.WriteAllText(Path.Combine(Path.GetDirectoryName(project)!, "Unit.cs"),
            "namespace Fixture; public sealed class Unit { public int Value => 3; }");
        var evaluator = new ProjectEvaluator(f.Workspace);
        var graph = await evaluator.EvaluateAsync(null, CancellationToken.None);
        Assert.True(graph.Complete, string.Join(",", graph.Issues.Select(i => i.Reason)));
        var target = Assert.Single(graph.Targets);
        Assert.Contains(target.Compile, member => member.Path == "Tests/Unit.cs");
        Assert.Equal("net10.0", target.Framework);
        var other = new ProjectEvaluator(f.Workspace);
        var outputs = new[] { Path.Combine(f.Checkout, "Tests", "bin"),
            Path.Combine(f.Checkout, "Tests", "obj"), Path.Combine(f.Container, "results") };
        var grant = new ExecutionGrant("fixture", RootIdentity.Read(f.Checkout),
            ExecutionOperation.Evaluate | ExecutionOperation.Build | ExecutionOperation.CompilerExtensions,
            outputs, FromProfile: false);
        var orphan = other.PrepareBuild(graph, target.Id, outputs[^1], grant);
        Assert.Equal(TestBuildBlocker.EvaluationUnavailable, orphan.Blocker);
        var refreshed = await evaluator.EvaluateAsync(null, CancellationToken.None);
        var old = evaluator.PrepareBuild(graph, target.Id, outputs[^1], grant);
        Assert.Equal(TestBuildBlocker.EvaluationUnavailable, old.Blocker);
        Assert.Single(refreshed.Targets);
    }

    // Optional end-to-end run requires an externally restored and already built SDK
    // style net10.0 VSTest project with a passing test and its local adapter, explicit
    // output permissions and Windows 10+.
    // It intentionally does NOT restore, download packages or run on an arbitrary checkout.
    [Fact]
    public async Task OptInAdmittedBuildAndVstestRequireCurrentIdentityAndCompleteClosure()
    {
        var root = Environment.GetEnvironmentVariable("SLOPHAWK_NATIVE_E2E_ROOT");
        var targetId = Environment.GetEnvironmentVariable("SLOPHAWK_NATIVE_E2E_TARGET");
        if (!OperatingSystem.IsWindowsVersionAtLeast(10) ||
            string.IsNullOrWhiteSpace(root) || string.IsNullOrWhiteSpace(targetId))
            return;
        using var fixture = new TempRoot(existingCheckout: root);
        var workspace = fixture.Workspace;
        var dotnet = NativeExecutionBridge.ResolveDotnetForTests(workspace,
            Environment.GetEnvironmentVariable("PATH") ?? "")!;
        Assert.NotNull(dotnet);
        var evaluator = new ProjectEvaluator(workspace);
        var graph = await evaluator.EvaluateAsync(null, CancellationToken.None);
        Assert.True(graph.Complete);
        var target = Assert.Single(graph.Targets, t => t.Id == targetId);
        var results = Path.Combine(fixture.Container, "results");
        var roots = graph.Targets.SelectMany(t =>
        {
            var dir = Path.GetDirectoryName(Path.Combine(root, t.ProjectPath.Replace('/',
                Path.DirectorySeparatorChar)))!;
            return new[] { Path.Combine(dir, "bin"), Path.Combine(dir, "obj") };
        }).Append(results).Distinct(Workspace.PathComparer).ToArray();
        fixture.WriteProfile(["evaluate", "build", "test", "compilerExtensions"], roots);
        var grant = ExecutionProfile.Read(workspace, RootIdentity.Read(root));
        var prepared = evaluator.PrepareBuild(graph, target.Id, results, grant);
        Assert.True(prepared.Ready, $"{prepared.Blocker}: {prepared.Detail}");
        var plan = prepared.Plan!;
        var assembly = plan.InvokedTargets[^1].TargetAssembly;
        var companions = new[] { Path.ChangeExtension(assembly, ".deps.json"),
            Path.ChangeExtension(assembly, ".runtimeconfig.json") };
        Assert.All(companions, path => Assert.True(File.Exists(path), path));
        Assert.True(File.Exists(assembly), "Test expects prebuilt bytes to bind executable identity.");
        var runtime = companions.Concat(Directory.EnumerateFiles(Path.GetDirectoryName(assembly)!, "*.dll")
                .Where(path => !Workspace.PathComparer.Equals(path, assembly)))
            .Order(StringComparer.Ordinal).ToArray();
        var adapters = runtime.Where(path => path.EndsWith(".testadapter.dll",
            StringComparison.OrdinalIgnoreCase)).ToArray();
        Assert.NotEmpty(adapters);
        var closure = new NativeExecutionClosure("e2e/v1", "vstest/10", "vstest/trx",
            plan.InvokedTargets.Where(t => t.TargetId != target.Id)
                .Select(t => t.TargetAssembly).ToArray(),
            runtime, plan.InvokedTargets.Select(t => t.ProjectPath)
                .Concat(plan.Assets.Select(a => a.Path)).ToArray(), adapters,
            plan.InvokedTargets.Select(t => t.TargetAssembly).Concat(runtime).ToArray(),
            runtime, runtime);
        var work = new NativeExecutionWork(root, dotnet, target.Id, results, closure,
            target.EvaluationFingerprint, ["owner-admitted-cause"], TimeoutSeconds: 900);
        var runner = Path.Combine(Path.GetDirectoryName(dotnet)!, "sdk", target.SdkVersion, "vstest.console.dll");
        var id = new NativeExecutionIdentity("native-e2e", root, RootIdentity.Read(root), target.Id,
            Guid.NewGuid(), 10, 20, Hash(Encoding.UTF8.GetBytes(JsonSerializer.Serialize(work, Web))),
            Hash(File.ReadAllBytes(dotnet)), grant.Revision, grant.Revision, "e2e-budget",
            100, 1, Hash(File.ReadAllBytes(assembly)), Hash(File.ReadAllBytes(runner)));
        // Explicit Build and Test grants are verified in one admitted worker. Results
        // are evidence for the owner's separate attempt/fence validation, not committed here.
        var build = await NativeExecutionBridge.ExecuteForTestsAsync(workspace,
            id with { AttemptId = 0, AttemptNumber = 0, ExecutableSha256 = null, RunnerSha256 = null },
            "build", work, CancellationToken.None);
        Assert.Equal("complete", build.Status);
        var runWork = work with { RunnerPath = runner };
        var runId = id with
        {
            InputSha256 = Hash(Encoding.UTF8.GetBytes(JsonSerializer.Serialize(runWork, Web)))
        };
        var run = await NativeExecutionBridge.ExecuteForTestsAsync(workspace, runId, "test",
            runWork, CancellationToken.None);
        Assert.True(run.Status == "complete", run.Reason);
        var fact = Assert.Single(run.Facts, item => item.Kind == "admitted_vstest");
        Assert.Equal((int)VstestDisposition.Passed, fact.Data.GetProperty("disposition").GetInt32());
    }

    private static string ResultsFile(ProcessStartInfo command)
    {
        var arg = command.ArgumentList.Single(s => s.StartsWith("/ResultsDirectory:", StringComparison.Ordinal));
        return Path.Combine(arg["/ResultsDirectory:".Length..], "results.trx");
    }

    private static async Task WriteTrxAsync(string path, string outcome, int total,
        int executed, int passed, int failed, CancellationToken token)
    {
        var started = DateTimeOffset.UtcNow;
        // The simulated runner can complete within one filesystem clock tick.
        // Give its receipt and file an explicit, ordered timestamp within the
        // classifier's accepted clock tolerance.
        var finished = started.AddMilliseconds(100);
        var results = total == 0 ? "" : failed > 0
            ? "<UnitTestResult outcome=\"Failed\" testName=\"Case\" />"
            : executed == 0 ? "<UnitTestResult outcome=\"NotExecuted\" testName=\"Case\" />"
            : "<UnitTestResult outcome=\"Passed\" testName=\"Case\" />";
        var bytes = Encoding.UTF8.GetBytes($"""
            <?xml version="1.0" encoding="utf-8"?>
            <TestRun id="01234567-89ab-cdef-0123-456789abcdef" xmlns="http://microsoft.com/schemas/VisualStudio/TeamTest/2010">
              <Times start="{started:O}" finish="{finished:O}" />
              <Results>{results}</Results>
              <ResultSummary outcome="{outcome}"><Counters total="{total}" executed="{executed}" passed="{passed}" failed="{failed}" /></ResultSummary>
            </TestRun>
            """);
        await File.WriteAllBytesAsync(path, bytes, token);
        File.SetLastWriteTimeUtc(path, finished.UtcDateTime);
    }

    private sealed class RecordingExecution(Func<ProcessStartInfo, CancellationToken, Task<WorkerResult>> execute)
        : IVstestAttemptExecution
    {
        internal bool Allowed = true;
        internal int Launches;
        public string ProfileRevision => "test-policy";
        public void Revalidate()
        {
            if (!Allowed)
                throw new UnauthorizedAccessException("Fixture authority revoked.");
        }
        public void RequireResultsRoot(string path) => Revalidate();
        public async Task<WorkerResult> RunAsync(ProcessStartInfo command, int maxOutput, CancellationToken token)
        {
            Revalidate();
            Launches++;
            return await execute(command, token);
        }
    }

    private sealed class TempRoot : IDisposable
    {
        internal readonly string Container = Path.Combine(Path.GetTempPath(), "sg-mig-10-" + Guid.NewGuid().ToString("N"));
        internal string Checkout { get; }
        internal Workspace Workspace { get; }
        private readonly bool ownsCheckout;

        internal TempRoot(string? existingCheckout = null)
        {
            Directory.CreateDirectory(Container);
            Checkout = existingCheckout ?? Path.Combine(Container, "checkout");
            ownsCheckout = existingCheckout is null;
            Directory.CreateDirectory(Checkout);
            Workspace = new Workspace(Checkout, Path.Combine(Container, "state"));
        }

        internal void WriteProfile(params string[] operations) => WriteProfile(operations, []);
        internal void WriteProfile(string[] operations, string[] outputRoots)
        {
            Workspace.CreatePrivateDirectory(Workspace.StateDirectory);
            var text = JsonSerializer.Serialize(new
            {
                schemaVersion = 1, rootIdentity = RootIdentity.Read(Checkout),
                operations, outputRoots
            });
            File.WriteAllText(ExecutionProfile.PathFor(Workspace), text);
        }

        public void Dispose()
        {
            if (ownsCheckout && Directory.Exists(Checkout))
                Directory.Delete(Checkout, recursive: true);
            if (Directory.Exists(Container))
                Directory.Delete(Container, recursive: true);
        }
    }

    private sealed class RunnerFixture : IDisposable
    {
        private readonly string container = Path.Combine(Path.GetTempPath(), "sg-mig-10-runner-" + Guid.NewGuid().ToString("N"));
        internal string AssemblyPath => Path.Combine(container, "output", "Case.dll");
        internal VstestAttemptRequest Request { get; }

        internal RunnerFixture()
        {
            var sdk = Path.Combine(container, "tools", "sdk", "10.0.404");
            var output = Path.Combine(container, "output");
            var results = Path.Combine(container, "results");
            Directory.CreateDirectory(sdk);
            Directory.CreateDirectory(output);
            Directory.CreateDirectory(results);
            VstestVerifiedFile Write(string path, string value)
            {
                File.WriteAllText(path, value);
                return new VstestVerifiedFile(path, Hash(Encoding.UTF8.GetBytes(value)));
            }
            var host = Write(Path.Combine(container, "tools", OperatingSystem.IsWindows() ? "dotnet.exe" : "dotnet"), "host");
            var runner = Write(Path.Combine(sdk, "vstest.console.dll"), "runner");
            var assembly = Write(AssemblyPath, "assembly");
            var deps = Write(Path.Combine(output, "Case.deps.json"), "{}");
            var runtime = Write(Path.Combine(output, "Case.runtimeconfig.json"), "{}");
            var paths = new[] { assembly, deps, runtime };
            var mappings = paths.Select(file => new BuildArtifactPathMapping("out/" + Path.GetFileName(file.Path),
                file.Path)).ToArray();
            var target = new ProjectTarget("Tests/Case.csproj|net10.0", "Tests/Case.csproj",
                "net10.0", "Debug", "AnyCPU", "", "10.0.404", true, [], [], new string('A', 64));
            var manifest = new BuildArtifactManifest(target, true, "map/v1", "vstest", "xunit/v3",
                [], [], [], [], [], mappings.Select(mapping => mapping.LogicalPath).ToArray());
            var captured = BuildArtifactCapture.Capture(manifest,
                new BuildArtifactReadScope([output], mappings, MappingTrusted: true));
            Assert.True(captured.Stamp.Complete);
            var attempt = new TestAttempt(42, 1,
                new ScheduleFence(Guid.NewGuid(), 1, 1, "physical-root", new GitRevision(null, null)),
                new SelectedTarget(target.Id, new string('B', 64), ["cause"]), "test-policy");
            Request = new VstestAttemptRequest(attempt, host, runner, "10.0.404", captured.Stamp,
                captured, assembly, [deps, runtime], results, TimeSpan.FromSeconds(30));
        }

        public void Dispose()
        {
            if (Directory.Exists(container))
                Directory.Delete(container, recursive: true);
        }
    }
}
