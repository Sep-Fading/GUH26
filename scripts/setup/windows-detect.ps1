# Lists where the Codex app is installed for this user, so setup can offer the Computer Use helper's path.
# Read-only: it takes no input, changes nothing and reads no credentials.
$ErrorActionPreference = 'SilentlyContinue'
$codex = @(Get-AppxPackage -Name 'OpenAI.Codex' | ForEach-Object {
  [ordered]@{ location = "$($_.InstallLocation)"; version = "$($_.Version)"; architecture = "$($_.Architecture)" } })
ConvertTo-Json -InputObject ([ordered]@{ codex = $codex }) -Depth 4 -Compress
