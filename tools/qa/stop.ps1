# Stop only the recorded process identity and its verified descendants.
[CmdletBinding()]
param([string]$Run)
. "$PSScriptRoot\_lib.ps1"
$ErrorActionPreference = 'Stop'
$nativeRun = if ($Run) { $Run } elseif (Test-Path -LiteralPath (Join-Path $QaRepoRoot '.qa\current')) { (Get-Content -LiteralPath (Join-Path $QaRepoRoot '.qa\current') -Raw).Trim() } else { $null }
if ($nativeRun) {
    $nativeCandidate = if ([IO.Path]::IsPathRooted($nativeRun)) { $nativeRun } else { Join-Path $QaRunRoot $nativeRun }
    $nativeDirectory = Resolve-QaChildPath -Path $nativeCandidate -Root $QaRunRoot
    Stop-QaNativeObserverWorkers -Run $nativeDirectory
}
$lifecycleLease = Enter-QaLifecycleLease
try {

if (-not $Run) {
    $pointer = Join-Path $QaRepoRoot '.qa\current'
    if (-not (Test-Path -LiteralPath $pointer)) { throw 'No QA run is recorded' }
    $Run = (Get-Content -LiteralPath $pointer -Raw).Trim()
}
$candidate = if ([IO.Path]::IsPathRooted($Run)) { $Run } else { Join-Path $QaRunRoot $Run }
$runDir = Resolve-QaChildPath -Path $candidate -Root $QaRunRoot
$current = Get-Content -LiteralPath (Join-Path $runDir 'current.json') -Raw | ConvertFrom-Json
if (-not $current.process_identity) { throw 'Legacy run lacks process identity; refusing PID-only stop' }
$process = Get-QaOwnedProcess -Identity $current.process_identity
if (-not $process) {
    Write-QaLog 'Recorded root is already gone; checking only explicitly recorded descendant identities'
    Write-Warning 'Children born after the last captured snapshot cannot be proven absent after root exit'
} else {
    $process.Dispose()
}
Stop-QaOwnedProcessTree -Identity $current.process_identity -RecordedDescendants @($current.descendant_identities)
Write-QaLog "Stopped. Run dir kept at $runDir"

} finally { $lifecycleLease.Dispose() }
