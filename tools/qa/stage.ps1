# tools/qa/stage.ps1 — build (if needed) and stage the GUI binary + its QA
# helper for isolated, lock-free launches. See tools/qa/AGENTS.md.
#
# Usage: powershell -File tools\qa\stage.ps1 [-Loopback <run>\loopback\loopback.json]
#
# Also builds fe-ext and stages fe-ext.exe beside fractalengine.exe: the sim packs the example extensions and builds
# its static index with it, and `fe-ext admin` drives the local admin channel (tools/qa/AGENTS.md section sim).
# -Loopback reads a run's loopback.json and verifies the static index tree it names with the staged fe-ext
# (`fe-ext index verify <site_dir> --index-key <index_key_id>`); the stage fails when the tree does not verify.
#
# If another cargo/rustc is already running (a build this script does not
# own — another lane's build, per the "cargo is single-tenant" rule), this
# SKIPS the build and reuses the newest already-staged build instead of
# racing it for the lock.
[CmdletBinding()]
param([ValidateRange(1,16)][int]$Jobs = 2, [switch]$NativeObserver, [string]$Loopback)

. "$PSScriptRoot\_lib.ps1"
$ErrorActionPreference = 'Stop'
$lifecycleLease = Enter-QaLifecycleLease
try {

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
    $existingManifest = Get-QaStageManifest -StageDir $existing
    if ($NativeObserver -and -not $existingManifest.native_observer) { throw 'Busy build cannot supply the requested native helper stage' }
    Write-Output $existing
    exit 0
}

Push-Location $QaRepoRoot
try {
    Write-QaLog "Building fractalengine and offline QA terrain helpers..."
    $env:CARGO_PROFILE_DEV_DEBUG = "0"
    $env:RUST_MIN_STACK = "134217728"
    # No `2>&1` here: PowerShell 5.1 wraps a native command's stderr lines in
    # NativeCommandError records (cosmetic noise — cargo's own "Finished"/
    # "Compiling" lines go to stderr), which is harmless since $LASTEXITCODE
    # (not $?) is what gates success below, but is confusing to read.
    & cargo build -p fractalengine --bin fractalengine -j $Jobs
    if ($LASTEXITCODE -ne 0) { throw "GUI build failed: $LASTEXITCODE" }
    & cargo build -p fe-terrain --example install_sample_hexons --example build_qa_terrain -j $Jobs
    $exitCode = $LASTEXITCODE
    if ($exitCode -ne 0) {
        Write-QaLog "cargo build failed with exit code $exitCode"
        exit $exitCode
    }
    & cargo build -p fe-format --example seal_hexon -j $Jobs
    if ($LASTEXITCODE -ne 0) { throw "QA archive sealer build failed: $LASTEXITCODE" }
    & cargo build -p fe-database --example legacy_adoption_fixture -j $Jobs
    if ($LASTEXITCODE -ne 0) { throw "QA legacy fixture helper build failed: $LASTEXITCODE" }
    & cargo build -p fe-ext --bin fe-ext -j $Jobs
    if ($LASTEXITCODE -ne 0) { throw "fe-ext build failed: $LASTEXITCODE" }
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
$terrainBuilderSrc = Join-Path $QaRepoRoot 'target/debug/examples/build_qa_terrain.exe'
if (-not (Test-Path -LiteralPath $terrainBuilderSrc)) { throw 'Offline QA terrain helper is missing after build' }
Copy-Item -LiteralPath $terrainBuilderSrc -Destination (Join-Path $stageDir 'build_qa_terrain.exe')
$sealerSrc = Join-Path $QaRepoRoot 'target/debug/examples/seal_hexon.exe'
if (-not (Test-Path -LiteralPath $sealerSrc)) { throw 'QA archive sealer is missing after build' }
Copy-Item -LiteralPath $sealerSrc -Destination (Join-Path $stageDir 'seal_hexon.exe')

$legacyFixtureSrc = Join-Path $QaRepoRoot 'target/debug/examples/legacy_adoption_fixture.exe'
if (-not (Test-Path -LiteralPath $legacyFixtureSrc)) { throw 'QA legacy fixture helper is missing after build' }
Copy-Item -LiteralPath $legacyFixtureSrc -Destination (Join-Path $stageDir 'legacy_adoption_fixture.exe')

$feExtSrc = Join-Path $QaRepoRoot 'target/debug/fe-ext.exe'
if (-not (Test-Path -LiteralPath $feExtSrc)) { throw 'fe-ext is missing after build' }
Copy-Item -LiteralPath $feExtSrc -Destination (Join-Path $stageDir 'fe-ext.exe')

$manifest = @{
    kind = 'fractalengine-qa-stage-v1'
    source_commit = (& git -C $QaRepoRoot rev-parse HEAD).Trim()
    source_dirty = [bool](& git -C $QaRepoRoot status --porcelain --untracked-files=no)
    source_fingerprint = $sourceFingerprint
    built_at = (Get-Date).ToUniversalTime().ToString('o')
    exe_sha256 = (Get-FileHash -LiteralPath (Join-Path $stageDir 'fractalengine.exe') -Algorithm SHA256).Hash
    seal_hexon_sha256 = (Get-FileHash -LiteralPath (Join-Path $stageDir 'seal_hexon.exe') -Algorithm SHA256).Hash
    legacy_fixture_sha256 = (Get-FileHash -LiteralPath (Join-Path $stageDir 'legacy_adoption_fixture.exe') -Algorithm SHA256).Hash
    fe_ext_sha256 = (Get-FileHash -LiteralPath (Join-Path $stageDir 'fe-ext.exe') -Algorithm SHA256).Hash
}
if ($NativeObserver) {
    $nativeOutput = @(& "$PSScriptRoot\native_observer\build.ps1" -OutputDirectory $stageDir)
    $manifest.native_observer = $nativeOutput[-1]
    if ($manifest.native_observer.schema -cne 'fe-native-helper-stage-v1') { throw 'Native helper build manifest unavailable' }
}
if ($Loopback) {
    $loopbackInfo = Get-Content -LiteralPath $Loopback -Raw | ConvertFrom-Json
    if ($loopbackInfo.schema -ne 'fe-qa-sim-loopback-v1') { throw "Not a sim loopback.json: $Loopback" }
    if (-not $loopbackInfo.site_dir) { throw "loopback.json names no static index tree (site_dir): the run has not staged its packages" }
    & (Join-Path $stageDir 'fe-ext.exe') index verify $loopbackInfo.site_dir --index-key $loopbackInfo.index_key_id
    if ($LASTEXITCODE -ne 0) { throw "The run's static index does not verify with the staged fe-ext: $($loopbackInfo.site_dir)" }
    Write-QaLog "Verified the run's static index ($($loopbackInfo.site_dir)) with the staged fe-ext"
}
if ((Get-QaSourceFingerprint) -ne $sourceFingerprint) { throw 'Build inputs changed while staging; rerun stage.ps1' }
$manifest | ConvertTo-Json | Set-Content -LiteralPath (Join-Path $stageDir 'stage.json') -Encoding UTF8

# Assets are copied into each run, never beside this staged executable.

Write-QaLog "Staged $sha -> $stageDir ($([math]::Round($sw.Elapsed.TotalSeconds, 1))s total)"
Write-Output $stageDir

} finally { $lifecycleLease.Dispose() }
