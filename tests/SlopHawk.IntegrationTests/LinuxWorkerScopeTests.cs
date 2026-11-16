using System.Text.Json;
using SlopHawk.Daemon;
using SlopHawk.WorkerFixture;
using Xunit;

namespace SlopHawk.IntegrationTests;

public sealed class LinuxWorkerScopeTests
{
    private const string Boot = "11111111-2222-3333-4444-555555555555";

    [Fact]
    public void JournalSurvivesASecondOwnerAndRejectsAChangedLease()
    {
        using var state = new TempDirectory();
        var first = new LinuxScopeJournal(state.Path);
        var lease = Guid.NewGuid();
        var pending = LinuxScopeIdentity.New(lease, Boot);
        first.Save(pending);
        Assert.Equal(pending, new LinuxScopeJournal(state.Path).Read(lease));
        var started = pending with { InvocationId = Guid.NewGuid().ToString("N"),
            ControlGroup = "/user.slice/app.slice/" + pending.UnitName };
        first.Save(started);
        Assert.Equal(started, new LinuxScopeJournal(state.Path).Read(lease));
        Assert.Throws<FileNotFoundException>(() => new LinuxScopeJournal(state.Path).Read(Guid.NewGuid()));
    }

    [Fact]
    public async Task FreshOwnerCanInspectAndDrainNestedScopeWithoutPids()
    {
        using var state = new TempDirectory();
        var lease = Guid.NewGuid();
        var identity = Launched(lease);
        new LinuxScopeJournal(state.Path).Save(identity);
        var manager = new FakeManager(identity) { Population = LinuxCgroupPopulation.Populated };
        var recovered = new LinuxWorkerScope(manager, new LinuxScopeJournal(state.Path), Boot);
        Assert.Equal(LinuxScopeDisposition.Populated, await recovered.InspectAsync(lease, CancellationToken.None));
        await recovered.StopAndDrainAsync(lease, TimeSpan.FromSeconds(1));
        Assert.Equal(1, manager.Kills);
        Assert.Equal(LinuxScopeDisposition.Drained, await recovered.InspectAsync(lease, CancellationToken.None));
    }

    [Theory]
    [InlineData((int)LinuxCgroupPopulation.Populated, "active", "running", (int)LinuxScopeDisposition.Populated)]
    [InlineData((int)LinuxCgroupPopulation.Empty, "active", "exited", (int)LinuxScopeDisposition.Drained)]
    [InlineData((int)LinuxCgroupPopulation.Missing, "failed", "failed", (int)LinuxScopeDisposition.Drained)]
    [InlineData((int)LinuxCgroupPopulation.Empty, "active", "running", (int)LinuxScopeDisposition.AmbiguousRecovery)]
    [InlineData((int)LinuxCgroupPopulation.Unknown, "inactive", "dead", (int)LinuxScopeDisposition.AmbiguousRecovery)]
    [InlineData((int)LinuxCgroupPopulation.Empty, "activating", "start", (int)LinuxScopeDisposition.AmbiguousRecovery)]
    public async Task ObservationsDoNotConfuseActivityWithProofOfDrainage(
        int population, string active, string sub, int expected)
    {
        using var state = new TempDirectory();
        var lease = Guid.NewGuid();
        var identity = Launched(lease);
        new LinuxScopeJournal(state.Path).Save(identity);
        var manager = new FakeManager(identity) { Population = (LinuxCgroupPopulation)population, ActiveState = active, SubState = sub };
        var scope = new LinuxWorkerScope(manager, new LinuxScopeJournal(state.Path), Boot);
        Assert.Equal((LinuxScopeDisposition)expected, await scope.InspectAsync(lease, CancellationToken.None));
    }

    [Fact]
    public async Task RecycledNameFailedQueryUnloadedUnitAndChangedBootAllHoldReservation()
    {
        using var state = new TempDirectory();
        var lease = Guid.NewGuid();
        var identity = Launched(lease);
        var journal = new LinuxScopeJournal(state.Path);
        journal.Save(identity);
        var manager = new FakeManager(identity) { Population = LinuxCgroupPopulation.Empty,
            ActiveState = "inactive", SubState = "dead" };
        var scope = new LinuxWorkerScope(manager, journal, Boot);
        manager.InvocationId = Guid.NewGuid().ToString("N");
        Assert.Equal(LinuxScopeDisposition.AmbiguousRecovery, await scope.InspectAsync(lease, CancellationToken.None));
        manager.InvocationId = identity.InvocationId!;
        manager.UnitLoaded = false;
        Assert.Equal(LinuxScopeDisposition.AmbiguousRecovery, await scope.InspectAsync(lease, CancellationToken.None));
        manager.UnitLoaded = true;
        manager.ManagerAvailable = false;
        Assert.Equal(LinuxScopeDisposition.Unavailable, await scope.InspectAsync(lease, CancellationToken.None));
        manager.ManagerAvailable = true;
        manager.Throws = true;
        Assert.Equal(LinuxScopeDisposition.Unavailable, await scope.InspectAsync(lease, CancellationToken.None));
        manager.Throws = false;
        Assert.Equal(LinuxScopeDisposition.AmbiguousRecovery,
            await new LinuxWorkerScope(manager, journal, Guid.NewGuid().ToString()).InspectAsync(lease, CancellationToken.None));
        Assert.Equal(0, manager.Kills);
    }

    [Fact]
    public async Task CancellationDoesNotSignalAnAmbiguousNameOrReleaseOnFailedDrain()
    {
        using var state = new TempDirectory();
        var lease = Guid.NewGuid();
        var identity = Launched(lease);
        var journal = new LinuxScopeJournal(state.Path);
        journal.Save(identity);
        var manager = new FakeManager(identity) { InvocationId = Guid.NewGuid().ToString("N") };
        var scope = new LinuxWorkerScope(manager, journal, Boot);
        await Assert.ThrowsAsync<IOException>(() => scope.StopAndDrainAsync(lease, TimeSpan.Zero));
        Assert.Equal(0, manager.Kills);
        manager.InvocationId = identity.InvocationId!;
        manager.DrainOnKill = false;
        await Assert.ThrowsAsync<IOException>(() => scope.StopAndDrainAsync(lease, TimeSpan.FromMilliseconds(100)));
        Assert.Equal(1, manager.Kills);
        Assert.Equal(LinuxScopeDisposition.Populated, await scope.InspectAsync(lease, CancellationToken.None));
    }

    [Fact]
    public async Task UnacknowledgedLaunchIsAlwaysAmbiguousEvenIfTheNameAppearsLoaded()
    {
        using var state = new TempDirectory();
        var lease = Guid.NewGuid();
        var pending = LinuxScopeIdentity.New(lease, Boot);
        var journal = new LinuxScopeJournal(state.Path);
        journal.Save(pending);
        var manager = new FakeManager(Launched(lease));
        var scope = new LinuxWorkerScope(manager, journal, Boot);
        Assert.Equal(LinuxScopeDisposition.AmbiguousRecovery, await scope.InspectAsync(lease, CancellationToken.None));
    }

    [Fact]
    public async Task FixtureStartPersistsPendingBeforeStartAndRejectsUnavailableManager()
    {
        if (!OperatingSystem.IsLinux()) return;
        using var state = new TempDirectory();
        using var output = new TempDirectory();
        var lease = Guid.NewGuid();
        var manager = new FakeManager(Launched(lease)) { UnitLoaded = false, ManagerAvailable = false };
        var journal = new LinuxScopeJournal(state.Path);
        var scope = new LinuxWorkerScope(manager, journal, Boot);
        var command = Commands.Create("tree", output.Path);
        await Assert.ThrowsAsync<NotSupportedException>(() => scope.StartFixtureAsync(lease, command.FileName,
            typeof(Commands).Assembly.Location, output.Path, CancellationToken.None));
        Assert.Equal(LinuxScopeIdentity.New(lease, Boot), journal.Read(lease));
        Assert.Equal(0, manager.Starts);
    }

    [Fact]
    public async Task AReusedLeaseCannotEraseItsPreviousVerifiedIdentity()
    {
        if (!OperatingSystem.IsLinux()) return;
        using var state = new TempDirectory();
        using var output = new TempDirectory();
        var lease = Guid.NewGuid();
        var identity = Launched(lease);
        var journal = new LinuxScopeJournal(state.Path);
        journal.Save(identity);
        var manager = new FakeManager(identity) { UnitLoaded = true };
        var scope = new LinuxWorkerScope(manager, journal, Boot);
        var command = Commands.Create("tree", output.Path);
        await Assert.ThrowsAsync<IOException>(() => scope.StartFixtureAsync(lease, command.FileName,
            typeof(Commands).Assembly.Location, output.Path, CancellationToken.None));
        Assert.Equal(identity, journal.Read(lease));
        Assert.Equal(0, manager.Starts);
    }

    [Fact]
    public async Task UnverifiableStartAcknowledgmentKeepsThePendingReservation()
    {
        if (!OperatingSystem.IsLinux()) return;
        using var state = new TempDirectory();
        using var output = new TempDirectory();
        var lease = Guid.NewGuid();
        var manager = new FakeManager(Launched(lease)) { UnitLoaded = false,
            PopulationAfterStart = LinuxCgroupPopulation.Unknown };
        var journal = new LinuxScopeJournal(state.Path);
        var scope = new LinuxWorkerScope(manager, journal, Boot);
        var command = Commands.Create("tree", output.Path);
        await Assert.ThrowsAsync<IOException>(() => scope.StartFixtureAsync(lease, command.FileName,
            typeof(Commands).Assembly.Location, output.Path, CancellationToken.None));
        Assert.Equal(LinuxScopeIdentity.New(lease, Boot), journal.Read(lease));
        Assert.Equal(LinuxScopeDisposition.AmbiguousRecovery,
            await scope.InspectAsync(lease, CancellationToken.None));
    }

    [Fact]
    public async Task FixtureStartAllowsOnlyTrustedExecutableAndPersistsValidatedManagerIdentity()
    {
        if (!OperatingSystem.IsLinux()) return;
        using var state = new TempDirectory();
        using var output = new TempDirectory();
        var lease = Guid.NewGuid();
        var manager = new FakeManager(Launched(lease)) { UnitLoaded = false };
        var journal = new LinuxScopeJournal(state.Path);
        manager.BeforeStart = () => Assert.Equal(LinuxScopeIdentity.New(lease, Boot), journal.Read(lease));
        var scope = new LinuxWorkerScope(manager, journal, Boot);
        var command = Commands.Create("tree", output.Path);
        await Assert.ThrowsAsync<UnauthorizedAccessException>(() => scope.StartFixtureAsync(lease,
            "/bin/sh", typeof(Commands).Assembly.Location, output.Path, CancellationToken.None));
        Assert.Equal(0, manager.Starts);
        var identity = await scope.StartFixtureAsync(lease, command.FileName,
            typeof(Commands).Assembly.Location, output.Path, CancellationToken.None);
        Assert.Equal(1, manager.Starts);
        Assert.Equal(identity, journal.Read(lease));
        Assert.NotNull(identity.InvocationId);
        Assert.Equal(LinuxScopeDisposition.Populated, await scope.InspectAsync(lease, CancellationToken.None));
    }

    // This does not run on unsupported hosts. The capability result is separately
    // documented in INTEGRATION.md; a green test run must not imply platform coverage.
    [Fact]
    public async Task OptionalRealUserManagerEnrollsChildAndSurvivesFreshOwner()
    {
        if (!OperatingSystem.IsLinux() || !File.Exists("/sys/fs/cgroup/cgroup.controllers")) return;
        var manager = new LinuxSystemdUserManager();
        var lease = Guid.NewGuid();
        var boot = File.ReadAllText("/proc/sys/kernel/random/boot_id").Trim();
        var available = await manager.ObserveAsync(LinuxScopeIdentity.New(lease, boot).UnitName, null, CancellationToken.None);
        if (!available.ManagerAvailable || available.UnitLoaded) return;
        using var state = new TempDirectory();
        using var output = new TempDirectory();
        var journal = new LinuxScopeJournal(state.Path);
        var scope = new LinuxWorkerScope(manager, journal, boot);
        LinuxScopeIdentity? started = null;
        try
        {
            using (var owner = System.Diagnostics.Process.Start(Commands.Create("linux-scope-owner",
                lease.ToString("N"), state.Path, output.Path))
                ?? throw new IOException("Could not start a separate Linux fixture owner."))
            {
                await owner.WaitForExitAsync().WaitAsync(TimeSpan.FromSeconds(12));
                Assert.Equal(0, owner.ExitCode);
            }
            // A different process created the service. Only its persisted
            // scope identity is available here; no parent process handle.
            started = journal.Read(lease);
            using var deadline = new CancellationTokenSource(TimeSpan.FromSeconds(8));
            var parent = Path.Combine(output.Path, "parent.json");
            var child = Path.Combine(output.Path, "child.json");
            var enrolled = Path.Combine(output.Path, "enrolled.cgroup");
            var childGroup = child + ".cgroup";
            while (!File.Exists(parent) || !File.Exists(child) || !File.Exists(enrolled) || !File.Exists(childGroup))
                await Task.Delay(40, deadline.Token);
            Assert.Equal(started.ControlGroup, File.ReadAllText(enrolled));
            Assert.Equal(started.ControlGroup, File.ReadAllText(childGroup));
            Assert.Equal(LinuxScopeDisposition.Populated,
                await new LinuxWorkerScope(manager, new LinuxScopeJournal(state.Path), boot)
                    .InspectAsync(lease, CancellationToken.None));
            await scope.StopAndDrainAsync(lease, TimeSpan.FromSeconds(8));
            Assert.False(JsonSerializer.Deserialize<FixtureProcess>(File.ReadAllText(parent))!.IsAlive());
            Assert.False(JsonSerializer.Deserialize<FixtureProcess>(File.ReadAllText(child))!.IsAlive());
        }
        finally
        {
            if (started is not null)
            {
                await scope.StopAndDrainAsync(lease, TimeSpan.FromSeconds(8));
                using var cleanup = new CancellationTokenSource(TimeSpan.FromSeconds(5));
                await manager.RemoveDrainedFixtureAsync(started, cleanup.Token);
            }
        }
    }

    private static LinuxScopeIdentity Launched(Guid lease)
    {
        var pending = LinuxScopeIdentity.New(lease, Boot);
        return pending with { InvocationId = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            ControlGroup = "/user.slice/app.slice/" + pending.UnitName };
    }

    private sealed class FakeManager(LinuxScopeIdentity identity) : ILinuxUserScopeManager
    {
        internal bool ManagerAvailable = true;
        internal bool UnitLoaded = true;
        internal bool DrainOnKill = true;
        internal bool Throws;
        internal string InvocationId = identity.InvocationId!;
        internal string ControlGroup = identity.ControlGroup!;
        internal string ActiveState = "active";
        internal string SubState = "running";
        internal LinuxCgroupPopulation Population = LinuxCgroupPopulation.Populated;
        internal LinuxCgroupPopulation PopulationAfterStart = LinuxCgroupPopulation.Populated;
        internal Action? BeforeStart;
        internal int Starts, Kills;

        public Task<LinuxScopeObservation> ObserveAsync(string unitName, string? knownControlGroup, CancellationToken token)
        {
            Assert.Equal(identity.UnitName, unitName);
            if (Throws) throw new IOException("Synthetic manager query fault.");
            return Task.FromResult(new LinuxScopeObservation(ManagerAvailable, UnitLoaded, InvocationId,
                ControlGroup, ActiveState, SubState, Population));
        }

        public Task StartFixtureAsync(string unitName, string host, string fixture, string output, CancellationToken token)
        {
            Assert.Equal(identity.UnitName, unitName);
            Assert.EndsWith("SlopHawk.WorkerFixture.dll", fixture, StringComparison.Ordinal);
            Assert.Equal("dotnet", Path.GetFileName(host));
            BeforeStart?.Invoke();
            Starts++;
            UnitLoaded = true;
            Population = PopulationAfterStart;
            return Task.CompletedTask;
        }

        public Task KillScopeAsync(string unitName, CancellationToken token)
        {
            Assert.Equal(identity.UnitName, unitName);
            Kills++;
            if (DrainOnKill)
            {
                Population = LinuxCgroupPopulation.Empty;
                ActiveState = "active";
                SubState = "exited";
            }
            return Task.CompletedTask;
        }
    }

    private sealed class TempDirectory : IDisposable
    {
        internal string Path { get; } = System.IO.Path.Combine(System.IO.Path.GetTempPath(),
            "slophawk-sg05-" + Guid.NewGuid().ToString("N"));
        internal TempDirectory() => Directory.CreateDirectory(Path);
        public void Dispose() => Directory.Delete(Path, recursive: true);
    }
}
