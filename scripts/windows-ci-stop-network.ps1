# Removes only a successfully installed disposable CI service after proven clean shutdown.
# -LeftoverDataOnly removes just the ProgramData folder a removed service left behind.
param([Parameter(Mandatory=$true)][ValidatePattern('^[a-f0-9]{32}$')][string]$Id, [switch]$PreserveInstallation,
    [switch]$LeftoverDataOnly)
$ErrorActionPreference = 'Stop'
$name = "MiniClawNetwork-$Id"

# Once the service has run, its `leases` journal is owned by and readable only by SYSTEM. An elevated
# administrator removes it with backup and restore semantics instead of changing its ACL or owner:
# SeBackupPrivilege grants read and SeRestorePrivilege grants DELETE regardless of the ACL to a handle
# opened with FILE_FLAG_BACKUP_SEMANTICS (Microsoft Learn: Privilege Constants, CreateFileW). Every
# entry is opened with FILE_FLAG_OPEN_REPARSE_POINT and a reparse point anywhere refuses the whole
# removal before anything is deleted, so no link is ever followed.
Add-Type -TypeDefinition @'
using System;
using System.Collections.Generic;
using System.ComponentModel;
using System.Runtime.InteropServices;
using Microsoft.Win32.SafeHandles;
public static class MiniClawBackupFolder {
    [StructLayout(LayoutKind.Sequential, Pack=4)] struct Privilege { public uint Count; public long Luid; public uint Attributes; }
    [DllImport("advapi32.dll", SetLastError=true)] static extern bool OpenProcessToken(IntPtr process, uint access, out IntPtr token);
    [DllImport("advapi32.dll", SetLastError=true, CharSet=CharSet.Unicode)] static extern bool LookupPrivilegeValueW(string system, string name, out long luid);
    [DllImport("advapi32.dll", SetLastError=true)] static extern bool AdjustTokenPrivileges(IntPtr token, bool all, ref Privilege state, uint length, out Privilege previous, out uint returned);
    [DllImport("kernel32.dll")] static extern bool CloseHandle(IntPtr handle);
    [DllImport("kernel32.dll", SetLastError=true, CharSet=CharSet.Unicode)] static extern SafeFileHandle CreateFileW(string name, uint access, uint share, IntPtr security, uint disposition, uint flags, IntPtr template);
    [DllImport("kernel32.dll", SetLastError=true)] static extern bool GetFileInformationByHandleEx(SafeFileHandle file, int type, byte[] buffer, uint size);
    [DllImport("kernel32.dll", SetLastError=true)] static extern bool SetFileInformationByHandle(SafeFileHandle file, int type, ref uint info, uint size);
    const uint Synchronize = 0x100000, Delete = 0x10000, ReadAttributes = 0x80, ListDirectory = 0x1;
    const uint BackupNoFollow = 0x02200000; // FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT
    const uint Directory = 0x10, ReparsePoint = 0x400;

    // Enables both privileges for one call and restores their previous state afterwards.
    static void WithPrivileges(Action action) {
        IntPtr token;
        if (!OpenProcessToken((IntPtr)(-1), 0x28, out token)) throw new Win32Exception();
        var restore = new List<Privilege>();
        try {
            foreach (var name in new[] { "SeBackupPrivilege", "SeRestorePrivilege" }) {
                var wanted = new Privilege { Count = 1, Attributes = 2 };
                Privilege previous; uint returned;
                if (!LookupPrivilegeValueW(null, name, out wanted.Luid)) throw new Win32Exception();
                if (!AdjustTokenPrivileges(token, false, ref wanted, 16, out previous, out returned)) throw new Win32Exception();
                if (Marshal.GetLastWin32Error() != 0) throw new UnauthorizedAccessException(name + " is not held by this administrator");
                if (previous.Count > 0) restore.Add(previous);
            }
            action();
        } finally {
            foreach (var previous in restore) { var state = previous; Privilege ignored; uint returned; AdjustTokenPrivileges(token, false, ref state, 16, out ignored, out returned); }
            CloseHandle(token);
        }
    }

    // Opens one entry without following it and reports its attributes.
    static SafeFileHandle Open(string path, uint access, out uint attributes) {
        var handle = CreateFileW(path, access | Synchronize | ReadAttributes, 7, IntPtr.Zero, 3, BackupNoFollow, IntPtr.Zero);
        if (handle.IsInvalid) throw new Win32Exception(Marshal.GetLastWin32Error(), path);
        var tag = new byte[8];
        if (!GetFileInformationByHandleEx(handle, 9, tag, 8)) { handle.Dispose(); throw new Win32Exception(Marshal.GetLastWin32Error(), path); }
        attributes = BitConverter.ToUInt32(tag, 0);
        if ((attributes & ReparsePoint) != 0) { handle.Dispose(); throw new UnauthorizedAccessException("Cleanup is refusing a reparse point: " + path); }
        return handle;
    }

    // Lists a folder through FILE_FULL_DIR_INFO (GetFileInformationByHandleEx) and walks it, deepest first.
    static void Walk(string path, List<KeyValuePair<string, bool>> found) {
        uint attributes;
        using (var handle = Open(path, ListDirectory, out attributes)) {
            if ((attributes & Directory) == 0) throw new UnauthorizedAccessException("Not a folder: " + path);
            var buffer = new byte[65536];
            for (int type = 15; GetFileInformationByHandleEx(handle, type, buffer, (uint)buffer.Length); type = 14) {
                for (int offset = 0; ; ) {
                    var name = System.Text.Encoding.Unicode.GetString(buffer, offset + 68, BitConverter.ToInt32(buffer, offset + 60));
                    var entry = BitConverter.ToUInt32(buffer, offset + 56);
                    if (name != "." && name != "..") {
                        var child = path + "\\" + name;
                        if ((entry & ReparsePoint) != 0) throw new UnauthorizedAccessException("Cleanup is refusing a reparse point: " + child);
                        if ((entry & Directory) != 0) Walk(child, found); else found.Add(new KeyValuePair<string, bool>(child, false));
                    }
                    var next = BitConverter.ToInt32(buffer, offset);
                    if (next == 0) break;
                    offset += next;
                }
            }
            if (Marshal.GetLastWin32Error() != 18) throw new Win32Exception(Marshal.GetLastWin32Error(), path);
        }
        found.Add(new KeyValuePair<string, bool>(path, true));
    }

    // Counts a folder's entries, read-only.
    public static int Count(string path) {
        int count = 0;
        WithPrivileges(() => { var found = new List<KeyValuePair<string, bool>>(); Walk(path, found); count = found.Count - 1; });
        return count;
    }

    // Removes a folder tree: everything is checked first, then each entry is deleted through its own handle.
    public static void Remove(string path) {
        WithPrivileges(() => {
            var found = new List<KeyValuePair<string, bool>>();
            Walk(path, found);
            foreach (var entry in found) {
                uint attributes;
                using (var handle = Open(entry.Key, Delete, out attributes)) {
                    if (((attributes & Directory) != 0) != entry.Value) throw new UnauthorizedAccessException("Entry changed during cleanup: " + entry.Key);
                    // FileDispositionInfoEx: FILE_DISPOSITION_FLAG_DELETE | IGNORE_READONLY_ATTRIBUTE; removed when this handle closes.
                    uint flags = 0x11;
                    if (!SetFileInformationByHandle(handle, 21, ref flags, 4)) throw new Win32Exception(Marshal.GetLastWin32Error(), entry.Key);
                }
            }
        });
    }
}
'@

# Removes exactly C:\ProgramData\MiniClawNetwork-<32 hex>, never anything reached through a link.
function Remove-ServiceDataFolder {
    $base = [Environment]::GetFolderPath('CommonApplicationData')
    $folder = [IO.Path]::Combine($base, $name)
    if ([IO.Path]::GetFileName($folder) -cnotmatch '^MiniClawNetwork-[a-f0-9]{32}$' -or
        ![string]::Equals([IO.Path]::GetDirectoryName($folder), $base, [StringComparison]::OrdinalIgnoreCase)) { throw 'Not a MiniClaw network data folder' }
    if (![IO.Directory]::Exists($folder)) { return }
    foreach ($path in @($base, $folder)) {
        if ((Get-Item -LiteralPath $path -Force).Attributes -band [IO.FileAttributes]::ReparsePoint) { throw "Cleanup is refusing a reparse point: $path" }
    }
    Write-Host "Removing $folder ($([MiniClawBackupFolder]::Count($folder)) entries inside)"
    [MiniClawBackupFolder]::Remove($folder)
    if ([IO.Directory]::Exists($folder) -or [IO.File]::Exists($folder)) { throw "Removal of $folder could not be verified" }
}

if ($LeftoverDataOnly) {
    # Only a folder whose service registration and program folder are already gone.
    if (Get-CimInstance Win32_Service -Filter "Name='$name'") { throw "$name is still registered; use the normal stop first" }
    if ([IO.Directory]::Exists((Join-Path ([Environment]::GetFolderPath('ProgramFiles')) $name))) { throw "$name still has a program folder" }
    Remove-ServiceDataFolder
    Write-Host "Removed leftover data folder of unregistered $name"
    return
}
$service = Get-Service -Name $name
if ($service.Status -ne 'Stopped') {
    Stop-Service -Name $name
    $service.WaitForStatus('Stopped', [TimeSpan]::FromSeconds(45))
}
$status = Get-CimInstance Win32_Service -Filter "Name='$name'"
if ($status.ExitCode -ne 0 -or $status.ServiceSpecificExitCode -ne 0) {
    throw "Preserving $name and recovery records: clean sandbox shutdown was not proven"
}
if ($PreserveInstallation) {
    Write-Warning "Preserving stopped broker $name and its configuration for failed-test recovery"
    return
}
# Uninstall: this start parameter makes the service remove its Winsock catalog entries and stop.
$service.Start('winsock-acl-remove')
$service.WaitForStatus('Stopped', [TimeSpan]::FromSeconds(60))
$winsock = [IO.File]::ReadAllText("$env:ProgramData\$name\winsock-acl-status.json") | ConvertFrom-Json
if (!$winsock.removed -or $winsock.sid -cnotmatch '^S-1-15-3-1024(-\d+){8}$') { throw "Preserving $name`: Winsock entries not removed" }
$left = @(Get-ChildItem 'Registry::HKEY_LOCAL_MACHINE\SYSTEM\CurrentControlSet\Services\WinSock*' -Recurse | Where-Object { $_.GetAccessControl().GetSecurityDescriptorSddlForm('Access').Contains($winsock.sid) })
if ($left.Count -or [Microsoft.Win32.Registry]::LocalMachine.GetAccessControl().GetSecurityDescriptorSddlForm('Access').Contains($winsock.sid)) { throw "Preserving $name`: registry keys still name $($winsock.sid)" }
Write-Host "Winsock entries removed: $(@($winsock.stripped).Count) keys, $(@($winsock.exact).Count) as recorded"
& "$env:SystemRoot\System32\sc.exe" delete $name | Out-Null
if ($LASTEXITCODE -ne 0) { throw 'Could not remove disposable service registration' }
Remove-Item -LiteralPath (Join-Path ([Environment]::GetFolderPath('ProgramFiles')) $name) -Recurse -Force
Remove-ServiceDataFolder
