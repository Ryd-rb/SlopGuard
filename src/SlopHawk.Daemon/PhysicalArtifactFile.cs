using System.Runtime.InteropServices;
using Microsoft.Win32.SafeHandles;

namespace SlopHawk.Daemon;

internal static class PhysicalArtifactFile
{
    private const uint StatxBasic = 0x7ff;
    private const int StatxNoFollow = 0x100;
    private const int StatxEmptyPath = 0x1000;
    private const int AtCwd = -100;

    internal readonly record struct FileIdentity(ulong Device, ulong Inode, uint Links);

    internal static bool TryPathIdentity(string path, bool expectDirectory, out FileIdentity identity)
    {
        identity = default;
        if (OperatingSystem.IsLinux())
        {
            if (Statx(AtCwd, path, StatxNoFollow, StatxBasic, out var stat) != 0 ||
                !ValidLinuxStat(stat) || (stat.Mode & 0xF000) != (expectDirectory ? 0x4000 : 0x8000) ||
                (!expectDirectory && stat.Links != 1))
                return false;
            identity = LinuxIdentity(stat);
            return true;
        }
        if (OperatingSystem.IsWindows())
        {
            if ((File.GetAttributes(path) & FileAttributes.ReparsePoint) != 0)
                return false;
            using var handle = OpenNoFollow(path, directory: expectDirectory);
            return TryHandleIdentity(handle, expectDirectory, out identity);
        }
        return false; // No unverified platform fallback.
    }

    // On Linux, walk each directory with openat/O_NOFOLLOW. A swapped symlink in
    // any parent cannot redirect the read into an unleased directory. The path
    // identities are also rechecked after the bytes have been captured.
    internal static SafeFileHandle OpenRead(string path)
    {
        if (!OperatingSystem.IsLinux())
            return OpenNoFollow(path, directory: false);
        const int oReadOnly = 0, oCloseOnExec = 0x80000;
        const int oNoFollow = 0x20000, oDirectory = 0x10000, oNonBlock = 0x800;
        var segments = path.Split('/', StringSplitOptions.RemoveEmptyEntries);
        using var root = OpenNoFollow("/", directory: true);
        var current = root;
        SafeFileHandle? nested = null;
        try
        {
            for (var index = 0; index < segments.Length - 1; index++)
            {
                var fd = LinuxOpenAt(current.DangerousGetHandle().ToInt32(), segments[index],
                    oReadOnly | oCloseOnExec | oNoFollow | oDirectory | oNonBlock);
                if (fd < 0)
                    throw new IOException("Artifact directory changed while opening artifact.");
                var next = new SafeFileHandle((nint)fd, ownsHandle: true);
                nested?.Dispose();
                nested = next;
                current = next;
            }
            var leaf = LinuxOpenAt(current.DangerousGetHandle().ToInt32(), segments[^1],
                oReadOnly | oCloseOnExec | oNoFollow | oNonBlock);
            if (leaf < 0)
                throw new IOException("Artifact changed while opening.");
            return new SafeFileHandle((nint)leaf, ownsHandle: true);
        }
        finally
        {
            nested?.Dispose();
        }
    }

    [DllImport("libc", EntryPoint = "openat", SetLastError = true)]
    private static extern int LinuxOpenAt(int directoryFileDescriptor, string name, int flags);

    // Use native no-follow opens: checking a path and then calling File.OpenHandle
    // would otherwise permit a symlink-swap race before the handle was acquired.
    private static SafeFileHandle OpenNoFollow(string path, bool directory)
    {
        if (OperatingSystem.IsLinux())
        {
            const int oReadOnly = 0, oCloseOnExec = 0x80000, oNoFollow = 0x20000, oNonBlock = 0x800;
            const int oDirectory = 0x10000;
            var fd = LinuxOpen(path, oReadOnly | oCloseOnExec | oNoFollow | oNonBlock |
                (directory ? oDirectory : 0));
            if (fd < 0)
                throw new IOException("Artifact cannot be opened without following links.");
            return new SafeFileHandle((nint)fd, ownsHandle: true);
        }
        if (OperatingSystem.IsWindows())
        {
            const uint fileReadAttributes = 0x80, shareReadWriteDelete = 7;
            const uint openExisting = 3, backupSemantics = 0x02000000;
            const uint openReparsePoint = 0x00200000;
            var handle = CreateFile(path, fileReadAttributes | (directory ? 0u : 0x80000000u),
                shareReadWriteDelete, nint.Zero, openExisting,
                backupSemantics | openReparsePoint, nint.Zero);
            if (handle.IsInvalid)
            {
                handle.Dispose();
                throw new IOException("Artifact cannot be opened without following links.");
            }
            return handle;
        }
        throw new PlatformNotSupportedException("Physical artifact identity is unavailable.");
    }

    [DllImport("libc", EntryPoint = "open", SetLastError = true)]
    private static extern int LinuxOpen(string path, int flags);

    [DllImport("kernel32.dll", EntryPoint = "CreateFileW", CharSet = CharSet.Unicode, SetLastError = true)]
    private static extern SafeFileHandle CreateFile(string path, uint access, uint sharing,
        nint security, uint disposition, uint flags, nint template);

    internal static bool TryHandleIdentity(SafeFileHandle handle, bool expectDirectory,
        out FileIdentity identity)
    {
        identity = default;
        if (OperatingSystem.IsLinux())
        {
            if (Statx(handle.DangerousGetHandle().ToInt32(), "", StatxEmptyPath,
                StatxBasic, out var stat) != 0 || !ValidLinuxStat(stat) ||
                (stat.Mode & 0xF000) != (expectDirectory ? 0x4000 : 0x8000) ||
                (!expectDirectory && stat.Links != 1))
                return false;
            identity = LinuxIdentity(stat);
            return true;
        }
        if (OperatingSystem.IsWindows() && GetFileInformationByHandle(handle, out var info) &&
            ((info.Attributes & (uint)FileAttributes.Directory) != 0) == expectDirectory &&
            (info.Attributes & (uint)FileAttributes.ReparsePoint) == 0 &&
            (expectDirectory || info.Links == 1))
        {
            identity = new FileIdentity(info.VolumeSerial,
                ((ulong)info.FileIndexHigh << 32) | info.FileIndexLow, info.Links);
            return true;
        }
        return false;
    }

    // Check that statx actually supplied type, mode, link count and inode.
    // Filesystems returning an incomplete identity cannot mint a receipt.
    private static bool ValidLinuxStat(LinuxStatx stat) =>
        (stat.Mask & 0x107) == 0x107 && stat.Inode != 0 && stat.Links != 0;

    private static FileIdentity LinuxIdentity(LinuxStatx stat) =>
        new(((ulong)stat.DeviceMajor << 32) | stat.DeviceMinor, stat.Inode, stat.Links);

    [StructLayout(LayoutKind.Explicit, Size = 256)]
    private struct LinuxStatx
    {
        [FieldOffset(0)] public uint Mask;
        [FieldOffset(16)] public uint Links;
        [FieldOffset(28)] public ushort Mode;
        [FieldOffset(32)] public ulong Inode;
        [FieldOffset(136)] public uint DeviceMajor;
        [FieldOffset(140)] public uint DeviceMinor;
    }

    [DllImport("libc", EntryPoint = "statx", SetLastError = true)]
    private static extern int Statx(int directoryFileDescriptor, string path, int flags,
        uint mask, out LinuxStatx data);

    [StructLayout(LayoutKind.Sequential)]
    private struct WindowsFileInfo
    {
        public uint Attributes;
        public uint CreationLow;
        public uint CreationHigh;
        public uint AccessLow;
        public uint AccessHigh;
        public uint WriteLow;
        public uint WriteHigh;
        public uint VolumeSerial;
        public uint SizeHigh;
        public uint SizeLow;
        public uint Links;
        public uint FileIndexHigh;
        public uint FileIndexLow;
    }

    [DllImport("kernel32.dll", SetLastError = true)]
    [return: MarshalAs(UnmanagedType.Bool)]
    private static extern bool GetFileInformationByHandle(SafeFileHandle handle,
        out WindowsFileInfo info);

}
