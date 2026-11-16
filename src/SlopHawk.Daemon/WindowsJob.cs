using System.ComponentModel;
using System.Diagnostics;
using System.IO.Pipes;
using System.Runtime.InteropServices;
using System.Text;
using Microsoft.Win32.SafeHandles;

namespace SlopHawk.Daemon;

internal sealed class WindowsJob : IDisposable
{
    private readonly SafeFileHandle handle;
    private bool terminated;

    internal WindowsJob(Guid id)
    {
        handle = CreateJobObject(IntPtr.Zero, Name(id));
        var error = Marshal.GetLastWin32Error();
        var limits = new ExtendedLimits { Basic = new BasicLimits { Flags = 0x2000 } }; // KILL_ON_JOB_CLOSE
        if (handle.IsInvalid || error == 183 ||
            !SetInformationJobObject(handle, 9, ref limits, (uint)Marshal.SizeOf<ExtendedLimits>()))
        {
            var cause = handle.IsInvalid || error == 183 ? error : Marshal.GetLastWin32Error();
            handle.Dispose();
            throw new IOException("Could not create an isolated worker job.", new Win32Exception(cause));
        }
    }

    private static string Name(Guid id) => $"Global\\SlopGuard.Worker.{id:N}";

    internal static bool IsDrained(Guid id)
    {
        if (!OperatingSystem.IsWindows())
            return false;
        using var observed = OpenForQuery(id);
        return observed.IsInvalid
            ? Marshal.GetLastWin32Error() == 2 // The job no longer exists in the global namespace.
            : QueryAccounting(observed, 1, out var accounting, (uint)Marshal.SizeOf<Accounting>(), IntPtr.Zero)
                && accounting.ActiveProcesses == 0;
    }

    internal static SafeFileHandle OpenForQuery(Guid id) =>
        OpenJobObject(0x0004, false, Name(id)); // JOB_OBJECT_QUERY

    internal async Task<WorkerResult> RunAsync(ProcessStartInfo command, int maxOutput, CancellationToken cancellationToken)
    {
        if (terminated || !Path.IsPathFullyQualified(command.FileName) || !Path.IsPathFullyQualified(command.WorkingDirectory)
            || command.UseShellExecute || command.Arguments.Length != 0)
            throw new IOException("A worker requires an available job and a fully specified executable, arguments and directory.");
        cancellationToken.ThrowIfCancellationRequested();
        using var stdout = new AnonymousPipeServerStream(PipeDirection.In, HandleInheritability.Inheritable);
        using var stderr = new AnonymousPipeServerStream(PipeDirection.In, HandleInheritability.Inheritable);
        using var stdin = new AnonymousPipeServerStream(PipeDirection.Out, HandleInheritability.Inheritable);

        using var process = Launch(command, stdin.ClientSafePipeHandle, stdout.ClientSafePipeHandle, stderr.ClientSafePipeHandle);
        stdin.DisposeLocalCopyOfClientHandle();
        stdin.Dispose(); // The worker receives EOF on stdin.
        stdout.DisposeLocalCopyOfClientHandle();
        stderr.DisposeLocalCopyOfClientHandle();

        using var stop = CancellationTokenSource.CreateLinkedTokenSource(cancellationToken);
        using var termination = stop.Token.Register(() => TerminateJobObject(handle, 1));
        using var output = new StreamReader(stdout, Encoding.UTF8);
        using var errors = new StreamReader(stderr, Encoding.UTF8);
        var readOutput = ReadAsync(output, maxOutput, stop);
        var readErrors = ReadAsync(errors, Math.Min(maxOutput, 32_768), stop);
        var exit = WaitForExitAsync(process, stop.Token);
        try
        {
            var exitCode = await exit;
            // A finished entry process may leave its inherited pipe handles open in
            // descendants. End those descendants before collecting the last output.
            await DrainAsync();
            await Task.WhenAll(readOutput, readErrors);
            cancellationToken.ThrowIfCancellationRequested();
            return new WorkerResult(exitCode, await readOutput, await readErrors);
        }
        catch
        {
            await stop.CancelAsync();
            await DrainAsync();
            try
            {
                await Task.WhenAll(readOutput, readErrors);
            }
            catch (Exception ex) when (ex is IOException or OperationCanceledException)
            {
                // The original failure is preserved after closing the pipes.
            }
            cancellationToken.ThrowIfCancellationRequested();
            if (readOutput.IsFaulted || readErrors.IsFaulted)
                throw new IOException("Worker output could not be collected within its budget.");
            throw;
        }
    }

    internal async Task DrainAsync()
    {
        if (ActiveProcesses() == 0)
            return;
        // The entry process can signal its handle just before job accounting settles.
        await Task.Delay(30);
        if (ActiveProcesses() == 0)
            return;
        terminated = true;
        if (!TerminateJobObject(handle, 1) && ActiveProcesses() != 0)
            throw Failure("Could not terminate the worker job.");
        var started = Stopwatch.GetTimestamp();
        while (ActiveProcesses() != 0)
        {
            if (Stopwatch.GetElapsedTime(started) >= TimeSpan.FromSeconds(5))
                throw new IOException("The worker process tree could not be confirmed empty.");
            await Task.Delay(30);
        }
    }

    private uint ActiveProcesses()
    {
        if (!QueryAccounting(handle, 1, out var accounting, (uint)Marshal.SizeOf<Accounting>(), IntPtr.Zero))
            throw Failure("Could not verify the worker process tree.");
        return accounting.ActiveProcesses;
    }

    private SafeFileHandle Launch(ProcessStartInfo command, SafePipeHandle stdin, SafePipeHandle stdout, SafePipeHandle stderr)
    {
        nuint size = 0;
        InitializeProcThreadAttributeList(IntPtr.Zero, 2, 0, ref size);
        if (size == 0)
            throw Failure("Could not size worker launch attributes.");
        var attributes = Marshal.AllocHGlobal((nint)size);
        var jobList = Marshal.AllocHGlobal(IntPtr.Size);
        var inherited = Marshal.AllocHGlobal(3 * IntPtr.Size);
        var environment = Marshal.StringToHGlobalUni(string.Join('\0', command.Environment
            .OrderBy(entry => entry.Key, StringComparer.OrdinalIgnoreCase).Select(entry => entry.Key + "=" + entry.Value)) + "\0\0");
        var initialized = false;
        try
        {
            if (!InitializeProcThreadAttributeList(attributes, 2, 0, ref size))
                throw Failure("Could not initialize worker launch attributes.");
            initialized = true;
            Marshal.WriteIntPtr(jobList, handle.DangerousGetHandle());
            var std = new[] { stdin.DangerousGetHandle(), stdout.DangerousGetHandle(), stderr.DangerousGetHandle() };
            Marshal.Copy(std, 0, inherited, std.Length);
            // Assign the job during process creation. No code can run before enrollment.
            if (!UpdateProcThreadAttribute(attributes, 0, 0x0002000d, jobList, (nuint)IntPtr.Size, IntPtr.Zero, IntPtr.Zero) ||
                !UpdateProcThreadAttribute(attributes, 0, 0x00020002, inherited, (nuint)(3 * IntPtr.Size), IntPtr.Zero, IntPtr.Zero))
                throw Failure("Could not set the worker job or its inherited handles.");
            var startup = new Startup
            {
                Info = new StartupInfo
                {
                    Size = (uint)Marshal.SizeOf<Startup>(),
                    Flags = 0x100, // STARTF_USESTDHANDLES
                    Stdin = std[0], Stdout = std[1], Stderr = std[2]
                },
                Attributes = attributes
            };
            var arguments = new StringBuilder(string.Join(' ', command.ArgumentList.Prepend(command.FileName).Select(Quote)));
            const uint flags = 0x00080000 | 0x08000000 | 0x00000400; // Extended startup, no window, Unicode environment.
            if (!CreateProcess(command.FileName, arguments, IntPtr.Zero, IntPtr.Zero, true, flags, environment,
                command.WorkingDirectory, ref startup, out var process))
                throw Failure("Could not launch the worker in its job.");
            using var thread = new SafeFileHandle(process.Thread, ownsHandle: true);
            return new SafeFileHandle(process.Process, ownsHandle: true);
        }
        finally
        {
            if (initialized)
                DeleteProcThreadAttributeList(attributes);
            Marshal.FreeHGlobal(attributes);
            Marshal.FreeHGlobal(jobList);
            Marshal.FreeHGlobal(inherited);
            Marshal.FreeHGlobal(environment);
        }
    }

    private static string Quote(string argument)
    {
        var quoted = new StringBuilder().Append('"');
        var slashes = 0;
        foreach (var character in argument)
        {
            if (character == '\\')
            {
                slashes++;
                continue;
            }
            quoted.Append('\\', character == '"' ? slashes * 2 + 1 : slashes).Append(character);
            slashes = 0;
        }
        return quoted.Append('\\', slashes * 2).Append('"').ToString();
    }

    private static async Task<int> WaitForExitAsync(SafeFileHandle process, CancellationToken token)
    {
        while (true)
        {
            token.ThrowIfCancellationRequested();
            switch (WaitForSingleObject(process, 0))
            {
                case 0:
                    return GetExitCodeProcess(process, out var code) ? unchecked((int)code)
                        : throw Failure("Could not read the worker exit code.");
                case 258: // WAIT_TIMEOUT
                    await Task.Delay(30, token);
                    break;
                default:
                    throw Failure("Could not wait for the worker process.");
            }
        }
    }

    private static async Task<string> ReadAsync(StreamReader reader, int limit, CancellationTokenSource stop)
    {
        var output = new StringBuilder();
        var buffer = new char[4096];
        while (true)
        {
            var read = await reader.ReadAsync(buffer, stop.Token);
            if (read == 0)
                return output.ToString();
            if (output.Length + read > limit)
            {
                await stop.CancelAsync();
                throw new IOException("Worker output exceeded its budget.");
            }
            output.Append(buffer, 0, read);
        }
    }

    private static IOException Failure(string message) => new(message, new Win32Exception());
    public void Dispose() => handle.Dispose();

    [StructLayout(LayoutKind.Sequential)]
    private struct BasicLimits
    {
        public long ProcessTime, JobTime;
        public uint Flags;
        public nuint MinimumWorkingSet, MaximumWorkingSet;
        public uint ActiveProcessLimit;
        public nuint Affinity;
        public uint Priority, SchedulingClass;
    }

    [StructLayout(LayoutKind.Sequential)]
    private struct ExtendedLimits
    {
        public BasicLimits Basic;
        public ulong ReadOperations, WriteOperations, OtherOperations, ReadBytes, WriteBytes, OtherBytes;
        public nuint ProcessMemory, JobMemory, PeakProcessMemory, PeakJobMemory;
    }

    [StructLayout(LayoutKind.Sequential)]
    private struct Accounting
    {
        public long UserTime, KernelTime, PeriodUserTime, PeriodKernelTime;
        public uint PageFaults, TotalProcesses, ActiveProcesses, TerminatedProcesses;
    }

    [StructLayout(LayoutKind.Sequential)]
    private struct Startup
    {
        public StartupInfo Info;
        public IntPtr Attributes;
    }

    [StructLayout(LayoutKind.Sequential)]
    private struct StartupInfo
    {
        public uint Size;
        public IntPtr Reserved, Desktop, Title;
        public uint X, Y, Width, Height, Columns, Rows, Fill, Flags;
        public ushort ShowWindow, ReservedBytes;
        public IntPtr ReservedData, Stdin, Stdout, Stderr;
    }

    [StructLayout(LayoutKind.Sequential)]
    private struct ProcessInfo
    {
        public IntPtr Process, Thread;
        public uint ProcessId, ThreadId;
    }

    [DllImport("kernel32.dll", EntryPoint = "CreateJobObjectW", ExactSpelling = true, CharSet = CharSet.Unicode, SetLastError = true)]
    private static extern SafeFileHandle CreateJobObject(IntPtr attributes, string name);
    [DllImport("kernel32.dll", EntryPoint = "OpenJobObjectW", ExactSpelling = true, CharSet = CharSet.Unicode, SetLastError = true)]
    private static extern SafeFileHandle OpenJobObject(uint access, bool inherit, string name);
    [DllImport("kernel32.dll", SetLastError = true)]
    private static extern bool SetInformationJobObject(SafeFileHandle job, int informationClass, ref ExtendedLimits limits, uint size);
    [DllImport("kernel32.dll", EntryPoint = "QueryInformationJobObject", SetLastError = true)]
    private static extern bool QueryAccounting(SafeFileHandle job, int informationClass, out Accounting accounting, uint size, IntPtr written);
    [DllImport("kernel32.dll", SetLastError = true)]
    private static extern bool TerminateJobObject(SafeFileHandle job, uint exitCode);
    [DllImport("kernel32.dll", SetLastError = true)]
    private static extern bool InitializeProcThreadAttributeList(IntPtr list, int count, uint flags, ref nuint size);
    [DllImport("kernel32.dll", SetLastError = true)]
    private static extern bool UpdateProcThreadAttribute(IntPtr list, uint flags, nuint attribute, IntPtr value,
        nuint size, IntPtr previous, IntPtr returned);
    [DllImport("kernel32.dll")]
    private static extern void DeleteProcThreadAttributeList(IntPtr list);
    [DllImport("kernel32.dll", EntryPoint = "CreateProcessW", ExactSpelling = true, CharSet = CharSet.Unicode, SetLastError = true)]
    private static extern bool CreateProcess(string executable, StringBuilder arguments, IntPtr processAttributes,
        IntPtr threadAttributes, bool inherit, uint flags, IntPtr environment, string directory, ref Startup startup, out ProcessInfo process);
    [DllImport("kernel32.dll", SetLastError = true)]
    private static extern uint WaitForSingleObject(SafeFileHandle handle, uint milliseconds);
    [DllImport("kernel32.dll", SetLastError = true)]
    private static extern bool GetExitCodeProcess(SafeFileHandle process, out uint code);
}
