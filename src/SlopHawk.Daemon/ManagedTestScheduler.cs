using System.Runtime.Versioning;
using System.Security.AccessControl;
using System.Security.Cryptography;
using System.Security.Principal;
using System.Text.Json;
using System.Text.Json.Serialization;
using System.Threading.Channels;
using Microsoft.Data.Sqlite;

namespace SlopHawk.Daemon;

internal sealed record ManagedTestResult(JsonElement Status, MigrationTable[]? Tables);

internal sealed record ManagedTestRegistration(string Path, string Sha256, string Digest)
{
    internal const string Name = "managed-test-peer.json";
    private static readonly JsonSerializerOptions Json = new(JsonSerializerDefaults.Web)
    {
        UnmappedMemberHandling = JsonUnmappedMemberHandling.Disallow
    };

    private sealed record Document(int Version, string RootIdentity, Executable Peer);
    private sealed record Executable(string Path, string Sha256);

    internal static ManagedTestRegistration? Read(Workspace workspace, string rootIdentity)
    {
        var path = System.IO.Path.Combine(workspace.StateDirectory, Name);
        try
        {
            Workspace.CheckDirectory(workspace.StateDirectory);
            if ((File.GetAttributes(path) & (FileAttributes.Directory | FileAttributes.ReparsePoint)) != 0)
                throw new UnauthorizedAccessException("The managed test registration must be a physical file.");
            using var stream = File.Open(path, FileMode.Open, FileAccess.Read, FileShare.Read);
            var privateFile = OperatingSystem.IsWindows() ? IsPrivateWindowsFile(stream) :
                OperatingSystem.IsLinux() && RootIdentity.IsPrivateLinuxFile(stream.SafeFileHandle);
            if (!privateFile || stream.Length is <= 0 or > 4096)
                throw new UnauthorizedAccessException("The managed test registration must be bounded and owner-only.");
            var bytes = new byte[(int)stream.Length];
            stream.ReadExactly(bytes);
            if (stream.ReadByte() != -1)
                throw new UnauthorizedAccessException("The managed test registration changed while reading.");
            var config = JsonSerializer.Deserialize<Document>(bytes, Json);
            if (config is not { Version: 1, Peer: { Path.Length: > 0, Sha256.Length: 64 } } ||
                config.RootIdentity != rootIdentity || !System.IO.Path.IsPathFullyQualified(config.Peer.Path) ||
                !config.Peer.Sha256.All(char.IsAsciiHexDigit))
                throw new UnauthorizedAccessException("The managed test registration is invalid or belongs to another workspace.");
            return new ManagedTestRegistration(config.Peer.Path, config.Peer.Sha256.ToUpperInvariant(),
                Convert.ToHexString(SHA256.HashData(bytes)));
        }
        catch (FileNotFoundException)
        {
            return null;
        }
    }

    [SupportedOSPlatform("windows")]
    private static bool IsPrivateWindowsFile(FileStream stream)
    {
        using var identity = WindowsIdentity.GetCurrent();
        var acl = new RawSecurityDescriptor(stream.GetAccessControl().GetSecurityDescriptorBinaryForm(), 0);
        if (identity.User is not { } owner || acl.Owner is not { } fileOwner || !owner.Equals(fileOwner) ||
            acl.DiscretionaryAcl is not { } entries)
            return false;
        foreach (GenericAce entry in entries)
        {
            if ((entry.AceFlags & AceFlags.InheritOnly) != 0 || entry.AceType == AceType.AccessDenied)
                continue;
            if (entry is not CommonAce { AceType: AceType.AccessAllowed } allow || allow.IsCallback ||
                allow.AccessMask != 0 && !owner.Equals(allow.SecurityIdentifier) &&
                !allow.SecurityIdentifier.IsWellKnown(WellKnownSidType.LocalSystemSid) &&
                !allow.SecurityIdentifier.IsWellKnown(WellKnownSidType.BuiltinAdministratorsSid) &&
                allow.SecurityIdentifier.Value != "S-1-3-4")
                return false;
        }
        return true;
    }
}

internal sealed class ManagedTestScheduler(SourceInventory inventory, WorkspaceMonitor monitor, Workspace workspace)
    : IWorkspaceTestScheduler
{
    private readonly Channel<bool> changes = Channel.CreateBounded<bool>(1);
    private readonly WorkerAdmission workers = new(workspace);
    private bool recovered;

    internal bool WatchesProviderInput(string path) =>
        (Path.GetExtension(path).ToLowerInvariant() is ".py" or ".pyi" or ".rs" or ".fluxr" or
             ".toml" or ".ini" or ".cfg" or ".lock" || Path.GetFileName(path) == "rust-toolchain") &&
        File.Exists(Path.Combine(workspace.StateDirectory, ManagedTestRegistration.Name));

    public void Signal() => changes.Writer.TryWrite(true);

    public async Task RunAsync(CancellationToken cancellationToken)
    {
        while (await changes.Reader.WaitToReadAsync(cancellationToken))
        {
            changes.Reader.TryRead(out _);
            var fence = monitor.CurrentSchedule();
            if (fence is null)
                continue;
            try
            {
                if (await AdvanceAsync(fence, cancellationToken))
                    Signal();
            }
            catch (UnauthorizedAccessException ex)
            {
                monitor.UpdateScheduler(new TestSchedulerStatus("blocked_policy", Error: ex.Message), fence);
            }
            catch (Exception ex) when (ex is IOException or JsonException or SqliteException or
                System.ComponentModel.Win32Exception)
            {
                monitor.UpdateScheduler(new TestSchedulerStatus("degraded", Error: ex.Message), fence);
                await Task.Delay(1000, cancellationToken);
                Signal();
            }
        }
    }

    private async Task<bool> AdvanceAsync(ScheduleFence fence, CancellationToken token)
    {
        var registration = ManagedTestRegistration.Read(workspace, fence.RootIdentity);
        if (registration is null)
        {
            monitor.UpdateScheduler(new TestSchedulerStatus("awaiting_adapter"), fence);
            return false;
        }
        if (!recovered && !workers.CanRecover())
        {
            monitor.UpdateScheduler(new TestSchedulerStatus("waiting_recovery"), fence);
            await Task.Delay(250, token);
            return true;
        }
        recovered = true;
        var basis = inventory.ExportMigration(fence.RootIdentity);
        if (basis.Revision != fence.Revision || !await monitor.VerifyScheduleAsync(fence, token))
            return false;
        monitor.UpdateScheduler(new TestSchedulerStatus("running"), fence);
        var result = await Task.Run(() => MigrationStateBridge.RunManagedTests(workspace, registration, basis, token), token);
        if (result.Status.ValueKind != JsonValueKind.Object)
            throw new IOException("The test peer omitted its scheduler status.");
        if (!await monitor.VerifyScheduleAsync(fence, token) || !Current())
            return false;
        var pending = result.Tables is { } tables ? inventory.CommitManagedTests(basis, tables, fence, Current) : null;
        if (result.Tables is not null && pending is null)
            return false;
        monitor.UpdateScheduler(result.Status, fence, pending);
        // An unchanged scope is deduplicated by the peer. A changed cause set
        // can schedule the next bounded set when the first set was committed.
        return pending is { State: "awaiting_selection" };

        bool Current() => !token.IsCancellationRequested && monitor.ScheduleIsCurrent(fence) &&
            !MigrationStateBridge.HasWriterMarker(workspace) &&
            ManagedTestRegistration.Read(workspace, fence.RootIdentity) == registration;
    }
}
