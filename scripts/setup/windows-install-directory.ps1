param([Parameter(Mandatory=$true)][string]$Path, [switch]$Create, [switch]$Kept)
# Claims a new installation folder with a protected ACL, or verifies an existing one. -Kept verifies a
# folder `miniclaw uninstall` left with the user's data, before setup installs into it again, and prints
# the current user's SID for the caller to compare with uninstall's marker.
# A backend copied from this folder becomes a LocalSystem service after UAC approval, so no
# other account may be able to write the folder, replace it, or change its permissions.
$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest
Add-Type -TypeDefinition @'
using System;
using System.ComponentModel;
using System.Runtime.InteropServices;
using Microsoft.Win32.SafeHandles;
public static class MiniClawInstallDirectory {
    [StructLayout(LayoutKind.Sequential)]
    private struct Attributes { public int Length; public IntPtr Descriptor; public int Inherit; }
    [StructLayout(LayoutKind.Sequential)]
    private struct Information { public uint Attributes; public System.Runtime.InteropServices.ComTypes.FILETIME Creation, Access, Write; public uint Volume, SizeHigh, SizeLow, Links, IndexHigh, IndexLow; }
    [DllImport("kernel32.dll", CharSet=CharSet.Unicode, SetLastError=true)]
    private static extern bool CreateDirectoryW(string path, ref Attributes attributes);
    [DllImport("kernel32.dll", CharSet=CharSet.Unicode, SetLastError=true)]
    private static extern SafeFileHandle CreateFileW(string name, uint access, uint share, IntPtr security, uint mode, uint flags, IntPtr template);
    [DllImport("kernel32.dll", SetLastError=true)]
    private static extern bool GetFileInformationByHandle(SafeFileHandle handle, out Information info);
    // Creates exactly one directory with its final ACL; an existing object is never adopted.
    public static void Create(string path, byte[] descriptor) {
        var pin = GCHandle.Alloc(descriptor, GCHandleType.Pinned);
        try {
            var attributes = new Attributes { Length=Marshal.SizeOf(typeof(Attributes)), Descriptor=pin.AddrOfPinnedObject() };
            if (!CreateDirectoryW(path, ref attributes)) throw new Win32Exception(Marshal.GetLastWin32Error());
        } finally { pin.Free(); }
    }
    // Pins one existing object without delete sharing and refuses reparse points.
    public static SafeFileHandle Open(string path, bool directory) {
        var handle = CreateFileW(path, directory ? 0x81u : 0x80u, 3, IntPtr.Zero, 3, 0x02200000, IntPtr.Zero);
        if (handle.IsInvalid) throw new Win32Exception(Marshal.GetLastWin32Error());
        try {
            Information info;
            if (!GetFileInformationByHandle(handle, out info)) throw new Win32Exception(Marshal.GetLastWin32Error());
            if ((info.Attributes & 0x400) != 0 || ((info.Attributes & 0x10) != 0) != directory) throw new Exception("Installation folder cannot traverse a reparse point or unexpected file type");
            return handle;
        } catch { handle.Dispose(); throw; }
    }
}
'@
$full = [IO.Path]::GetFullPath($Path)
if ($full -notmatch '^[A-Za-z]:\\' -or $full.Substring(2).Contains(':')) { throw 'Installation folder must use a local drive path' }
if ($full.Length -le 3) { throw 'Installation folder cannot be a drive root' }
$user = [Security.Principal.WindowsIdentity]::GetCurrent().User.Value
# The installing user, SYSTEM, Administrators and TrustedInstaller already control this computer.
$trusted = @($user, 'S-1-5-18', 'S-1-5-32-544', 'S-1-5-80-956008885-3418522649-1831038044-1853292631-2271478464')

# Returns the access rules of one pinned object after checking its owner.
function Get-CheckedRules([string]$Entry) {
    $acl = Get-Acl -LiteralPath $Entry
    if ($acl.GetOwner([Security.Principal.SecurityIdentifier]).Value -notin $trusted) {
        throw "Installation folder has an unexpected owner: $Entry"
    }
    $rules = $acl.GetAccessRules($true, $true, [Security.Principal.SecurityIdentifier])
    if ($rules.Count -eq 0) { throw "Installation folder has no access rules: $Entry" }
    return ,$rules
}

# An ancestor may let others add siblings, but never replace, rename or re-permission this path.
function Assert-Ancestor([string]$Entry) {
    foreach ($rule in (Get-CheckedRules $Entry)) {
        if ($rule.AccessControlType -ne 'Allow' -or $rule.IdentityReference.Value -in $trusted) { continue }
        # Inherit-only rules do not authorize changes to this ancestor itself.
        $inheritOnly = ($rule.PropagationFlags -band [Security.AccessControl.PropagationFlags]::InheritOnly) -ne 0
        if (!$inheritOnly -and (([int]$rule.FileSystemRights -band 0x500D0150) -ne 0)) {
            throw "Installation folder ancestor permits replacement or permission changes by another account: $Entry"
        }
    }
}

# Inside the installation another account may at most read and execute.
function Assert-Private([string]$Entry) {
    foreach ($rule in (Get-CheckedRules $Entry)) {
        if ($rule.AccessControlType -ne 'Allow' -or $rule.IdentityReference.Value -in $trusted) { continue }
        # ReadAndExecute, Synchronize, GENERIC_READ and GENERIC_EXECUTE are the only tolerated rights.
        if (([int]$rule.FileSystemRights -band (-bnot 0xA01200A9)) -ne 0) {
            throw "Installation folder grants another account write access: $Entry"
        }
    }
}

$chain = [Collections.Generic.List[string]]::new()
$directory = [IO.DirectoryInfo]::new($full).Parent
while ($null -ne $directory) { $chain.Add($directory.FullName); $directory = $directory.Parent }
$chain.Reverse()
$pins = [Collections.Generic.List[IDisposable]]::new()
try {
    foreach ($entry in $chain) {
        # Open root-to-leaf without delete sharing before validating each pinned directory.
        $pins.Add([MiniClawInstallDirectory]::Open($entry, $true))
        Assert-Ancestor $entry
    }
    if ($Create) {
        $acl = [Security.AccessControl.DirectorySecurity]::new()
        $acl.SetAccessRuleProtection($true, $false)
        $acl.SetOwner([Security.Principal.SecurityIdentifier]::new($user))
        foreach ($sid in @($user, 'S-1-5-18', 'S-1-5-32-544')) {
            $acl.AddAccessRule([Security.AccessControl.FileSystemAccessRule]::new(
                [Security.Principal.SecurityIdentifier]::new($sid), 'FullControl',
                'ContainerInherit,ObjectInherit', 'None', 'Allow'))
        }
        try { [MiniClawInstallDirectory]::Create($full, $acl.GetSecurityDescriptorBinaryForm()) }
        catch {
            if ([IO.Directory]::Exists($full) -or [IO.File]::Exists($full)) { throw 'Installation folder already exists' }
            throw
        }
        $pins.Add([MiniClawInstallDirectory]::Open($full, $true))
        if (!(Get-Acl -LiteralPath $full).AreAccessRulesProtected) { throw 'Installation folder did not keep its protected permissions' }
        Assert-Private $full
    } elseif ($Kept) {
        # Exactly what uninstall leaves: this user's protected, private folder holding only `data`, with
        # uninstall's marker in it. Nothing is changed; anything else is refused and never adopted.
        $pins.Add([MiniClawInstallDirectory]::Open($full, $true))
        $acl = Get-Acl -LiteralPath $full
        if (!$acl.AreAccessRulesProtected) { throw 'Kept folder lost its protected permissions' }
        if ($acl.GetOwner([Security.Principal.SecurityIdentifier]).Value -ne $user) { throw 'Kept folder is not owned by this user' }
        Assert-Private $full
        $entries = @([IO.Directory]::GetFileSystemEntries($full))
        if ($entries.Count -ne 1 -or [IO.Path]::GetFileName($entries[0]) -ne 'data') { throw 'Kept folder holds more than its data folder' }
        $data = Join-Path $full 'data'
        $pins.Add([MiniClawInstallDirectory]::Open($data, $true))
        Assert-Private $data
        $marker = Join-Path $data 'kept-after-uninstall.json'
        $pins.Add([MiniClawInstallDirectory]::Open($marker, $false))
        Assert-Private $marker
        [Console]::Out.Write($user)
    } else {
        # The folder, its runtime and the backend that becomes a service must all be private.
        $pins.Add([MiniClawInstallDirectory]::Open($full, $true))
        Assert-Private $full
        foreach ($name in @('runtime', 'runtime\bin')) {
            $child = Join-Path $full $name
            $pins.Add([MiniClawInstallDirectory]::Open($child, $true))
            Assert-Private $child
        }
        $backend = Join-Path $full 'runtime\bin\miniclaw-server.exe'
        if ([IO.File]::Exists($backend) -or [IO.Directory]::Exists($backend)) {
            $pins.Add([MiniClawInstallDirectory]::Open($backend, $false))
            Assert-Private $backend
        }
    }
} catch {
    [Console]::Error.WriteLine($_.Exception.Message)
    exit 1
} finally {
    for ($i = $pins.Count - 1; $i -ge 0; $i--) { $pins[$i].Dispose() }
}
