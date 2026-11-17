using System.Buffers.Binary;
using System.Diagnostics;
using System.Security.Cryptography;
using System.Text;
using System.Text.Json;
using System.Text.Json.Serialization;

namespace SlopHawk.Daemon;

/// <summary>
/// Versioned semantic transition for a *controlled* native peer. The .NET daemon
/// mutex, root identity, worker admission ledger and schema-v5 inventory are fences;
/// none of them is inferred from repository files or an untrusted peer response.
/// This facade is intentionally separate from the shipped IPC/MCP wire versions.
/// </summary>
public static class MigrationStateBridge
{
    private const string Protocol = "slophawk.migration-job/1";
    private const int MaxFrame = 128 * 1024;
    private const int ChunkBytes = 72 * 1024;
    private const int MaxReason = 240;
    private static readonly JsonSerializerOptions Json = new(JsonSerializerDefaults.Web)
    {
        UnmappedMemberHandling = JsonUnmappedMemberHandling.Disallow
    };

    private sealed record WriterMarker(int Version, string Root, string RootIdentity, long Revision,
        string Epoch, string NativeSha256, string ProfileSha256, string State, string BackupSha256);
    private sealed record JobIdentity(string Protocol, string Operation, string Job, string Root,
        string RootIdentity, string Epoch, long CommittedRevision, string InputSha256,
        string ToolSha256, string PolicySha256, string BudgetRef, string GrantRef, string StateDirectory);
    private sealed record PeerResult(byte[] Payload, string Digest);

    public static string ReadRootIdentity(Workspace workspace) => RootIdentity.Read(workspace.Root);
    public static void RequirePhysicalDirectory(string path) => Workspace.CheckDirectory(path);

    /// <summary>Also gates ordinary legacy start/status/recheck while the native writer is recorded.</summary>
    public static bool HasWriterMarker(Workspace workspace) =>
        PathEntryExists(MarkerPath(workspace)) || PathEntryExists(MarkerPath(workspace) + ".pending");

    private static bool PathEntryExists(string path)
    {
        try { _ = File.GetAttributes(path); return true; }
        catch (FileNotFoundException) { return false; }
        catch (DirectoryNotFoundException) { return false; }
    }

    public static void RequireLegacyWriter(Workspace workspace)
    {
        // A corrupt, unexpected or partially written writer marker is a blocker.
        // The separate forensic backup alone does not represent native authority.
        if (HasWriterMarker(workspace))
            throw new IOException("The workspace is transferred or transition-uncertain. " +
                "Use 'slophawk engine rollback' with the matching pinned native peer; " +
                "do not start a second legacy writer.");
    }

    public static void RequireNativeWriter(Workspace workspace, string nativeSha256, string profileSha256)
        => _ = ReadNativeWriter(workspace, nativeSha256, profileSha256);

    private static WriterMarker ReadNativeWriter(Workspace workspace, string nativeSha256, string profileSha256)
    {
        if (PathEntryExists(MarkerPath(workspace) + ".pending"))
            throw new IOException("The native writer has an unfinished transition.");
        var marker = ReadMarker(MarkerPath(workspace));
        if (marker.Version != 1 || marker.State != "native" ||
            !Workspace.PathComparer.Equals(marker.Root, workspace.Root) ||
            marker.RootIdentity != RootIdentity.Read(workspace.Root) ||
            marker.NativeSha256 != nativeSha256 || marker.ProfileSha256 != profileSha256)
            throw new IOException("The selected native engine has no matching committed writer transfer.");
        return marker;
    }

    public static void TransferToNative(Workspace workspace, string peerPath, string nativeSha256,
        string profileSha256, string grantRef)
    {
        RequireExternalPeer(workspace, peerPath, nativeSha256, profileSha256, grantRef);
        RequestLegacyStop(workspace);
        WithWriterLock(workspace, () =>
        {
            RequireLegacyWriter(workspace);
            var root = RootIdentity.Read(workspace.Root);
            // Open (non-migrating) inventory first: unsupported durable versions are
            // rejected before a writer marker or worker-admission mutation.
            var inventory = new SourceInventory(workspace);
            var snapshot = inventory.ExportMigration(root);
            if (!new WorkerAdmission(workspace).CanRecover())
                throw new IOException("An admitted worker has not proved its process-tree drainage.");
            var payload = JsonSerializer.SerializeToUtf8Bytes(snapshot, Json);
            var backupSha = Hash(payload);
            var epoch = Guid.NewGuid().ToString("N");
            var marker = new WriterMarker(1, workspace.Root, root, snapshot.Revision, epoch,
                nativeSha256, profileSha256, "transition-uncertain", backupSha);
            WriteBackup(workspace, epoch, payload, backupSha);
            WriteMarker(workspace, marker); // fail-closed before the peer can claim a writer
            var identity = Identity("import_state_v1", workspace, root, epoch, snapshot.Revision,
                backupSha, nativeSha256, snapshot.Policy, grantRef);
            var result = RunPeer(peerPath, identity, payload, wantExport: false);
            if (result.Payload.Length != 0)
                throw new IOException("The native importer returned unexpected state bytes.");
            // Full identity and complete/drained/writerClaimed terminal were verified.
            if (RootIdentity.Read(workspace.Root) != root)
                throw new IOException("The physical root changed during native import.");
            WriteMarker(workspace, marker with { State = "native" });
        });
    }

    public static void RollbackToLegacy(Workspace workspace, string peerPath, string nativeSha256,
        string profileSha256, string grantRef)
    {
        RequireExternalPeer(workspace, peerPath, nativeSha256, profileSha256, grantRef);
        WithWriterLock(workspace, () =>
        {
            var path = MarkerPath(workspace);
            var staging = path + ".pending";
            var staged = PathEntryExists(staging) ? ReadMarker(staging) : null;
            var committed = PathEntryExists(path) ? ReadMarker(path) : null;
            var marker = committed ?? staged ?? throw new IOException("No migration writer was recorded.");
            var root = RootIdentity.Read(workspace.Root);
            if (marker.Version != 1 || !Workspace.PathComparer.Equals(marker.Root, workspace.Root) ||
                marker.RootIdentity != root || marker.NativeSha256 != nativeSha256 ||
                marker.ProfileSha256 != profileSha256 ||
                marker.State is not ("native" or "transition-uncertain" or "rollback-uncertain"))
                throw new IOException("The recorded native writer identity is missing.");
            var inventory = new SourceInventory(workspace);
            // Reject unsupported local durable schema without disturbing the native writer.
            var previous = inventory.ExportMigration(root);
            var priorDigest = Hash(JsonSerializer.SerializeToUtf8Bytes(previous, Json));
            var matchesOriginal = previous.Revision == marker.Revision && priorDigest == marker.BackupSha256;
            if (!matchesOriginal && marker.State != "rollback-uncertain")
                throw new IOException("The previous managed inventory changed after writer handoff.");
            if (!new WorkerAdmission(workspace).CanRecover())
                throw new IOException("A managed worker has not proved drainage.");
            if (staged is not null)
            {
                if (committed is null && staged.State == "transition-uncertain")
                    File.Move(staging, path);
                else if (committed is not null && (committed.State, staged.State) is
                             ("transition-uncertain", "native") or ("native", "rollback-uncertain") &&
                         staged == committed with { State = staged.State })
                    File.Delete(staging); // recover from the last committed phase
                else
                    throw new IOException("The staged migration marker cannot be reconciled with its writer.");
            }
            if (marker.State == "transition-uncertain")
            {
                var abort = Identity("abort_import_v1", workspace, root, marker.Epoch,
                    marker.Revision, Hash([]), nativeSha256, previous.Policy, grantRef);
                var aborted = RunPeer(peerPath, abort, [], wantExport: false);
                if (aborted.Payload.Length != 0 || RootIdentity.Read(workspace.Root) != root)
                    throw new IOException("The uncertain native import could not be retired.");
                File.Delete(path);
                return;
            }
            if (marker.State == "native")
                WriteMarker(workspace, marker with { State = "rollback-uncertain" });
            var identity = Identity("export_and_stop_v1", workspace, root, marker.Epoch,
                marker.Revision, Hash([]), nativeSha256, previous.Policy, grantRef);
            var exported = RunPeer(peerPath, identity, [], wantExport: true);
            if (exported.Payload.Length is 0 or > SourceInventory.MaximumMigrationBytes)
                throw new IOException("The native peer did not return bounded rollback state.");
            var snapshot = JsonSerializer.Deserialize<MigrationSnapshot>(exported.Payload, Json)
                ?? throw new IOException("The native peer returned an empty state snapshot.");
            if (matchesOriginal)
                inventory.ImportMigration(snapshot, root, marker.Revision);
            else if (priorDigest != Hash(JsonSerializer.SerializeToUtf8Bytes(snapshot, Json)))
                throw new IOException("The managed rollback state differs from the stopped native snapshot.");
            if (RootIdentity.Read(workspace.Root) != root)
                throw new IOException("The physical root changed during managed rollback.");
            // A fresh .NET daemon obtains a new publication epoch; no transferred
            // report or derived index is represented as current by this import.
            File.Delete(path);
        });
    }

    public static string ReadNativeState(Workspace workspace, string peerPath, string nativeSha256,
        string profileSha256, string grantRef, bool reconcile)
    {
        RequireExternalPeer(workspace, peerPath, nativeSha256, profileSha256, grantRef);
        byte[]? payload = null;
        WithWriterLock(workspace, () =>
        {
            var marker = ReadNativeWriter(workspace, nativeSha256, profileSha256);
            var path = BackupPath(workspace, marker.Epoch);
            if (!Guid.TryParseExact(marker.Epoch, "N", out _) ||
                (File.GetAttributes(path) & (FileAttributes.ReparsePoint | FileAttributes.Directory)) != 0)
                throw new IOException("The migration backup identity is invalid.");
            using var input = File.OpenRead(path);
            if (input.Length is <= 0 or > SourceInventory.MaximumMigrationBytes)
                throw new IOException("The migration backup exceeds its interchange limit.");
            var bytes = new byte[(int)input.Length];
            input.ReadExactly(bytes);
            if (input.ReadByte() != -1 || Hash(bytes) != marker.BackupSha256)
                throw new IOException("The migration backup changed after transfer.");
            var initial = JsonSerializer.Deserialize<MigrationSnapshot>(bytes, Json)
                ?? throw new IOException("The migration backup is empty.");
            var requested = reconcile ? "reconcile_state_v1" : "inspect_state_v1";
            var identity = Identity(requested, workspace, marker.RootIdentity, marker.Epoch,
                marker.Revision, Hash([]), nativeSha256, initial.Policy, grantRef);
            payload = RunPeer(peerPath, identity, [], wantExport: true).Payload;
            if (ReadNativeWriter(workspace, nativeSha256, profileSha256) != marker || payload.Length is <= 0 or > MaxFrame)
                throw new IOException("The native state observation changed or was incomplete.");
        });
        return Encoding.UTF8.GetString(payload!);
    }

    private static JobIdentity Identity(string operation, Workspace workspace, string rootIdentity,
        string epoch, long revision, string inputSha256, string toolSha256, string? policy, string grantRef) =>
        new(Protocol, operation, Guid.NewGuid().ToString("N"), workspace.Root, rootIdentity,
            epoch, revision, inputSha256, toolSha256, Hash(Encoding.UTF8.GetBytes(policy ?? "")),
            "migration-state-16m-frame-128k", grantRef, workspace.StateDirectory);

    private static void RequestLegacyStop(Workspace workspace)
    {
        // The matching daemon's existing Stop command drains its monitor and
        // scheduler before the named mutex becomes available.
        var reply = new DaemonConnection(workspace).SendAsync(DaemonCommand.Stop).GetAwaiter().GetResult();
        if (reply is { Ok: false })
            throw new IOException(reply.Error ?? "The legacy daemon could not be quiesced.");
    }

    private static void WithWriterLock(Workspace workspace, Action operation)
    {
        using var mutex = new Mutex(false, workspace.InstanceName);
        var acquired = false;
        try
        {
            try { acquired = mutex.WaitOne(TimeSpan.FromSeconds(15)); }
            catch (AbandonedMutexException) { acquired = true; }
            if (!acquired)
                throw new IOException("The previous daemon did not release its writer authority after Stop.");
            operation();
        }
        finally
        {
            if (acquired)
                mutex.ReleaseMutex();
        }
    }

    private static PeerResult RunPeer(string path, JobIdentity identity, byte[] input, bool wantExport)
    {
        if (identity.InputSha256 != Hash(input))
            throw new IOException("The migration job identity disagrees with its exact input digest.");
        var info = new ProcessStartInfo(path)
        {
            UseShellExecute = false, CreateNoWindow = true, RedirectStandardInput = true,
            RedirectStandardOutput = true, RedirectStandardError = true,
        };
        info.ArgumentList.Add("--slophawk-migration-job-v1");
        using var process = Process.Start(info) ?? throw new IOException("The qualified native peer did not start.");
        using var deadline = new CancellationTokenSource(TimeSpan.FromSeconds(45));
        // The separate stderr reader is bounded, preventing output backpressure or
        // untrusted diagnostics from exhausting memory.
        var stderr = ReadStderrAsync(process.StandardError.BaseStream, deadline.Token);
        try
        {
            WriteFrame(process.StandardInput.BaseStream,
                new { kind = "request", identity, bytes = input.Length, sha256 = Hash(input) }, deadline.Token);
            for (var offset = 0; offset < input.Length; offset += ChunkBytes)
                WriteFrame(process.StandardInput.BaseStream, new { kind = "chunk", index = offset / ChunkBytes,
                    data = Convert.ToBase64String(input.AsSpan(offset, Math.Min(ChunkBytes, input.Length - offset))) }, deadline.Token);
            WriteFrame(process.StandardInput.BaseStream, new { kind = "end", identity }, deadline.Token);
            process.StandardInput.Close();

            using var received = new MemoryStream();
            var index = 0;
            var terminalSeen = false;
            string? digest = null;
            while (true)
            {
                using var frame = ReadFrame(process.StandardOutput.BaseStream, deadline.Token);
                if (frame is null)
                    break;
                var value = frame.RootElement;
                var kind = RequiredText(value, "kind", 24);
                if (kind == "chunk" && wantExport && !terminalSeen)
                {
                    if (!value.TryGetProperty("index", out var indexValue) ||
                        indexValue.ValueKind != JsonValueKind.Number || !indexValue.TryGetInt32(out var chunkIndex) ||
                        chunkIndex != index++)
                        throw new IOException("Native export chunks are missing or out of order.");
                    var encoded = RequiredText(value, "data", MaxFrame);
                    var block = Convert.FromBase64String(encoded);
                    if (block.Length > ChunkBytes || received.Length + block.Length > SourceInventory.MaximumMigrationBytes)
                        throw new IOException("The native export exceeds the bounded interchange.");
                    received.Write(block);
                }
                else if (kind == "terminal" && !terminalSeen)
                {
                    terminalSeen = true;
                    if (!value.TryGetProperty("identity", out var echoedJson) ||
                        echoedJson.ValueKind != JsonValueKind.Object)
                        throw new IOException("The migration terminal lacks a full job identity.");
                    var echoed = echoedJson.Deserialize<JobIdentity>(Json);
                    if (echoed != identity)
                        throw new IOException("The migration terminal did not echo the full job identity.");
                    var status = RequiredText(value, "status", 32);
                    if (status is not ("complete" or "partial" or "unsupported" or "deferred" or
                        "cancelled" or "stale" or "failed"))
                        throw new IOException("The migration terminal has an unknown outcome.");
                    var reason = RequiredText(value, "reason", MaxReason);
                    if (!value.TryGetProperty("omissions", out var omissions))
                        throw new IOException("The peer omitted its scope omissions.");
                    if (omissions.ValueKind != JsonValueKind.Array || omissions.GetArrayLength() > 32 ||
                        omissions.EnumerateArray().Any(item => item.ValueKind != JsonValueKind.String ||
                            item.GetString() is not { Length: <= MaxReason }))
                        throw new IOException("The peer returned an unbounded omission set.");
                    if (status != "complete" || omissions.GetArrayLength() != 0)
                        throw new IOException($"Native migration {status}: {reason}");
                    if (!value.TryGetProperty("drained", out var drainedField) ||
                        drainedField.ValueKind is not (JsonValueKind.True or JsonValueKind.False) ||
                        !value.TryGetProperty("writerClaimed", out var claimedField) ||
                        claimedField.ValueKind is not (JsonValueKind.True or JsonValueKind.False))
                        throw new IOException("The peer omitted its drain and writer proof.");
                    var drained = drainedField.GetBoolean();
                    var claimed = claimedField.GetBoolean();
                    if (!drained || claimed != (identity.Operation is "import_state_v1" or "inspect_state_v1" or "reconcile_state_v1"))
                        throw new IOException("The native peer did not prove drain and writer transfer.");
                    digest = RequiredText(value, "payloadSha256", 64);
                }
                else
                    throw new IOException("The peer sent an unexpected frame or a duplicate terminal.");
            }
            process.WaitForExitAsync(deadline.Token).GetAwaiter().GetResult();
            if (!terminalSeen || process.ExitCode != 0 || digest is null ||
                digest != Hash(received.ToArray()))
            {
                var details = stderr.GetAwaiter().GetResult();
                throw new IOException("Native migration exited without a valid complete terminal and payload digest." +
                    (details.Length == 0 ? "" : $" {details}"));
            }
            return new PeerResult(received.ToArray(), digest);
        }
        catch (OperationCanceledException exception)
        {
            throw new IOException("The native migration job exceeded its deadline.", exception);
        }
        finally
        {
            if (!process.HasExited)
            {
                try { process.Kill(entireProcessTree: true); }
                catch (InvalidOperationException) { /* exited between probe and kill */ }
            }
            try { stderr.WaitAsync(TimeSpan.FromSeconds(2)).GetAwaiter().GetResult(); }
            catch (OperationCanceledException) { /* cancelled diagnostics are not evidence */ }
            catch (TimeoutException) { throw new IOException("Native peer diagnostics did not close after termination."); }
        }
    }

    private static async Task<string> ReadStderrAsync(Stream stream, CancellationToken token)
    {
        var buffer = new byte[4097];
        var total = 0;
        while (total < buffer.Length)
        {
            var count = await stream.ReadAsync(buffer.AsMemory(total), token);
            if (count == 0)
                return new string(Encoding.UTF8.GetString(buffer, 0, total).Where(c => !char.IsControl(c)).Take(240).ToArray());
            total += count;
        }
        // If the peer keeps emitting output, a bounded cap is a terminal failure.
        throw new IOException("Native peer diagnostics exceeded the 4 KiB limit.");
    }

    private static string RequiredText(JsonElement value, string name, int max)
    {
        if (!value.TryGetProperty(name, out var property) || property.ValueKind != JsonValueKind.String ||
            property.GetString() is not { } result || result.Length > max)
            throw new IOException($"The native peer omitted bounded field '{name}'.");
        return result;
    }

    internal static void WriteFrame(Stream stream, object frame, CancellationToken deadline)
    {
        var payload = JsonSerializer.SerializeToUtf8Bytes(frame, Json);
        if (payload.Length is <= 0 or > MaxFrame)
            throw new IOException("An outbound migration frame exceeds the 128 KiB limit.");
        Span<byte> prefix = stackalloc byte[4];
        BinaryPrimitives.WriteInt32LittleEndian(prefix, payload.Length);
        stream.WriteAsync(prefix.ToArray(), deadline).GetAwaiter().GetResult();
        stream.WriteAsync(payload, deadline).GetAwaiter().GetResult();
        stream.FlushAsync(deadline).GetAwaiter().GetResult();
    }

    internal static JsonDocument? ReadFrame(Stream stream, CancellationToken deadline)
    {
        var prefix = new byte[4];
        var first = stream.ReadAsync(prefix.AsMemory(0, 1), deadline).AsTask().GetAwaiter().GetResult();
        if (first == 0) return null;
        stream.ReadExactlyAsync(prefix.AsMemory(1), deadline).AsTask().GetAwaiter().GetResult();
        var length = BinaryPrimitives.ReadInt32LittleEndian(prefix);
        if (length is <= 0 or > MaxFrame)
            throw new IOException("The native peer sent an oversized or malformed frame.");
        var buffer = new byte[length];
        stream.ReadExactlyAsync(buffer.AsMemory(), deadline).AsTask().GetAwaiter().GetResult();
        return JsonDocument.Parse(buffer, new JsonDocumentOptions { MaxDepth = 24 });
    }

    private static void RequireExternalPeer(Workspace workspace, string path, string digest,
        string profile, string grant)
    {
        if (!Path.IsPathFullyQualified(path) || !File.Exists(path) ||
            !Path.IsPathFullyQualified(Path.GetDirectoryName(path)!) ||
            !IsDigest(digest) || !IsDigest(profile) || string.IsNullOrWhiteSpace(grant) || grant.Length > 128 ||
            ExecutionProfile.Contains(workspace.Root, path))
            throw new IOException("A digest-checked external native binary and grant are required.");
        Workspace.CheckDirectory(Path.GetDirectoryName(path)!);
        if ((File.GetAttributes(path) & (FileAttributes.ReparsePoint | FileAttributes.Directory)) != 0 ||
            !string.Equals(DigestFile(path), digest, StringComparison.Ordinal))
            throw new IOException("The selected native peer binary changed or is not a physical file.");
    }

    private static string MarkerPath(Workspace workspace) =>
        Path.Combine(workspace.StateDirectory, "migration-engine.json");
    private static string BackupPath(Workspace workspace, string epoch) =>
        Path.Combine(workspace.StateDirectory, $"migration-backup-{epoch}.json");
    private static string Hash(ReadOnlySpan<byte> value) => Convert.ToHexString(SHA256.HashData(value));
    private static string DigestFile(string path)
    {
        using var stream = File.OpenRead(path);
        return Convert.ToHexString(SHA256.HashData(stream));
    }
    private static bool IsDigest(string value) => value.Length == 64 && value.All(c =>
        c is >= '0' and <= '9' or >= 'A' and <= 'F');

    private static void WriteBackup(Workspace workspace, string epoch, byte[] bytes, string sha)
    {
        Workspace.CreatePrivateDirectory(workspace.StateDirectory);
        var path = BackupPath(workspace, epoch);
        if (File.Exists(path))
            throw new IOException("A previous migration backup requires owner review before another transition.");
        using (var stream = new FileStream(path, FileMode.CreateNew, FileAccess.Write, FileShare.None))
        {
            stream.Write(bytes);
            stream.Flush(flushToDisk: true);
        }
        if (Hash(File.ReadAllBytes(path)) != sha)
            throw new IOException("The migration recovery backup changed while being staged.");
    }

    private static void WriteMarker(Workspace workspace, WriterMarker marker)
    {
        Workspace.CreatePrivateDirectory(workspace.StateDirectory);
        var path = MarkerPath(workspace);
        if (File.Exists(path) && (File.GetAttributes(path) & FileAttributes.ReparsePoint) != 0)
            throw new IOException("The writer marker is not an ordinary file.");
        var staging = path + ".pending";
        if (File.Exists(staging))
            throw new IOException("An unreviewed writer-marker staging file already exists.");
        try
        {
            using (var stream = new FileStream(staging, FileMode.CreateNew, FileAccess.Write, FileShare.None))
            {
                JsonSerializer.Serialize(stream, marker, Json);
                stream.Flush(flushToDisk: true);
            }
            File.Move(staging, path, overwrite: true);
        }
        finally
        {
            if (File.Exists(staging) && (File.GetAttributes(staging) & FileAttributes.ReparsePoint) == 0)
                File.Delete(staging);
        }
    }

    private static WriterMarker ReadMarker(string path)
    {
        if ((File.GetAttributes(path) & (FileAttributes.ReparsePoint | FileAttributes.Directory)) != 0)
            throw new IOException("The migration writer marker is not an ordinary file.");
        using var stream = File.OpenRead(path);
        if (stream.Length is < 1 or > 4096)
            throw new IOException("The migration writer marker is not bounded.");
        return JsonSerializer.Deserialize<WriterMarker>(stream, Json)
            ?? throw new IOException("The migration writer marker is empty.");
    }
}
