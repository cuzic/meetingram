. "$PSScriptRoot/hide-windows-runner-details.ps1"

function Invoke-MeetingramCtl {
    param([string]$Ctl, [ValidateSet('start', 'stop', 'status')][string]$Command)

    $settings = [System.Diagnostics.ProcessStartInfo]::new($Ctl)
    $settings.UseShellExecute = $false
    $settings.RedirectStandardOutput = $true
    $settings.RedirectStandardError = $true
    $settings.ArgumentList.Add('--json')
    $settings.ArgumentList.Add($Command)
    $process = [System.Diagnostics.Process]::Start($settings)
    try {
        # The auto-launched GUI may inherit the pipes. Read one JSON line
        # and wait only for meetingramctl itself, without waiting for pipe EOF.
        $line = $process.StandardOutput.ReadLineAsync()
        if (-not $process.WaitForExit(30000)) {
            $process.Kill()
            throw "meetingramctl $Command timed out"
        }
        if ($process.ExitCode -ne 0) {
            throw "meetingramctl $Command failed with exit code $($process.ExitCode)"
        }
        return $line.WaitAsync([TimeSpan]::FromSeconds(5)).GetAwaiter().GetResult()
    } finally {
        $process.StandardOutput.Dispose()
        $process.StandardError.Dispose()
        $process.Dispose()
    }
}

function Save-MeetingramStatus {
    param([string]$Ctl, [string]$Path, [bool]$Recording)

    $json = Invoke-MeetingramCtl $Ctl status
    Hide-WindowsRunnerDetails $json | Set-Content -LiteralPath $Path -Encoding utf8
    $status = ConvertFrom-Json -InputObject $json
    if (($status.recording -isnot [bool]) -or ($status.recording -ne $Recording)) {
        throw "Expected meetingramctl status to report recording:$($Recording.ToString().ToLowerInvariant())"
    }
}

function Invoke-MeetingramScenario {
    param([string]$Ctl, [string]$Output, [scriptblock]$PlayTone)

    Save-MeetingramStatus $Ctl (Join-Path $Output 'status-startup.json') $false
    Show-MeetingramWindow
    Start-Sleep -Seconds 2
    Save-Screenshot 'startup.png'

    Invoke-MeetingramCtl $Ctl start | Out-Null
    Start-Sleep -Seconds 2
    Save-MeetingramStatus $Ctl (Join-Path $Output 'status-recording.json') $true
    & $PlayTone
    Start-Sleep -Seconds 5
    Save-Screenshot 'recording.png'
    Start-Sleep -Seconds 17

    Invoke-MeetingramCtl $Ctl stop | Out-Null
    Save-MeetingramStatus $Ctl (Join-Path $Output 'status-stopped.json') $false
    Start-Sleep -Seconds 4
    Save-Screenshot 'stopped.png'
}
