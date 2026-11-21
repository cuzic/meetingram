# Seed microphone consent for the meetingram Windows E2E runner.
# The ConsentStore registry entries back the desktop microphone privacy toggle.

$ErrorActionPreference = "Continue"

# Both HKLM (machine-wide) and HKCU (the runner process's own user) are written,
# since it's not confirmed which one this runner image's effective policy reads —
# cheap to set both rather than guess.
$paths = @(
    "HKLM:\SOFTWARE\Microsoft\Windows\CurrentVersion\CapabilityAccessManager\ConsentStore\microphone",
    "HKLM:\SOFTWARE\Microsoft\Windows\CurrentVersion\CapabilityAccessManager\ConsentStore\microphone\NonPackaged",
    "HKCU:\Software\Microsoft\Windows\CurrentVersion\CapabilityAccessManager\ConsentStore\microphone",
    "HKCU:\Software\Microsoft\Windows\CurrentVersion\CapabilityAccessManager\ConsentStore\microphone\NonPackaged"
)

foreach ($path in $paths) {
    try {
        New-Item -Path $path -Force -ErrorAction Stop | Out-Null
        Set-ItemProperty -Path $path -Name "Value" -Value "Allow" -Type String -Force -ErrorAction Stop
        Write-Host "Set microphone consent Allow at ${path}"
    } catch {
        Write-Host "Failed to seed ${path}"
    }
}

Write-Host "Microphone consent seeding attempted. Actual effectiveness is only known when meetingram opens its capture stream."
