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
    double? PssBytes, double? CgroupPeakBytes, double? LatencyMilliseconds);
internal sealed record SampleCapture(string Platform, string Engine, string Scenario, string BinarySha256,
    int ElapsedMilliseconds, bool ScopeComplete, string[] ScopeOmissions, SamplePoint[] Samples);

internal static class Sampler
{
    internal static async Task<SampleCapture> CaptureAsync(SampleSpec spec)
    {
        if (spec.SchemaVersion != 1 || (!OperatingSystem.IsWindows() && !OperatingSystem.IsLinux()) ||
            spec.Platform != (OperatingSystem.IsWindows() ? "windows" : "linux") ||
            spec.Engine is not ("native" or "legacy") || string.IsNullOrWhiteSpace(spec.Scenario) ||
            spec.ElapsedMilliseconds is < 1000 or > 600_000 || spec.IntervalMilliseconds is < 20 or > 5000 ||
            !Path.IsPathFullyQualified(spec.BinaryPath) || !Path.IsPathFullyQualified(spec.WorkingDirectory) ||
            spec.Arguments is null || spec.Arguments.Length > 64 ||
            (File.GetAttributes(spec.BinaryPath) & (FileAttributes.ReparsePoint | FileAttributes.Directory)) != 0)
            throw new IOException("The requested sampler scope, paths, platform or duration are invalid.");
        using (var stream = File.OpenRead(spec.BinaryPath))
            if (Convert.ToHexString(SHA256.HashData(stream)) != spec.BinarySha256)
                throw new IOException("Sampler executable differs from the exact release-binary digest.");

        var errors = new HashSet<string>(StringComparer.Ordinal);
        var samples = new List<SamplePoint>();
        var timer = Stopwatch.StartNew();
        using var scope = StartSampleProcess(spec);
        var recordedLatencies = ReadLatencies(spec.LatencyEventsPath);
        while (timer.ElapsedMilliseconds < spec.ElapsedMilliseconds)
        {
            try
            {
                var roots = OperatingSystem.IsWindows()
                    ? WindowsProcesses(scope.RootPid)
                    : LinuxProcesses(scope.RootPid, spec.LinuxCgroupPath);
                if (roots.Count == 0 || !roots.Contains(scope.RootPid) && !scope.HasExited)
                    errors.Add("root_process_or_tree_missing");
                double? privateBytes = null, commitBytes = null, jobBytes = null, pssBytes = null, cgroupBytes = null;
                if (OperatingSystem.IsWindows())
                {
                    privateBytes = 0;
                    commitBytes = 0;
                    foreach (var pid in roots)
                    {
                        var counters = WindowsMetrics(pid);
                        if (counters is null) { errors.Add("windows_process_counters_incomplete"); continue; }
                        privateBytes += counters.Value.PrivateWorkingSet;
                        commitBytes += counters.Value.Commit;
                    }
                    jobBytes = scope.ReadJobPeak();
                    if (jobBytes is null) errors.Add("missing_job_peak");
                    var activeJobProcesses = scope.ReadJobActive();
                    if (activeJobProcesses is null || activeJobProcesses != roots.Count)
                        errors.Add("job_membership_does_not_match_process_tree");
                }
                else
                {
                    pssBytes = 0;
                    foreach (var pid in roots)
                    {
                        var pss = LinuxPss(pid);
                        if (pss is null) { errors.Add("linux_pss_unavailable"); continue; }
                        pssBytes += pss.Value;
                    }
                    cgroupBytes = LinuxCgroupPeak(spec.LinuxCgroupPath);
                    if (cgroupBytes is null) errors.Add("missing_cgroup_peak");
                }
                samples.Add(new SamplePoint(timer.ElapsedMilliseconds, roots.Count,
                    privateBytes, commitBytes, jobBytes, pssBytes, cgroupBytes,
                    recordedLatencies.Count > samples.Count ? recordedLatencies[samples.Count] : null));
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
        return new SampleCapture(spec.Platform, spec.Engine, spec.Scenario, spec.BinarySha256,
            (int)timer.ElapsedMilliseconds, errors.Count == 0, errors.Order().ToArray(), samples.ToArray());
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
        // A bare Process.Start cannot enroll a process before first execution in a
        // delegated cgroup. Collect observations but mark scope unqualified unless
        // the owner uses an external supervisor and attests the containment gate.
        var info = new ProcessStartInfo(spec.BinaryPath)
        {
            WorkingDirectory = spec.WorkingDirectory, UseShellExecute = false,
            CreateNoWindow = true
        };
        foreach (var arg in spec.Arguments)
            info.ArgumentList.Add(arg);
        var child = Process.Start(info) ?? throw new IOException("The sampled process did not start.");
        return new LinuxSampleScope(child);
    }

    private abstract class SampleScope : IDisposable
    {
        internal abstract int RootPid { get; }
        internal abstract bool HasExited { get; }
        internal abstract bool EnrolledBeforeExecution { get; }
        internal virtual double? ReadJobPeak() => null;
        internal virtual int? ReadJobActive() => null;
        internal abstract bool Drain();
        public abstract void Dispose();
    }
    private sealed class LinuxSampleScope(Process process) : SampleScope
    {
        internal override int RootPid => process.Id;
        internal override bool HasExited => process.HasExited;
        internal override bool EnrolledBeforeExecution => false;
        internal override bool Drain()
        {
            if (!process.HasExited)
                process.Kill(entireProcessTree: true);
            process.WaitForExit(5000);
            return false; // no persisted, independently queried cgroup scope
        }
        public override void Dispose() => process.Dispose();
    }

    private static HashSet<int> LinuxProcesses(int root, string? cgroup)
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
        var active = new HashSet<int> { root };
        var grew = true;
        while (grew)
        {
            grew = false;
            foreach (var pair in parents)
                if (!active.Contains(pair.Key) && active.Contains(pair.Value))
                    grew |= active.Add(pair.Key);
        }
        if (cgroup is not null)
        {
            var membersPath = Path.Combine(cgroup, "cgroup.procs");
            var members = File.ReadAllLines(membersPath)
                .Select(int.Parse).ToHashSet();
            if (!active.IsSubsetOf(members))
                return []; // scope leakage is not a valid memory sample
            active.UnionWith(members); // include children reparented after parent exit
        }
        return active;
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

    private static HashSet<int> WindowsProcesses(int root)
    {
        var parents = new Dictionary<int, int>();
        var snapshot = CreateToolhelp32Snapshot(2, 0);
        if (snapshot == new IntPtr(-1)) throw new Win32Exception();
        try
        {
            var entry = new ProcessEntry { Size = (uint)Marshal.SizeOf<ProcessEntry>() };
            if (Process32FirstW(snapshot, ref entry))
                do { parents[(int)entry.Pid] = (int)entry.ParentPid; }
                while (Process32NextW(snapshot, ref entry));
        }
        finally { CloseHandle(snapshot); }
        var active = new HashSet<int> { root };
        var grew = true;
        while (grew)
        {
            grew = false;
            foreach (var pair in parents)
                if (!active.Contains(pair.Key) && active.Contains(pair.Value))
                    grew |= active.Add(pair.Key);
        }
        return active;
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
        internal override int? ReadJobActive()
        {
            var accounting = new JobBasicAccounting();
            return QueryInformationJobObject(job, 1, ref accounting,
                (uint)Marshal.SizeOf<JobBasicAccounting>(), IntPtr.Zero)
                    ? checked((int)accounting.ActiveProcesses) : null;
        }
        internal override bool Drain()
        {
            if (!TerminateJobObject(job, 1)) return false;
            if (WaitForSingleObject(process, 5000) != 0) return false;
            // A killed root alone is insufficient; an empty process count from
            // JobBasicAccountingInformation confirms all supervised children drained.
            var accounting = new JobBasicAccounting();
            return QueryInformationJobObject(job, 1, ref accounting,
                (uint)Marshal.SizeOf<JobBasicAccounting>(), IntPtr.Zero) && accounting.ActiveProcesses == 0;
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

    [StructLayout(LayoutKind.Sequential, CharSet = CharSet.Unicode)]
    private struct ProcessEntry
    {
        internal uint Size, Usage, Pid;
        internal IntPtr Heap;
        internal uint Module, Threads, ParentPid;
        internal int BasePriority;
        internal uint Flags;
        [MarshalAs(UnmanagedType.ByValTStr, SizeConst = 260)] internal string? Name;
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

    [DllImport("kernel32", SetLastError = true)] private static extern IntPtr CreateToolhelp32Snapshot(uint flags, uint pid);
    [DllImport("kernel32", EntryPoint = "Process32FirstW", ExactSpelling = true, CharSet = CharSet.Unicode, SetLastError = true)]
    [return: MarshalAs(UnmanagedType.Bool)] private static extern bool Process32FirstW(IntPtr snapshot, ref ProcessEntry entry);
    [DllImport("kernel32", EntryPoint = "Process32NextW", ExactSpelling = true, CharSet = CharSet.Unicode, SetLastError = true)]
    [return: MarshalAs(UnmanagedType.Bool)] private static extern bool Process32NextW(IntPtr snapshot, ref ProcessEntry entry);
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
