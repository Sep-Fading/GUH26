# Native isolation acceptance for an elevated disposable Windows guest or CI runner.
# The sandbox suite also runs as a new standard user.
param([ValidateSet('', 'aarch64-pc-windows-msvc', 'x86_64-pc-windows-msvc')][string]$Target = '')
$ErrorActionPreference = 'Stop'
# Rejects wrong-architecture executables.
function Assert-TestMachine([string]$Path, [uint16]$Expected) {
    $stream = [IO.File]::OpenRead($Path)
    $reader = [IO.BinaryReader]::new($stream)
    try {
        if ($stream.Length -lt 64 -or $reader.ReadUInt16() -ne 0x5a4d) { throw "Invalid PE executable: $Path" }
        $stream.Position = 60
        $offset = $reader.ReadUInt32()
        if ($offset -lt 64 -or $offset + 6 -gt $stream.Length) { throw "Invalid PE header: $Path" }
        $stream.Position = $offset
        if ($reader.ReadUInt32() -ne 0x4550 -or $reader.ReadUInt16() -ne $Expected) { throw "Wrong executable architecture: $Path" }
    } finally { $reader.Dispose() }
}

$identity = [Security.Principal.WindowsIdentity]::GetCurrent()
$principal = [Security.Principal.WindowsPrincipal]::new($identity)
if (!$principal.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)) {
    throw 'Run native acceptance elevated in a disposable Windows test machine'
}
# The test lists live beside this script as data; a list this script was not reviewed with is refused.
$lists = Join-Path $PSScriptRoot 'windows-native-tests.psd1'
$sha = [Security.Cryptography.SHA256]::Create()
$seen = ([BitConverter]::ToString($sha.ComputeHash([Text.Encoding]::UTF8.GetBytes(([IO.File]::ReadAllText($lists) -replace "`r", ''))))).Replace('-', '').ToLowerInvariant()
if ($seen -ne '31349d9efbcf824765288e097b54c30f8c0211654f22f2f7cc31c31582269eba') { throw "Unreviewed native test lists: $seen" }
$tests = Import-PowerShellDataFile -LiteralPath $lists
$repository = Split-Path -Parent $PSScriptRoot
$oldData = $env:MINICLAW_DATA_DIR
$oldNetwork = $env:MINICLAW_NETWORK_CONFIG
$serviceId = $null
$data = $null
$passed = $false
$shell = "$env:SystemRoot\System32\WindowsPowerShell\v1.0\powershell.exe"
Push-Location $repository
try {
    $targetArguments = @()
    $chosenTarget = $Target
    if ($Target) { $targetArguments = @('--target', $Target) }
    else {
        $compiler = & rustc -vV
        if ($LASTEXITCODE -ne 0) { throw 'Cannot determine default Rust host architecture' }
        $chosenTarget = (($compiler | Select-String '^host: ').Line -replace '^host: ', '').Trim()
    }
    $machine = switch ($chosenTarget) {
        'aarch64-pc-windows-msvc' { 0xaa64 }
        'x86_64-pc-windows-msvc' { 0x8664 }
        default { throw "Unsupported native test architecture: $chosenTarget" }
    }
    # MiniClaw's private Node: its seed library first, because the backend is built with its hash.
    $seedLibrary = & node scripts/windows-native-runtimes.mjs seed $chosenTarget | Select-Object -Last 1
    if ($LASTEXITCODE -ne 0) { throw 'OpenSSL seed library build failed' }
    Assert-TestMachine $seedLibrary $machine
    $env:MINICLAW_OPENSSL_SEED_SHA256 = (Get-FileHash -LiteralPath $seedLibrary -Algorithm SHA256).Hash.ToLowerInvariant()
    $built = & cargo build --locked @targetArguments --bin miniclaw-server --example provider_fixture --message-format=json
    if ($LASTEXITCODE -ne 0) { throw 'Native backend build failed' }
    $artifacts = $built | ForEach-Object { $_ | ConvertFrom-Json } | Where-Object { $_.reason -eq 'compiler-artifact' -and $_.executable -and !$_.profile.test }
    $backendArtifact = @($artifacts | Where-Object { $_.target.name -eq 'miniclaw-server' -and $_.target.kind -contains 'bin' })
    $providerArtifact = @($artifacts | Where-Object { $_.target.name -eq 'provider_fixture' -and $_.target.kind -contains 'example' })
    if ($backendArtifact.Count -ne 1 -or $providerArtifact.Count -ne 1) { throw 'Missing or ambiguous native build artifacts' }
    $backend = (Resolve-Path -LiteralPath $backendArtifact[0].executable).Path
    $provider = (Resolve-Path -LiteralPath $providerArtifact[0].executable).Path
    Assert-TestMachine $backend $machine
    Assert-TestMachine $provider $machine
    Write-Host "Selected target: $chosenTarget; backend: $backend; provider fixture: $provider"

    $records = & cargo test --locked @targetArguments --all-targets --no-run --message-format=json
    if ($LASTEXITCODE -ne 0) { throw 'Cannot compile native acceptance before broker installation' }
    $testArtifacts = @($records | ForEach-Object { $_ | ConvertFrom-Json } | Where-Object { $_.reason -eq 'compiler-artifact' -and $_.executable -and $_.profile.test })
    # Selects one exact test target and checks its machine before execution.
    function Test-Binary([string]$Name, [string]$Kind = 'test') {
        $selected = @($testArtifacts | Where-Object { $_.target.name -eq $Name -and $_.target.kind -contains $Kind })
        if ($selected.Count -ne 1) { throw "Missing or ambiguous test target: $Name ($Kind)" }
        Assert-TestMachine $selected[0].executable $machine
        return $selected[0].executable
    }
    # Runs compiled artifacts without relinking the service executable.
    function Run-Test([string]$Name, [string]$Kind = 'test', [string]$Filter = '', [switch]$Ignored) {
        $binary = Test-Binary $Name $Kind
        $arguments = @('--nocapture', '--test-threads=1')
        if ($Filter) { $arguments += $Filter }
        if ($Ignored) { $arguments += '--ignored' }
        $listed = & $binary @arguments --list
        if ($LASTEXITCODE -ne 0 -or !($listed | Where-Object { $_ -match ': test$' })) { throw "No native tests selected: $Name $Filter" }
        & $binary @arguments
        if ($LASTEXITCODE -ne 0) { throw "Native acceptance failed: $Name $Filter ($LASTEXITCODE)" }
    }
    $unitBinary = Test-Binary 'miniclaw_server' 'lib'
    $arch = if ($machine -eq 0xaa64) { 'arm64' } else { 'x64' }
    & node scripts/windows-native-runtimes.mjs stage $arch $seedLibrary $backend $unitBinary (Test-Binary 'windows_runtimes')
    if ($LASTEXITCODE -ne 0) { throw 'Cannot stage the verified sandbox runtimes beside test artifacts' }

    $serviceId = $id = [Guid]::NewGuid().ToString('N')
    $sha = (Get-FileHash -LiteralPath $backend -Algorithm SHA256).Hash.ToLowerInvariant()
    $source = & $shell -NoProfile -ExecutionPolicy Bypass -File scripts/setup/windows-network-install.ps1 -Source $backend -ExpectedSha256 $sha -OwnerSid $identity.User.Value -Id $id
    if ($LASTEXITCODE -ne 0) { throw 'Cannot install disposable native broker' }
    $env:MINICLAW_DATA_DIR = $data = Join-Path $env:TEMP ('MiniClaw-Native-CI-' + [Guid]::NewGuid().ToString('N'))
    & $backend --secure-data-dir $data
    if ($LASTEXITCODE -ne 0) { throw 'Cannot create private native test state' }
    $private = Join-Path $data 'windows-network.json'
    $OutputEncoding = [Text.UTF8Encoding]::new($false)
    Get-Content -LiteralPath ($source | Select-Object -Last 1) -Raw -Encoding utf8 | & $backend --write-private-file $private
    if ($LASTEXITCODE -ne 0) { throw 'Cannot save private native broker configuration' }
    $env:MINICLAW_NETWORK_CONFIG = $private
    if (!$env:MINICLAW_TEST_GIT_ARCHIVE) { throw 'Pinned MINICLAW_TEST_GIT_ARCHIVE is required' }
    & node scripts/windows-native-runtimes.mjs git $arch $backend
    if ($LASTEXITCODE -ne 0) { throw 'Cannot install verified private Git acceptance runtime' }
    $before = & $backend --network-epoch
    if ($LASTEXITCODE -ne 0) { throw 'Cannot authenticate boot epoch' }
    Restart-Service -Name "MiniClawNetwork-$id"
    (Get-Service -Name "MiniClawNetwork-$id").WaitForStatus('Running', [TimeSpan]::FromSeconds(20))
    $after = & $backend --network-epoch
    if ($LASTEXITCODE -ne 0 -or $before -cne $after) { throw 'Service restart changed the OS boot epoch' }

    Run-Test 'miniclaw-server' 'bin'
    foreach ($name in $tests.Integration) { Run-Test $name }
    foreach ($entry in $tests.Ignored) { $binaryName, $filter = $entry -split ' ', 2; Run-Test $binaryName 'test' $filter -Ignored }
    foreach ($filter in $tests.Library) { Run-Test 'miniclaw_server' 'lib' $filter }
    # The gate with this installation's service running, then switched off for one whole process.
    $env:MINICLAW_GATE_EXPECT = 'ready'
    Run-Test 'miniclaw_server' 'lib' $tests.GateReady -Ignored
    $env:MINICLAW_GATE_EXPECT = $null
    $env:MINICLAW_DISABLE_SANDBOX = '1'
    try { Run-Test 'miniclaw_server' 'lib' $tests.GateSwitchedOff -Ignored }
    finally { $env:MINICLAW_DISABLE_SANDBOX = $null }
    foreach ($filter in $tests.LibraryIgnored) { Run-Test 'miniclaw_server' 'lib' $filter -Ignored }
    # Language runtimes; a test whose input this runner did not provide is named, not run.
    if (!$env:MINICLAW_TEST_NODE) { $env:MINICLAW_TEST_NODE = (Get-Command node.exe).Source }
    foreach ($entry in $tests.Runtimes) {
        $needs, $test = $entry -split ' ', 2
        if ($needs -eq '-' -or [Environment]::GetEnvironmentVariable($needs)) { Run-Test 'windows_runtimes' 'test' $test -Ignored }
        else { Write-Warning "Not run: $test needs $needs" }
    }

    & $shell -NoProfile -ExecutionPolicy Bypass -File scripts/windows-native-standard-user.ps1 -Backend $backend -LibraryTests $unitBinary -BrokerTests (Test-Binary 'windows_broker' 'test') -AclTests (Test-Binary 'windows_acl_inheritance' 'test') -NetworkTests (Test-Binary 'windows_network' 'test') -RuntimesTests (Test-Binary 'windows_runtimes' 'test') -ExpectedMachine $machine
    if ($LASTEXITCODE -ne 0) { throw "Native acceptance command failed: $LASTEXITCODE" }

    # Winsock catalog entries and networked runs.
    Run-Test 'windows_winsock_acl' -Ignored
    Run-Test 'windows_network' -Ignored
    $passed = $true
} finally {
    try {
        if ($serviceId -and (Get-Service -Name "MiniClawNetwork-$serviceId" -ErrorAction SilentlyContinue)) {
            $preserve = @()
            if (!$passed) { $preserve = @('-PreserveInstallation') }
            & $shell -NoProfile -ExecutionPolicy Bypass -File scripts/windows-ci-stop-network.ps1 -Id $serviceId @preserve
            if ($LASTEXITCODE -ne 0) { throw 'Preserving native test state because clean broker shutdown was not proven' }
        }
        if ($data -and (Test-Path -LiteralPath $data)) {
            if ($passed) { Remove-Item -LiteralPath $data -Recurse -Force }
            else { Write-Warning "Native acceptance failed; preserving private recovery state at $data" }
        }
    } finally {
        $env:MINICLAW_DATA_DIR = $oldData
        $env:MINICLAW_NETWORK_CONFIG = $oldNetwork
        $env:MINICLAW_OPENSSL_SEED_SHA256 = $null
        Pop-Location
    }
}
