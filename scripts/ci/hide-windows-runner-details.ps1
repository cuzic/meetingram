function Hide-WindowsRunnerDetails {
    param([string]$Text)

    foreach ($value in @($env:USERPROFILE, $env:GITHUB_WORKSPACE, $env:RUNNER_TEMP,
                        $env:CARGO_HOME, $env:COMPUTERNAME, $env:USERNAME)) {
        if ($value) {
            foreach ($form in @($value, $value.Replace('\', '/'), $value.Replace('\', '\\')) | Select-Object -Unique) {
                $Text = $Text -replace [regex]::Escape($form), '[runner]'
            }
        }
    }
    return $Text
}
