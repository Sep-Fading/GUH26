param([Parameter(Mandatory=$true)][string]$HelperPath)
$ErrorActionPreference = 'Stop'
$signature = Get-AuthenticodeSignature -LiteralPath $HelperPath
@{ valid = ($signature.Status -eq 'Valid'); subject = $signature.SignerCertificate.Subject; issuer = $signature.SignerCertificate.Issuer } | ConvertTo-Json -Compress
