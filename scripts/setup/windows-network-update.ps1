param(
    [Parameter(Mandatory=$true)][string]$Source,
    [Parameter(Mandatory=$true)][ValidatePattern('^[a-f0-9]{64}$')][string]$ExpectedSha256,
    [Parameter(Mandatory=$true)][ValidatePattern('^S-1-(?:5-21|12-1)(?:-(?:0|[1-9]\d*)){4}$')][string]$OwnerSid,
    [Parameter(Mandatory=$true)][ValidatePattern('^[a-f0-9]{32}$')][string]$Id
)
$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest
if ([Security.Principal.SecurityIdentifier]::new($OwnerSid).Value -cne $OwnerSid) { throw 'Owner SID must be canonical' }
$principal = [Security.Principal.WindowsPrincipal]::new([Security.Principal.WindowsIdentity]::GetCurrent())
if (!$principal.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)) { throw 'Updating network isolation requires administrator approval' }
$name = "MiniClawNetwork-$Id"
$folder = Join-Path ([Environment]::GetFolderPath('ProgramFiles')) $name
$configFolder = Join-Path ([Environment]::GetFolderPath('CommonApplicationData')) $name
$binary = Join-Path $folder 'miniclaw-network-service.exe'
$configPath = Join-Path $configFolder 'service.json'
$expectedCommand = '"' + $binary + '" --network-service "' + $configPath + '"'
# Reject weakened installations before reading their configuration or writing elevated files.
function Assert-ProtectedPath([string]$Path) {
    $acl = Get-Acl -LiteralPath $Path
    $owner = $acl.GetOwner([Security.Principal.SecurityIdentifier]).Value
    if ($owner -notin @('S-1-5-18', 'S-1-5-32-544') -or !$acl.AreAccessRulesProtected) {
        throw 'Service ownership or inheritance is not protected'
    }
    foreach ($rule in $acl.GetAccessRules($true, $true, [Security.Principal.SecurityIdentifier])) {
        if ($rule.AccessControlType -ne 'Allow') { continue }
        $sid = $rule.IdentityReference.Value
        if ($sid -in @('S-1-5-18', 'S-1-5-32-544')) { continue }
        if ($sid -ne $OwnerSid -or (([int]$rule.FileSystemRights -band (-bnot 0x1200a9)) -ne 0)) {
            throw 'Service path is writable by an unprivileged identity'
        }
    }
}

# Pin ownership as well as access rules: an unprivileged owner could otherwise rewrite its DACL.
function Copy-ProtectedBinary([string]$From, [string]$Path) {
    $acl = [Security.AccessControl.FileSecurity]::new()
    $acl.SetAccessRuleProtection($true, $false)
    $acl.SetOwner([Security.Principal.SecurityIdentifier]::new('S-1-5-32-544'))
    foreach ($sid in @('S-1-5-18', 'S-1-5-32-544')) {
        $acl.AddAccessRule([Security.AccessControl.FileSystemAccessRule]::new(
            [Security.Principal.SecurityIdentifier]::new($sid), 'FullControl', 'Allow'))
    }
    $acl.AddAccessRule([Security.AccessControl.FileSystemAccessRule]::new(
        [Security.Principal.SecurityIdentifier]::new($OwnerSid), 'ReadAndExecute', 'Allow'))
    $target = [IO.FileStream]::new($Path, [IO.FileMode]::CreateNew,
        [Security.AccessControl.FileSystemRights]::Write, [IO.FileShare]::None,
        4096, [IO.FileOptions]::None, $acl)
    try {
        $inputFile = [IO.File]::OpenRead($From)
        try { $inputFile.CopyTo($target) } finally { $inputFile.Dispose() }
    } finally { $target.Dispose() }
}

foreach ($path in @($folder, $configFolder, $binary, $configPath)) {
    if ((Get-Item -LiteralPath $path).Attributes -band [IO.FileAttributes]::ReparsePoint) { throw 'Service paths cannot be reparse points' }
    Assert-ProtectedPath $path
}
$config = [IO.File]::ReadAllText($configPath) | ConvertFrom-Json
$status = Get-CimInstance Win32_Service -Filter "Name='$name'"
if (!$status -or $status.PathName -cne $expectedCommand -or $status.StartName -ne 'LocalSystem' -or
    $config.owner_sid -ne $OwnerSid -or $config.service_name -cne $name -or
    $config.pipe_name -cne $name -or $config.service_exe -cne $binary) { throw 'Installed service identity does not match' }

# Counts a folder only SYSTEM may open, without changing its ACL or owner: SeBackupPrivilege grants
# read access regardless of the ACL when the handle is opened with FILE_FLAG_BACKUP_SEMANTICS
# (Microsoft Learn: Privilege Constants, CreateFileW). A reparse point is refused, never followed.
function Get-BackupEntryCount([string]$Path) {
    if (!('MiniClawBackupRead' -as [type])) { Add-Type -TypeDefinition @'
using System;using System.Runtime.InteropServices;using Microsoft.Win32.SafeHandles;
public static class MiniClawBackupRead{
[StructLayout(LayoutKind.Sequential,Pack=4)]struct P{public uint C;public long L;public uint A;}
[DllImport("advapi32.dll",SetLastError=true)]static extern bool OpenProcessToken(IntPtr p,uint a,out IntPtr t);
[DllImport("advapi32.dll",SetLastError=true,CharSet=CharSet.Unicode)]static extern bool LookupPrivilegeValueW(string s,string n,out long l);
[DllImport("advapi32.dll",SetLastError=true)]static extern bool AdjustTokenPrivileges(IntPtr t,bool d,ref P n,uint b,out P o,out uint r);
[DllImport("kernel32.dll")]static extern bool CloseHandle(IntPtr h);
[DllImport("kernel32.dll",SetLastError=true,CharSet=CharSet.Unicode)]static extern SafeFileHandle CreateFileW(string n,uint a,uint s,IntPtr x,uint c,uint f,IntPtr t);
[DllImport("kernel32.dll",SetLastError=true)]static extern bool GetFileInformationByHandleEx(SafeFileHandle h,int c,byte[] b,uint s);
static Exception E(){return new System.ComponentModel.Win32Exception();}
public static int Count(string path){
IntPtr t;if(!OpenProcessToken((IntPtr)(-1),0x28,out t))throw E();
P n=new P{C=1,A=2},o;uint r;
try{if(!LookupPrivilegeValueW(null,"SeBackupPrivilege",out n.L))throw E();
if(!AdjustTokenPrivileges(t,false,ref n,16,out o,out r))throw E();
try{if(Marshal.GetLastWin32Error()!=0)throw new UnauthorizedAccessException("SeBackupPrivilege is not held");
using(var h=CreateFileW(path,0x100081,7,IntPtr.Zero,3,0x02200000,IntPtr.Zero)){
if(h.IsInvalid)throw E();var b=new byte[65536];
if(!GetFileInformationByHandleEx(h,9,b,8))throw E();
if((BitConverter.ToUInt32(b,0)&0x410)!=0x10)throw new UnauthorizedAccessException("Not a plain folder: "+path);
int k=0;for(int c=15;GetFileInformationByHandleEx(h,c,b,65536);c=14)for(int i=0;;){
string m=System.Text.Encoding.Unicode.GetString(b,i+68,BitConverter.ToInt32(b,i+60));if(m!="."&&m!="..")k++;
int x=BitConverter.ToInt32(b,i);if(x==0)break;i+=x;}
if(Marshal.GetLastWin32Error()!=18)throw E();return k;}}
finally{if(o.C>0)AdjustTokenPrivileges(t,false,ref o,16,out n,out r);}}
finally{CloseHandle(t);}}}
'@ }
    return [MiniClawBackupRead]::Count($Path)
}
# A clean stop reports NO_ERROR. ERROR_SERVICE_NEVER_STARTED (1077) means the handler never ran,
# which is safe only while the lease journal holds no recovery record.
function Test-ServiceDrained($State) {
    if ($State.ExitCode -eq 0 -and $State.ServiceSpecificExitCode -eq 0) { return $true }
    if ($State.ExitCode -ne 1077 -or $State.ServiceSpecificExitCode -ne 0) { return $false }
    $leases = Join-Path $configFolder 'leases'
    if (![IO.Directory]::Exists($leases)) { return $true }
    # Once the service has run, the journal folder belongs to SYSTEM alone.
    try { return @([IO.Directory]::EnumerateFileSystemEntries($leases)).Count -eq 0 }
    catch [UnauthorizedAccessException] { return (Get-BackupEntryCount $leases) -eq 0 }
}

# A stopped service is safe to replace only after its handler confirms complete job teardown.
function Stop-DrainedService {
    $service = Get-Service -Name $name
    if ($service.Status -ne 'Stopped') {
        Stop-Service -Name $name
        $service.WaitForStatus('Stopped', [TimeSpan]::FromSeconds(45))
    }
    $state = Get-CimInstance Win32_Service -Filter "Name='$name'"
    if (!$state -or $state.State -ne 'Stopped' -or !(Test-ServiceDrained $state)) {
        throw 'Network service did not confirm sandbox cleanup; its binary and recovery records are retained'
    }
}

$nonce = [Guid]::NewGuid().ToString('N')
$staged = Join-Path $folder "update-$nonce.exe"
$backup = Join-Path $folder "previous-$nonce.exe"
$stopped = $false
$swapped = $false
# One updater holds this protected file exclusively for staging, drain, swap and recovery.
$updateLock = [IO.FileStream]::new((Join-Path $folder 'update.lock'), [IO.FileMode]::OpenOrCreate,
    [Security.AccessControl.FileSystemRights]::Write, [IO.FileShare]::None,
    4096, [IO.FileOptions]::None, (Get-Acl -LiteralPath $configPath))
try {
    Copy-ProtectedBinary $Source $staged
    if ((Get-FileHash -LiteralPath $staged -Algorithm SHA256).Hash.ToLowerInvariant() -ne $ExpectedSha256) {
        throw 'Copied service executable does not match the reviewed backend'
    }
    Stop-DrainedService
    $stopped = $true
    [IO.File]::Move($binary, $backup)
    try { [IO.File]::Move($staged, $binary) } catch { [IO.File]::Move($backup, $binary); throw }
    $swapped = $true
    Start-Service -Name $name
    (Get-Service -Name $name).WaitForStatus('Running', [TimeSpan]::FromSeconds(20))
    [IO.File]::Delete($backup)
} catch {
    $failure = $_
    if ($swapped) {
        try {
            Stop-DrainedService
            [IO.File]::Delete($binary)
            [IO.File]::Move($backup, $binary)
            Start-Service -Name $name
        } catch { Write-Warning "Service update recovery needs administrator attention: $name. Protected binaries and recovery records are retained." }
    } elseif ($stopped) {
        # The drain succeeded but the swap did not; the original binary resumes service.
        try {
            if (![IO.File]::Exists($binary) -and [IO.File]::Exists($backup)) { [IO.File]::Move($backup, $binary) }
            if (![IO.File]::Exists($binary)) { throw 'Original service executable is missing' }
            Start-Service -Name $name
        } catch { Write-Warning "Service restart needs administrator attention: $name. Protected binaries and recovery records are retained." }
    }
    throw $failure
} finally {
    try { if ([IO.File]::Exists($staged)) { [IO.File]::Delete($staged) } } finally { $updateLock.Dispose() }
}
