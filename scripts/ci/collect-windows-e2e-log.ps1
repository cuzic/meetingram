$ErrorActionPreference = 'Stop'
. "$PSScriptRoot/hide-windows-runner-details.ps1"

$logPath = Join-Path $env:APPDATA 'meetingram/app.log'
if (-not (Test-Path -LiteralPath $logPath)) { throw 'meetingram app.log was not created' }
$log = Get-Content -LiteralPath $logPath -Raw
if ($null -eq $log) { $log = '' }
$log = Hide-WindowsRunnerDetails $log

$output = Join-Path $env:RUNNER_TEMP 'e2e-windows-recording/app.log'
Set-Content -LiteralPath $output -Value $log -Encoding utf8 -NoNewline
Write-Host $log
