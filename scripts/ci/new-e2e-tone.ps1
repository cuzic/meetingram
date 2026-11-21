param([Parameter(Mandatory)][string]$Path)

$sampleRate = 16000
$sampleCount = 24 * $sampleRate
$dataSize = $sampleCount * 2
$encoding = [System.Text.Encoding]::ASCII
$writer = [System.IO.BinaryWriter]::new([System.IO.File]::Create($Path))
try {
    $writer.Write($encoding.GetBytes('RIFF'))
    $writer.Write([int](36 + $dataSize))
    $writer.Write($encoding.GetBytes('WAVEfmt '))
    $writer.Write([int]16)
    $writer.Write([int16]1) # PCM
    $writer.Write([int16]1) # mono
    $writer.Write([int]$sampleRate)
    $writer.Write([int](2 * $sampleRate))
    $writer.Write([int16]2)
    $writer.Write([int16]16)
    $writer.Write($encoding.GetBytes('data'))
    $writer.Write([int]$dataSize)
    for ($i = 0; $i -lt $sampleCount; $i++) {
        # Two seconds at 440 Hz, followed by one second of silence.
        $sample = if (($i % (3 * $sampleRate)) -lt (2 * $sampleRate)) {
            8000 * [Math]::Sin(2 * [Math]::PI * 440 * $i / $sampleRate)
        } else {
            0
        }
        $writer.Write([int16]$sample)
    }
} finally {
    $writer.Dispose()
}
