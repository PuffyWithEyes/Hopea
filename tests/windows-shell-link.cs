// SPDX-FileCopyrightText: 2026 Daniil Klimov
// SPDX-License-Identifier: GPL-3.0-or-later
using System;
using System.Runtime.InteropServices;
using System.Runtime.InteropServices.ComTypes;
using System.Text;

public static class WindowsShellLink
{
    // Use the Unicode IID explicitly. WScript.Shell can return ANSI paths with
    // question marks for characters outside the runner's active code page.
    [ComImport, Guid("00021401-0000-0000-C000-000000000046")]
    private class ShellLinkObject { }

    [ComImport, Guid("000214F9-0000-0000-C000-000000000046"),
     InterfaceType(ComInterfaceType.InterfaceIsIUnknown)]
    private interface IShellLinkW
    {
        void GetPath([Out, MarshalAs(UnmanagedType.LPWStr)] StringBuilder path, int count, IntPtr data, uint flags);
        void GetIDList(out IntPtr list);
        void SetIDList(IntPtr list);
        void GetDescription([Out, MarshalAs(UnmanagedType.LPWStr)] StringBuilder text, int count);
        void SetDescription([MarshalAs(UnmanagedType.LPWStr)] string text);
        void GetWorkingDirectory([Out, MarshalAs(UnmanagedType.LPWStr)] StringBuilder path, int count);
        void SetWorkingDirectory([MarshalAs(UnmanagedType.LPWStr)] string path);
        void GetArguments([Out, MarshalAs(UnmanagedType.LPWStr)] StringBuilder arguments, int count);
        void SetArguments([MarshalAs(UnmanagedType.LPWStr)] string arguments);
        void GetHotkey(out ushort hotkey);
        void SetHotkey(ushort hotkey);
        void GetShowCmd(out int command);
        void SetShowCmd(int command);
        void GetIconLocation([Out, MarshalAs(UnmanagedType.LPWStr)] StringBuilder path, int count, out int index);
        void SetIconLocation([MarshalAs(UnmanagedType.LPWStr)] string path, int index);
        void SetRelativePath([MarshalAs(UnmanagedType.LPWStr)] string path, uint reserved);
        void Resolve(IntPtr window, uint flags);
        void SetPath([MarshalAs(UnmanagedType.LPWStr)] string path);
    }

    public sealed class Details
    {
        public string TargetPath { get; internal set; }
        public string WorkingDirectory { get; internal set; }
        public string Arguments { get; internal set; }
        public string IconPath { get; internal set; }
        public int IconIndex { get; internal set; }
    }

    public static Details Read(string filename)
    {
        object instance = new ShellLinkObject();
        try
        {
            ((IPersistFile)instance).Load(filename, 0); // STGM_READ; never rewrite or resolve the link.
            var link = (IShellLinkW)instance;
            var target = new StringBuilder(32768);
            var directory = new StringBuilder(32768);
            var arguments = new StringBuilder(32768);
            var icon = new StringBuilder(32768);
            link.GetPath(target, target.Capacity, IntPtr.Zero, 0);
            link.GetWorkingDirectory(directory, directory.Capacity);
            link.GetArguments(arguments, arguments.Capacity);
            link.GetIconLocation(icon, icon.Capacity, out int iconIndex);
            return new Details {
                TargetPath = target.ToString(), WorkingDirectory = directory.ToString(),
                Arguments = arguments.ToString(), IconPath = icon.ToString(), IconIndex = iconIndex
            };
        }
        finally { Marshal.FinalReleaseComObject(instance); }
    }

    // Used only by the regression test to create temporary fixture links.
    public static void CreateFixture(string filename, string target, string directory, string icon, int iconIndex)
    {
        object instance = new ShellLinkObject();
        try
        {
            var link = (IShellLinkW)instance;
            link.SetPath(target);
            link.SetWorkingDirectory(directory);
            link.SetIconLocation(icon, iconIndex);
            ((IPersistFile)instance).Save(filename, true);
        }
        finally { Marshal.FinalReleaseComObject(instance); }
    }
}
