param(
    [Parameter(Mandatory=$true)][ValidatePattern('^[a-f0-9]{32}$')][string]$Id,
    [Parameter(Mandatory=$true)][ValidatePattern('^S-1-(?:5-21|12-1)(?:-(?:0|[1-9]\d*)){4}$')][string]$OwnerSid
)
# Uninstall: removes one installation's network service. The stopped service is started once with
# `winsock-acl-remove` so it deletes its own Winsock catalog entries; the registry is then checked for
# its capability SID, and only after that are the service, its program folder and its data folder removed.
$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest
if ([Security.Principal.SecurityIdentifier]::new($OwnerSid).Value -cne $OwnerSid) { throw 'Owner SID must be canonical' }
$principal = [Security.Principal.WindowsPrincipal]::new([Security.Principal.WindowsIdentity]::GetCurrent())
if (!$principal.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)) { throw 'Removing network isolation requires administrator approval' }
$name = "MiniClawNetwork-$Id"
$base = [Environment]::GetFolderPath('CommonApplicationData')
$folder = Join-Path ([Environment]::GetFolderPath('ProgramFiles')) $name
$configFolder = Join-Path $base $name
foreach ($path in @($folder, $configFolder)) {
    if ((Test-Path -LiteralPath $path) -and ((Get-Item -LiteralPath $path -Force).Attributes -band [IO.FileAttributes]::ReparsePoint)) { throw "Refusing a reparse point: $path" }
}

# Removes a folder only SYSTEM may list (the service's `leases` journal) without changing its ACL or owner:
# SeBackupPrivilege and SeRestorePrivilege grant read and DELETE to a handle opened with
# FILE_FLAG_BACKUP_SEMANTICS (Microsoft Learn: Privilege Constants, CreateFileW). Every entry is opened
# with FILE_FLAG_OPEN_REPARSE_POINT; a reparse point anywhere stops the removal before anything is deleted.
Add-Type -TypeDefinition @'
using System;using System.Collections.Generic;using System.Runtime.InteropServices;using Microsoft.Win32.SafeHandles;
public static class MiniClawBackupRemove{
[StructLayout(LayoutKind.Sequential,Pack=4)]struct P{public uint C;public long L;public uint A;}
[DllImport("advapi32.dll",SetLastError=true)]static extern bool OpenProcessToken(IntPtr p,uint a,out IntPtr t);
[DllImport("advapi32.dll",SetLastError=true,CharSet=CharSet.Unicode)]static extern bool LookupPrivilegeValueW(string s,string n,out long l);
[DllImport("advapi32.dll",SetLastError=true)]static extern bool AdjustTokenPrivileges(IntPtr t,bool d,ref P n,uint b,out P o,out uint r);
[DllImport("kernel32.dll")]static extern bool CloseHandle(IntPtr h);
[DllImport("kernel32.dll",SetLastError=true,CharSet=CharSet.Unicode)]static extern SafeFileHandle CreateFileW(string n,uint a,uint s,IntPtr x,uint c,uint f,IntPtr t);
[DllImport("kernel32.dll",SetLastError=true)]static extern bool GetFileInformationByHandleEx(SafeFileHandle h,int c,byte[] b,uint s);
[DllImport("kernel32.dll",SetLastError=true)]static extern bool SetFileInformationByHandle(SafeFileHandle h,int c,ref uint i,uint s);
static Exception E(string p){return new System.ComponentModel.Win32Exception(Marshal.GetLastWin32Error(),p);}
static SafeFileHandle Open(string p,uint a,out bool dir){
var h=CreateFileW(p,a|0x100080,7,IntPtr.Zero,3,0x02200000,IntPtr.Zero);if(h.IsInvalid)throw E(p);
var b=new byte[8];if(!GetFileInformationByHandleEx(h,9,b,8)){h.Dispose();throw E(p);}
uint x=BitConverter.ToUInt32(b,0);if((x&0x400)!=0){h.Dispose();throw new UnauthorizedAccessException("Refusing a reparse point: "+p);}
dir=(x&0x10)!=0;return h;}
static void Walk(string p,List<KeyValuePair<string,bool>> f){bool dir;
using(var h=Open(p,1,out dir)){if(!dir)throw new UnauthorizedAccessException("Not a folder: "+p);
var b=new byte[65536];for(int c=15;GetFileInformationByHandleEx(h,c,b,65536);c=14)for(int i=0;;){
string m=System.Text.Encoding.Unicode.GetString(b,i+68,BitConverter.ToInt32(b,i+60));uint a=BitConverter.ToUInt32(b,i+56);
if(m!="."&&m!=".."){string q=p+"\\"+m;if((a&0x400)!=0)throw new UnauthorizedAccessException("Refusing a reparse point: "+q);
if((a&0x10)!=0)Walk(q,f);else f.Add(new KeyValuePair<string,bool>(q,false));}
int n=BitConverter.ToInt32(b,i);if(n==0)break;i+=n;}
if(Marshal.GetLastWin32Error()!=18)throw E(p);}
f.Add(new KeyValuePair<string,bool>(p,true));}
public static int Remove(string path){IntPtr t;if(!OpenProcessToken((IntPtr)(-1),0x28,out t))throw E("token");
var undo=new List<P>();try{foreach(var n in new[]{"SeBackupPrivilege","SeRestorePrivilege"}){
P w=new P{C=1,A=2},o;uint r;if(!LookupPrivilegeValueW(null,n,out w.L))throw E(n);
if(!AdjustTokenPrivileges(t,false,ref w,16,out o,out r))throw E(n);
if(Marshal.GetLastWin32Error()!=0)throw new UnauthorizedAccessException(n+" is not held");if(o.C>0)undo.Add(o);}
var f=new List<KeyValuePair<string,bool>>();Walk(path,f);
foreach(var e in f){bool dir;using(var h=Open(e.Key,0x10000,out dir)){
if(dir!=e.Value)throw new UnauthorizedAccessException("Entry changed during removal: "+e.Key);
uint flags=0x11;if(!SetFileInformationByHandle(h,21,ref flags,4))throw E(e.Key);}}
return f.Count-1;}
finally{foreach(var o in undo){var s=o;P x;uint r;AdjustTokenPrivileges(t,false,ref s,16,out x,out r);}CloseHandle(t);}}}
'@

$service = Get-CimInstance Win32_Service -Filter "Name='$name'"
if ($service) {
    $binary = Join-Path $folder 'miniclaw-network-service.exe'
    $configPath = Join-Path $configFolder 'service.json'
    $config = [IO.File]::ReadAllText($configPath) | ConvertFrom-Json
    if ($service.PathName -cne ('"' + $binary + '" --network-service "' + $configPath + '"') -or $service.StartName -ne 'LocalSystem' -or
        $config.owner_sid -cne $OwnerSid -or $config.service_name -cne $name) { throw "$name does not belong to this installation" }
    $control = Get-Service -Name $name
    if ($control.Status -ne 'Stopped') {
        Stop-Service -Name $name
        $control.WaitForStatus('Stopped', [TimeSpan]::FromSeconds(45))
    }
    $state = Get-CimInstance Win32_Service -Filter "Name='$name'"
    # A clean stop reports NO_ERROR; ERROR_SERVICE_NEVER_STARTED (1077) is clean only before any journal exists.
    $leases = Join-Path $configFolder 'leases'
    if (!(($state.ExitCode -eq 0 -or ($state.ExitCode -eq 1077 -and ![IO.Directory]::Exists($leases))) -and $state.ServiceSpecificExitCode -eq 0)) {
        throw "$name did not confirm sandbox cleanup (exit $($state.ExitCode)/$($state.ServiceSpecificExitCode)); nothing was removed. Restart Windows, then run miniclaw uninstall again."
    }
    $control.Start('winsock-acl-remove')
    $control.WaitForStatus('Stopped', [TimeSpan]::FromSeconds(60))
    $winsock = [IO.File]::ReadAllText((Join-Path $configFolder 'winsock-acl-status.json')) | ConvertFrom-Json
    if (!$winsock.removed -or $winsock.sid -cnotmatch '^S-1-15-3-1024(-\d+){8}$') { throw "$name could not remove its Winsock entries; the service is kept" }
    $left = @(Get-ChildItem 'Registry::HKEY_LOCAL_MACHINE\SYSTEM\CurrentControlSet\Services\WinSock*' -Recurse | Where-Object { $_.GetAccessControl().GetSecurityDescriptorSddlForm('Access').Contains($winsock.sid) })
    if ($left.Count -or [Microsoft.Win32.Registry]::LocalMachine.GetAccessControl().GetSecurityDescriptorSddlForm('Access').Contains($winsock.sid)) { throw "Registry keys still name $($winsock.sid); the service is kept" }
    Write-Output "Winsock entries removed: $(@($winsock.stripped).Count) keys ($($winsock.sid))"
    & "$env:SystemRoot\System32\sc.exe" delete $name | Out-Null
    if ($LASTEXITCODE -ne 0) { throw "Could not delete service $name" }
    $deadline = [DateTime]::UtcNow.AddSeconds(30)
    while ((Get-CimInstance Win32_Service -Filter "Name='$name'") -and [DateTime]::UtcNow -lt $deadline) { Start-Sleep -Milliseconds 250 }
    if (Get-CimInstance Win32_Service -Filter "Name='$name'") { throw "$name is still registered" }
    Write-Output "Service $name removed"
}
if ([IO.Directory]::Exists($folder)) { Remove-Item -LiteralPath $folder -Recurse -Force; Write-Output "Removed $folder" }
if ([IO.Directory]::Exists($configFolder)) {
    if ([IO.Path]::GetFileName($configFolder) -cnotmatch '^MiniClawNetwork-[a-f0-9]{32}$' -or ![string]::Equals([IO.Path]::GetDirectoryName($configFolder), $base, [StringComparison]::OrdinalIgnoreCase)) { throw 'Not a MiniClaw network data folder' }
    $count = [MiniClawBackupRemove]::Remove($configFolder)
    Write-Output "Removed $configFolder ($count entries)"
}
if ([IO.Directory]::Exists($folder) -or [IO.Directory]::Exists($configFolder)) { throw "Removal of $name could not be verified" }
