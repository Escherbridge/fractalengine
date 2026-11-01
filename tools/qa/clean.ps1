# tools/qa/clean.ps1 — prune .qa/stage and .qa/run, keeping the latest 3 of
# each plus anything newer than -Days (default 7). Never touches a run dir
# whose current.json still points at a LIVE pid (an in-flight QA session),
# even if it is old.
#
# Usage: powershell -File tools\qa\clean.ps1 [-Days 7] [-Keep 3] [-DryRun]
[CmdletBinding()]
param(
    [int]$Days = 7,
    [int]$Keep = 3,
    [switch]$DryRun
)

. "$PSScriptRoot\_lib.ps1"

$cutoff = (Get-Date).AddDays(-$Days)

function Prune-QaDir {
    param([string]$Root, [string]$Label)
    if (-not (Test-Path $Root)) { return }
    $dirs = Get-ChildItem $Root -Directory -ErrorAction SilentlyContinue | Sort-Object LastWriteTime -Descending
    if ($dirs.Count -le $Keep) {
        Write-QaLog "$Label`: $($dirs.Count) <= keep $Keep — nothing to prune"
        return
    }
    $candidates = $dirs | Select-Object -Skip $Keep | Where-Object { $_.LastWriteTime -lt $cutoff }
    foreach ($d in $candidates) {
        $isLiveRun = $false
        $cj = Join-Path $d.FullName "current.json"
        if (Test-Path $cj) {
            try {
                $c = Get-Content $cj -Raw | ConvertFrom-Json
                if (Get-Process -Id $c.pid -ErrorAction SilentlyContinue) { $isLiveRun = $true }
            } catch {}
        }
        if ($isLiveRun) {
            Write-QaLog "$Label`: SKIP $($d.Name) — pid still alive"
            continue
        }
        if ($DryRun) {
            Write-QaLog "$Label`: would remove $($d.FullName) (last write $($d.LastWriteTime))"
        } else {
            Write-QaLog "$Label`: removing $($d.FullName) (last write $($d.LastWriteTime))"
            Remove-Item $d.FullName -Recurse -Force -ErrorAction SilentlyContinue
        }
    }
}

Prune-QaDir -Root $QaStageRoot -Label "stage"
Prune-QaDir -Root $QaRunRoot -Label "run"
