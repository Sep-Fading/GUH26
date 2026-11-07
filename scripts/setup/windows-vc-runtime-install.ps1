# Runs Microsoft's Visual C++ Redistributable with administrator rights, only after re-checking the exact
# bytes it runs: they are copied into a new folder that only SYSTEM and Administrators can change, hashed
# while copied, and the copy must carry a valid Microsoft Corporation signature. Exit codes: the
# installer's own, 536870913 (copy differs from the verified download), 536870914 (not signed by Microsoft).
param([Parameter(Mandatory=$true)][string]$Source, [Parameter(Mandatory=$true)][string]$ExpectedSha256)
$ErrorActionPreference = 'Stop'
$microsoft = '(^|,\s*)O=Microsoft Corporation(,|$)'
$security = New-Object System.Security.AccessControl.DirectorySecurity
$security.SetAccessRuleProtection($true, $false)
foreach ($sid in 'S-1-5-18', 'S-1-5-32-544') {
    $identity = New-Object System.Security.Principal.SecurityIdentifier($sid)
    $security.AddAccessRule((New-Object System.Security.AccessControl.FileSystemAccessRule($identity, 'FullControl', 'ContainerInherit,ObjectInherit', 'None', 'Allow')))
}
$folder = Join-Path $env:ProgramData ('MiniClawVcRuntime-' + [Guid]::NewGuid().ToString('N'))
if (Test-Path -LiteralPath $folder) { exit 536870913 }
# The folder is created with its protected access list in one call.
[void][IO.Directory]::CreateDirectory($folder, $security)
try {
    if (@(Get-ChildItem -LiteralPath $folder -Force).Count) { exit 536870913 }
    $item = Get-Item -LiteralPath $Source
    if ($item.PSIsContainer -or $item.Length -gt 64MB -or $item.Name -notmatch '^VC_redist\.(arm64|x64)\.exe$') { exit 536870913 }
    $bytes = [IO.File]::ReadAllBytes($Source)
    $sha = [Security.Cryptography.SHA256]::Create()
    try { $actual = [BitConverter]::ToString($sha.ComputeHash($bytes)).Replace('-', '').ToLowerInvariant() } finally { $sha.Dispose() }
    if ($actual -ne $ExpectedSha256) { exit 536870913 }
    $copy = Join-Path $folder $item.Name
    [IO.File]::WriteAllBytes($copy, $bytes)
    $signature = Get-AuthenticodeSignature -LiteralPath $copy
    $signer = $signature.SignerCertificate
    if ($signature.Status -ne 'Valid' -or !$signer -or $signer.Subject -notmatch $microsoft -or $signer.Issuer -notmatch $microsoft) { exit 536870914 }
    $process = Start-Process -FilePath $copy -ArgumentList '/install', '/quiet', '/norestart' -Wait -PassThru
    exit $process.ExitCode
} finally {
    Remove-Item -LiteralPath $folder -Recurse -Force -ErrorAction SilentlyContinue
}
