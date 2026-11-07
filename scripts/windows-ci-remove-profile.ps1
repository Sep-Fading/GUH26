# Removes only a successfully tested disposable account's unloaded Windows profile.
# DeleteProfileW: https://learn.microsoft.com/windows/win32/api/userenv/nf-userenv-deleteprofilew
param([Parameter(Mandatory=$true)][string]$Sid, [Parameter(Mandatory=$true)][string]$AccountName)
$ErrorActionPreference = 'Stop'
if ($AccountName -cnotmatch '^mc[0-9a-f]{14}$' -or $Sid -notmatch '^S-1-5-21-[0-9-]+$') { throw 'Not a disposable acceptance identity' }
$account = Get-LocalUser -SID ([Security.Principal.SecurityIdentifier]::new($Sid))
if ($account.Name -cne $AccountName -or $account.SID.Value -ne $Sid -or $Sid -eq [Security.Principal.WindowsIdentity]::GetCurrent().User.Value) { throw 'Disposable profile identity mismatch' }
$deadline = [DateTime]::UtcNow.AddSeconds(30)
do {
  $profiles = @(Get-CimInstance -ClassName Win32_UserProfile -Filter "SID='$Sid'")
  if ($profiles.Count -ne 1) { throw 'Expected exactly one disposable user profile' }
  $profile = $profiles[0]
  if ($profile.Special) { throw 'Refusing a special profile' }
  if (!$profile.Loaded) { break }
  Start-Sleep -Milliseconds 250
} while ([DateTime]::UtcNow -lt $deadline)
if ($profile.Loaded) { throw 'Disposable profile remains loaded; preserving it' }
$path = [IO.Path]::GetFullPath($profile.LocalPath).TrimEnd('\')
$profilesRoot = [Environment]::ExpandEnvironmentVariables((Get-ItemProperty -LiteralPath 'HKLM:\SOFTWARE\Microsoft\Windows NT\CurrentVersion\ProfileList').ProfilesDirectory)
$profilesRoot = [IO.Path]::GetFullPath($profilesRoot).TrimEnd('\')
if ([IO.Path]::GetFileName($path) -cne $AccountName -or ![string]::Equals([IO.Path]::GetDirectoryName($path), $profilesRoot, [StringComparison]::OrdinalIgnoreCase)) { throw 'Disposable profile path mismatch' }
$item = Get-Item -LiteralPath $path -Force
if (!$item.PSIsContainer -or ($item.Attributes -band [IO.FileAttributes]::ReparsePoint)) { throw 'Disposable profile must be an ordinary directory' }
# Windows creates a profile folder owned by SYSTEM (observed for every profile on the test guests,
# the signed-in owner's included) and gives the account Full control. The folder must carry that
# explicit grant for this account and no grant for any other user account.
$acl = Get-Acl -LiteralPath $path
if ($acl.GetOwner([Security.Principal.SecurityIdentifier]).Value -notin @($Sid, 'S-1-5-18', 'S-1-5-32-544')) { throw 'Disposable profile directory owner mismatch' }
$allowed = @($acl.GetAccessRules($true, $false, [Security.Principal.SecurityIdentifier]) | Where-Object { $_.AccessControlType -eq 'Allow' })
$full = [Security.AccessControl.FileSystemRights]::FullControl
if (!($allowed | Where-Object { $_.IdentityReference.Value -eq $Sid -and ($_.FileSystemRights -band $full) -eq $full })) { throw 'Disposable profile directory is not granted to its account' }
if ($allowed | Where-Object { $_.IdentityReference.Value -match '^S-1-(5-21|12-1)-' -and $_.IdentityReference.Value -ne $Sid }) { throw 'Disposable profile directory is shared with another account' }
Add-Type -TypeDefinition @'
using System;
using System.Runtime.InteropServices;
public static class MiniClawAcceptanceProfile {
    [DllImport("userenv.dll", CharSet=CharSet.Unicode, ExactSpelling=true, SetLastError=true)]
    [return: MarshalAs(UnmanagedType.Bool)]
    public static extern bool DeleteProfileW(string sid, string path, IntPtr computer);
}
'@
# The computer name must be NULL (Microsoft Learn, DeleteProfileW); PowerShell would pass a
# string-typed $null as an empty string, which Windows refuses with ERROR_INVALID_PARAMETER.
if (![MiniClawAcceptanceProfile]::DeleteProfileW($Sid, $path, [IntPtr]::Zero)) {
  $code = [Runtime.InteropServices.Marshal]::GetLastWin32Error()
  throw "DeleteProfileW failed with Windows error $code; preserving the account"
}
if (@(Get-CimInstance -ClassName Win32_UserProfile -Filter "SID='$Sid'").Count -ne 0 -or (Test-Path -LiteralPath $path)) { throw 'Profile cleanup could not be verified; preserving the account' }
Write-Host "Removed verified disposable profile for $AccountName"
