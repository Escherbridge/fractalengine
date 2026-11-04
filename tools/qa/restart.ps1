# Restart one owned run without replacing its persisted dataset; see AGENTS.md.
[CmdletBinding()]
param(
    [Parameter(Mandatory)][ValidateNotNullOrEmpty()][string]$Run,
    [string]$Stage,
    [ValidateRange(1,65535)][int]$ApiPort,
    [switch]$DisableSyntheticInput,
    [switch]$A11yTiming,
    [switch]$A11yContinuous,
    [switch]$NativeObserver,
    [switch]$AllowStaleStage
)
$launchOptions = @{}
foreach ($key in $PSBoundParameters.Keys) {
    if ($key -eq 'Run') { $launchOptions.ResumeRun = $Run }
    else { $launchOptions[$key] = $PSBoundParameters[$key] }
}
& "$PSScriptRoot\launch.ps1" @launchOptions
