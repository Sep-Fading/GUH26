# Runs real sandbox acceptance under a disposable non-administrator account.
param([Parameter(Mandatory=$true)][string]$Backend, [Parameter(Mandatory=$true)][string]$LibraryTests, [Parameter(Mandatory=$true)][string]$BrokerTests, [Parameter(Mandatory=$true)][string]$AclTests, [Parameter(Mandatory=$true)][string]$NetworkTests, [Parameter(Mandatory=$true)][string]$RuntimesTests, [Parameter(Mandatory=$true)][ValidateSet(34404,43620)][uint16]$ExpectedMachine, [switch]$BrokerDiagnostic)
$ErrorActionPreference = 'Stop'
# The programs copied for the account, by the name they get there.
$copies = [ordered]@{ 'miniclaw-server.exe' = $Backend; 'native-tests.exe' = $LibraryTests; 'broker-tests.exe' = $BrokerTests; 'acl-tests.exe' = $AclTests; 'network-tests.exe' = $NetworkTests; 'runtimes-tests.exe' = $RuntimesTests }
# Every copied executable must belong to the explicitly selected architecture.
foreach ($key in @($copies.Keys)) {
  $path = $copies[$key] = (Resolve-Path -LiteralPath $copies[$key]).Path
  $stream = [IO.File]::OpenRead($path)
  $reader = [IO.BinaryReader]::new($stream)
  try {
    if ($stream.Length -lt 64 -or $reader.ReadUInt16() -ne 0x5a4d) { throw "Invalid PE executable: $path" }
    $stream.Position = 60; $offset = $reader.ReadUInt32()
    if ($offset -lt 64 -or $offset + 6 -gt $stream.Length) { throw "Invalid PE header: $path" }
    $stream.Position = $offset
    if ($reader.ReadUInt32() -ne 0x4550 -or $reader.ReadUInt16() -ne $ExpectedMachine) { throw "Wrong standard-user test architecture: $path" }
  } finally { $reader.Dispose() }
}
# The test lists live beside this script as data; a list this script was not reviewed with is refused.
$lists = Join-Path $PSScriptRoot 'windows-native-tests.psd1'
$sha = [Security.Cryptography.SHA256]::Create()
$seen = ([BitConverter]::ToString($sha.ComputeHash([Text.Encoding]::UTF8.GetBytes(([IO.File]::ReadAllText($lists) -replace "`r", ''))))).Replace('-', '').ToLowerInvariant()
if ($seen -ne '31349d9efbcf824765288e097b54c30f8c0211654f22f2f7cc31c31582269eba') { throw "Unreviewed native test lists: $seen" }
$tests = Import-PowerShellDataFile -LiteralPath $lists
$repository = Split-Path -Parent $PSScriptRoot
Push-Location $repository
$name = 'mc' + [Guid]::NewGuid().ToString('N').Substring(0, 14)
$plain = [Guid]::NewGuid().ToString('N') + '!aA9'
$password = ConvertTo-SecureString $plain -AsPlainText -Force
$credential = [PSCredential]::new("$env:COMPUTERNAME\$name", $password)
$directory = Join-Path $env:ProgramData ("MiniClaw-Acceptance-" + [Guid]::NewGuid().ToString('N'))
$account = $null
$serviceId = $null
$passed = $false
try {
  $account = New-LocalUser -Name $name -Password $password -AccountNeverExpires
  Add-LocalGroupMember -SID 'S-1-5-32-545' -Member $account
  New-Item -ItemType Directory -Path $directory | Out-Null
  # A per-user installation owns its runtime files: the sandbox adds package ACEs to the bundled
  # shell, which needs WRITE_DAC. Modify alone would test an installation the user cannot manage.
  & "$env:SystemRoot\System32\icacls.exe" $directory /inheritance:r /grant:r '*S-1-5-18:(OI)(CI)F' '*S-1-5-32-544:(OI)(CI)F' "*$($account.SID.Value):(OI)(CI)F"
  if ($LASTEXITCODE -ne 0) { throw 'Cannot prepare standard-user test directory' }
  foreach ($copy in $copies.GetEnumerator()) { Copy-Item -LiteralPath $copy.Value -Destination (Join-Path $directory $copy.Key) }
  Copy-Item -LiteralPath $lists -Destination (Join-Path $directory 'tests.psd1')
  # The pinned shell, ConPTY, private Node and Python hook, as staged beside the library tests.
  foreach ($runtime in @('shell', 'conpty', 'node', 'python')) {
    Copy-Item -LiteralPath (Join-Path (Split-Path -Parent $copies['native-tests.exe']) $runtime) -Destination (Join-Path $directory $runtime) -Recurse
  }
  $backend = $copies['miniclaw-server.exe']
  $serviceId = [Guid]::NewGuid().ToString('N')
  $sha = (Get-FileHash -LiteralPath $backend -Algorithm SHA256).Hash.ToLowerInvariant()
  $source = & "$env:SystemRoot\System32\WindowsPowerShell\v1.0\powershell.exe" -NoProfile -ExecutionPolicy Bypass -File scripts/setup/windows-network-install.ps1 -Source $backend -ExpectedSha256 $sha -OwnerSid $account.SID.Value -Id $serviceId
  if ($LASTEXITCODE -ne 0) { throw 'Cannot install standard-user broker' }
  $source | Select-Object -Last 1 | Set-Content -LiteralPath (Join-Path $directory 'service-source.txt') -Encoding utf8
  if ($BrokerDiagnostic) { Set-Content -LiteralPath (Join-Path $directory 'broker-diagnostic.flag') -Value 'test-only' -Encoding ascii }
  @'
$ErrorActionPreference = 'Stop'
$principal = [Security.Principal.WindowsPrincipal]::new([Security.Principal.WindowsIdentity]::GetCurrent())
if ($principal.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)) { throw 'Acceptance must run without administrator membership' }
$env:USERPROFILE = [Environment]::GetFolderPath('UserProfile')
$env:HOME = $env:USERPROFILE
$env:APPDATA = [Environment]::GetFolderPath('ApplicationData')
$env:LOCALAPPDATA = [Environment]::GetFolderPath('LocalApplicationData')
$env:TEMP = Join-Path $PSScriptRoot 'temp'
$env:TMP = $env:TEMP
& "$PSScriptRoot\miniclaw-server.exe" --secure-data-dir $env:TEMP
if ($LASTEXITCODE -ne 0) { throw 'Cannot create private standard-user temporary folder' }
$env:MINICLAW_DATA_DIR = Join-Path $PSScriptRoot 'data'
& "$PSScriptRoot\miniclaw-server.exe" --secure-data-dir $env:MINICLAW_DATA_DIR
if ($LASTEXITCODE -ne 0) { throw 'Cannot create private standard-user state' }
$env:MINICLAW_NETWORK_CONFIG = Join-Path $env:MINICLAW_DATA_DIR 'windows-network.json'
$source = (Get-Content -LiteralPath "$PSScriptRoot\service-source.txt" -Raw -Encoding utf8).Trim()
$OutputEncoding = [Text.UTF8Encoding]::new($false)
Get-Content -LiteralPath $source -Raw -Encoding utf8 | & "$PSScriptRoot\miniclaw-server.exe" --write-private-file $env:MINICLAW_NETWORK_CONFIG
if ($LASTEXITCODE -ne 0) { throw 'Cannot copy standard-user broker configuration' }
$diagnostic = 'windows_network_broker::native::tests::standard_user_boot_auth_diagnostic'
$listed = & "$PSScriptRoot\native-tests.exe" --exact $diagnostic --list
if ($LASTEXITCODE -ne 0 -or $listed -notcontains ($diagnostic + ': test')) { throw 'Missing standard-user broker authentication/rights test' }
& "$PSScriptRoot\native-tests.exe" --exact $diagnostic --ignored --nocapture
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
if (Test-Path -LiteralPath (Join-Path $PSScriptRoot 'broker-diagnostic.flag')) { exit 0 }
$tests = Import-PowerShellDataFile -LiteralPath "$PSScriptRoot\tests.psd1"
foreach ($filter in $tests.StandardLibrary) {
  $listed = & "$PSScriptRoot\native-tests.exe" $filter --list
  if ($LASTEXITCODE -ne 0 -or !($listed | Where-Object { $_ -match ": test$" })) { throw "No native tests selected: $filter" }
  & "$PSScriptRoot\native-tests.exe" $filter --test-threads=1
  if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
}
# The gate is open for this account with its own service, and the switch closes it for one process.
$env:MINICLAW_GATE_EXPECT = 'ready'
& "$PSScriptRoot\native-tests.exe" --exact $tests.GateReady --ignored --nocapture
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
$env:MINICLAW_GATE_EXPECT = $null
$env:MINICLAW_DISABLE_SANDBOX = '1'
& "$PSScriptRoot\native-tests.exe" --exact $tests.GateSwitchedOff --ignored --nocapture
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
$env:MINICLAW_DISABLE_SANDBOX = $null
# Exact tests: file policy, then the ignored ones that need the broker (old-SID isolation,
# attestation, recovery, hard links, network).
foreach ($entry in $tests.StandardExact) {
  $kind, $mode, $test = $entry -split ' ', 3
  $binary = "$PSScriptRoot\$kind-tests.exe"
  $extra = @(); if ($mode -eq 'ignored') { $extra = @('--ignored') }
  $listed = & $binary --exact $test --list
  if ($LASTEXITCODE -ne 0 -or $listed -notcontains ($test + ': test')) { throw "Missing standard-user test: $test" }
  & $binary --exact $test @extra --nocapture
  if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
}
# These private folders admit only this account, so it removes them itself once everything passed.
Remove-Item -LiteralPath $env:MINICLAW_DATA_DIR, $env:TEMP -Recurse -Force
'@ | Set-Content -LiteralPath (Join-Path $directory 'run.ps1') -Encoding utf8
  $process = Start-Process -FilePath "$env:SystemRoot\System32\WindowsPowerShell\v1.0\powershell.exe" -Credential $credential -LoadUserProfile -WorkingDirectory $directory -ArgumentList @('-NoLogo', '-NoProfile', '-NonInteractive', '-ExecutionPolicy', 'Bypass', '-File', ('"' + (Join-Path $directory 'run.ps1') + '"')) -RedirectStandardOutput (Join-Path $directory 'stdout.log') -RedirectStandardError (Join-Path $directory 'stderr.log') -PassThru
  # Retain the process handle before it exits; PS5 otherwise may lose ExitCode.
  $retainedProcessHandle = $process.Handle
  if (!$process.WaitForExit(600000)) {
    & "$env:SystemRoot\System32\taskkill.exe" /PID $process.Id /T /F
    if ($LASTEXITCODE -ne 0) { throw 'Could not stop timed-out acceptance tree; preserving its account and state' }
    if (!$process.WaitForExit(10000)) { throw 'Timed-out acceptance tree did not exit' }
    throw 'Standard-user acceptance timed out'
  }
  Get-Content -LiteralPath (Join-Path $directory 'stdout.log')
  Get-Content -LiteralPath (Join-Path $directory 'stderr.log')
  $exitCode = $process.ExitCode
  if ($null -eq $exitCode) { throw 'Standard-user process did not provide an exit code' }
  if ($exitCode -ne 0) { throw "Standard-user tests failed: $exitCode" }
  $passed = $true
} finally {
  if ($serviceId -and (Get-Service -Name "MiniClawNetwork-$serviceId" -ErrorAction SilentlyContinue)) {
    $preserve = @()
    if (!$passed) { $preserve = @('-PreserveInstallation') }
    & "$env:SystemRoot\System32\WindowsPowerShell\v1.0\powershell.exe" -NoProfile -ExecutionPolicy Bypass -File scripts/windows-ci-stop-network.ps1 -Id $serviceId @preserve
    if ($LASTEXITCODE -ne 0) { throw 'Preserving standard-user account and files for uncertain service recovery' }
  }
  if ($passed) {
    if ($account) {
      $process.Dispose()
      & "$env:SystemRoot\System32\WindowsPowerShell\v1.0\powershell.exe" -NoProfile -ExecutionPolicy Bypass -File scripts/windows-ci-remove-profile.ps1 -Sid $account.SID.Value -AccountName $name
      if ($LASTEXITCODE -ne 0) { throw 'Tests passed but profile cleanup failed; preserving account and acceptance files' }
      Remove-LocalUser -SID $account.SID
    }
    if (Test-Path -LiteralPath $directory) { Remove-Item -LiteralPath $directory -Recurse -Force }
  } else {
    Write-Warning "Acceptance failed; preserving disposable account $name and private recovery state at $directory"
  }
}
Pop-Location
