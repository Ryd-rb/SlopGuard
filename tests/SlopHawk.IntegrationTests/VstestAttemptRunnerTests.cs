using System.Diagnostics;
using System.Security.Cryptography;
using System.Text;
using SlopHawk.Daemon;
using Xunit;

namespace SlopHawk.IntegrationTests;

public sealed class VstestAttemptRunnerTests
{
    [Theory]
    [InlineData("Completed", 1, 1, 1, 0, 0, (int)VstestDisposition.Passed)]
    [InlineData("Failed", 1, 1, 0, 1, 1, (int)VstestDisposition.Failed)]
    [InlineData("Completed", 0, 0, 0, 0, 0, (int)VstestDisposition.ZeroDiscovered)]
    [InlineData("Completed", 1, 0, 0, 0, 0, (int)VstestDisposition.AllSkipped)]
    public async Task CompleteReceiptsUseTheOwnedAttemptAndTheActualExitCode(string outcome,
        int total, int executed, int passed, int failed, int exitCode, int expected)
    {
        using var setup = new Setup();
        var execution = new FakeExecution(async (command, token) =>
        {
            AssertTypedAssemblyOnlyCommand(setup, command);
            var bytes = Trx(outcome, total, executed, passed, failed);
            await File.WriteAllBytesAsync(ResultFile(command), bytes, token);
            return new WorkerResult(exitCode, "runner stdout", "runner stderr");
        });
        var result = await VstestAttemptRunner.RunForTestsAsync(setup.Request, execution, CancellationToken.None);

        Assert.Equal(VstestAttemptGate.Launched, result.Gate);
        Assert.True((VstestDisposition)expected == result.Classification!.Disposition, $"Expected={(VstestDisposition)expected}; Actual={result.Classification.Disposition}; Health={result.Classification.Artifact.Health}; Reason={result.Classification.Reason}; LastWrite={result.Process?.ArtifactLastWriteUtc:O}; Start={result.Process?.StartedUtc:O}; ArtifactFinished={result.Classification.Artifact.FinishedUtc:O}");
        Assert.Equal(VstestArtifactHealth.Complete, result.Classification.Artifact.Health);
        Assert.True(result.Process!.Exited);
        Assert.Equal(exitCode, result.Process.ExitCode);
        Assert.Equal(Convert.ToHexString(SHA256.HashData(await File.ReadAllBytesAsync(
            Path.Combine(result.ResultsDirectory!, "results.trx")))), result.Classification.Artifact.Sha256);
        Assert.Equal("runner stdout", result.StandardOutput);
        Assert.Equal("runner stderr", result.StandardError);
        Assert.Equal(1, execution.Calls);
    }

    [Fact]
    public async Task MissingOrUnmatchedBuildProvenanceNeverStartsVstest()
    {
        using var setup = new Setup();
        var execution = new FakeExecution((_, _) => throw new Exception("Should not launch"));
        foreach (var request in new[]
        {
            setup.Request with { BuiltIdentity = null },
            setup.Request with { CurrentBuild = null },
            setup.Request with { CurrentBuild = setup.Request.CurrentBuild! with { TargetId = "different target" } },
            setup.Request with { BuiltIdentity = setup.Request.BuiltIdentity! with { DomainVersion = 0 } },
            setup.Request with { BuiltIdentity = setup.Request.BuiltIdentity! with { Sha256 = new string('F', 64) } },
            setup.Request with { Assembly = setup.Request.Assembly with { Sha256 = new string('F', 64) } }
        })
        {
            var result = await VstestAttemptRunner.RunForTestsAsync(request, execution, CancellationToken.None);
            Assert.Equal(VstestAttemptGate.MissingBuildProvenance, result.Gate);
            Assert.Null(result.Classification);
            Assert.Null(result.ResultsDirectory);
        }
        Assert.Equal(0, execution.Calls);
    }

    [Fact]
    public async Task SameRootStaleReceiptCannotSatisfyANewAttempt()
    {
        using var setup = new Setup();
        // A previous worker or attacker left a convincing TRX at the leased root.
        File.WriteAllBytes(Path.Combine(setup.ResultsRoot, "results.trx"), Trx("Completed", 1, 1, 1, 0));
        var execution = new FakeExecution((command, _) =>
        {
            Assert.False(File.Exists(ResultFile(command)));
            return Task.FromResult(new WorkerResult(0, "", ""));
        });
        var result = await VstestAttemptRunner.RunForTestsAsync(setup.Request, execution, CancellationToken.None);
        Assert.Equal(VstestArtifactHealth.Missing, result.Classification!.Artifact.Health);
        Assert.Equal(VstestDisposition.ArtifactFailure, result.Classification.Disposition);
        Assert.NotEqual(setup.ResultsRoot, result.ResultsDirectory);
        Assert.True(File.Exists(Path.Combine(setup.ResultsRoot, "results.trx")));
    }

    [Fact]
    public async Task SuccessiveRunsNeverReuseAnEarlierAttemptSubdirectory()
    {
        using var setup = new Setup();
        var execution = Writer(() => Trx("Completed", 1, 1, 1, 0));
        var one = await VstestAttemptRunner.RunForTestsAsync(setup.Request, execution, CancellationToken.None);
        var two = await VstestAttemptRunner.RunForTestsAsync(setup.Request, execution, CancellationToken.None);
        Assert.Equal(VstestDisposition.Passed, one.Classification!.Disposition);
        Assert.Equal(VstestDisposition.Passed, two.Classification!.Disposition);
        Assert.NotEqual(one.ResultsDirectory, two.ResultsDirectory);
        Assert.Equal(2, execution.Calls);
    }

    [Fact]
    public async Task ExtraOrMultipleResultsAreRejectedWithoutChoosingAFavoriteTrx()
    {
        using var setup = new Setup();
        var execution = new FakeExecution(async (command, token) =>
        {
            await File.WriteAllBytesAsync(ResultFile(command), Trx("Completed", 1, 1, 1, 0), token);
            await File.WriteAllBytesAsync(Path.Combine(Path.GetDirectoryName(ResultFile(command))!, "other.trx"),
                Trx("Completed", 1, 1, 1, 0), token);
            return new WorkerResult(0, "", "");
        });
        var result = await VstestAttemptRunner.RunForTestsAsync(setup.Request, execution, CancellationToken.None);
        Assert.Equal(VstestDisposition.ArtifactFailure, result.Classification!.Disposition);
        Assert.Equal(VstestArtifactHealth.Inconsistent, result.Classification.Artifact.Health);
    }

    [Fact]
    public async Task MalformedTrxCannotOverrideAConfirmedZeroExit()
    {
        using var setup = new Setup();
        var result = await VstestAttemptRunner.RunForTestsAsync(setup.Request,
            Writer(() => Encoding.UTF8.GetBytes("<TestRun>")), CancellationToken.None);
        Assert.Equal(VstestDisposition.ArtifactFailure, result.Classification!.Disposition);
        Assert.Equal(VstestArtifactHealth.Malformed, result.Classification.Artifact.Health);
    }

    [Fact]
    public async Task LinkedTrxIsNotReadOrClassified()
    {
        if (!OperatingSystem.IsLinux())
            return; // Creating a symlink requires different privileges on Windows.
        using var setup = new Setup();
        var outside = Path.Combine(setup.Root, "outside.trx");
        File.WriteAllBytes(outside, Trx("Completed", 1, 1, 1, 0));
        var execution = new FakeExecution((command, _) =>
        {
            File.CreateSymbolicLink(ResultFile(command), outside);
            return Task.FromResult(new WorkerResult(0, "", ""));
        });
        var result = await VstestAttemptRunner.RunForTestsAsync(setup.Request, execution, CancellationToken.None);
        Assert.Equal(VstestDisposition.ArtifactFailure, result.Classification!.Disposition);
        Assert.Equal(VstestArtifactHealth.Inconsistent, result.Classification.Artifact.Health);
    }

    [Fact]
    public async Task OldTimestampInsideAUniqueDirectoryIsRejectedAsStale()
    {
        using var setup = new Setup();
        var execution = new FakeExecution(async (command, token) =>
        {
            var path = ResultFile(command);
            await File.WriteAllBytesAsync(path, Trx("Completed", 1, 1, 1, 0), token);
            File.SetLastWriteTimeUtc(path, DateTime.UtcNow.AddMinutes(-5));
            return new WorkerResult(0, "", "");
        });
        var result = await VstestAttemptRunner.RunForTestsAsync(setup.Request, execution, CancellationToken.None);
        Assert.Equal(VstestDisposition.ArtifactFailure, result.Classification!.Disposition);
        Assert.Equal(VstestArtifactHealth.Stale, result.Classification.Artifact.Health);
    }

    [Fact]
    public async Task OversizedReceiptDoesNotNeedToBeLoadedIntoMemory()
    {
        using var setup = new Setup();
        var execution = new FakeExecution((command, _) =>
        {
            using (var stream = File.Create(ResultFile(command)))
                stream.SetLength(VstestTrxClassifier.MaxArtifactBytes + 1);
            return Task.FromResult(new WorkerResult(0, "", ""));
        });
        var result = await VstestAttemptRunner.RunForTestsAsync(setup.Request, execution, CancellationToken.None);
        Assert.Equal(VstestArtifactHealth.Oversized, result.Classification!.Artifact.Health);
        Assert.Null(result.Classification.Artifact.Sha256);
    }

    [Fact]
    public async Task ChangedBuildBytesAfterTestCannotBeReportedAsPassed()
    {
        using var setup = new Setup();
        var execution = new FakeExecution(async (command, token) =>
        {
            await File.WriteAllBytesAsync(ResultFile(command), Trx("Completed", 1, 1, 1, 0), token);
            await File.AppendAllTextAsync(setup.AssemblyPath, "tamper", token);
            return new WorkerResult(0, "", "");
        });
        var result = await VstestAttemptRunner.RunForTestsAsync(setup.Request, execution, CancellationToken.None);
        Assert.Equal(VstestDisposition.ArtifactFailure, result.Classification!.Disposition);
        Assert.NotEqual(VstestDisposition.Passed, result.Classification.Disposition);
    }

    [Fact]
    public async Task DeniedAuthorityBeforeStartDoesNotLaunch()
    {
        using var setup = new Setup();
        var execution = Writer(() => Trx("Completed", 1, 1, 1, 0));
        execution.Allowed = false;
        var result = await VstestAttemptRunner.RunForTestsAsync(setup.Request, execution, CancellationToken.None);
        Assert.Equal(VstestAttemptGate.DeniedAuthority, result.Gate);
        Assert.Null(result.Classification);
        Assert.Equal(0, execution.Calls);
    }

    [Fact]
    public async Task AnOutputRootOutsideTheLeaseIsRejectedBeforeCreatingAnAttemptDirectory()
    {
        using var setup = new Setup();
        var execution = Writer(() => Trx("Completed", 1, 1, 1, 0));
        execution.AllowedRoot = Path.Combine(setup.Root, "different-output");
        var result = await VstestAttemptRunner.RunForTestsAsync(setup.Request, execution, CancellationToken.None);

        Assert.Equal(VstestAttemptGate.DeniedAuthority, result.Gate);
        Assert.Equal(0, execution.Calls);
        Assert.Empty(Directory.EnumerateFileSystemEntries(setup.ResultsRoot));
    }

    [Fact]
    public async Task AuthorityLostInsideTheLeaseLaunchIsTypedAndNeverAccepted()
    {
        using var setup = new Setup();
        var execution = new FakeExecution((_, _) =>
            throw new UnauthorizedAccessException("Revoked between wrapper and lease launch."));
        var result = await VstestAttemptRunner.RunForTestsAsync(setup.Request, execution, CancellationToken.None);
        Assert.Equal(VstestAttemptGate.DeniedAuthority, result.Gate);
        Assert.Null(result.Classification);
        Assert.Equal(1, execution.Calls);
    }

    [Fact]
    public async Task RevocationAfterExitFencesResultAcceptance()
    {
        using var setup = new Setup();
        FakeExecution? execution = null;
        execution = new FakeExecution(async (command, token) =>
        {
            await File.WriteAllBytesAsync(ResultFile(command), Trx("Completed", 1, 1, 1, 0), token);
            execution!.Allowed = false;
            return new WorkerResult(0, "", "");
        });
        var result = await VstestAttemptRunner.RunForTestsAsync(setup.Request, execution, CancellationToken.None);
        Assert.Equal(VstestAttemptGate.DeniedAuthority, result.Gate);
        Assert.Null(result.Classification);
        Assert.Equal(1, execution.Calls);
    }

    [Fact]
    public async Task TimeoutIsNotAZeroDiscoveryOrFailedTest()
    {
        using var setup = new Setup();
        var execution = new FakeExecution(async (_, token) =>
        {
            await Task.Delay(Timeout.InfiniteTimeSpan, token);
            throw new Exception("unreachable");
        });
        var request = setup.Request with { Timeout = TimeSpan.FromSeconds(1) };
        var result = await VstestAttemptRunner.RunForTestsAsync(request, execution, CancellationToken.None);
        Assert.Equal(VstestDisposition.Timeout, result.Classification!.Disposition);
        Assert.Equal(VstestRunnerHealth.TimedOut, result.Classification.Runner);
        Assert.False(result.Process!.Exited);
    }

    [Fact]
    public async Task CallerCancellationIsDistinctFromTimeout()
    {
        using var setup = new Setup();
        using var stop = new CancellationTokenSource(TimeSpan.FromMilliseconds(100));
        var execution = new FakeExecution(async (_, token) =>
        {
            await Task.Delay(Timeout.InfiniteTimeSpan, token);
            throw new Exception("unreachable");
        });
        var result = await VstestAttemptRunner.RunForTestsAsync(setup.Request, execution, stop.Token);
        Assert.Equal(VstestDisposition.Cancelled, result.Classification!.Disposition);
        Assert.Equal(VstestRunnerHealth.Cancelled, result.Classification.Runner);
        Assert.False(result.Process!.Exited);
    }

    [Fact]
    public async Task UnconfirmedExitNeverAcceptsAnOtherwisePassingReceipt()
    {
        using var setup = new Setup();
        var execution = new FakeExecution(async (command, token) =>
        {
            await File.WriteAllBytesAsync(ResultFile(command), Trx("Completed", 1, 1, 1, 0), token);
            throw new IOException("Process scope was not confirmed empty.");
        });
        var result = await VstestAttemptRunner.RunForTestsAsync(setup.Request, execution, CancellationToken.None);
        Assert.False(result.Process!.Exited);
        Assert.Null(result.Process.ExitCode);
        Assert.True(result.Classification!.Disposition == VstestDisposition.Incomplete, $"Actual={result.Classification.Disposition}, Health={result.Classification.Artifact.Health}, Reason={result.Classification.Reason}, Start={result.Process?.StartedUtc:O}, LastWrite={result.Process?.ArtifactLastWriteUtc:O}, TrxFinish={result.Classification.Artifact.FinishedUtc:O}");
        Assert.Null(result.Classification.SchedulerOutcome);
    }

    [Fact]
    public async Task TamperedRunnerOrMissingCompanionCannotLaunch()
    {
        using var setup = new Setup();
        var execution = Writer(() => Trx("Completed", 1, 1, 1, 0));
        var altered = setup.Request with { DotnetHost = setup.Request.DotnetHost with { Sha256 = new string('F', 64) } };
        var failed = await VstestAttemptRunner.RunForTestsAsync(altered, execution, CancellationToken.None);
        Assert.Equal(VstestAttemptGate.InvalidInputs, failed.Gate);
        File.Delete(Path.Combine(Path.GetDirectoryName(setup.AssemblyPath)!, "Case.runtimeconfig.json"));
        failed = await VstestAttemptRunner.RunForTestsAsync(setup.Request, execution, CancellationToken.None);
        Assert.Equal(VstestAttemptGate.InvalidInputs, failed.Gate);
        Assert.Equal(0, execution.Calls);
    }

    [Fact]
    public async Task BoundedMetadataDoesNotExposeEntireRunnerStreams()
    {
        using var setup = new Setup();
        var execution = new FakeExecution(async (command, token) =>
        {
            await File.WriteAllBytesAsync(ResultFile(command), Trx("Completed", 1, 1, 1, 0), token);
            return new WorkerResult(0, new string('A', 40_000), new string('B', 40_000));
        });
        var result = await VstestAttemptRunner.RunForTestsAsync(setup.Request, execution, CancellationToken.None);
        Assert.Equal(VstestAttemptRunner.MaxOutputCharacters, result.StandardOutput.Length);
        Assert.Equal(VstestAttemptRunner.MaxOutputCharacters, result.StandardError.Length);
    }

    [VstestFixtureFact]
    public async Task ConfiguredSdkRunsAnActualPrebuiltAssembly()
    {
        var host = Environment.GetEnvironmentVariable("SLOPHAWK_VSTEST_DOTNET")!;
        var assembly = Environment.GetEnvironmentVariable("SLOPHAWK_VSTEST_FIXTURE_DLL")!;
        using var setup = new Setup();
        var sdk = Environment.GetEnvironmentVariable("SLOPHAWK_VSTEST_SDK_VERSION") ?? "10.0.404";
        var runner = Path.Combine(Path.GetDirectoryName(host)!, "sdk", sdk, "vstest.console.dll");
        static VstestVerifiedFile Verified(string path) => new(Path.GetFullPath(path),
            Convert.ToHexString(SHA256.HashData(File.ReadAllBytes(path))));
        var files = Directory.EnumerateFiles(Path.GetDirectoryName(assembly)!).ToArray();
        var companions = files.Where(path => !Path.GetFullPath(path).Equals(Path.GetFullPath(assembly),
            OperatingSystem.IsWindows() ? StringComparison.OrdinalIgnoreCase : StringComparison.Ordinal))
            .Select(Verified).ToArray();
        var verifiedAssembly = Verified(assembly);
        var build = setup.ObserveBuild(sdk, verifiedAssembly, companions);
        var request = setup.Request with
        {
            DotnetHost = Verified(host), VstestConsole = Verified(runner), SdkVersion = sdk,
            Assembly = verifiedAssembly, Companions = companions,
            BuiltIdentity = build.Stamp, CurrentBuild = build
        };
        VstestAttemptResult report;
        if (OperatingSystem.IsWindows())
        {
            var root = Directory.CreateDirectory(Path.Combine(setup.Root, "fixture-workspace")).FullName;
            var workspace = new Workspace(root, Path.Combine(setup.Root, "fixture-state"));
            var grant = new ExecutionGrant(request.Attempt.ProfileRevision, RootIdentity.Read(root),
                ExecutionOperation.Test, [setup.ResultsRoot], FromProfile: false);
            var (lease, waiting) = new WorkerAdmission(workspace)
                .TryAcquire(grant, new WorkerRequirements(ExecutionOperation.Test, true, [setup.ResultsRoot]));
            Assert.True(lease is not null, waiting);
            await using var worker = lease!;
            report = await VstestAttemptRunner.RunAsync(request, worker, CancellationToken.None);
        }
        else
        {
            report = await VstestAttemptRunner.RunForTestsAsync(request,
                new PortableProcessExecution(), CancellationToken.None);
        }
        var expectFailure = Environment.GetEnvironmentVariable("SLOPHAWK_VSTEST_FIXTURE_EXPECTED") == "Failed";
        var disposition = expectFailure ? VstestDisposition.Failed : VstestDisposition.Passed;
        Assert.Equal(VstestAttemptGate.Launched, report.Gate);
        Assert.True(report.Classification!.Disposition == disposition,
            $"Actual={report.Classification.Disposition}, Reason={report.Classification.Reason}, stdout={report.StandardOutput}, stderr={report.StandardError}");
        Assert.Equal(VstestArtifactHealth.Complete, report.Classification.Artifact.Health);
        Assert.Equal(expectFailure ? 1 : 0, report.Process!.ExitCode);
        Assert.True(report.Process.Exited);
    }

    public sealed class VstestFixtureFactAttribute : FactAttribute
    {
        public VstestFixtureFactAttribute()
        {
            if (string.IsNullOrEmpty(Environment.GetEnvironmentVariable("SLOPHAWK_VSTEST_DOTNET")) ||
                string.IsNullOrEmpty(Environment.GetEnvironmentVariable("SLOPHAWK_VSTEST_FIXTURE_DLL")))
                Skip = "An explicit SDK and prebuilt VSTest fixture are required.";
        }
    }

    // A runner-format integration exercise on Linux, deliberately NOT the Windows
    // Job Object/process-tree supervisor used by WorkerLease in production.
    private sealed class PortableProcessExecution : IVstestAttemptExecution
    {
        public string ProfileRevision => "test-profile";
        public void Revalidate() { }
        public void RequireResultsRoot(string path) { }
        public async Task<WorkerResult> RunAsync(ProcessStartInfo command, int maxOutput, CancellationToken cancellationToken)
        {
            command.RedirectStandardOutput = true;
            command.RedirectStandardError = true;
            using var process = Process.Start(command) ?? throw new IOException("Could not start portable VSTest.");
            var output = process.StandardOutput.ReadToEndAsync(cancellationToken);
            var errors = process.StandardError.ReadToEndAsync(cancellationToken);
            try
            {
                await process.WaitForExitAsync(cancellationToken);
                var stdout = await output;
                var stderr = await errors;
                if (stdout.Length > maxOutput || stderr.Length > maxOutput)
                    throw new IOException("Portable fixture exceeded its output budget.");
                return new WorkerResult(process.ExitCode, stdout, stderr);
            }
            catch
            {
                if (!process.HasExited)
                    process.Kill(entireProcessTree: true);
                await process.WaitForExitAsync(CancellationToken.None);
                throw;
            }
        }
    }

    private static FakeExecution Writer(Func<byte[]> receipt) => new(async (command, token) =>
    {
        await File.WriteAllBytesAsync(ResultFile(command), receipt(), token);
        return new WorkerResult(0, "", "");
    });

    private static void AssertTypedAssemblyOnlyCommand(Setup setup, ProcessStartInfo command)
    {
        Assert.Equal(setup.Request.DotnetHost.Path, command.FileName);
        Assert.False(command.UseShellExecute);
        Assert.Equal("", command.Arguments);
        Assert.Equal("exec", command.ArgumentList[0]);
        Assert.Equal(setup.Request.VstestConsole.Path, command.ArgumentList[1]);
        Assert.Equal(setup.AssemblyPath, command.ArgumentList[2]);
        Assert.Equal("/Logger:trx;LogFileName=results.trx", command.ArgumentList[3]);
        Assert.StartsWith("/ResultsDirectory:", command.ArgumentList[4]);
        Assert.Equal(5, command.ArgumentList.Count);
        Assert.True(Directory.Exists(Path.GetDirectoryName(ResultFile(command))));
    }

    private static string ResultFile(ProcessStartInfo command) =>
        Path.Combine(command.ArgumentList.Last()["/ResultsDirectory:".Length..], "results.trx");

    private static byte[] Trx(string outcome, int total, int executed, int passed, int failed)
    {
        var started = DateTimeOffset.UtcNow.AddMilliseconds(-100).ToString("O");
        var finished = DateTimeOffset.UtcNow.ToString("O");
        var results = total == 0 ? "" : failed > 0
            ? "<UnitTestResult outcome=\"Failed\" testName=\"Case\" />"
            : executed == 0 ? "<UnitTestResult outcome=\"NotExecuted\" testName=\"Case\" />"
            : "<UnitTestResult outcome=\"Passed\" testName=\"Case\" />";
        return Encoding.UTF8.GetBytes($"""
            <?xml version="1.0" encoding="utf-8"?>
            <TestRun id="01234567-89ab-cdef-0123-456789abcdef" xmlns="http://microsoft.com/schemas/VisualStudio/TeamTest/2010">
              <Times start="{started}" finish="{finished}" />
              <Results>{results}</Results>
              <ResultSummary outcome="{outcome}"><Counters total="{total}" executed="{executed}" passed="{passed}" failed="{failed}" /></ResultSummary>
            </TestRun>
            """);
    }

    private sealed class FakeExecution(Func<ProcessStartInfo, CancellationToken, Task<WorkerResult>> run)
        : IVstestAttemptExecution
    {
        internal bool Allowed = true;
        internal int Calls;
        internal string? AllowedRoot;
        public string ProfileRevision => "test-profile";
        public void Revalidate()
        {
            if (!Allowed)
                throw new UnauthorizedAccessException("Test profile revoked.");
        }
        public void RequireResultsRoot(string path)
        {
            Revalidate();
            if (AllowedRoot is not null && !ExecutionProfile.Contains(AllowedRoot, path))
                throw new UnauthorizedAccessException("The fixture did not lease the output root.");
        }
        public Task<WorkerResult> RunAsync(ProcessStartInfo command, int maxOutput, CancellationToken cancellationToken)
        {
            Calls++;
            Assert.Equal(16_384, maxOutput);
            Revalidate();
            return run(command, cancellationToken);
        }
    }

    private sealed class Setup : IDisposable
    {
        private const string TargetId = "tests/Case.csproj|net10.0";
        internal string Root { get; } = Path.Combine(Path.GetTempPath(), "sg12-vstest-" + Guid.NewGuid().ToString("N"));
        internal string ResultsRoot => Path.Combine(Root, "results");
        internal string AssemblyPath => Path.Combine(Root, "built", "Case.dll");
        internal VstestAttemptRequest Request { get; }

        internal Setup()
        {
            Directory.CreateDirectory(Path.Combine(Root, "tools", "sdk", "10.0.404"));
            Directory.CreateDirectory(Path.Combine(Root, "built"));
            Directory.CreateDirectory(ResultsRoot);
            var host = Write(Path.Combine(Root, "tools", OperatingSystem.IsWindows() ? "dotnet.exe" : "dotnet"),
                "approved host");
            var runner = Write(Path.Combine(Root, "tools", "sdk", "10.0.404", "vstest.console.dll"), "approved runner");
            var assembly = Write(AssemblyPath, "prebuilt assembly");
            var deps = Write(Path.Combine(Root, "built", "Case.deps.json"), "{}");
            var runtime = Write(Path.Combine(Root, "built", "Case.runtimeconfig.json"), "{}");
            var build = ObserveBuild("10.0.404", assembly, [deps, runtime]);
            Request = new VstestAttemptRequest(
                new TestAttempt(42, 1, new ScheduleFence(Guid.NewGuid(), 1, 1, "root", null!),
                    new SelectedTarget(TargetId, new string('A', 64), ["cause"]), "test-profile"),
                host, runner, "10.0.404", build.Stamp, build, assembly, [deps, runtime], ResultsRoot,
                TimeSpan.FromSeconds(30));
        }

        internal BuildArtifactCaptureResult ObserveBuild(string sdk, VstestVerifiedFile assembly,
            IReadOnlyList<VstestVerifiedFile> companions)
        {
            var mappings = companions.Prepend(assembly).Select(file =>
                new BuildArtifactPathMapping("out/" + Path.GetFileName(file.Path), file.Path)).ToArray();
            var target = new ProjectTarget(TargetId, "tests/Case.csproj", "net10.0", "Debug",
                "AnyCPU", "", sdk, true, [], [], new string('A', 64));
            var manifest = new BuildArtifactManifest(target, true, "root/v1", "VSTest", "xunit/3.1.5",
                [], [], [], [], [], mappings.Select(mapping => mapping.LogicalPath).ToArray());
            var result = BuildArtifactCapture.Capture(manifest,
                new BuildArtifactReadScope([Path.GetDirectoryName(assembly.Path)!], mappings, MappingTrusted: true));
            Assert.True(result.Stamp.Complete, string.Join(", ", result.Stamp.Problems));
            return result;
        }

        private static VstestVerifiedFile Write(string path, string content)
        {
            File.WriteAllText(path, content);
            return new VstestVerifiedFile(path,
                Convert.ToHexString(SHA256.HashData(File.ReadAllBytes(path))));
        }

        public void Dispose()
        {
            if (Directory.Exists(Root))
                Directory.Delete(Root, recursive: true);
        }
    }
}
