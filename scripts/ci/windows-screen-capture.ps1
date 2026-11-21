function Start-ScreenRecording {
    param([string]$Output)

    $ffmpeg = Get-Command ffmpeg -CommandType Application | Select-Object -First 1
    $settings = [System.Diagnostics.ProcessStartInfo]::new($ffmpeg.Source)
    $settings.WorkingDirectory = $Output
    $settings.UseShellExecute = $false
    $settings.RedirectStandardInput = $true
    $settings.CreateNoWindow = $true
    foreach ($argument in @(
        '-hide_banner', '-loglevel', 'error', '-nostats', '-progress', 'capture-progress.txt',
        '-f', 'gdigrab', '-framerate', '10', '-i', 'desktop', '-t', '180',
        '-vf', 'pad=ceil(iw/2)*2:ceil(ih/2)*2', '-c:v', 'libx264', '-preset', 'ultrafast',
        '-pix_fmt', 'yuv420p', '-y', 'desktop.mp4'
    )) {
        $settings.ArgumentList.Add($argument)
    }
    return [System.Diagnostics.Process]::Start($settings)
}

function Wait-ScreenRecording {
    param([System.Diagnostics.Process]$Recorder, [string]$ProgressPath)

    $timer = [System.Diagnostics.Stopwatch]::StartNew()
    while ($timer.Elapsed.TotalSeconds -lt 15) {
        if ($Recorder.HasExited) { throw "ffmpeg exited during startup with code $($Recorder.ExitCode)" }
        if (Test-Path -LiteralPath $ProgressPath) {
            if (Select-String -LiteralPath $ProgressPath -Pattern '^frame=[1-9][0-9]*$' -Quiet) { return }
        }
        Start-Sleep -Milliseconds 250
    }
    throw 'ffmpeg did not capture a desktop frame within 15 seconds'
}

function Stop-ScreenRecording {
    param([System.Diagnostics.Process]$Recorder)

    try {
        if (-not $Recorder.HasExited) {
            try {
                $Recorder.StandardInput.WriteLine('q')
                $Recorder.StandardInput.Flush()
            } catch {
                Write-Host 'Could not send q to ffmpeg. Waiting for its recording limit.'
            }
        }
        if (-not $Recorder.WaitForExit(190000)) { throw 'ffmpeg did not finish before its recording deadline' }
        if ($Recorder.ExitCode -ne 0) { throw "ffmpeg recording failed with exit code $($Recorder.ExitCode)" }
    } finally {
        $Recorder.Dispose()
    }
}

function Save-Screenshot {
    param([string]$Name)

    ffmpeg -hide_banner -loglevel error -f gdigrab -i desktop -frames:v 1 -update 1 -y $Name
    if ($LASTEXITCODE -ne 0) { throw "Could not capture $Name" }
}

function Show-MeetingramWindow {
    $shell = New-Object -ComObject WScript.Shell
    Add-Type -Name Foreground -Namespace MeetingramCI -MemberDefinition @'
[System.Runtime.InteropServices.DllImport("user32.dll")]
public static extern System.IntPtr GetForegroundWindow();
'@
    for ($attempt = 0; $attempt -lt 20; $attempt++) {
        $app = Get-Process -Name desktop -ErrorAction SilentlyContinue | Select-Object -First 1
        if ($app -and $app.MainWindowHandle -ne 0) {
            $null = $shell.AppActivate($app.Id)
            if ([MeetingramCI.Foreground]::GetForegroundWindow() -eq $app.MainWindowHandle) {
                Write-Host 'meetingram window is in the foreground'
                return
            }
        }
        Start-Sleep -Milliseconds 250
    }
    throw 'Could not bring the meetingram window to the foreground'
}
