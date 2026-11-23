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
    param([string]$SessionId, [int]$RecordingSeconds = 0)

    if ([string]::IsNullOrEmpty($SessionId)) { throw 'Status did not report a session ID' }
    $database = Join-Path $env:APPDATA 'meetingram/session-store.sqlite3'
    @'
import json
import sqlite3
import sys
import tomllib
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
recording_seconds = int(sys.argv[3])
if recording_seconds == 0:
    sys.exit(0 if transcripts > 0 else "No final transcript rows during recording")

scenario = Path(sys.argv[4])
speed = tomllib.loads((scenario / "scenario.toml").read_text(encoding="utf-8"))["speed"]
with (scenario / "utterances.jsonl").open(encoding="utf-8") as utterances:
    end_times = [json.loads(row)["end_ms"] // speed for row in utterances if row.strip()]
expected = len(end_times)
# The playback clock starts after the recording stopwatch. Count the full
# script and require spare time for the scripted providers to connect.
if not end_times or max(end_times) + 5_000 > recording_seconds * 1000:
    sys.exit("Scripted playback must finish at least five seconds before stop")
print(f"Expected final transcript rows: {expected}")
sys.exit(0 if expected > 0 and transcripts == expected and gaps == 0 else 1)
'@ | python - $database $SessionId $RecordingSeconds $env:MEETINGRAM_E2E_SCENARIO
    if ($LASTEXITCODE -ne 0) { throw 'Session failed the transcript or gap assertion' }
}

function Read-TourSteps {
    @'
import sys
import tomllib
from pathlib import Path

scenario = Path(sys.argv[1]) / "scenario.toml"
for index, step in enumerate(tomllib.loads(scenario.read_text(encoding="utf-8"))["tour"], 1):
    print(f"{index} {step['name']} {step['action']}")
'@ | python - $env:MEETINGRAM_E2E_SCENARIO
    if ($LASTEXITCODE -ne 0) { throw 'Could not read tour steps' }
}

function Capture-TourSteps {
    param([hashtable]$Tour)

    while ($Tour.Next -lt $Tour.Steps.Count) {
        $number, $name, $action = $Tour.Steps[$Tour.Next] -split ' '
        $stem = "step-$number-$name"
        if (-not (Test-Path -LiteralPath (Join-Path $Tour.Output "$stem.ready") -PathType Leaf)) {
            break
        }
        Start-Sleep -Milliseconds 250
        Save-Screenshot "$stem.png"
        New-Item -ItemType File -Path (Join-Path $Tour.Output "step-$number.shot") | Out-Null
        $Tour.Next++
        $Tour.Completed[$action] = $true
        Write-Host "Tour screenshot: $stem.png"
    }
}

function Wait-TourAction {
    param([hashtable]$Tour, [string]$Action, [int]$TimeoutSeconds)

    $clock = [System.Diagnostics.Stopwatch]::StartNew()
    do {
        Capture-TourSteps $Tour
        if ($Tour.Completed.ContainsKey($Action)) { return }
        Start-Sleep -Milliseconds 250
    } while ($clock.Elapsed.TotalSeconds -lt $TimeoutSeconds)
    throw "Tour did not reach $Action"
}

function Get-LiveAppliedRevision {
    param([string]$SessionId)

    $database = Join-Path $env:APPDATA 'meetingram/session-store.sqlite3'
    $revision = @'
import sqlite3
import sys
from pathlib import Path

uri = Path(sys.argv[1]).resolve().as_uri() + "?mode=ro"
try:
    with sqlite3.connect(uri, uri=True) as connection:
        result = connection.execute(
            "SELECT COALESCE(MAX(rev), 0) FROM graph_patches "
            "WHERE session_id = ? AND source = 'live' AND json_array_length(ops_json) > 0",
            (sys.argv[2],),
        ).fetchone()[0]
except sqlite3.Error:
    sys.exit("Could not read the applied live graph revision")
print(result)
'@ | python - $database $SessionId
    if ($LASTEXITCODE -ne 0) { throw 'Could not check the applied live graph revision' }
    return [long]$revision
}

function Assert-LiveGraph {
    param($Status, [string]$Log, [long]$AppliedRevision)

    $failures = @()
    if (-not $Status.graph.enabled -or $Status.graph.rev -lt 1) {
        $failures += 'Recording graph.rev did not advance'
    }
    # The core also assigns revisions to empty patches that mark utterances extracted.
    if ($AppliedRevision -lt 1) { $failures += 'Recording graph.rev advanced without any applied live graph changes' }
    if (-not (Select-String -LiteralPath $Log -Pattern 'scripted LLM.*kind="incremental".*matched=[1-9][0-9]*' -Quiet)) {
        $failures += 'Scripted LLM did not match any new utterances'
    }
    if ($failures.Count) { throw ($failures -join '. ') }
}

function Invoke-MeetingramScenario {
    param([string]$Ctl, [string]$Output, [scriptblock]$PlayTone)

    $tour = @{ Steps = @(Read-TourSteps); Next = 0; Output = $Output; Completed = @{} }
    $log = Join-Path $env:APPDATA 'meetingram/app.log'
    Save-MeetingramStatus $Ctl (Join-Path $Output 'status-startup.json') $false | Out-Null
    Show-MeetingramWindow
    Start-Sleep -Seconds 2
    Save-Screenshot 'startup.png'

    Invoke-MeetingramCtl $Ctl start | Out-Null
    $recordingTime = [System.Diagnostics.Stopwatch]::StartNew()
    & $PlayTone
    do {
        Capture-TourSteps $tour
        Start-Sleep -Seconds 1
        $status = Save-MeetingramStatus $Ctl (Join-Path $Output 'status-recording.json') $true
        $connected = ($status.transcription_status.self_status -ceq 'Connected' -and
                      $status.transcription_status.remote_status -ceq 'Connected')
    } until ($connected -or $recordingTime.Elapsed.TotalSeconds -ge 20)
    if (-not $connected) { throw 'Both scripted STT tracks must report Connected during recording' }
    Write-Host 'Both scripted STT tracks connected'

    while ($recordingTime.Elapsed.TotalSeconds -lt 12) {
        Capture-TourSteps $tour
        Start-Sleep -Milliseconds 250
    }
    Save-Screenshot 'recording.png'
    while ($recordingTime.Elapsed.TotalSeconds -lt 50) {
        Capture-TourSteps $tour
        Start-Sleep -Milliseconds 250
    }
    $active = Save-MeetingramStatus $Ctl (Join-Path $Output 'status-recording-mid.json') $true
    Assert-SessionTranscript $active.last_session_id
    $appliedRevision = Get-LiveAppliedRevision $active.last_session_id
    Write-Host "Latest applied live graph revision: $appliedRevision"
    Assert-LiveGraph $active $log $appliedRevision

    $recordingSeconds = 100
    while ($recordingTime.Elapsed.TotalSeconds -lt $recordingSeconds) {
        Capture-TourSteps $tour
        Start-Sleep -Milliseconds 250
    }
    if (-not $tour.Completed.ContainsKey('live_evidence')) { throw 'Live evidence tour step was missing' }

    Invoke-MeetingramCtl $Ctl stop | Out-Null
    Wait-TourAction $tour 'consolidated' 75
    $stopped = Save-MeetingramStatus $Ctl (Join-Path $Output 'status-stopped.json') $false
    Save-Screenshot 'stopped.png'
    if ($stopped.last_session_id -ne $active.last_session_id) {
        throw 'Stopped status reported a different session ID'
    }
    if (-not $stopped.graph.consolidated -or $stopped.graph.rev -lt 1) {
        throw 'Stopped graph did not reach consolidation'
    }
    Assert-SessionTranscript $stopped.last_session_id $recordingSeconds

    # The app acknowledges rebuild_started only after observing the revision drop
    # and rebuild_complete only after observing a new consolidated revision.
    Wait-TourAction $tour 'rebuild_complete' 120
    $rebuilt = Save-MeetingramStatus $Ctl (Join-Path $Output 'status-rebuilt.json') $false
    if (-not $rebuilt.graph.consolidated -or $rebuilt.graph.rev -lt 1) {
        throw 'Rebuilt graph did not reach consolidation'
    }

    Invoke-MeetingramCtl $Ctl start | Out-Null
    & $PlayTone
    Start-Sleep -Seconds 30
    Invoke-MeetingramCtl $Ctl stop | Out-Null
    Wait-TourAction $tour 'second_session' 75
    if ($tour.Next -ne $tour.Steps.Count) { throw 'Tour did not capture every step' }
    $second = Save-MeetingramStatus $Ctl (Join-Path $Output 'status-second.json') $false
    if ($second.last_session_id -eq $stopped.last_session_id -or -not $second.graph.consolidated) {
        throw 'Second session did not report a distinct consolidated graph'
    }

    $gapNotices = @(Select-String -LiteralPath $log -SimpleMatch 'gap retranscription disabled under e2e scenario')
    if ($gapNotices.Count -ne 1) { throw 'Expected one startup notice that gap retranscription is disabled' }

    $publishFailures = @(Select-String -LiteralPath $log -SimpleMatch 'failed to publish SegmentFinalized')
    Write-Host "SegmentFinalized publish failures: $($publishFailures.Count)"
    if ($publishFailures.Count -ne 0) { throw 'Recording reported a SegmentFinalized publish failure' }
    if (Select-String -LiteralPath $log -SimpleMatch 'failed to persist' -Quiet) {
        throw 'Recording reported a persistence failure'
    }
    if (Select-String -LiteralPath $log -SimpleMatch 'live transcription: failed to record gap start' -Quiet) {
        throw 'Live transcription reported a gap that could not be recorded'
    }
    foreach ($decision in 'approve', 'reject') {
        $saved = @(Select-String -LiteralPath $log -Pattern ('scripted graph review saved.*decision="' + $decision + '"'))
        if ($saved.Count -ne 1) { throw "Expected exactly one saved $decision review" }
    }
    $clock = [System.Diagnostics.Stopwatch]::StartNew()
    do {
        $completed = @(Select-String -LiteralPath $log -Pattern 'scripted tour.*result="ok"')
        if ($completed.Count -eq $tour.Steps.Count) { return }
        Start-Sleep -Milliseconds 100
    } while ($clock.Elapsed.TotalSeconds -lt 5)
    throw 'Expected one successful log entry per tour step'
}
