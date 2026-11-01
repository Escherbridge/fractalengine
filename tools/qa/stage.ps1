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

$sw = [System.Diagnostics.Stopwatch]::StartNew()

$busy = Get-Process cargo, rustc -ErrorAction SilentlyContinue
if ($busy) {
    $existing = Get-QaLatestStageDir
    if (-not $existing) {
        Write-QaLog "cargo/rustc is running (pid $($busy[0].Id)) and nothing is staged yet — cannot build or reuse. Re-run once the other build finishes."
        exit 1
    }
    Write-QaLog "cargo/rustc is running (pid $($busy[0].Id)) — another lane owns the build. Reusing the newest staged build instead of racing it:"
    Write-QaLog "  $existing"
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
    & cargo build -p fractalengine -p fe-terrain --example install_sample_hexons -j4
    $exitCode = $LASTEXITCODE
    if ($exitCode -ne 0) {
        Write-QaLog "cargo build failed with exit code $exitCode"
        exit $exitCode
    }
} finally {
    Pop-Location
}

$sha = (& git -C $QaRepoRoot rev-parse --short HEAD).Trim()
$stageDir = Join-Path $QaStageRoot $sha
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

# DELIBERATELY NOT staging an "assets" folder beside the exe: fe_runtime::app::
# packaged_root() treats "<exe dir>/assets exists" as "this is a packaged
# build" and cd's the whole process into the exe's directory before DB_PATH
# ("data/fractalengine.db", CWD-relative) is ever touched. Every isolated
# run dir would then collide on the SAME data/ under this one stage dir
# instead of getting its own. launch.ps1 junctions
# "<run>/fractalengine/assets" -> the real repo assets instead, which keeps
# packaged_root() returning None (the "workspace checkout" branch) so the
# CWD-relative DB/blob paths stay isolated per run. See tools/qa/AGENTS.md
# §cwd-isolation for the full story (this was verified empirically, not
# assumed — the task text that asked for isolation also asked to verify it).

Write-QaLog "Staged $sha -> $stageDir ($([math]::Round($sw.Elapsed.TotalSeconds, 1))s total)"
Write-Output $stageDir
