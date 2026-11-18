using System.Diagnostics;
using System.Globalization;
using System.Runtime.InteropServices;

namespace SlopHawk.RustMigrationQualification;

internal static partial class Sampler
{
    private sealed class LinuxMeasurementScope : SampleScope
    {
        private const string CgroupRoot = "/sys/fs/cgroup";
        private readonly string unit;
        private readonly Observation start;
        private readonly string? slice;
        private bool stopped;

        private LinuxMeasurementScope(string unit, Observation start, string? slice)
        {
            this.unit = unit;
            this.start = start;
            this.slice = slice;
        }

        internal override int RootPid => start.MainPid;
        internal override string CgroupPath
        {
            get
            {
                var service = Path.Combine(CgroupRoot, start.Group[1..]);
                return slice is null ? service : Path.GetDirectoryName(service)!;
            }
        }
        internal override bool HasExited => !File.Exists($"/proc/{RootPid}/cgroup");
        internal override bool EnrolledBeforeExecution => true;

        internal static LinuxMeasurementScope Start(SampleSpec spec)
        {
            if (!File.Exists(Path.Combine(CgroupRoot, "cgroup.controllers")))
                throw new IOException("Linux measurement needs a live user manager and cgroup v2.");
            var identity = Guid.NewGuid().ToString("N");
            var unit = $"slophawk-measure-{identity}.service";
            var slice = spec.Scenario == "checker" ? $"slophawkmeasure{identity}.slice" : null;
            string[] placement = slice is null ? [] : ["--slice=" + slice];
            var seconds = (spec.ElapsedMilliseconds + 999) / 1000 + 15;
            Run("/usr/bin/systemd-run", ["--user", "--quiet", $"--unit={unit}",
                "--expand-environment=no", "--service-type=exec", "--remain-after-exit",
                "--property=ExitType=cgroup", "--property=KillMode=control-group",
                "--property=SendSIGKILL=yes", "--property=TimeoutStopSec=3s",
                $"--property=RuntimeMaxSec={seconds}s", $"--working-directory={spec.WorkingDirectory}",
                .. placement, "--", spec.BinaryPath, .. spec.Arguments]);
            var observed = Read(unit);
            if (!observed.Loaded || !observed.Managed || observed.MainPid < 1 || !observed.BelongsTo(unit) ||
                observed.Terminal || !File.ReadLines($"/proc/{observed.MainPid}/cgroup").Contains("0::" + observed.Group) ||
                slice is not null && Path.GetFileName(Path.GetDirectoryName(observed.Group)) != slice)
                throw new IOException($"The measurement unit {unit} did not prove enrollment before sampling.");
            return new(unit, observed, slice);
        }

        internal override bool Drain()
        {
            if (stopped) return true;
            try
            {
                var observed = Read(unit);
                if (!Current(observed)) return false;
                if (!observed.Terminal)
                    Run("/usr/bin/systemctl", ["--user", "kill", "--kill-whom=all", "--signal=SIGKILL", unit]);
                var deadline = Stopwatch.StartNew();
                while (deadline.Elapsed < TimeSpan.FromSeconds(4))
                {
                    observed = Read(unit);
                    if (!Current(observed)) return false;
                    if (observed.Terminal && Empty())
                    {
                        Run("/usr/bin/systemctl", ["--user", observed.Failed ? "reset-failed" : "stop", unit]);
                        if (slice is not null && Directory.Exists(CgroupPath))
                            Run("/usr/bin/systemctl", ["--user", "stop", slice]);
                        stopped = true;
                        return true;
                    }
                    Thread.Sleep(35);
                }
            }
            catch (IOException) { }
            return false;
        }

        public override void Dispose()
        {
            if (!stopped) _ = Drain();
        }

        private bool Current(Observation observed) => observed.Loaded && observed.Managed &&
            observed.Invocation == start.Invocation && (observed.Group == start.Group ||
                observed.Terminal && observed.Group.Length == 0);

        private bool Empty()
        {
            try
            {
                return File.ReadLines(Path.Combine(CgroupPath, "cgroup.events")).Contains("populated 0");
            }
            catch (FileNotFoundException) { return !Directory.Exists(CgroupPath); }
            catch (DirectoryNotFoundException) { return !Directory.Exists(CgroupPath); }
        }

        private static Observation Read(string unit)
        {
            var output = Run("/usr/bin/systemctl", ["--user", "show", "--all", "--no-pager",
                "--property=LoadState,ActiveState,SubState,InvocationID,ControlGroup,MainPID,ExitType,KillMode,RemainAfterExit", unit]);
            var fields = new Dictionary<string, string>(StringComparer.Ordinal);
            foreach (var line in output.Split('\n', StringSplitOptions.RemoveEmptyEntries))
            {
                var split = line.IndexOf('=');
                if (split <= 0 || !fields.TryAdd(line[..split], line[(split + 1)..].TrimEnd('\r')))
                    throw new IOException("Invalid measurement unit observation.");
            }
            string Get(string key) => fields.GetValueOrDefault(key) ?? throw new IOException($"Missing {key} in unit observation.");
            if (!int.TryParse(Get("MainPID"), NumberStyles.None, CultureInfo.InvariantCulture, out var pid) ||
                !Guid.TryParseExact(Get("InvocationID"), "N", out var invocation) || invocation == Guid.Empty)
                throw new IOException("Invalid measurement scope identity.");
            return new(Get("LoadState") == "loaded", Get("ExitType") == "cgroup" &&
                Get("KillMode") == "control-group" && Get("RemainAfterExit") == "yes",
                invocation, Get("ControlGroup"), pid, Get("ActiveState"), Get("SubState"));
        }

        private static string Run(string program, string[] arguments)
        {
            var directory = $"/run/user/{GetEffectiveUserId()}";
            var info = new ProcessStartInfo(program) { UseShellExecute = false, RedirectStandardOutput = true, RedirectStandardError = true };
            info.Environment.Remove("DBUS_SYSTEM_BUS_ADDRESS");
            info.Environment["XDG_RUNTIME_DIR"] = directory;
            info.Environment["DBUS_SESSION_BUS_ADDRESS"] = "unix:path=" + Path.Combine(directory, "bus");
            foreach (var argument in arguments) info.ArgumentList.Add(argument);
            using var process = Process.Start(info) ?? throw new IOException("Could not reach the user scope manager.");
            using var deadline = new CancellationTokenSource(TimeSpan.FromSeconds(4));
            var stdout = ReadBoundedAsync(process.StandardOutput, deadline.Token);
            var stderr = ReadBoundedAsync(process.StandardError, deadline.Token);
            try
            {
                Task.WhenAll(stdout, stderr, process.WaitForExitAsync(deadline.Token)).GetAwaiter().GetResult();
            }
            catch (OperationCanceledException error)
            {
                if (!process.HasExited) process.Kill();
                throw new IOException("User scope manager helper exceeded its deadline.", error);
            }
            if (process.ExitCode != 0) throw new IOException("User scope manager refused the operation. " + stderr.Result);
            return stdout.Result;
        }

        private static async Task<string> ReadBoundedAsync(StreamReader reader, CancellationToken token)
        {
            var buffer = new char[8193];
            var length = 0;
            while (length < buffer.Length)
            {
                var count = await reader.ReadAsync(buffer.AsMemory(length), token);
                if (count == 0) return new string(buffer, 0, length);
                length += count;
            }
            throw new IOException("User scope manager emitted oversized output.");
        }

        [DllImport("libc", EntryPoint = "geteuid")]
        private static extern uint GetEffectiveUserId();

        private sealed record Observation(bool Loaded, bool Managed, Guid Invocation, string Group, int MainPid,
            string Active, string Sub)
        {
            internal bool Terminal => (Active, Sub) is ("active", "exited") or ("inactive", "dead") or ("failed", "failed");
            internal bool Failed => Active == "failed";
            internal bool BelongsTo(string unit) => Group.StartsWith('/') && Group.EndsWith("/" + unit, StringComparison.Ordinal) &&
                Group.Length < 2048 && !Group.Contains("//", StringComparison.Ordinal) &&
                !Group.Split('/').Any(part => part is "." or "..") && !Group.Any(char.IsControl);
        }
    }
}
