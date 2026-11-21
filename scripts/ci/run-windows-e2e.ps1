$ErrorActionPreference = 'Stop'
. "$PSScriptRoot/windows-screen-capture.ps1"
. "$PSScriptRoot/windows-e2e-scenario.ps1"

$ctl = (Resolve-Path 'target/x86_64-pc-windows-msvc/debug/meetingramctl.exe').Path
$output = Join-Path $env:RUNNER_TEMP 'e2e-windows-recording'
$progress = Join-Path $output 'capture-progress.txt'
$tone = Join-Path $env:RUNNER_TEMP 'meetingram-tone.wav'
$sessions = Join-Path $env:APPDATA 'meetingram/sessions'
$existingSessions = @(Get-ChildItem -LiteralPath $sessions -Directory -ErrorAction SilentlyContinue)
& "$PSScriptRoot/new-e2e-tone.ps1" -Path $tone
$player = [System.Media.SoundPlayer]::new($tone)

Add-Type -AssemblyName System.Windows.Forms
$screen = [System.Windows.Forms.SystemInformation]::VirtualScreen
Write-Host "Desktop resolution: $($screen.Width)x$($screen.Height)"

Push-Location $output
try {
    $player.Load()
    $recorder = Start-ScreenRecording $output
    try {
        Wait-ScreenRecording $recorder $progress
        Invoke-MeetingramScenario -Ctl $ctl -Output $output -PlayTone { $player.Play() }
    } finally {
        Stop-ScreenRecording $recorder
    }

    $newSessions = @(Get-ChildItem -LiteralPath $sessions -Directory |
        Where-Object { $_.Name -notin $existingSessions.Name })
    if ($newSessions.Count -eq 0) { throw 'No new meetingram session directory was created' }
    Write-Host "New meetingram session directories: $($newSessions.Count)"
} finally {
    Pop-Location
    $player.Dispose()
    Remove-Item -LiteralPath $progress, $tone -Force -ErrorAction SilentlyContinue
}
