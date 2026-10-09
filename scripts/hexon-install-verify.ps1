<#
.SYNOPSIS
    F16 (A29) V1: install verification for the two F15-produced `.hexon`
    tilesets (switzerland-zurich-alps, japan-mount-fuji) via the real
    `install_sample_hexons` example -- the same seam the Hexon Manager UI's
    install flow bottoms out on (`fe_terrain::tiles::HexonStore::install_tileset`).

.DESCRIPTION
    conductor/tracks/mission_prodready_continuation_20261008/m6-gis-regions-design.md.

    Phase 1: runs `cargo run -p fe-terrain --example install_sample_hexons`
    against both real dist archives into a fresh `FE_HEXON_DIR`, TWICE, to
    prove the install path is idempotent (install, then refresh -- never a
    duplicate registry entry or an error on re-run).

    Phase 2: runs the `#[ignore]`'d integration test
    `fe-terrain/tests/gis_hexon_install_test.rs` (via `-- --ignored`, with
    FE_GIS_DIST_DIR set) which asserts registry/meta correctness (bounds,
    zoom range, tile counts, has_satellite) AND that `backfill_scale_fields` produces a
    ground_sample_distance_m / native_scale within 5% of an INDEPENDENTLY
    computed expected value (standard Web-Mercator tile-resolution formula;
    see the test file's doc comment for the math) -- not just "is non-null".

    Never touches gis-tile-etl (reads its dist/ dir only). Does not start a
    relay. Serial build (caller owns the build lock).

.PARAMETER DistDir
    Directory containing switzerland-zurich-alps.hexon and
    japan-mount-fuji.hexon. Defaults to the sibling gis-tile-etl repo's dist/.
#>
param(
    [string]$DistDir = (Join-Path (Split-Path (Split-Path $PSScriptRoot -Parent) -Parent) 'gis-tile-etl\dist')
)

$ErrorActionPreference = 'Continue'
$script:Failures = @()
$script:Passed = 0
$script:Checked = 0

function Write-Section($title) {
    Write-Host ""
    Write-Host "=== $title ===" -ForegroundColor Cyan
}

function Assert-True($condition, $message) {
    $script:Checked++
    if ($condition) {
        $script:Passed++
        $line = "  [PASS] $message"
        Write-Host $line -ForegroundColor Green
    } else {
        $script:Failures += $message
        $line = "  [FAIL] $message"
        Write-Host $line -ForegroundColor Red
    }
    # DEC-C22 FIX M: tee every assertion into the committed log, not just
    # console -- $logPath is set during Setup, below.
    if ($logPath) { Add-Content -Path $logPath -Value $line }
}

$RepoRoot = (Resolve-Path (Join-Path $PSScriptRoot '..')).Path
Set-Location $RepoRoot

Write-Section "Disk guard"
$drive = Get-PSDrive -Name C
$freeGb = [math]::Round($drive.Free / 1GB, 1)
Write-Host "  C: free = $freeGb GB"
if ($freeGb -lt 8) {
    Write-Host "STOP: less than 8 GB free on C:. Aborting before any build." -ForegroundColor Red
    exit 1
}

# DEC-C22 FIX M: work dir + log path are set up BEFORE the first Assert-True
# call below (dist-archive existence) so every assertion lands in the
# committed log, not just the console.
$stamp = Get-Date -Format 'yyyyMMdd-HHmmss'
$script:WorkDir = Join-Path $env:TEMP "fe-hexon-install-verify-$stamp"
New-Item -ItemType Directory -Path $script:WorkDir -Force | Out-Null
$hexonDir = Join-Path $script:WorkDir 'hexons'
$logPath = Join-Path $PSScriptRoot 'hexon-install-verify.log'
Write-Host "  Work dir: $script:WorkDir"
Write-Host "  Log: $logPath"
"" | Set-Content -Path $logPath

Write-Section "Resolve F15 dist archives"
Write-Host "  DistDir: $DistDir"
$switzerlandPath = Join-Path $DistDir 'switzerland-zurich-alps.hexon'
$japanPath = Join-Path $DistDir 'japan-mount-fuji.hexon'
Assert-True (Test-Path $switzerlandPath) "found $switzerlandPath"
Assert-True (Test-Path $japanPath) "found $japanPath"
if ($script:Failures.Count -gt 0) {
    Write-Host "Cannot continue without both dist archives." -ForegroundColor Red
    exit 1
}

$env:RUST_MIN_STACK = '134217728'
if (-not $env:CARGO_BUILD_JOBS) { $env:CARGO_BUILD_JOBS = '2' }

function Invoke-Install($runLabel) {
    Write-Host "  --- install_sample_hexons run: $runLabel ---"
    "=== install_sample_hexons run: $runLabel ===" | Add-Content -Path $logPath
    $out = & cargo run -q -p fe-terrain --example install_sample_hexons -- $switzerlandPath $japanPath 2>&1
    $out | Add-Content -Path $logPath
    $out | ForEach-Object { Write-Host "    $_" }
    return ($out -join "`n")
}

try {
    Write-Section "Phase 1: install_sample_hexons (serial, run twice for idempotency)"
    $env:FE_HEXON_DIR = $hexonDir

    $firstOut = Invoke-Install "1st (fresh install)"
    Assert-True ($firstOut -match 'installed tileset-switzerland-zurich-alps') "1st run: installed switzerland tileset"
    Assert-True ($firstOut -match 'installed tileset-japan-mount-fuji') "1st run: installed japan tileset"
    Assert-True ($firstOut -match '2 tileset\(s\) now installed') "1st run: registry reports 2 tilesets"

    $secondOut = Invoke-Install "2nd (refresh, proves idempotency)"
    Assert-True ($secondOut -match 'refreshed tileset-switzerland-zurich-alps') "2nd run: refreshed (not re-installed/errored) switzerland tileset"
    Assert-True ($secondOut -match 'refreshed tileset-japan-mount-fuji') "2nd run: refreshed (not re-installed/errored) japan tileset"
    Assert-True ($secondOut -match '2 tileset\(s\) now installed') "2nd run: registry still reports exactly 2 tilesets (no duplicates)"

    $registryPath = Join-Path $hexonDir 'registry.json'
    Assert-True (Test-Path $registryPath) "registry.json written to FE_HEXON_DIR"
    if (Test-Path $registryPath) {
        $registry = Get-Content $registryPath -Raw | ConvertFrom-Json
        Assert-True ($registry.tilesets.Count -eq 2) "registry.json lists exactly 2 tilesets"
    }

    Write-Section "Phase 2: gis_hexon_install_test (meta + backfilled-scale assertions, FE_GIS_DIST_DIR)"
    $env:FE_GIS_DIST_DIR = $DistDir
    # DEC-C22 FIX L: the test is #[ignore]'d by default (no silent skip-as-
    # green in a plain workspace sweep) -- `--ignored` is required to run it
    # at all, and a run that didn't execute it (0 passed) is a failure here.
    "=== cargo test -p fe-terrain --test gis_hexon_install_test -- --ignored ===" | Add-Content -Path $logPath
    # No -q: quiet mode hides test NAMES (prints '.'), which would defeat the
    # ran-and-passed assertion below.
    $testOut = & cargo test -p fe-terrain --test gis_hexon_install_test -- --ignored --nocapture 2>&1
    $testOut | Add-Content -Path $logPath
    $testOut | ForEach-Object { Write-Host "    $_" }
    $testExit = $LASTEXITCODE
    $testOutText = ($testOut -join "`n")

    Assert-True ($testExit -eq 0) "gis_hexon_install_test exited 0"
    Assert-True ($testOutText -match 'install_f15_hexons_backfills_correct_scale_and_meta.*ok') "test actually RAN (via --ignored) and passed, not silently skipped"
    Assert-True ($testOutText -match '1 passed') "exactly one test ran (no 0-passed/all-ignored silent skip)"
    Assert-True ($testOutText -match 'tileset-switzerland-zurich-alps: gsd=') "switzerland: backfilled GSD line printed"
    Assert-True ($testOutText -match 'tileset-japan-mount-fuji: gsd=') "japan: backfilled GSD line printed"
}
catch {
    $script:Failures += "UNHANDLED EXCEPTION: $_"
    Write-Host "UNHANDLED EXCEPTION: $_" -ForegroundColor Red
}
finally {
    Remove-Item Env:\FE_HEXON_DIR -ErrorAction SilentlyContinue
    Remove-Item Env:\FE_GIS_DIST_DIR -ErrorAction SilentlyContinue
    if ($script:WorkDir -and (Test-Path $script:WorkDir)) {
        Write-Host ""
        Write-Host "  Removing temp dir $script:WorkDir"
        Remove-Item -Recurse -Force $script:WorkDir -ErrorAction SilentlyContinue
    }

    Write-Host ""
    Write-Host "=== SUMMARY ===" -ForegroundColor Cyan
    Write-Host "Checks: $script:Passed / $script:Checked passed"
    # DEC-C22 FIX M: the summary is evidence too -- append it to the log, not
    # just the console, so a re-run's committed log is self-contained.
    if ($logPath) {
        Add-Content -Path $logPath -Value ""
        Add-Content -Path $logPath -Value "=== SUMMARY ==="
        Add-Content -Path $logPath -Value "Checks: $script:Passed / $script:Checked passed"
    }
    if ($script:Failures.Count -eq 0) {
        Write-Host "PASS: hexon install verify (F16/A29 V1) - all checks passed" -ForegroundColor Green
        if ($logPath) { Add-Content -Path $logPath -Value "PASS: hexon install verify (F16/A29 V1) - all checks passed" }
    } else {
        Write-Host "FAIL: hexon install verify (F16/A29 V1) - $($script:Failures.Count) check(s) failed:" -ForegroundColor Red
        $script:Failures | ForEach-Object { Write-Host "  - $_" -ForegroundColor Red }
        if ($logPath) {
            Add-Content -Path $logPath -Value "FAIL: hexon install verify (F16/A29 V1) - $($script:Failures.Count) check(s) failed:"
            $script:Failures | ForEach-Object { Add-Content -Path $logPath -Value "  - $_" }
        }
    }
    Write-Host "Full log: $logPath"
}

if ($script:Failures.Count -gt 0) { exit 1 } else { exit 0 }
