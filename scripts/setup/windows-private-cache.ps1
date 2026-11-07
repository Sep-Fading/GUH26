param([Parameter(Mandatory=$true)][string]$CachePath, [switch]$Hold)
$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest
Add-Type -TypeDefinition @'
using System;
using System.ComponentModel;
using System.Runtime.InteropServices;
using Microsoft.Win32.SafeHandles;
public static class MiniClawCachePin {
    [StructLayout(LayoutKind.Sequential)]
    private struct Information { public uint Attributes; public System.Runtime.InteropServices.ComTypes.FILETIME Creation, Access, Write; public uint Volume, SizeHigh, SizeLow, Links, IndexHigh, IndexLow; }
    [DllImport("kernel32.dll", CharSet=CharSet.Unicode, SetLastError=true)]
    private static extern SafeFileHandle CreateFileW(string name, uint access, uint share, IntPtr security, uint mode, uint flags, IntPtr template);
    [DllImport("kernel32.dll", SetLastError=true)]
    private static extern bool GetFileInformationByHandle(SafeFileHandle handle, out Information info);
    [DllImport("kernel32.dll", SetLastError=true)]
    private static extern bool GetFileInformationByHandleEx(SafeFileHandle handle, int kind, out uint flags, uint size);
    public static SafeFileHandle Open(string path) {
        // FILE_LIST_DIRECTORY engages share-access checks; attributes-only handles do not pin names.
        var handle = CreateFileW(path, 0x81, 3, IntPtr.Zero, 3, 0x02200000, IntPtr.Zero);
        if (handle.IsInvalid) throw new Win32Exception(Marshal.GetLastWin32Error());
        try {
            Information info;
            if (!GetFileInformationByHandle(handle, out info)) throw new Win32Exception(Marshal.GetLastWin32Error());
            if ((info.Attributes & 0x400) != 0 || (info.Attributes & 0x10) == 0) throw new Exception("Download cache cannot traverse a reparse point or file");
            uint flags;
            if (!GetFileInformationByHandleEx(handle, 23, out flags, 4)) throw new Win32Exception(Marshal.GetLastWin32Error());
            if (flags != 0) throw new Exception("Download cache cannot traverse case-sensitive directories");
            return handle;
        } catch { handle.Dispose(); throw; }
    }
}
'@
$path = [IO.Path]::GetFullPath($CachePath)
if ($path -notmatch '^[A-Za-z]:\\' -or $path.Substring(2).Contains(':')) { throw 'Download cache must use a local drive path' }
$user = [Security.Principal.WindowsIdentity]::GetCurrent().User.Value
$allowed = @($user, 'S-1-5-18', 'S-1-5-32-544', 'S-1-5-80-956008885-3418522649-1831038044-1853292631-2271478464')
$chain = [Collections.Generic.List[string]]::new()
$directory = [IO.DirectoryInfo]::new($path)
while ($null -ne $directory) { $chain.Add($directory.FullName); $directory = $directory.Parent }
$chain.Reverse()
$pins = [Collections.Generic.List[IDisposable]]::new()
try {
    foreach ($entry in $chain) {
        # Open root-to-leaf without delete sharing before validating each pinned directory.
        $pins.Add([MiniClawCachePin]::Open($entry))
        $acl = Get-Acl -LiteralPath $entry
        if ($acl.GetOwner([Security.Principal.SecurityIdentifier]).Value -notin $allowed) {
            throw 'Download cache ancestor has an unexpected owner'
        }
        $rules = $acl.GetAccessRules($true, $true, [Security.Principal.SecurityIdentifier])
        if ($rules.Count -eq 0) { throw 'Download cache has no private access rules' }
        foreach ($rule in $rules) {
            if ($rule.AccessControlType -ne 'Allow' -or $rule.IdentityReference.Value -in $allowed) { continue }
            # Inherit-only rules do not authorize changes to this ancestor itself.
            $inheritOnly = ($rule.PropagationFlags -band [Security.AccessControl.PropagationFlags]::InheritOnly) -ne 0
            if ($entry -eq $path) { throw 'Download cache grants another account access' }
            if (!$inheritOnly -and (([int]$rule.FileSystemRights -band 0x500D0150) -ne 0)) {
                throw 'Download cache ancestor permits replacement or permission changes by another account'
            }
        }
    }
    if ($Hold) { [Console]::WriteLine('ready'); [Console]::Out.Flush(); [void][Console]::ReadLine() }
} finally {
    for ($i = $pins.Count - 1; $i -ge 0; $i--) { $pins[$i].Dispose() }
}
