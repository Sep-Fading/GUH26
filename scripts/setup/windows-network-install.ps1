param(
    [Parameter(Mandatory=$true)][string]$Source,
    [Parameter(Mandatory=$true)][ValidatePattern('^[a-f0-9]{64}$')][string]$ExpectedSha256,
    [Parameter(Mandatory=$true)][ValidatePattern('^S-1-(?:5-21|12-1)(?:-(?:0|[1-9]\d*)){4}$')][string]$OwnerSid,
    [Parameter(Mandatory=$true)][ValidatePattern('^[a-f0-9]{32}$')][string]$Id
)
$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest
if ([Security.Principal.SecurityIdentifier]::new($OwnerSid).Value -cne $OwnerSid) { throw 'Owner SID must be canonical' }
Add-Type -TypeDefinition @'
using System;
using System.ComponentModel;
using System.Runtime.InteropServices;
public static class MiniClawServiceDirectory {
    [StructLayout(LayoutKind.Sequential)]
    private struct Attributes { public int Length; public IntPtr Descriptor; public int Inherit; }
    [DllImport("kernel32.dll", CharSet=CharSet.Unicode, SetLastError=true)]
    private static extern bool CreateDirectoryW(string path, ref Attributes attributes);
    // Creates exactly one directory with its final ACL, failing if another object exists.
    public static void Create(string path, byte[] descriptor) {
        var pin = GCHandle.Alloc(descriptor, GCHandleType.Pinned);
        try {
            var attributes = new Attributes { Length=Marshal.SizeOf(typeof(Attributes)), Descriptor=pin.AddrOfPinnedObject() };
            if (!CreateDirectoryW(path, ref attributes)) throw new Win32Exception(Marshal.GetLastWin32Error());
        } finally { pin.Free(); }
    }
}
'@

# Creates a protected directory atomically; an existing destination is never adopted.
function New-ProtectedDirectory([string]$Path, [bool]$OwnerCanRead) {
    if ([IO.Directory]::Exists($Path) -or [IO.File]::Exists($Path)) { throw 'Service destination already exists' }
    $acl = [Security.AccessControl.DirectorySecurity]::new()
    $acl.SetAccessRuleProtection($true, $false)
    $acl.SetOwner([Security.Principal.SecurityIdentifier]::new('S-1-5-32-544'))
    foreach ($sid in @('S-1-5-18', 'S-1-5-32-544')) {
        $acl.AddAccessRule([Security.AccessControl.FileSystemAccessRule]::new(
            [Security.Principal.SecurityIdentifier]::new($sid), 'FullControl',
            'ContainerInherit,ObjectInherit', 'None', 'Allow'))
    }
    if ($OwnerCanRead) {
        $acl.AddAccessRule([Security.AccessControl.FileSystemAccessRule]::new(
            [Security.Principal.SecurityIdentifier]::new($OwnerSid), 'ReadAndExecute',
            'ContainerInherit,ObjectInherit', 'None', 'Allow'))
    }
    [MiniClawServiceDirectory]::Create($Path, $acl.GetSecurityDescriptorBinaryForm())
}

# Returns administrative ownership and exact access for atomically created files.
function Get-ServiceFileSecurity([bool]$OwnerCanRead) {
    $acl = [Security.AccessControl.FileSecurity]::new()
    $acl.SetAccessRuleProtection($true, $false)
    $acl.SetOwner([Security.Principal.SecurityIdentifier]::new('S-1-5-32-544'))
    foreach ($sid in @('S-1-5-18', 'S-1-5-32-544')) {
        $acl.AddAccessRule([Security.AccessControl.FileSystemAccessRule]::new(
            [Security.Principal.SecurityIdentifier]::new($sid), 'FullControl', 'Allow'))
    }
    if ($OwnerCanRead) {
        $acl.AddAccessRule([Security.AccessControl.FileSystemAccessRule]::new(
            [Security.Principal.SecurityIdentifier]::new($OwnerSid), 'ReadAndExecute', 'Allow'))
    }
    return $acl
}

# Opens a new protected file with no sharing and its final owner already assigned.
function New-ServiceFile([string]$Path, [bool]$OwnerCanRead) {
    return [IO.FileStream]::new($Path, [IO.FileMode]::CreateNew,
        [Security.AccessControl.FileSystemRights]::Write, [IO.FileShare]::None,
        4096, [IO.FileOptions]::None, (Get-ServiceFileSecurity $OwnerCanRead))
}

# Writes bounded configuration through the same newly created protected handle.
function Write-ServiceConfig([string]$Path, [byte[]]$Bytes, [bool]$OwnerCanRead) {
    $file = New-ServiceFile $Path $OwnerCanRead
    try { $file.Write($Bytes, 0, $Bytes.Length) } finally { $file.Dispose() }
}

$identity = [Security.Principal.WindowsIdentity]::GetCurrent()
$principal = [Security.Principal.WindowsPrincipal]::new($identity)
if (!$principal.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)) {
    throw 'Installing the network isolation service requires Windows administrator approval'
}
if ($Source -notmatch '^[A-Za-z]:\\' -or $Source.Substring(2).Contains(':')) {
    throw 'The backend must be a local Windows executable'
}
$sourceFile = Get-Item -LiteralPath $Source
if ($sourceFile.PSIsContainer -or ($sourceFile.Attributes -band [IO.FileAttributes]::ReparsePoint)) {
    throw 'The backend must be a regular executable'
}
$name = "MiniClawNetwork-$Id"
$programs = [Environment]::GetFolderPath('ProgramFiles')
$commonData = [Environment]::GetFolderPath('CommonApplicationData')
$binaryFolder = Join-Path $programs $name
$configFolder = Join-Path $commonData $name
# A clean stop reports NO_ERROR. ERROR_SERVICE_NEVER_STARTED (1077) means the handler never ran,
# which is safe only while the lease journal holds no recovery record.
function Test-ServiceDrained($State) {
    if ($State.ExitCode -eq 0 -and $State.ServiceSpecificExitCode -eq 0) { return $true }
    if ($State.ExitCode -ne 1077 -or $State.ServiceSpecificExitCode -ne 0) { return $false }
    $leases = Join-Path $configFolder 'leases'
    if (![IO.Directory]::Exists($leases)) { return $true }
    # Once the service has run, the journal folder belongs to SYSTEM alone.
    try { return @([IO.Directory]::EnumerateFileSystemEntries($leases)).Count -eq 0 }
    # A service created by this run that never started cannot have written it; refuse rather than guess.
    catch [UnauthorizedAccessException] { throw 'The network service journal is readable only by SYSTEM; it is kept for administrator recovery' }
}
$createdFolders = @()
$createdService = $false
try {
    New-ProtectedDirectory $binaryFolder $true
    $createdFolders += $binaryFolder
    New-ProtectedDirectory $configFolder $true
    $createdFolders += $configFolder
    $binary = Join-Path $binaryFolder 'miniclaw-network-service.exe'
    $targetFile = New-ServiceFile $binary $true
    try {
        $inputFile = [IO.File]::OpenRead($sourceFile.FullName)
        try { $inputFile.CopyTo($targetFile) } finally { $inputFile.Dispose() }
    } finally { $targetFile.Dispose() }
    $actual = (Get-FileHash -LiteralPath $binary -Algorithm SHA256).Hash.ToLowerInvariant()
    if ($actual -ne $ExpectedSha256) { throw 'Copied service executable does not match the reviewed backend' }
    $secretBytes = New-Object byte[] 32
    $rng = [Security.Cryptography.RandomNumberGenerator]::Create()
    try { $rng.GetBytes($secretBytes) } finally { $rng.Dispose() }
    $secret = ([BitConverter]::ToString($secretBytes)).Replace('-', '').ToLowerInvariant()
    $configuration = @{ owner_sid=$OwnerSid; pipe_name=$name; secret_hex=$secret;
        service_name=$name; service_exe=$binary } | ConvertTo-Json -Compress
    $serviceConfig = Join-Path $configFolder 'service.json'
    $clientConfig = Join-Path $configFolder 'client.json'
    $utf8 = [Text.UTF8Encoding]::new($false)
    Write-ServiceConfig $serviceConfig ($utf8.GetBytes($configuration)) $false
    Write-ServiceConfig $clientConfig ($utf8.GetBytes($configuration)) $true
    $command = '"' + $binary + '" --network-service "' + $serviceConfig + '"'
    New-Service -Name $name -BinaryPathName $command -DisplayName 'MiniClaw network isolation' `
        -Description 'Restricts approved MiniClaw AppContainer jobs to their assigned local proxy.' `
        -StartupType Automatic | Out-Null
    $createdService = $true
    Start-Service -Name $name
    (Get-Service -Name $name).WaitForStatus('Running', [TimeSpan]::FromSeconds(20))
    # The service adds its Winsock catalog read entries before it reports Running.
    if (!([IO.File]::ReadAllText((Join-Path $configFolder 'winsock-acl-status.json')) | ConvertFrom-Json).ok) { throw 'Winsock catalog access could not be configured' }
    Write-Output $clientConfig
} catch {
    $failure = $_
    $canRemoveFiles = $true
    if ($createdService) {
        try {
            $service = Get-Service -Name $name
            if ($service.Status -ne 'Stopped') {
                Stop-Service -Name $name
                $service.WaitForStatus('Stopped', [TimeSpan]::FromSeconds(30))
            }
            $status = Get-CimInstance Win32_Service -Filter "Name='$name'"
            if (!$status -or $status.State -ne 'Stopped' -or !(Test-ServiceDrained $status)) {
                throw 'Network service could not confirm sandbox cleanup'
            }
            # This start parameter makes the service remove its Winsock catalog entries and stop.
            $service.Start('winsock-acl-remove')
            $service.WaitForStatus('Stopped', [TimeSpan]::FromSeconds(60))
            if (!([IO.File]::ReadAllText((Join-Path $configFolder 'winsock-acl-status.json')) | ConvertFrom-Json).removed) { throw 'Winsock catalog entries were not removed' }
            & "$env:SystemRoot\System32\sc.exe" delete $name | Out-Null
            if ($LASTEXITCODE -ne 0) { throw 'Could not remove failed network service registration' }
        } catch {
            $canRemoveFiles = $false
            Write-Warning "Network service cleanup needs administrator attention: $name"
        }
    }
    if ($canRemoveFiles) {
        foreach ($folder in $createdFolders) { Remove-Item -LiteralPath $folder -Recurse -Force }
    }
    throw $failure
}
