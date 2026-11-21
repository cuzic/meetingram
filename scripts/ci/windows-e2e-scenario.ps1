. "$PSScriptRoot/hide-windows-runner-details.ps1"

function Save-MeetingramStatus {
    param([string]$Ctl, [string]$Path, [bool]$Recording)

    $json = & $Ctl --json status 2>$null
    if ($LASTEXITCODE -ne 0) { throw "meetingramctl status failed with exit code $LASTEXITCODE" }
    Hide-WindowsRunnerDetails ($json -join "`n") | Set-Content -LiteralPath $Path -Encoding utf8
    $status = ConvertFrom-Json -InputObject ($json -join "`n")
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

    & $Ctl start 2>$null | Out-Null
    if ($LASTEXITCODE -ne 0) { throw "meetingramctl start failed with exit code $LASTEXITCODE" }
    Start-Sleep -Seconds 2
    Save-MeetingramStatus $Ctl (Join-Path $Output 'status-recording.json') $true
    & $PlayTone
    Start-Sleep -Seconds 5
    Save-Screenshot 'recording.png'
    Start-Sleep -Seconds 17

    & $Ctl stop 2>$null | Out-Null
    if ($LASTEXITCODE -ne 0) { throw "meetingramctl stop failed with exit code $LASTEXITCODE" }
    Save-MeetingramStatus $Ctl (Join-Path $Output 'status-stopped.json') $false
    Start-Sleep -Seconds 4
    Save-Screenshot 'stopped.png'
}
