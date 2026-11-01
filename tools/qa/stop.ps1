# tools/qa/stop.ps1 — stop a run started by launch.ps1. Kills the EXACT pid
# recorded in current.json plus its whole process tree (the Tauri backend
# spawns a child msedgewebview2.exe that outlives a plain Stop-Process on
# the root — verified empirically, see tools/qa/AGENTS.md §process-tree).
# Never kills by name or pattern: other sessions' fractalengine/webview2
# processes must survive this.
#
# Usage: powershell -File tools\qa\stop.ps1 [-Run <run-dir-or-timestamp>]
[CmdletBinding()]
param(
    [string]$Run
)

. "$PSScriptRoot\_lib.ps1"

if ($Run) {
    $runDir = if (Test-Path $Run) { (Resolve-Path $Run).Path } else { Join-Path $QaRunRoot $Run }
} else {
    $pointer = Join-Path $QaRepoRoot ".qa\current"
    if (-not (Test-Path $pointer)) {
        Write-QaLog "No -Run given and no .qa\current pointer — nothing to stop"
        exit 1
    }
    $runDir = (Get-Content $pointer -Raw).Trim()
}

$currentPath = Join-Path $runDir "current.json"
if (-not (Test-Path $currentPath)) {
    Write-QaLog "No current.json under $runDir — nothing to stop"
    exit 1
}
$current = Get-Content $currentPath -Raw | ConvertFrom-Json
$rootId = [int]$current.pid

$tree = Get-QaProcessTree -RootId $rootId
$alive = $tree | Where-Object { Get-Process -Id $_ -ErrorAction SilentlyContinue }
if ($alive.Count -eq 0) {
    Write-QaLog "pid $rootId (and its tree) is already gone"
    exit 0
}

Write-QaLog "Stopping process tree rooted at pid ${rootId}: $($alive -join ', ')"
foreach ($id in $alive) {
    Stop-Process -Id $id -Force -ErrorAction SilentlyContinue
}

Start-Sleep -Milliseconds 500
$stillAlive = $tree | Where-Object { Get-Process -Id $_ -ErrorAction SilentlyContinue }
if ($stillAlive.Count -gt 0) {
    Write-QaLog "Still alive after Stop-Process: $($stillAlive -join ', ') — retrying once"
    Start-Sleep -Milliseconds 1000
    foreach ($id in $stillAlive) { Stop-Process -Id $id -Force -ErrorAction SilentlyContinue }
    Start-Sleep -Milliseconds 500
    $stillAlive = $tree | Where-Object { Get-Process -Id $_ -ErrorAction SilentlyContinue }
}

if ($stillAlive.Count -gt 0) {
    Write-QaLog "FAILED to stop: $($stillAlive -join ', ')"
    exit 1
}
Write-QaLog "Stopped. Run dir kept at $runDir"
