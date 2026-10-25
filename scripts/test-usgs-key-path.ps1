$ErrorActionPreference = 'Stop'
$fixture = Join-Path $PSScriptRoot ('..\.temp-gev\key-path-test-' + [Guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $fixture | Out-Null
$explicit = Join-Path $fixture 'presence-only-fixture'
Set-Content -LiteralPath $explicit -Value 'synthetic presence fixture; no key'
$previous = $env:LOCALAPPDATA
try {
    $env:LOCALAPPDATA = Join-Path $fixture 'different-executor-profile'
    $result = & (Join-Path $PSScriptRoot 'usgs-key.ps1') -Action Status -SecretPath $explicit | ConvertFrom-Json
    if (-not $result.ProtectedFilePresent) { throw 'Explicit profile path ignored' }
    $default = & (Join-Path $PSScriptRoot 'usgs-key.ps1') -Action Status | ConvertFrom-Json
    if ($default.ProtectedFilePresent) { throw 'Default unexpectedly read interactive profile' }
    Write-Output 'PASS: explicit secret path works across differing LOCALAPPDATA; default remains isolated.'
} finally { $env:LOCALAPPDATA = $previous }
