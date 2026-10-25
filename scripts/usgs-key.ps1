# Run locally in PowerShell; never pass the key as an argument or paste it in chat.
[CmdletBinding()]
param(
    [Parameter(Mandatory)][ValidateSet('Set', 'Run', 'Status', 'Verify')][string]$Action,
    [string]$SecretPath,
    [string[]]$JobArguments = @()
)
$ErrorActionPreference = 'Stop'
if ($env:OS -ne 'Windows_NT') { throw 'Windows DPAPI storage requires Windows.' }
$secretDirectory = Join-Path $env:LOCALAPPDATA 'Rainfolk'
if (-not $SecretPath) { $SecretPath = Join-Path $secretDirectory 'usgs-api-key.clixml' }
$secretDirectory = Split-Path -Parent $SecretPath
if ($Action -eq 'Status') {
    [pscustomobject]@{ ProtectedFilePresent = (Test-Path -LiteralPath $SecretPath) } | ConvertTo-Json -Compress
    return
}
if ($Action -eq 'Set') {
    if (Test-Path -LiteralPath $secretPath) { throw 'A stored key already exists; no overwrite performed.' }
    $secret = Read-Host 'Paste the USGS key locally (masked)' -AsSecureString
    if ($secret.Length -eq 0) { throw 'Empty key; nothing saved.' }
    New-Item -ItemType Directory -Path $secretDirectory -Force | Out-Null
    # Windows DPAPI encrypts SecureString for this Windows user on this computer.
    $secret | Export-Clixml -LiteralPath $secretPath -NoClobber
    Write-Output 'USGS key encrypted locally; value suppressed.'
    return
}
if (-not (Test-Path -LiteralPath $secretPath)) { throw 'Run this script with -Action Set first.' }
if ($Action -eq 'Run' -and $JobArguments.Count -eq 0) { throw 'Explicit well-jobs arguments required; no default collection.' }
$secret = Import-Clixml -LiteralPath $secretPath
if ($secret -isnot [Security.SecureString]) { throw 'Invalid protected secret file.' }
$previous = [Environment]::GetEnvironmentVariable('USGS_API_KEY', 'Process')
$pointer = [Runtime.InteropServices.Marshal]::SecureStringToBSTR($secret)
try {
    $env:USGS_API_KEY = [Runtime.InteropServices.Marshal]::PtrToStringBSTR($pointer)
    if ($Action -eq 'Verify') {
        & python -B (Join-Path $PSScriptRoot 'verify-usgs-key.py')
    } else {
        $executable = Join-Path $PSScriptRoot '..\crates\us-groundwater\target\debug\well-jobs.exe'
        & $executable @JobArguments
    }
    if ($LASTEXITCODE -ne 0) { throw 'USGS command failed; see sanitized error.' }
} finally {
    [Environment]::SetEnvironmentVariable('USGS_API_KEY', $previous, 'Process')
    [Runtime.InteropServices.Marshal]::ZeroFreeBSTR($pointer)
    $secret.Dispose()
}
