using System;
using System.IO;
using System.Runtime.InteropServices;
using System.Text;
using Microsoft.Win32.SafeHandles;

// Bootstrap file identity; see tools/qa/native_observer/AGENTS.md.
public static class QaNativeFileIdentity
{
    [StructLayout(LayoutKind.Sequential)]
    struct Information
    {
        public uint attributes;
        public System.Runtime.InteropServices.ComTypes.FILETIME created, accessed, written;
        public uint volume, sizeHigh, sizeLow, links, indexHigh, indexLow;
    }

    [DllImport("kernel32.dll", SetLastError = true)]
    static extern bool GetFileInformationByHandle(SafeFileHandle file, out Information information);
    [DllImport("kernel32.dll", CharSet = CharSet.Unicode, SetLastError = true)]
    static extern uint GetFinalPathNameByHandleW(SafeFileHandle file, StringBuilder path, uint count, uint flags);

    public static string Read(FileStream file, string expectedPath)
    {
        Information info;
        if (!GetFileInformationByHandle(file.SafeFileHandle, out info) || (info.attributes & 0x410) != 0 || info.links != 1)
            throw new InvalidOperationException("native_file_identity_refused");
        var path = new StringBuilder(32768);
        uint count = GetFinalPathNameByHandleW(file.SafeFileHandle, path, 32768, 0);
        string actual = path.ToString();
        if (actual.StartsWith(@"\\?\", StringComparison.Ordinal)) actual = actual.Substring(4);
        if (count == 0 || count >= 32768 || !String.Equals(actual, expectedPath, StringComparison.OrdinalIgnoreCase))
            throw new InvalidOperationException("native_file_path_changed");
        return info.volume.ToString("x8") + info.indexHigh.ToString("x8") + info.indexLow.ToString("x8");
    }
}
