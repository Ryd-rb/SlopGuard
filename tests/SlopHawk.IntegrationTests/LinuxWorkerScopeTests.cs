using System.Diagnostics;
using System.Text.Json;
using SlopHawk.Daemon;
using SlopHawk.WorkerFixture;
using Xunit.Abstractions;
using Xunit;

namespace SlopHawk.IntegrationTests;

public sealed class LinuxWorkerScopeTests(ITestOutputHelper output)
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
        var proof = await recovered.StopAndDrainAsync(lease, TimeSpan.FromSeconds(1));
        Assert.Equal(identity, proof.Identity);
        Assert.Equal(1, manager.Kills);
        Assert.Equal(LinuxScopeDisposition.Drained, await recovered.InspectAsync(lease, CancellationToken.None));
    }

    [Theory]
    [InlineData((int)LinuxCgroupPopulation.Populated, "active", "running", (int)LinuxScopeDisposition.Populated)]
    [InlineData((int)LinuxCgroupPopulation.Empty, "active", "exited", (int)LinuxScopeDisposition.Drained)]
    [InlineData((int)LinuxCgroupPopulation.Missing, "failed", "failed", (int)LinuxScopeDisposition.AmbiguousRecovery)]
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
        await Assert.ThrowsAsync<IOException>(() => scope.StopAndDrainAsync(lease, TimeSpan.FromSeconds(1)));
        Assert.Equal(0, manager.Kills);
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

    [Fact]
    public async Task SameInvocationMayTransitionFromRunningAndEmptyToVerifiedRemoval()
    {
        using var state = new TempDirectory();
        var lease = Guid.NewGuid();
        var identity = Launched(lease);
        var journal = new LinuxScopeJournal(state.Path);
        journal.Save(identity);
        var manager = new FakeManager(identity) { WitnessOnKill = LinuxCgroupPopulation.Removed };
        manager.ObservationsAfterKill.Enqueue(("active", "running", identity.ControlGroup!, LinuxCgroupPopulation.Empty));
        manager.ObservationsAfterKill.Enqueue(("deactivating", "stop-sigkill", identity.ControlGroup!, LinuxCgroupPopulation.Missing));
        manager.ObservationsAfterKill.Enqueue(("failed", "failed", "", LinuxCgroupPopulation.Missing));
        var scope = new LinuxWorkerScope(manager, journal, Boot);

        Assert.Equal(identity, (await scope.StopAndDrainAsync(lease, TimeSpan.FromSeconds(2))).Identity);
        Assert.Empty(manager.ObservationsAfterKill);
        Assert.Equal(1, manager.Kills);
        Assert.True(manager.WitnessDisposed);
    }

    [Theory]
    [InlineData("")]
    [InlineData(null)]
    public async Task ManagerMustClaimTheRecordedCgroupBeforeSignaling(string? reported)
    {
        using var state = new TempDirectory();
        var lease = Guid.NewGuid();
        var identity = Launched(lease);
        var journal = new LinuxScopeJournal(state.Path);
        journal.Save(identity);
        var manager = new FakeManager(identity) { ControlGroup = reported };
        var scope = new LinuxWorkerScope(manager, journal, Boot);

        Assert.Equal(LinuxScopeDisposition.AmbiguousRecovery, await scope.InspectAsync(lease, CancellationToken.None));
        await Assert.ThrowsAsync<IOException>(() => scope.StopAndDrainAsync(lease, TimeSpan.FromSeconds(1)));
        Assert.Equal(0, manager.Kills);
    }

    [Fact]
    public async Task RecycledInvocationAfterPinningIsNeverSignaled()
    {
        using var state = new TempDirectory();
        var lease = Guid.NewGuid();
        var identity = Launched(lease);
        var journal = new LinuxScopeJournal(state.Path);
        journal.Save(identity);
        var manager = new FakeManager(identity);
        manager.OnPin = () => manager.InvocationId = Guid.NewGuid().ToString("N");
        var scope = new LinuxWorkerScope(manager, journal, Boot);

        await Assert.ThrowsAsync<IOException>(() => scope.StopAndDrainAsync(lease, TimeSpan.FromSeconds(1)));
        Assert.Equal(0, manager.Kills);
        Assert.True(manager.WitnessDisposed);
    }

    [Fact]
    public async Task ReplacementAtFormerPathPreventsDrainProofEvenIfOriginalWasRemoved()
    {
        using var state = new TempDirectory();
        var lease = Guid.NewGuid();
        var identity = Launched(lease);
        var journal = new LinuxScopeJournal(state.Path);
        journal.Save(identity);
        var manager = new FakeManager(identity)
        {
            PopulationOnKill = LinuxCgroupPopulation.Empty,
            WitnessOnKill = LinuxCgroupPopulation.Removed
        };
        var scope = new LinuxWorkerScope(manager, journal, Boot);

        await Assert.ThrowsAsync<IOException>(() => scope.StopAndDrainAsync(lease, TimeSpan.FromMilliseconds(120)));
        Assert.Equal(identity, journal.Read(lease));
        Assert.Equal(1, manager.Kills);
    }

    [Fact]
    public async Task MissingCgroupAfterKillDoesNotProveDrainageOrReleaseReservation()
    {
        using var state = new TempDirectory();
        var lease = Guid.NewGuid();
        var identity = Launched(lease);
        var journal = new LinuxScopeJournal(state.Path);
        journal.Save(identity);
        var manager = new FakeManager(identity) { PopulationOnKill = LinuxCgroupPopulation.Missing };
        var scope = new LinuxWorkerScope(manager, journal, Boot);
        await Assert.ThrowsAsync<IOException>(() => scope.StopAndDrainAsync(lease, TimeSpan.FromMilliseconds(120)));
        Assert.Equal(1, manager.Kills);
        Assert.Equal(identity, journal.Read(lease));
        Assert.Equal(LinuxScopeDisposition.AmbiguousRecovery,
            await new LinuxWorkerScope(manager, new LinuxScopeJournal(state.Path), Boot)
                .InspectAsync(lease, CancellationToken.None));
    }

    [Fact]
    public async Task ReusedInvocationAfterTheSignalCannotProduceADrainProof()
    {
        using var state = new TempDirectory();
        var lease = Guid.NewGuid();
        var identity = Launched(lease);
        var journal = new LinuxScopeJournal(state.Path);
        journal.Save(identity);
        var manager = new FakeManager(identity) { ReuseOnKill = true };
        var scope = new LinuxWorkerScope(manager, journal, Boot);
        await Assert.ThrowsAsync<IOException>(() => scope.StopAndDrainAsync(lease, TimeSpan.FromSeconds(1)));
        Assert.Equal(1, manager.Kills);
        Assert.Equal(LinuxScopeDisposition.AmbiguousRecovery,
            await new LinuxWorkerScope(manager, new LinuxScopeJournal(state.Path), Boot)
                .InspectAsync(lease, CancellationToken.None));
        Assert.Equal(identity, journal.Read(lease));
    }

    [Fact]
    public async Task PreviousIncompleteIdentityHoldsTheLeaseWithoutStartingAService()
    {
        if (!OperatingSystem.IsLinux()) return;
        using var state = new TempDirectory();
        using var output = new TempDirectory();
        var lease = Guid.NewGuid();
        var manager = new FakeManager(Launched(lease)) { UnitLoaded = false,
            IncludePreviousIdentity = true, InvocationId = Guid.NewGuid().ToString("N") };
        var journal = new LinuxScopeJournal(state.Path);
        var command = Commands.Create("tree", output.Path);
        await Assert.ThrowsAsync<IOException>(() => new LinuxWorkerScope(manager, journal, Boot)
            .StartFixtureAsync(lease, command.FileName, typeof(Commands).Assembly.Location,
                output.Path, CancellationToken.None));
        Assert.Equal(LinuxScopeIdentity.New(lease, Boot), journal.Read(lease));
        Assert.Equal(0, manager.Starts);
    }

    [Fact]
    public async Task AcknowledgmentOfExitedUnitIsNotAServiceLaunchReceipt()
    {
        if (!OperatingSystem.IsLinux()) return;
        using var state = new TempDirectory();
        using var output = new TempDirectory();
        var lease = Guid.NewGuid();
        var manager = new FakeManager(Launched(lease)) { UnitLoaded = false,
            ActiveAfterStart = "inactive", SubAfterStart = "dead" };
        var journal = new LinuxScopeJournal(state.Path);
        var command = Commands.Create("tree", output.Path);
        await Assert.ThrowsAsync<IOException>(() => new LinuxWorkerScope(manager, journal, Boot)
            .StartFixtureAsync(lease, command.FileName, typeof(Commands).Assembly.Location,
                output.Path, CancellationToken.None));
        Assert.Equal(1, manager.Starts);
        Assert.Equal(LinuxScopeIdentity.New(lease, Boot), journal.Read(lease));
    }

    [Fact]
    public async Task TimedOutStartHoldsAnAcceptedButUnacknowledgedScope()
    {
        if (!OperatingSystem.IsLinux()) return;
        using var state = new TempDirectory();
        using var output = new TempDirectory();
        var lease = Guid.NewGuid();
        var manager = new FakeManager(Launched(lease)) { UnitLoaded = false, HangOnStart = true };
        var journal = new LinuxScopeJournal(state.Path);
        var command = Commands.Create("tree", output.Path);
        using var timeout = new CancellationTokenSource(TimeSpan.FromMilliseconds(100));
        await Assert.ThrowsAnyAsync<OperationCanceledException>(() =>
            new LinuxWorkerScope(manager, journal, Boot).StartFixtureAsync(lease,
                command.FileName, typeof(Commands).Assembly.Location, output.Path, timeout.Token));
        Assert.Equal(1, manager.Starts);
        Assert.True(manager.UnitLoaded);
        Assert.Equal(LinuxCgroupPopulation.Populated, manager.Population);
        Assert.Equal(LinuxScopeIdentity.New(lease, Boot), journal.Read(lease));
        var fresh = new LinuxWorkerScope(manager, journal, Boot);
        Assert.Equal(LinuxScopeDisposition.AmbiguousRecovery, await fresh.InspectAsync(lease, CancellationToken.None));
        await Assert.ThrowsAsync<IOException>(() => fresh.StopAndDrainAsync(lease, TimeSpan.FromSeconds(1)));
        Assert.Equal(0, manager.Kills);
    }

    [Theory]
    [InlineData("populated 0\n", (int)LinuxCgroupPopulation.Empty)]
    [InlineData("populated 1\nfrozen 0\n", (int)LinuxCgroupPopulation.Populated)]
    [InlineData("populated 0\npopulated 1\n", (int)LinuxCgroupPopulation.Unknown)]
    [InlineData("populated 4\n", (int)LinuxCgroupPopulation.Unknown)]
    [InlineData("frozen 0\n", (int)LinuxCgroupPopulation.Unknown)]
    public void CgroupEventsRequireOneUnambiguousPopulatedValue(string text, int expected) =>
        Assert.Equal((LinuxCgroupPopulation)expected, LinuxSystemdUserManager.ParsePopulation(text));

    [Fact]
    public void AUnitCgroupCannotEscapeOrAliasTheVerifiedPath()
    {
        var unit = LinuxScopeIdentity.New(Guid.NewGuid(), Boot).UnitName;
        Assert.True(LinuxSystemdUserManager.IsUnitControlGroup("/user.slice/app.slice/" + unit, unit));
        foreach (var group in new[] { "/user.slice/../" + unit,
                     "/user.slice//" + unit, "/user.slice/./" + unit,
                     "/user.slice/" + unit + "\n", "/user.slice/" + unit + "/child",
                     "/" + new string('x', 2050) + "/" + unit })
            Assert.False(LinuxSystemdUserManager.IsUnitControlGroup(group, unit));
    }

    [Fact]
    public async Task RemovalRequiresAProofFromTheCurrentBoot()
    {
        if (!OperatingSystem.IsLinux()) return;
        var identity = Launched(Guid.NewGuid()); // Deliberately not this host's boot.
        await Assert.ThrowsAsync<IOException>(() => new LinuxSystemdUserManager()
            .RemoveDrainedFixtureAsync(new LinuxScopeDrainProof(identity), CancellationToken.None));
    }

    [Fact]
    public void AnOrdinaryDirectoryCannotWitnessLinuxCgroupDrainage()
    {
        using var directory = new TempDirectory();
        File.WriteAllText(Path.Combine(directory.Path, "cgroup.events"), "populated 0\n");
        using var witness = LinuxPopulationWitness.Open(directory.Path);
        Assert.Null(witness);
    }

    [Theory]
    [InlineData("LoadState=loaded\nActiveState=active\nSubState=running\nControlGroup=/abc\n")]
    [InlineData("LoadState=loaded\nActiveState=active\nSubState=running\nInvocationID=abcd\n")]
    [InlineData("LoadState=loaded\nLoadState=loaded\nActiveState=active\nSubState=running\nInvocationID=abcd\nControlGroup=/abc\n")]
    [InlineData("GarbageNoEquals\nLoadState=loaded\nActiveState=active\nSubState=running\nInvocationID=abcd\nControlGroup=/abc\n")]
    [InlineData("LoadState=masked\nActiveState=inactive\nSubState=dead\nInvocationID=\nControlGroup=\n")]
    [InlineData("LoadState=not-found\nActiveState=active\nSubState=running\nInvocationID=\nControlGroup=\n")]
    [InlineData("LoadState=not-found\nActiveState=inactive\nSubState=dead\nInvocationID=aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\nControlGroup=\n")]
    [InlineData("LoadState=loaded\nActiveState=active\nSubState=running\nInvocationID=00000000000000000000000000000000\nControlGroup=\n")]
    public void IncompleteDuplicateAndMalformedManagerRepliesAreUnavailable(string reply)
    {
        var unit = LinuxScopeIdentity.New(Guid.NewGuid(), Boot).UnitName;
        Assert.False(LinuxSystemdUserManager.ParseObservation(reply, unit, null).ManagerAvailable);
    }

    [Fact]
    public void CompleteManagerReplyPreservesItsReportedInvocationAndUnitPath()
    {
        var unit = LinuxScopeIdentity.New(Guid.NewGuid(), Boot).UnitName;
        var group = "/user.slice/app.slice/" + unit;
        var response = $"LoadState=loaded\nActiveState=active\nSubState=running\nInvocationID={Guid.NewGuid():N}\nControlGroup={group}\n";
        var observed = LinuxSystemdUserManager.ParseObservation(response, unit, null);
        Assert.True(observed.ManagerAvailable);
        Assert.True(observed.UnitLoaded);
        Assert.Equal(group, observed.ControlGroup);
        Assert.Equal("active", observed.ActiveState);
        Assert.Equal("running", observed.SubState);
        Assert.NotNull(observed.InvocationId);
    }

    // This is an opt-in-by-capability live test: xUnit reports SKIPPED rather than
    // green when a user manager/cgroup v2 cannot be exercised. It never creates
    // a manager, changes cgroup delegation or requires privilege.
    [LiveUserManagerFact]
    public async Task LiveUserManagerStartsVerifiedServiceAndDrainsItsKnownDescendant()
    {

        var manager = new LinuxSystemdUserManager();
        var lease = Guid.NewGuid();
        var boot = File.ReadAllText("/proc/sys/kernel/random/boot_id").Trim();
        using var probe = new CancellationTokenSource(TimeSpan.FromSeconds(6));
        LinuxScopeObservation available;
        try
        {
            available = await manager.ObserveAsync(LinuxScopeIdentity.New(lease, boot).UnitName,
                null, probe.Token);
        }
        catch (OperationCanceledException ex)
        {
            throw new IOException("The user-manager probe timed out after the live prerequisite passed.", ex);
        }
        if (!available.ManagerAvailable)
            throw new IOException("The user manager became unavailable after the live prerequisite passed.");
        Assert.True(LinuxSystemdUserManager.IsAbsent(available));

        using var state = new TempDirectory();
        using var outputDirectory = new TempDirectory();
        var journal = new LinuxScopeJournal(state.Path);
        LinuxWorkerScope? recovered = null;

        // A different owner can die after the durable pending write and before
        // requesting any unit. Its replacement must hold that reservation.
        using (var pendingState = new TempDirectory())
        {
            var interrupted = Guid.NewGuid();
            await RunFixtureOwnerAsync("linux-scope-owner-pending", interrupted.ToString("N"), pendingState.Path);
            var pendingJournal = new LinuxScopeJournal(pendingState.Path);
            Assert.Equal(LinuxScopeIdentity.New(interrupted, boot), pendingJournal.Read(interrupted));
            Assert.Equal(LinuxScopeDisposition.AmbiguousRecovery,
                await new LinuxWorkerScope(manager, pendingJournal, boot)
                    .InspectAsync(interrupted, CancellationToken.None));
            var pendingUnit = await manager.ObserveAsync(LinuxScopeIdentity.New(interrupted, boot).UnitName,
                null, CancellationToken.None);
            Assert.True(pendingUnit.ManagerAvailable);
            Assert.True(LinuxSystemdUserManager.IsAbsent(pendingUnit));
            output.WriteLine("SG-17 interrupted owner: journal pending; NO service was launched.");
        }

        LinuxScopeIdentity? started = null;
        LinuxScopeDrainProof? proof = null;
        var cancellationAttempted = false;
        try
        {
            await RunFixtureOwnerAsync("linux-scope-owner", lease.ToString("N"), state.Path, outputDirectory.Path);

            // The starter has exited. Only its persisted boot, InvocationID and
            // cgroup identity are available to this fresh owner.
            started = journal.Read(lease);
            recovered = new LinuxWorkerScope(manager, new LinuxScopeJournal(state.Path), boot);
            Assert.NotNull(started.InvocationId);
            Assert.NotNull(started.ControlGroup);
            using var deadline = new CancellationTokenSource(TimeSpan.FromSeconds(8));
            var parent = Path.Combine(outputDirectory.Path, "parent.json");
            var child = Path.Combine(outputDirectory.Path, "child.json");
            var enrolled = Path.Combine(outputDirectory.Path, "enrolled.cgroup");
            var childGroup = child + ".cgroup";
            while (!File.Exists(parent) || !File.Exists(child) ||
                   !File.Exists(enrolled) || !File.Exists(childGroup))
                await Task.Delay(40, deadline.Token);
            Assert.Equal(started.ControlGroup, File.ReadAllText(enrolled));
            Assert.Equal(started.ControlGroup, File.ReadAllText(childGroup));
            Assert.NotEqual(JsonSerializer.Deserialize<FixtureProcess>(File.ReadAllText(parent))!.Id,
                JsonSerializer.Deserialize<FixtureProcess>(File.ReadAllText(child))!.Id);
            Assert.Equal(LinuxScopeDisposition.Populated, await recovered.InspectAsync(lease, deadline.Token));
            output.WriteLine($"SG-17 LIVE SERVICE EXERCISED: {started.UnitName}; boot={started.BootId}; invocation={started.InvocationId}; cgroup={started.ControlGroup}");

            cancellationAttempted = true;
            proof = await recovered.StopAndDrainAsync(lease, TimeSpan.FromSeconds(8));
            Assert.Equal(started, proof.Identity);
            using var gone = new CancellationTokenSource(TimeSpan.FromSeconds(5));
            while (JsonSerializer.Deserialize<FixtureProcess>(File.ReadAllText(parent))!.IsAlive() ||
                   JsonSerializer.Deserialize<FixtureProcess>(File.ReadAllText(child))!.IsAlive())
                await Task.Delay(40, gone.Token);
            output.WriteLine("SG-17 live scope: same verified cgroup proved empty and both recorded processes exited.");
        }
        finally
        {
            // Never signal a name whose identity was not durably acknowledged.
            // The unacknowledged fixture has its own systemd runtime bound.
            if (!cancellationAttempted && ReadAcknowledged(journal, lease) is not null)
            {
                recovered ??= new LinuxWorkerScope(manager, journal, boot);
                proof = await recovered.StopAndDrainAsync(lease, TimeSpan.FromSeconds(8));
            }
            if (proof is not null)
            {
                using var cleanup = new CancellationTokenSource(TimeSpan.FromSeconds(5));
                await manager.RemoveDrainedFixtureAsync(proof, cleanup.Token);
            }
        }
    }

    private static LinuxScopeIdentity? ReadAcknowledged(LinuxScopeJournal journal, Guid lease)
    {
        try
        {
            var identity = journal.Read(lease);
            return identity.InvocationId is null ? null : identity;
        }
        catch (Exception ex) when (ex is IOException or UnauthorizedAccessException)
        {
            return null;
        }
    }

    private static async Task RunFixtureOwnerAsync(params string[] args)
    {
        using var owner = Process.Start(Commands.Create(args))
            ?? throw new IOException("Could not start the Linux fixture owner.");
        using var timeout = new CancellationTokenSource(TimeSpan.FromSeconds(12));
        try
        {
            await owner.WaitForExitAsync(timeout.Token);
        }
        catch (OperationCanceledException) when (timeout.IsCancellationRequested)
        {
            try { owner.Kill(); } // The local test helper, never the transient service.
            catch (InvalidOperationException) { /* The helper already exited. */ }
            await owner.WaitForExitAsync().WaitAsync(TimeSpan.FromSeconds(2));
            throw;
        }
        Assert.Equal(0, owner.ExitCode);
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
        internal bool IncludePreviousIdentity;
        internal bool DrainOnKill = true;
        internal bool Throws;
        internal string InvocationId = identity.InvocationId!;
        internal string? ControlGroup = identity.ControlGroup;
        internal string ActiveState = "active";
        internal string SubState = "running";
        internal LinuxCgroupPopulation Population = LinuxCgroupPopulation.Populated;
        internal LinuxCgroupPopulation PopulationAfterStart = LinuxCgroupPopulation.Populated;
        internal LinuxCgroupPopulation? PopulationOnKill;
        internal string ActiveAfterStart = "active";
        internal string SubAfterStart = "running";
        internal bool HangOnStart;
        internal bool ReuseOnKill;
        internal Action? BeforeStart;
        internal Action? OnPin;
        internal LinuxCgroupPopulation? WitnessOnKill;
        internal bool WitnessDisposed;
        internal Queue<(string ActiveState, string SubState, string ControlGroup, LinuxCgroupPopulation Population)>
            ObservationsAfterKill { get; } = new();
        internal int Starts, Kills;

        public Task<LinuxScopeObservation> ObserveAsync(string unitName, string? knownControlGroup, CancellationToken token)
        {
            Assert.Equal(identity.UnitName, unitName);
            if (Throws) throw new IOException("Synthetic manager query fault.");
            if (Kills != 0 && ObservationsAfterKill.TryDequeue(out var update))
                (ActiveState, SubState, ControlGroup, Population) = update;
            return Task.FromResult(new LinuxScopeObservation(ManagerAvailable, UnitLoaded,
                UnitLoaded || IncludePreviousIdentity ? InvocationId : "",
                UnitLoaded || IncludePreviousIdentity ? ControlGroup : "",
                UnitLoaded ? ActiveState : "inactive", UnitLoaded ? SubState : "dead",
                UnitLoaded ? Population : LinuxCgroupPopulation.Unknown));
        }

        public async Task StartFixtureAsync(string unitName, string host, string fixture, string output, CancellationToken token)
        {
            Assert.Equal(identity.UnitName, unitName);
            Assert.EndsWith("SlopHawk.WorkerFixture.dll", fixture, StringComparison.Ordinal);
            Assert.Equal("dotnet", Path.GetFileName(host));
            BeforeStart?.Invoke();
            Starts++;
            UnitLoaded = true;
            Population = PopulationAfterStart;
            ActiveState = ActiveAfterStart;
            SubState = SubAfterStart;
            if (HangOnStart)
                await Task.Delay(Timeout.InfiniteTimeSpan, token);
        }

        public Task KillScopeAsync(string unitName, CancellationToken token)
        {
            Assert.Equal(identity.UnitName, unitName);
            Kills++;
            if (ReuseOnKill)
                InvocationId = Guid.NewGuid().ToString("N");
            if (DrainOnKill)
            {
                Population = PopulationOnKill ?? LinuxCgroupPopulation.Empty;
                ActiveState = "active";
                SubState = "exited";
            }
            return Task.CompletedTask;
        }

        public ILinuxCgroupWitness PinCgroup(LinuxScopeIdentity requested)
        {
            Assert.Equal(identity, requested);
            OnPin?.Invoke();
            return new FakeWitness(this);
        }

        private sealed class FakeWitness(FakeManager manager) : ILinuxCgroupWitness
        {
            public LinuxCgroupPopulation Read() => manager.Kills != 0 && manager.WitnessOnKill is { } witnessed
                ? witnessed
                : manager.Population;

            public void Dispose() => manager.WitnessDisposed = true;
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

// xUnit 2.9 does not support runtime dynamic skip exceptions. A discovery-time
// fact attribute makes unsupported VMs explicitly SKIPPED, never silently green.
[AttributeUsage(AttributeTargets.Method)]
public sealed class LiveUserManagerFactAttribute : FactAttribute
{
    public LiveUserManagerFactAttribute()
    {
        const string unavailable = "SG-17 LIVE SERVICE NOT EXERCISED: ";
        if (!OperatingSystem.IsLinux())
        {
            Skip = unavailable + "not Linux";
            return;
        }
        if (!File.Exists("/sys/fs/cgroup/cgroup.controllers"))
        {
            Skip = unavailable + "cgroup v2 unavailable";
            return;
        }
        if (!File.Exists("/usr/bin/systemctl") || !File.Exists("/usr/bin/systemd-run"))
        {
            Skip = unavailable + "user systemd tools missing";
            return;
        }
        try
        {
            var start = new System.Diagnostics.ProcessStartInfo("/usr/bin/systemctl")
            {
                UseShellExecute = false, RedirectStandardOutput = true,
                RedirectStandardError = true, CreateNoWindow = true
            };
            foreach (var argument in new[] { "--user", "show", "--all", "--no-pager", "--property=Version" })
                start.ArgumentList.Add(argument);
            using var process = System.Diagnostics.Process.Start(start);
            if (process is null)
            {
                Skip = unavailable + "could not start systemctl --user";
                return;
            }
            if (!process.WaitForExit(3_000))
            {
                process.Kill(); // Only the read-only systemctl probe, never a service.
                Skip = unavailable + "systemctl --user probe timed out";
                return;
            }
            if (process.ExitCode != 0)
                Skip = unavailable + "systemctl --user cannot reach an existing manager: " +
                    process.StandardError.ReadToEnd().Trim();
        }
        catch (Exception ex) when (ex is IOException or UnauthorizedAccessException or System.ComponentModel.Win32Exception)
        {
            Skip = unavailable + ex.Message;
        }
    }
}
