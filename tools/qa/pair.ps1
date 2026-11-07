# Two-root owner/viewer wrapper over peers.ps1 (-Count 2); see AGENTS.md §peers.
[CmdletBinding()]
param(
    [Parameter(Mandatory)][ValidateSet('Start','Stop','Info')][string]$Action,
    [string]$OwnerRun,
    [string]$Stage,
    [ValidateRange(1024,65535)][int]$OwnerPort = 8882,
    [ValidateRange(1024,65535)][int]$ViewerPort = 8883,
    [switch]$AllowStaleStage,
    [switch]$NativeObserver
)
$ErrorActionPreference = 'Stop'
$options = @{ Action = $Action; Count = 2; AllowStaleStage = $AllowStaleStage; NativeObserver = $NativeObserver }
if ($Action -eq 'Start') {
    if ($OwnerRun) { throw 'Start always creates two fresh roots; do not supply OwnerRun' }
    if ($OwnerPort -eq $ViewerPort) { throw 'Pair requires distinct high loopback API ports' }
    $options.Ports = @($OwnerPort, $ViewerPort)
    if ($Stage) { $options.Stage = $Stage }
} else {
    if (-not $OwnerRun) { throw 'Info/Stop require the explicit owner run; .qa/current is never used for a pair' }
    $options.OwnerRun = $OwnerRun
}
& "$PSScriptRoot\peers.ps1" @options
