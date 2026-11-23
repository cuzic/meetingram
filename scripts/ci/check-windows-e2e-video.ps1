$ErrorActionPreference = 'Stop'
$output = Join-Path $env:RUNNER_TEMP 'e2e-windows-recording'
$video = Join-Path $output 'desktop.mp4'
if (-not (Test-Path -LiteralPath $video -PathType Leaf) -or (Get-Item -LiteralPath $video).Length -eq 0) {
    throw 'Desktop MP4 is missing or empty'
}
$json = ffprobe -v error -select_streams v:0 -show_entries 'stream=codec_name,pix_fmt,width,height:format=duration' -of json $video
if ($LASTEXITCODE -ne 0) { throw "ffprobe failed with exit code $LASTEXITCODE" }
$info = ConvertFrom-Json -InputObject ($json -join "`n")
$stream = $info.streams | Select-Object -First 1
$duration = [double]$info.format.duration
Write-Host "Video: $($stream.codec_name), $($stream.pix_fmt), $($stream.width)x$($stream.height), ${duration}s"
if ($stream.codec_name -ne 'h264' -or $stream.pix_fmt -ne 'yuv420p' -or $duration -le 0 -or $duration -gt 600) {
    throw 'Expected an H.264 yuv420p video of up to 600 seconds'
}
ffmpeg -v error -xerror -i $video -map 0:v:0 -f null -
if ($LASTEXITCODE -ne 0) { throw "MP4 could not be decoded: exit code $LASTEXITCODE" }
. "$PSScriptRoot/windows-e2e-scenario.ps1"
$env:MEETINGRAM_E2E_SCENARIO = (Resolve-Path 'apps/e2e-scripted/scenarios/design').Path
$names = @('startup.png', 'recording.png', 'stopped.png')
foreach ($step in (Read-TourSteps)) {
    $number, $name, $action = $step -split ' '
    $stem = "step-$number-$name"
    if (-not (Test-Path -LiteralPath (Join-Path $output "$stem.ready") -PathType Leaf)) {
        throw "Tour step $stem did not signal readiness"
    }
    $names += "$stem.png"
}
foreach ($name in $names) {
    $image = Join-Path $output $name
    if (-not (Test-Path -LiteralPath $image -PathType Leaf) -or (Get-Item -LiteralPath $image).Length -eq 0) {
        throw "Screenshot $name is missing or empty"
    }
}
