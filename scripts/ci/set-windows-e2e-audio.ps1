$ErrorActionPreference = 'Stop'
try {
    Install-PackageProvider -Name NuGet -MinimumVersion 2.8.5.201 -Force | Out-Null
} catch {
    Write-Host 'NuGet provider setup failed. Install-Module will retry provider setup.'
}
Set-PSRepository -Name PSGallery -InstallationPolicy Trusted
Install-Module -Name AudioDeviceCmdlets -Force -Scope CurrentUser
Import-Module AudioDeviceCmdlets

$devices = Get-AudioDevice -List
foreach ($endpoint in @(
    @{ Type = 'Recording'; Name = 'CABLE Output' }
    @{ Type = 'Playback'; Name = 'CABLE Input' }
)) {
    $device = $devices | Where-Object {
        $_.Type -eq $endpoint.Type -and $_.Name -like "*$($endpoint.Name)*"
    } | Select-Object -First 1
    if (-not $device) { throw "Missing audio endpoint $($endpoint.Name)" }
    Set-AudioDevice -ID $device.ID | Out-Null
    Write-Host "Default $($endpoint.Type) device set to $($endpoint.Name)"
}
