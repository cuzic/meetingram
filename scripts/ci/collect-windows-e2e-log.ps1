$ErrorActionPreference = 'Stop'
$logPath = Join-Path $env:APPDATA 'meetingram/app.log'
if (-not (Test-Path -LiteralPath $logPath)) { throw 'meetingram app.log was not created' }
$log = Get-Content -LiteralPath $logPath -Raw
if ($null -eq $log) { $log = '' }
foreach ($value in @($env:USERPROFILE, $env:GITHUB_WORKSPACE, $env:RUNNER_TEMP,
                    $env:CARGO_HOME, $env:COMPUTERNAME, $env:USERNAME)) {
    if ($value) {
        foreach ($form in @($value, $value.Replace('\', '/'), $value.Replace('\', '\\')) | Select-Object -Unique) {
            $log = $log -replace [regex]::Escape($form), '[runner]'
        }
    }
}

$output = Join-Path $env:RUNNER_TEMP 'e2e-windows-recording/app.log'
Set-Content -LiteralPath $output -Value $log -Encoding utf8 -NoNewline
Write-Host $log
