<#
Weekly USGWD source check as a Windows scheduled task.

  powershell -File scripts/schedule-usgwd-check.ps1 Register [-Day Monday] [-Time 07:00]
  powershell -File scripts/schedule-usgwd-check.ps1 Status
  powershell -File scripts/schedule-usgwd-check.ps1 RunNow
  powershell -File scripts/schedule-usgwd-check.ps1 Remove

The task runs only `acquire-usgwd.py check`: two small listing requests to
HydroShare, compared with the pinned release. It never downloads well data,
rebuilds maps or publishes. Results land in .temp-gev/usgwd/status/:
  source-check.json   latest result (outcome: pinned-data-unchanged,
                      review-needed or check-failed)
  history.log         one line per run
Runs as the current user, only when logged on, no stored password.
#>
param(
    [Parameter(Position = 0, Mandatory)][ValidateSet('Register', 'Status', 'Run', 'RunNow', 'Remove')][string]$Action,
    [ValidateSet('Monday', 'Tuesday', 'Wednesday', 'Thursday', 'Friday', 'Saturday', 'Sunday')][string]$Day = 'Monday',
    [string]$Time = '07:00',
    [string]$Release = '.temp-gev/usgwd/release-2d173fb3.json'
)
$ErrorActionPreference = 'Stop'
$TaskName = 'Rainfolk USGWD source check'
$Repo = (Resolve-Path (Join-Path $PSScriptRoot '..')).Path
$Python = (Get-Command python -ErrorAction Stop).Source

switch ($Action) {
    'Register' {
        if (-not (Test-Path (Join-Path $Repo $Release))) { throw "Pinned release not found: $Release" }
        $taskAction = New-ScheduledTaskAction -Execute 'powershell.exe' -WorkingDirectory $Repo `
            -Argument "-NoProfile -NonInteractive -WindowStyle Hidden -ExecutionPolicy Bypass -File `"$PSCommandPath`" Run -Release `"$Release`""
        $trigger = New-ScheduledTaskTrigger -Weekly -DaysOfWeek $Day -At $Time
        $settings = New-ScheduledTaskSettingsSet -StartWhenAvailable -ExecutionTimeLimit (New-TimeSpan -Minutes 10) -MultipleInstances IgnoreNew
        $principal = New-ScheduledTaskPrincipal -UserId "$env:USERDOMAIN\$env:USERNAME" -LogonType Interactive -RunLevel Limited
        Register-ScheduledTask -TaskName $TaskName -Action $taskAction -Trigger $trigger -Settings $settings -Principal $principal `
            -Description 'Checks the USGWD HydroShare file list against the pinned Rainfolk release. Read-only; no downloads.' | Out-Null
        "Registered '$TaskName': every $Day at $Time."
    }
    'Status' {
        $task = Get-ScheduledTask -TaskName $TaskName -ErrorAction SilentlyContinue
        if (-not $task) { 'Not registered.'; break }
        $info = $task | Get-ScheduledTaskInfo
        "State: $($task.State)  Last run: $($info.LastRunTime)  Last result: $($info.LastTaskResult)  Next run: $($info.NextRunTime)"
        $status = Join-Path $Repo '.temp-gev/usgwd/status/source-check.json'
        if (Test-Path $status) { $s = Get-Content $status -Raw | ConvertFrom-Json; "Latest check: $($s.checkedAt) -> $($s.outcome)" }
    }
    'Run' {
        # What the scheduled task executes.
        Set-Location -LiteralPath $Repo
        $statusDir = Join-Path $Repo '.temp-gev/usgwd/status'
        New-Item -ItemType Directory -Force $statusDir | Out-Null
        & $Python -B scripts/acquire-usgwd.py check $Release --status (Join-Path $statusDir 'source-check.json') | Out-Null
        $code = $LASTEXITCODE
        $outcome = try { (Get-Content (Join-Path $statusDir 'source-check.json') -Raw | ConvertFrom-Json).outcome } catch { 'no-status' }
        Add-Content (Join-Path $statusDir 'history.log') ('{0:o} exit={1} {2}' -f (Get-Date), $code, $outcome)
        exit $code
    }
    'RunNow' { Start-ScheduledTask -TaskName $TaskName; 'Started.' }
    'Remove' { Unregister-ScheduledTask -TaskName $TaskName -Confirm:$false; 'Removed.' }
}
