using System.ComponentModel;
using System.Diagnostics;
using System.Globalization;
using System.Runtime.InteropServices;
using System.Security.Cryptography;
using System.Text;
using System.Text.Json;

namespace SlopHawk.RustMigrationQualification;

internal sealed record SampleSpec(int SchemaVersion, string Platform, string Engine, string Scenario,
    string BinaryPath, string BinarySha256, string[] Arguments, string WorkingDirectory,
    int ElapsedMilliseconds, int IntervalMilliseconds, string? LinuxCgroupPath,
    string? LatencyEventsPath);
internal sealed record SamplePoint(long SinceStartMilliseconds, int Processes,
    double? PrivateWorkingSetBytes, double? CommitBytes, double? JobPeakBytes,
    double? PssBytes, double? CgroupPeakBytes, double? LatencyMilliseconds,
    double? ControllerResidentBytes = null);
internal sealed record SampleCapture(string Platform, string Engine, string Scenario, string BinarySha256,
    int ElapsedMilliseconds, bool ScopeComplete, string[] ScopeOmissions, SamplePoint[] Samples,
    string? ScopeControllerSha256 = null);
internal sealed record SampleController(string Path, string Sha256, string[] Arguments);

internal static partial class Sampler
{
    internal static async Task<SampleCapture> CaptureAsync(SampleSpec spec, Func<Task>? ready = null,
        Func<Task<IReadOnlyList<double>>>? workload = null, Action? launching = null, SampleController? controller = null)
    {
        if (spec.SchemaVersion != 1 || (!OperatingSystem.IsWindows() && !OperatingSystem.IsLinux()) ||
            spec.Platform != (OperatingSystem.IsWindows() ? "windows" : "linux") ||
            spec.Engine is not ("native" or "legacy") || string.IsNullOrWhiteSpace(spec.Scenario) ||
            spec.ElapsedMilliseconds is < 1000 or > 600_000 || spec.IntervalMilliseconds is < 20 or > 5000 ||
            !Path.IsPathFullyQualified(spec.BinaryPath) || !Path.IsPathFullyQualified(spec.WorkingDirectory) ||
            spec.Arguments is null || spec.Arguments.Length > 64 ||
            (File.GetAttributes(spec.BinaryPath) & (FileAttributes.ReparsePoint | FileAttributes.Directory)) != 0)
            throw new IOException("The requested sampler scope, paths, platform or duration are invalid.");
        VerifyExecutable(spec.BinaryPath, spec.BinarySha256);
        var launch = spec;
        if (controller is not null)
        {
            VerifyExecutable(controller.Path, controller.Sha256);
            launch = spec with { BinaryPath = controller.Path, BinarySha256 = controller.Sha256, Arguments = controller.Arguments };
        }

        var errors = new HashSet<string>(StringComparer.Ordinal);
        var samples = new List<SamplePoint>();
        launching?.Invoke();
        using var scope = StartSampleProcess(launch);
        if (ready is not null) await ready();
        var timer = Stopwatch.StartNew();
        var execution = workload?.Invoke();
        var recordedLatencies = ReadLatencies(spec.LatencyEventsPath);
        while (timer.ElapsedMilliseconds < spec.ElapsedMilliseconds)
        {
            try
            {
                var roots = OperatingSystem.IsWindows()
                    ? ((WindowsJobScope)scope).Members()
                    : LinuxMembers(scope.CgroupPath);
                double? privateBytes = null, commitBytes = null, jobBytes = null, pssBytes = null, cgroupBytes = null;
                double? controllerBytes = null;
                if (OperatingSystem.IsWindows())
                {
                    privateBytes = 0;
                    commitBytes = 0;
                    var countersComplete = true;
                    foreach (var pid in roots)
                    {
                        var counters = WindowsMetrics(pid);
                        if (counters is null) { countersComplete = false; break; }
                        privateBytes += counters.Value.PrivateWorkingSet;
                        if (controller is not null && pid == scope.RootPid)
                            controllerBytes = counters.Value.PrivateWorkingSet;
                        commitBytes += counters.Value.Commit;
                    }
                    jobBytes = scope.ReadJobPeak();
                    // A process can leave the Job between the membership and counter reads.
                    // Discard that observation instead of recording a partial working set.
                    if (!roots.SetEquals(((WindowsJobScope)scope).Members()))
                    {
                        await Task.Delay(spec.IntervalMilliseconds);
                        continue;
                    }
                    if (!countersComplete) errors.Add("windows_process_counters_incomplete");
                    if (jobBytes is null) errors.Add("missing_job_peak");
                }
                else
                {
                    pssBytes = 0;
                    var countersComplete = true;
                    foreach (var pid in roots)
                    {
                        var pss = LinuxPss(pid);
                        if (pss is null) { countersComplete = false; break; }
                        pssBytes += pss.Value;
                        if (controller is not null && pid == scope.RootPid) controllerBytes = pss;
                    }
                    cgroupBytes = LinuxCgroupPeak(scope.CgroupPath);
                    var descendants = LinuxDescendants(scope.RootPid);
                    if (!roots.SetEquals(LinuxMembers(scope.CgroupPath)))
                    {
                        await Task.Delay(spec.IntervalMilliseconds);
                        continue;
                    }
                    if (scope.CgroupPath is { } scopePath && HasUncontainedLinuxDescendant(descendants, roots, scopePath))
                        errors.Add("linux_descendant_outside_cgroup");
                    if (!countersComplete) errors.Add("linux_pss_unavailable");
                    if (cgroupBytes is null) errors.Add("missing_cgroup_peak");
                }
                if (roots.Count == 0 || !roots.Contains(scope.RootPid) && !scope.HasExited)
                    errors.Add("root_process_or_tree_missing");
                samples.Add(new SamplePoint(timer.ElapsedMilliseconds, roots.Count,
                    privateBytes, commitBytes, jobBytes, pssBytes, cgroupBytes,
                    recordedLatencies.Count > samples.Count ? recordedLatencies[samples.Count] : null, controllerBytes));
            }
            catch (Exception ex) when (ex is IOException or UnauthorizedAccessException or Win32Exception or FormatException)
            {
                errors.Add("sample_read_failed");
            }
            await Task.Delay(spec.IntervalMilliseconds);
        }
        // The containment owner must drain its entire process scope. Killing a root
        // PID alone does not establish drainage and cannot count as qualification.
        if (!scope.Drain())
            errors.Add("process_tree_not_drained");
        if (samples.Count < 20)
            errors.Add("insufficient_samples");
        if (!scope.EnrolledBeforeExecution)
            errors.Add("process_scope_not_enrolled_before_execution");
        if (execution is not null)
        {
            if (!execution.IsCompleted) errors.Add("workload_outlasted_sample_scope");
            var events = await execution;
            for (var index = 0; index < Math.Min(events.Count, samples.Count); index++)
                samples[index] = samples[index] with { LatencyMilliseconds = events[index] };
        }
        return new SampleCapture(spec.Platform, spec.Engine, spec.Scenario, spec.BinarySha256,
            (int)timer.ElapsedMilliseconds, errors.Count == 0, errors.Order().ToArray(), samples.ToArray(), controller?.Sha256);
    }

    internal static void VerifyExecutable(string path, string digest)
    {
        if (!Path.IsPathFullyQualified(path) || (File.GetAttributes(path) &
            (FileAttributes.ReparsePoint | FileAttributes.Directory)) != 0)
            throw new IOException("Measurement executable must be a physical absolute file.");
        using var stream = File.OpenRead(path);
        if (Convert.ToHexString(SHA256.HashData(stream)) != digest)
            throw new IOException("Measurement executable differs from its pinned binary digest.");
    }

    private static IReadOnlyList<double> ReadLatencies(string? path)
    {
        if (path is null || !File.Exists(path)) return [];
        using var stream = File.OpenRead(path);
        if (stream.Length > 1024 * 1024)
            throw new IOException("The latency event receipt is oversized.");
        var values = new List<double>();
        foreach (var line in File.ReadLines(path))
        {
            using var item = JsonDocument.Parse(line);
            var value = item.RootElement.GetProperty("elapsedMs").GetDouble();
            if (!double.IsFinite(value) || value < 0 || values.Count == 100_000)
                throw new IOException("The latency event receipt is invalid or unbounded.");
            values.Add(value);
        }
        return values;
    }

    private static SampleScope StartSampleProcess(SampleSpec spec)
    {
        if (OperatingSystem.IsWindows())
            return WindowsJobScope.Start(spec);
        return LinuxMeasurementScope.Start(spec);
    }

    private abstract class SampleScope : IDisposable
    {
        internal abstract int RootPid { get; }
        internal abstract bool HasExited { get; }
        internal abstract bool EnrolledBeforeExecution { get; }
        internal virtual string? CgroupPath => null;
        internal virtual double? ReadJobPeak() => null;
        internal abstract bool Drain();
        public abstract void Dispose();
    }

    private static HashSet<int> LinuxMembers(string? cgroup)
    {
        if (cgroup is null) throw new IOException("Linux measurement scope has no cgroup.");
        var members = new HashSet<int>();
        var groups = new Queue<string>();
        groups.Enqueue(cgroup);
        var count = 0;
        while (groups.TryDequeue(out var group))
        {
            try
            {
                foreach (var line in File.ReadLines(Path.Combine(group, "cgroup.procs")))
                    members.Add(int.Parse(line, CultureInfo.InvariantCulture));
                foreach (var child in Directory.EnumerateDirectories(group))
                {
                    if (++count > 4096)
                        throw new IOException("The measurement cgroup tree exceeded its scope.");
                    try
                    {
                        if ((File.GetAttributes(child) & FileAttributes.ReparsePoint) != 0)
                            throw new IOException("Unexpected linked measurement cgroup.");
                        groups.Enqueue(child);
                    }
                    catch (IOException error) when (error is FileNotFoundException or DirectoryNotFoundException)
                    { /* The child disappeared before it could have any members. */ }
                }
            }
            catch (IOException error) when (group != cgroup && error is FileNotFoundException or DirectoryNotFoundException)
            { /* A retired child cgroup has no members. The second snapshot still verifies stability. */ }
        }
        return members;
    }

    private static HashSet<int> LinuxDescendants(int root)
    {
        var parents = new Dictionary<int, int>();
        foreach (var path in Directory.EnumerateDirectories("/proc"))
        {
            if (!int.TryParse(Path.GetFileName(path), out var pid)) continue;
            try
            {
                var stat = File.ReadAllText(Path.Combine(path, "stat"));
                var end = stat.LastIndexOf(')');
                if (end == -1) continue;
                var fields = stat[(end + 2)..].Split(' ', StringSplitOptions.RemoveEmptyEntries);
                if (fields.Length > 1 && int.TryParse(fields[1], out var ppid))
                    parents[pid] = ppid;
            }
            catch (IOException) { /* disappeared between /proc enumeration and read */ }
            catch (UnauthorizedAccessException) { /* not readable: caller must fail coverage */ }
        }
        var active = new HashSet<int>();
        var grew = true;
        while (grew)
        {
            grew = false;
            foreach (var pair in parents)
                if (pair.Value == root || active.Contains(pair.Value))
                    grew |= active.Add(pair.Key);
        }
        return active;
    }

    private static bool HasUncontainedLinuxDescendant(HashSet<int> descendants, HashSet<int> measured, string cgroup)
    {
        const string kernelRoot = "/sys/fs/cgroup";
        var group = cgroup[kernelRoot.Length..];
        foreach (var pid in descendants.Except(measured))
        {
            var process = $"/proc/{pid}";
            try
            {
                var current = File.ReadLines(Path.Combine(process, "cgroup")).FirstOrDefault(line => line.StartsWith("0::", StringComparison.Ordinal));
                if (current is null || current[3..] != group && !current[3..].StartsWith(group + "/", StringComparison.Ordinal))
                    return true;
            }
            catch (IOException error) when (error is FileNotFoundException or DirectoryNotFoundException || !Directory.Exists(process))
            {
                // A checker can start and finish between equal cgroup snapshots.
                // An exited PID cannot be evidence of an escaped running process.
            }
        }
        return false;
    }

    private static double? LinuxPss(int pid)
    {
        try
        {
            foreach (var line in File.ReadLines($"/proc/{pid}/smaps_rollup"))
                if (line.StartsWith("Pss:", StringComparison.Ordinal))
                {
                    var number = line[4..].Trim().Split(' ', StringSplitOptions.RemoveEmptyEntries)[0];
                    return long.Parse(number, CultureInfo.InvariantCulture) * 1024d;
                }
        }
        catch (IOException) { }
        catch (UnauthorizedAccessException) { }
        return null;
    }

    private static double? LinuxCgroupPeak(string? cgroup)
    {
        if (string.IsNullOrWhiteSpace(cgroup)) return null;
        var path = Path.GetFullPath(cgroup);
        if (!path.StartsWith("/sys/fs/cgroup/", StringComparison.Ordinal)) return null;
        var value = File.ReadAllText(Path.Combine(path, "memory.peak")).Trim();
        return long.TryParse(value, CultureInfo.InvariantCulture, out var bytes) ? bytes : null;
    }

    private static (double PrivateWorkingSet, double Commit)? WindowsMetrics(int pid)
    {
        IntPtr handle = OpenProcess(0x1000 | 0x0400, false, (uint)pid);
        if (handle == IntPtr.Zero) return null;
        try
        {
            var memory = new ProcessMemoryCountersEx2 { Size = (uint)Marshal.SizeOf<ProcessMemoryCountersEx2>() };
            return GetProcessMemoryInfo(handle, ref memory, memory.Size)
                ? (memory.PrivateWorkingSet.ToInt64(), memory.PagefileUsage.ToInt64()) : null;
        }
        finally { CloseHandle(handle); }
    }

    private sealed class WindowsJobScope : SampleScope
    {
        private IntPtr job;
        private IntPtr process;
        private uint pid;
        private WindowsJobScope(IntPtr job, IntPtr process, uint pid)
        {
            this.job = job; this.process = process; this.pid = pid;
        }
        internal static WindowsJobScope Start(SampleSpec spec)
        {
            var job = CreateJobObjectW(IntPtr.Zero, null);
            if (job == IntPtr.Zero) throw new Win32Exception();
            var limits = new JobExtendedLimits();
            limits.Basic.LimitFlags = 0x2000; // JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE
            var bytes = Marshal.SizeOf<JobExtendedLimits>();
            if (!SetInformationJobObject(job, 9, ref limits, (uint)bytes))
            { CloseHandle(job); throw new Win32Exception(); }
            var startup = new StartupInfo { Size = (uint)Marshal.SizeOf<StartupInfo>() };
            var cmd = new StringBuilder(string.Join(" ", new[] { spec.BinaryPath }.Concat(spec.Arguments)
                .Select(QuoteWindows)));
            if (!CreateProcessW(spec.BinaryPath, cmd, IntPtr.Zero, IntPtr.Zero, false, 0x00000004 | 0x08000000,
                IntPtr.Zero, spec.WorkingDirectory, ref startup, out var info))
            { CloseHandle(job); throw new Win32Exception(); }
            try
            {
                if (!AssignProcessToJobObject(job, info.Process))
                    throw new Win32Exception(); // never run code outside the tracked Job
                if (ResumeThread(info.Thread) == uint.MaxValue)
                    throw new Win32Exception();
                return new WindowsJobScope(job, info.Process, info.ProcessId);
            }
            catch
            {
                TerminateProcess(info.Process, 1);
                CloseHandle(info.Process);
                CloseHandle(job);
                throw;
            }
            finally { CloseHandle(info.Thread); }
        }
        internal override int RootPid => (int)pid;
        internal override bool HasExited => WaitForSingleObject(process, 0) == 0;
        internal override bool EnrolledBeforeExecution => true;
        internal override double? ReadJobPeak()
        {
            var info = new JobExtendedLimits();
            return QueryInformationJobObject(job, 9, ref info, (uint)Marshal.SizeOf<JobExtendedLimits>(), IntPtr.Zero)
                ? info.PeakJobMemory.ToInt64() : null;
        }
        internal HashSet<int> Members()
        {
            // This Job forbids breakaway and is assigned before the root runs.
            // Class 3 enumerates the kernel-owned membership, including nested Jobs.
            const int headerSize = 2 * sizeof(uint);
            const int capacity = 4096;
            var size = headerSize + capacity * IntPtr.Size;
            var buffer = Marshal.AllocHGlobal(size);
            try
            {
                if (!QueryInformationJobObject(job, 3, buffer, (uint)size, IntPtr.Zero))
                    throw new Win32Exception();
                var count = Marshal.ReadInt32(buffer, sizeof(uint));
                if (count is < 0 or > capacity || count != Marshal.ReadInt32(buffer))
                    throw new IOException("Windows Job membership is incomplete.");
                var members = new HashSet<int>();
                for (var index = 0; index < count; index++)
                    members.Add(checked((int)Marshal.ReadIntPtr(buffer, headerSize + index * IntPtr.Size)));
                return members;
            }
            finally { Marshal.FreeHGlobal(buffer); }
        }
        internal override bool Drain()
        {
            if (!TerminateJobObject(job, 1)) return false;
            if (WaitForSingleObject(process, 5000) != 0) return false;
            // A killed root alone is insufficient; an empty process count from
            // JobBasicAccountingInformation confirms all supervised children drained.
            var clock = Stopwatch.StartNew();
            do
            {
                var accounting = new JobBasicAccounting();
                if (!QueryInformationJobObject(job, 1, ref accounting,
                    (uint)Marshal.SizeOf<JobBasicAccounting>(), IntPtr.Zero)) return false;
                if (accounting.ActiveProcesses == 0) return true;
                Thread.Sleep(10);
            } while (clock.ElapsedMilliseconds < 5000);
            return false;
        }
        public override void Dispose()
        {
            if (process != IntPtr.Zero) CloseHandle(process);
            if (job != IntPtr.Zero) CloseHandle(job);
            process = job = IntPtr.Zero;
        }
    }

    private static string QuoteWindows(string arg)
    {
        if (arg.Length > 0 && !arg.Any(c => char.IsWhiteSpace(c) || c is '"' or '\\')) return arg;
        var b = new StringBuilder("\"");
        var slash = 0;
        foreach (var c in arg)
        {
            if (c == '\\') { slash++; continue; }
            if (c == '"') { b.Append('\\', slash * 2 + 1); b.Append('"'); slash = 0; continue; }
            b.Append('\\', slash); slash = 0; b.Append(c);
        }
        b.Append('\\', slash * 2); b.Append('"');
        return b.ToString();
    }

    [StructLayout(LayoutKind.Sequential)]
    private struct ProcessMemoryCountersEx2
    {
        internal uint Size, PageFaults;
        internal IntPtr PeakWorkingSet, WorkingSet, PeakPaged, Paged, PeakNonPaged, NonPaged;
        internal IntPtr PagefileUsage, PeakPagefile, PrivateUsage, PrivateWorkingSet;
        internal ulong SharedCommit;
    }
    [StructLayout(LayoutKind.Sequential)]
    private struct StartupInfo
    {
        internal uint Size;
        internal IntPtr Reserved, Desktop, Title;
        internal uint X, Y, Width, Height, XCharacters, YCharacters, Fill, Flags;
        internal ushort ShowWindow, Reserved2;
        internal IntPtr ReservedBuffer, StandardInput, StandardOutput, StandardError;
    }
    [StructLayout(LayoutKind.Sequential)]
    private struct ProcessInfo
    {
        internal IntPtr Process, Thread;
        internal uint ProcessId, ThreadId;
    }
    [StructLayout(LayoutKind.Sequential)]
    private struct JobBasicLimits
    {
        internal long ProcessTime, JobTime;
        internal uint LimitFlags;
        internal IntPtr MinWorkingSet, MaxWorkingSet;
        internal uint ActiveProcessLimit;
        internal IntPtr Affinity;
        internal uint PriorityClass, SchedulingClass;
    }
    [StructLayout(LayoutKind.Sequential)]
    private struct JobIoCounters
    {
        internal ulong ReadOperations, WriteOperations, OtherOperations, ReadBytes, WriteBytes, OtherBytes;
    }
    [StructLayout(LayoutKind.Sequential)]
    private struct JobExtendedLimits
    {
        internal JobBasicLimits Basic;
        internal JobIoCounters Io;
        internal IntPtr ProcessMemoryLimit, JobMemoryLimit, PeakProcessMemory, PeakJobMemory;
    }
    [StructLayout(LayoutKind.Sequential)]
    private struct JobBasicAccounting
    {
        internal long TotalUserTime, TotalKernelTime, ThisPeriodTotalUserTime, ThisPeriodTotalKernelTime;
        internal uint TotalPageFaultCount, TotalProcesses, ActiveProcesses, TotalTerminatedProcesses;
    }

    [DllImport("kernel32", SetLastError = true)] private static extern IntPtr OpenProcess(uint access, bool inherit, uint pid);
    [DllImport("psapi", SetLastError = true)]
    [return: MarshalAs(UnmanagedType.Bool)] private static extern bool GetProcessMemoryInfo(IntPtr process,
        ref ProcessMemoryCountersEx2 counters, uint size);
    [DllImport("kernel32", SetLastError = true)]
    [return: MarshalAs(UnmanagedType.Bool)] private static extern bool CloseHandle(IntPtr handle);
    [DllImport("kernel32", EntryPoint = "CreateJobObjectW", ExactSpelling = true, CharSet = CharSet.Unicode, SetLastError = true)]
    private static extern IntPtr CreateJobObjectW(IntPtr attributes, string? name);
    [DllImport("kernel32", SetLastError = true)]
    [return: MarshalAs(UnmanagedType.Bool)] private static extern bool SetInformationJobObject(IntPtr job,
        int infoClass, ref JobExtendedLimits info, uint size);
    [DllImport("kernel32", SetLastError = true)]
    [return: MarshalAs(UnmanagedType.Bool)] private static extern bool QueryInformationJobObject(IntPtr job,
        int infoClass, ref JobExtendedLimits info, uint size, IntPtr returnLength);
    [DllImport("kernel32", EntryPoint = "QueryInformationJobObject", SetLastError = true)]
    [return: MarshalAs(UnmanagedType.Bool)] private static extern bool QueryInformationJobObject(IntPtr job,
        int infoClass, ref JobBasicAccounting info, uint size, IntPtr returnLength);
    [DllImport("kernel32", SetLastError = true)]
    [return: MarshalAs(UnmanagedType.Bool)] private static extern bool QueryInformationJobObject(IntPtr job,
        int infoClass, IntPtr buffer, uint size, IntPtr returnLength);
    [DllImport("kernel32", SetLastError = true)]
    [return: MarshalAs(UnmanagedType.Bool)] private static extern bool AssignProcessToJobObject(IntPtr job, IntPtr process);
    [DllImport("kernel32", SetLastError = true)]
    [return: MarshalAs(UnmanagedType.Bool)] private static extern bool TerminateJobObject(IntPtr job, uint code);
    [DllImport("kernel32", SetLastError = true)]
    [return: MarshalAs(UnmanagedType.Bool)] private static extern bool TerminateProcess(IntPtr process, uint code);
    [DllImport("kernel32", SetLastError = true)] private static extern uint ResumeThread(IntPtr thread);
    [DllImport("kernel32", SetLastError = true)] private static extern uint WaitForSingleObject(IntPtr handle, uint ms);
    [DllImport("kernel32", EntryPoint = "CreateProcessW", ExactSpelling = true, CharSet = CharSet.Unicode, SetLastError = true)]
    [return: MarshalAs(UnmanagedType.Bool)] private static extern bool CreateProcessW(string application,
        StringBuilder commandLine, IntPtr processAttributes, IntPtr threadAttributes, bool inheritHandles,
        uint flags, IntPtr environment, string currentDirectory, ref StartupInfo startup, out ProcessInfo process);
}
