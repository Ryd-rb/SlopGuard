using System.ComponentModel;
using System.Runtime.InteropServices;
using Microsoft.Win32.SafeHandles;

namespace SlopHawk.Daemon;

internal static class RootIdentity
{
    internal static bool IsPrivateLinuxFile(SafeFileHandle file)
    {
        const int emptyPath = 0x1000;
        const uint ownerAndMode = 0xb;
        return Statx(file.DangerousGetHandle().ToInt32(), "", emptyPath, ownerAndMode, out var info) == 0 &&
            (info.Mask & ownerAndMode) == ownerAndMode && (info.Mode & 0xf03f) == 0x8000 && info.User == EffectiveUid();
    }

    internal static string Read(string root)
    {
        Workspace.CheckDirectory(root);
        if (OperatingSystem.IsWindows())
        {
            const uint openExisting = 3;
            const uint openDirectoryWithoutFollowingLinks = 0x02200000;
            const int fileIdInfo = 18;
            using var handle = CreateFile(root, 0, FileShare.ReadWrite | FileShare.Delete, IntPtr.Zero,
                openExisting, openDirectoryWithoutFollowingLinks, IntPtr.Zero);
            if (handle.IsInvalid || !GetFileInformationByHandleEx(handle, fileIdInfo, out var id, 24))
                throw new IOException("Could not read the workspace directory identity.", new Win32Exception());
            return $"windows:{id.Volume:x16}:{id.FileIdLow:x16}{id.FileIdHigh:x16}";
        }
        if (OperatingSystem.IsLinux())
        {
            const int currentDirectory = -100;
            const int noFollow = 0x100;
            const uint type = 0x1;
            const uint inode = 0x100;
            const uint birthTime = 0x800;
            const ushort fileType = 0xf000;
            const ushort directory = 0x4000;
            if (Statx(currentDirectory, root, noFollow, type | inode | birthTime, out var info) != 0 ||
                (info.Mask & (type | inode)) != (type | inode) || (info.Mode & fileType) != directory)
                throw new IOException("Could not read the workspace directory identity.");
            return $"linux:{info.DeviceMajor}:{info.DeviceMinor}:{info.Inode}:{info.BirthSeconds}:{info.BirthNanoseconds}";
        }
        // The other platforms supported by .NET expose directory birth time through FileSystemInfo.
        return $"birth:{Directory.GetCreationTimeUtc(root).Ticks}";
    }

    [StructLayout(LayoutKind.Sequential)]
    private struct WindowsFileId
    {
        public ulong Volume;
        public ulong FileIdLow;
        public ulong FileIdHigh;
    }

    // Fixed Linux statx ABI. Omitted members remain reserved in the output buffer.
    [StructLayout(LayoutKind.Explicit, Size = 256)]
    private struct LinuxStatx
    {
        [FieldOffset(0)] public uint Mask;
        [FieldOffset(20)] public uint User;
        [FieldOffset(28)] public ushort Mode;
        [FieldOffset(32)] public ulong Inode;
        [FieldOffset(80)] public long BirthSeconds;
        [FieldOffset(88)] public uint BirthNanoseconds;
        [FieldOffset(136)] public uint DeviceMajor;
        [FieldOffset(140)] public uint DeviceMinor;
    }

    [DllImport("kernel32.dll", EntryPoint = "CreateFileW", ExactSpelling = true, CharSet = CharSet.Unicode, SetLastError = true)]
    private static extern SafeFileHandle CreateFile(string path, uint access, FileShare share,
        IntPtr security, uint disposition, uint flags, IntPtr template);

    [DllImport("kernel32.dll", SetLastError = true)]
    [return: MarshalAs(UnmanagedType.Bool)]
    private static extern bool GetFileInformationByHandleEx(SafeFileHandle handle, int informationClass,
        out WindowsFileId id, uint size);

    [DllImport("libc", EntryPoint = "statx", SetLastError = true)]
    private static extern int Statx(int directory, [MarshalAs(UnmanagedType.LPUTF8Str)] string path,
        int flags, uint mask, out LinuxStatx info);

    [DllImport("libc", EntryPoint = "geteuid")]
    private static extern uint EffectiveUid();
}
