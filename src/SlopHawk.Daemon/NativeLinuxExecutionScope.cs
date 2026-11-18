using System.Diagnostics;
using System.Runtime.InteropServices;
using System.Runtime.Versioning;

namespace SlopHawk.Daemon;

// The native owner keeps the user-wide reservation and proves cgroup drainage.
// This peer only borrows the containing invocation for its direct children.
[SupportedOSPlatform("linux")]
internal sealed class NativeLinuxExecutionScope(string journal, string unit, string invocation, string cgroup)
{
    internal static async Task<NativeLinuxExecutionScope?> TryAttachAsync(CancellationToken token)
    {
        const string prefix = "slophawk-shadow-";
        const string suffix = ".service";
        var cgroup = ReadCgroup();
        if (cgroup is null || !cgroup.StartsWith("/user.slice/", StringComparison.Ordinal))
            return null;
        var unit = Path.GetFileName(cgroup);
        if (!unit.StartsWith(prefix, StringComparison.Ordinal) ||
            !unit.EndsWith(suffix, StringComparison.Ordinal) ||
            !Guid.TryParseExact(unit[prefix.Length..^suffix.Length], "N", out _))
            return null;
        var invocation = Environment.GetEnvironmentVariable("INVOCATION_ID");
        if (invocation is null || !Guid.TryParseExact(invocation, "N", out _))
            return null;
        var uid = EffectiveUser();
        var runtime = $"/run/user/{uid}";
        var directory = Path.Combine(runtime, "slophawk-native-worker-v1");
        var journal = Path.Combine(directory, "pending");
        var scope = new NativeLinuxExecutionScope(journal, unit, invocation, cgroup);
        // The manager may exec the peer while the native owner is still
        // collecting the acknowledged invocation and updating its journal.
        var started = Stopwatch.GetTimestamp();
        while (Stopwatch.GetElapsedTime(started) < TimeSpan.FromSeconds(3))
        {
            token.ThrowIfCancellationRequested();
            if (Private(runtime, directory: true, uid) && Private(directory, directory: true, uid) &&
                scope.IsCurrent(uid))
                return scope;
            await Task.Delay(25, token);
        }
        return null;
    }

    internal void RequireCurrent()
    {
        if (!IsCurrent(EffectiveUser()))
            throw new UnauthorizedAccessException("The containing native worker invocation changed.");
    }

    private bool IsCurrent(uint uid)
    {
        try
        {
            if (ReadCgroup() != cgroup || !Private(journal, directory: false, uid))
                return false;
            using var input = new FileStream(journal, FileMode.Open, FileAccess.Read, FileShare.Read | FileShare.Delete);
            if (input.Length is <= 0 or > 2200)
                return false;
            using var reader = new StreamReader(input);
            return reader.ReadLine() == unit && reader.ReadLine() == invocation &&
                reader.ReadLine() == cgroup && reader.ReadLine() is null;
        }
        catch (IOException)
        {
            return false;
        }
    }

    internal async Task<WorkerResult> RunAsync(ProcessStartInfo command, int maxOutput, CancellationToken token)
    {
        RequireCurrent();
        // The Windows Job launcher installs its own capture handles.
        // Process.Start on Linux needs the redirects on the start info.
        command.RedirectStandardOutput = true;
        command.RedirectStandardError = true;
        using var deadline = CancellationTokenSource.CreateLinkedTokenSource(token);
        var result = await ProjectEvaluator.RunExplicitAsync(command, deadline, maxOutput);
        RequireCurrent();
        return result;
    }

    private static string? ReadCgroup()
    {
        using var input = new FileStream("/proc/self/cgroup", FileMode.Open, FileAccess.Read, FileShare.Read);
        using var reader = new StreamReader(input);
        var line = reader.ReadLine();
        if (line is not { Length: < 2048 } || !line.StartsWith("0::/", StringComparison.Ordinal) ||
            reader.ReadLine() is not null)
            return null;
        return line[3..];
    }

    private static bool Private(string path, bool directory, uint uid)
    {
        const int currentDirectory = -100;
        const int noFollow = 0x100;
        const uint fields = 0x1 | 0x2 | 0x8; // type, mode and owner
        const ushort typeMask = 0xf000;
        const ushort otherMode = 0x003f;
        return Statx(currentDirectory, path, noFollow, fields, out var info) == 0 &&
            (info.Mask & fields) == fields && info.Owner == uid && (info.Mode & otherMode) == 0 &&
            (info.Mode & typeMask) == (directory ? 0x4000 : 0x8000);
    }

    [StructLayout(LayoutKind.Explicit, Size = 256)]
    private struct LinuxStatx
    {
        [FieldOffset(0)] public uint Mask;
        [FieldOffset(20)] public uint Owner;
        [FieldOffset(28)] public ushort Mode;
    }

    [DllImport("libc", EntryPoint = "geteuid")]
    private static extern uint EffectiveUser();

    [DllImport("libc", EntryPoint = "statx", SetLastError = true)]
    private static extern int Statx(int atDirectory, [MarshalAs(UnmanagedType.LPUTF8Str)] string path,
        int flags, uint mask, out LinuxStatx info);
}
