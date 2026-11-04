# Prune confined QA directories, retaining live runs and their stages.
[CmdletBinding()]
param([ValidateRange(0,36500)][int]$Days = 7, [ValidateRange(0,10000)][int]$Keep = 3, [switch]$DryRun)
. "$PSScriptRoot\_lib.ps1"
$ErrorActionPreference = 'Stop'
$lifecycleLease = Enter-QaLifecycleLease
try {
$cutoff = (Get-Date).AddDays(-$Days)
$liveRuns = @{}
$liveStages = @{}

function Assert-QaTreeWithoutReparse {
    param([string]$Path)
    $item = Get-Item -LiteralPath $Path -Force
    if ($item.Attributes -band [IO.FileAttributes]::ReparsePoint) { throw "Reparse point retained: $Path" }
    if ($item.PSIsContainer) {
        foreach ($child in Get-ChildItem -LiteralPath $Path -Force) { Assert-QaTreeWithoutReparse -Path $child.FullName }
    }
}

foreach ($directory in @(Get-ChildItem -LiteralPath $QaRunRoot -Directory -ErrorAction SilentlyContinue)) {
    $record = Join-Path $directory.FullName 'current.json'
    if (-not (Test-Path -LiteralPath $record)) { continue }
    try {
        $current = Get-Content -LiteralPath $record -Raw | ConvertFrom-Json
        foreach ($retained in @(Get-QaNativeRetainedStages -Run $directory.FullName)) {
            $liveRuns[$directory.FullName] = $true
            $liveStages[$retained] = $true
        }
        if (-not $current.process_identity) {
            $liveRuns[$directory.FullName] = $true
            if ($current.stage) { $liveStages[[IO.Path]::GetFullPath($current.stage)] = $true }
            continue
        }
        foreach ($identity in (@($current.process_identity) + @($current.descendant_identities))) {
            if (-not $identity) { continue }
            $process = Get-QaOwnedProcess -Identity $identity
            if ($process) {
                $process.Dispose()
                $liveRuns[$directory.FullName] = $true
                $liveStages[[IO.Path]::GetFullPath($current.stage)] = $true
            }
        }
    } catch {
        throw "Cannot establish QA run ownership; cleanup refused: $record`: $_"
    }
}

foreach ($root in @($QaStageRoot, $QaRunRoot)) {
    if (-not (Test-Path -LiteralPath $root)) { continue }
    $directories = @(Get-ChildItem -LiteralPath $root -Directory | Sort-Object LastWriteTime -Descending)
    foreach ($directory in @($directories | Select-Object -Skip $Keep | Where-Object { $_.LastWriteTime -lt $cutoff })) {
        if ($liveRuns.ContainsKey($directory.FullName) -or $liveStages.ContainsKey($directory.FullName)) { continue }
        try {
            $target = Resolve-QaChildPath -Path $directory.FullName -Root $root
            Assert-QaTreeWithoutReparse -Path $target
        } catch { Write-QaLog "Retaining unsafe cleanup candidate: $_"; continue }
        if ($DryRun) { Write-QaLog "Would remove $target" }
        else { Remove-Item -LiteralPath $target -Recurse -Force -ErrorAction Stop }
    }
}

} finally { $lifecycleLease.Dispose() }
