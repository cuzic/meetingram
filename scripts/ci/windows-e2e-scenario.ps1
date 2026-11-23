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
    return $status
}

function Assert-SessionTranscript {
    param([string]$SessionId)

    if ([string]::IsNullOrEmpty($SessionId)) { throw 'Stopped status did not report a session ID' }
    $database = Join-Path $env:APPDATA 'meetingram/session-store.sqlite3'
    @'
import sqlite3
import sys
from pathlib import Path

uri = Path(sys.argv[1]).resolve().as_uri() + "?mode=ro"
try:
    with sqlite3.connect(uri, uri=True) as connection:
        transcripts = connection.execute(
            "SELECT count(*) FROM transcript_segments WHERE session_id = ? AND is_final = 1",
            (sys.argv[2],),
        ).fetchone()[0]
        gaps = connection.execute(
            "SELECT count(*) FROM transcription_gaps WHERE session_id = ?", (sys.argv[2],)
        ).fetchone()[0]
except sqlite3.Error:
    sys.exit("Could not query the session database")

print(f"Final transcript rows: {transcripts}, transcription gaps: {gaps}")
sys.exit(0 if transcripts > 0 and gaps == 0 else 1)
'@ | python - $database $SessionId
    if ($LASTEXITCODE -ne 0) { throw 'Stopped session failed the transcript or gap assertion' }
}

function Invoke-MeetingramScenario {
    param([string]$Ctl, [string]$Output, [scriptblock]$PlayTone)

    Save-MeetingramStatus $Ctl (Join-Path $Output 'status-startup.json') $false | Out-Null
    Show-MeetingramWindow
    Start-Sleep -Seconds 2
    Save-Screenshot 'startup.png'

    Invoke-MeetingramCtl $Ctl start | Out-Null
    $recordingTime = [System.Diagnostics.Stopwatch]::StartNew()
    & $PlayTone
    do {
        Start-Sleep -Seconds 1
        $status = Save-MeetingramStatus $Ctl (Join-Path $Output 'status-recording.json') $true
        $connected = ($status.transcription_status.self_status -ceq 'Connected' -and
                      $status.transcription_status.remote_status -ceq 'Connected')
    } until ($connected -or $recordingTime.Elapsed.TotalSeconds -ge 20)
    if (-not $connected) { throw 'Both scripted STT tracks must report Connected during recording' }
    Write-Host 'Both scripted STT tracks connected'
    Start-Sleep -Seconds 12
    Save-Screenshot 'recording.png'
    while ($recordingTime.Elapsed.TotalSeconds -lt 100) {
        Start-Sleep -Seconds 1
    }

    Invoke-MeetingramCtl $Ctl stop | Out-Null
    $stopped = Save-MeetingramStatus $Ctl (Join-Path $Output 'status-stopped.json') $false
    Start-Sleep -Seconds 4
    Save-Screenshot 'stopped.png'
    Assert-SessionTranscript $stopped.last_session_id

    # Core v0.11 creates the session row after live transcription ends, so a
    # gap insert could fail before it becomes visible to the database check.
    $log = Join-Path $env:APPDATA 'meetingram/app.log'
    if (Select-String -LiteralPath $log -SimpleMatch 'live transcription: failed to record gap start' -Quiet) {
        throw 'Live transcription reported a gap that could not be recorded'
    }
}
