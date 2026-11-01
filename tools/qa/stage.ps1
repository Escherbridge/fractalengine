# tools/qa/stage.ps1 — build (if needed) and stage the GUI binary + its QA
# helper for isolated, lock-free launches. See tools/qa/AGENTS.md.
#
# Usage: powershell -File tools\qa\stage.ps1
#
# If another cargo/rustc is already running (a build this script does not
# own — another lane's build, per the "cargo is single-tenant" rule), this
# SKIPS the build and reuses the newest already-staged build instead of
# racing it for the lock.
[CmdletBinding()]
param()

. "$PSScriptRoot\_lib.ps1"
$ErrorActionPreference = 'Stop'

$sw = [System.Diagnostics.Stopwatch]::StartNew()
$sourceFingerprint = Get-QaSourceFingerprint

$busy = Get-Process cargo, rustc -ErrorAction SilentlyContinue
if ($busy) {
    $existing = Get-QaLatestStageDir
    if (-not $existing) {
        Write-QaLog "cargo/rustc is running (pid $($busy[0].Id)) and nothing is staged yet — cannot build or reuse. Re-run once the other build finishes."
        exit 1
    }
    Write-QaLog "cargo/rustc is running (pid $($busy[0].Id)) — another lane owns the build. Reusing the newest staged build instead of racing it:"
    Write-QaLog "  $existing"
    $null = Get-QaStageManifest -StageDir $existing
    Write-Output $existing
    exit 0
}

Push-Location $QaRepoRoot
try {
    Write-QaLog "Building fractalengine + install_sample_hexons (cargo build -p fractalengine -p fe-terrain --example install_sample_hexons)..."
    $env:CARGO_PROFILE_DEV_DEBUG = "0"
    $env:RUST_MIN_STACK = "134217728"
    # No `2>&1` here: PowerShell 5.1 wraps a native command's stderr lines in
    # NativeCommandError records (cosmetic noise — cargo's own "Finished"/
    # "Compiling" lines go to stderr), which is harmless since $LASTEXITCODE
    # (not $?) is what gates success below, but is confusing to read.
    & cargo build -p fractalengine --bin fractalengine -j4
    if ($LASTEXITCODE -ne 0) { throw "GUI build failed: $LASTEXITCODE" }
    & cargo build -p fe-terrain --example install_sample_hexons -j4
    $exitCode = $LASTEXITCODE
    if ($exitCode -ne 0) {
        Write-QaLog "cargo build failed with exit code $exitCode"
        exit $exitCode
    }
} finally {
    Pop-Location
}

$sha = (& git -C $QaRepoRoot rev-parse --short HEAD).Trim()
$stageDir = Resolve-QaChildPath -Path (Join-Path $QaStageRoot "$sha-$((Get-Date).ToString('yyyyMMddHHmmssfff'))") -Root $QaStageRoot
New-Item -ItemType Directory -Force -Path $stageDir | Out-Null

$exeSrc = Join-Path $QaRepoRoot "target\debug\fractalengine.exe"
if (-not (Test-Path $exeSrc)) {
    Write-QaLog "build reported success but $exeSrc does not exist — aborting stage"
    exit 1
}
Copy-Item $exeSrc (Join-Path $stageDir "fractalengine.exe") -Force

# Runtime DLLs beside the exe (none today — Bevy/winit/wgpu link statically
# on this toolchain — but copy whatever shows up so a future dynamically
# linked dependency doesn't silently break a staged run).
Get-ChildItem (Join-Path $QaRepoRoot "target\debug") -Filter "*.dll" -ErrorAction SilentlyContinue |
    ForEach-Object { Copy-Item $_.FullName $stageDir -Force }

$hexonInstallerSrc = Join-Path $QaRepoRoot "target\debug\examples\install_sample_hexons.exe"
if (Test-Path $hexonInstallerSrc) {
    Copy-Item $hexonInstallerSrc (Join-Path $stageDir "install_sample_hexons.exe") -Force
}

$manifest = @{
    kind = 'fractalengine-qa-stage-v1'
    source_commit = (& git -C $QaRepoRoot rev-parse HEAD).Trim()
    source_dirty = [bool](& git -C $QaRepoRoot status --porcelain --untracked-files=no)
    source_fingerprint = $sourceFingerprint
    built_at = (Get-Date).ToUniversalTime().ToString('o')
    exe_sha256 = (Get-FileHash -LiteralPath (Join-Path $stageDir 'fractalengine.exe') -Algorithm SHA256).Hash
}
if ((Get-QaSourceFingerprint) -ne $sourceFingerprint) { throw 'Build inputs changed while staging; rerun stage.ps1' }
$manifest | ConvertTo-Json | Set-Content -LiteralPath (Join-Path $stageDir 'stage.json') -Encoding UTF8

# Assets are copied into each run, never beside this staged executable.

Write-QaLog "Staged $sha -> $stageDir ($([math]::Round($sw.Elapsed.TotalSeconds, 1))s total)"
Write-Output $stageDir
