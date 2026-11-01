# tools/qa/launch.ps1 — start a staged build in a fresh, isolated run dir
# and (for the "basic" fixture) seed a deterministic scene over REST before
# handing back control. See tools/qa/AGENTS.md for the full design and the
# non-obvious gotchas this script works around.
#
# Usage: powershell -File tools\qa\launch.ps1 [-Stage <sha>] [-Fixture basic|none]
[CmdletBinding()]
param(
    [string]$Stage,
    [ValidateSet("basic", "none")]
    [string]$Fixture = "basic"
)

. "$PSScriptRoot\_lib.ps1"

$swTotal = [System.Diagnostics.Stopwatch]::StartNew()

# --- Resolve the stage dir -------------------------------------------------
if ($Stage) {
    $stageDir = Join-Path $QaStageRoot $Stage
} else {
    $stageDir = Get-QaLatestStageDir
}
if (-not $stageDir -or -not (Test-Path $stageDir)) {
    Write-QaLog "No staged build found (run tools\qa\stage.ps1 first). Looked in: $stageDir"
    exit 1
}
$exe = Join-Path $stageDir "fractalengine.exe"
if (-not (Test-Path $exe)) {
    Write-QaLog "Staged build at $stageDir has no fractalengine.exe"
    exit 1
}
Write-QaLog "Using staged build: $stageDir"

# --- Isolated run dir --------------------------------------------------------
$ts = (Get-Date).ToString("yyyyMMdd-HHmmss-fff")
$run = Join-Path $QaRunRoot $ts
New-Item -ItemType Directory -Force -Path $run | Out-Null
New-Item -ItemType Directory -Force -Path (Join-Path $run "data") | Out-Null
New-Item -ItemType Directory -Force -Path (Join-Path $run "fractalengine") | Out-Null

# CWD-relative DB isolation depends on NOT triggering fe_runtime::app::
# packaged_root() (see stage.ps1's comment) — so the real assets tree is
# junctioned in under the workspace-checkout layout
# ("<run>/fractalengine/assets") instead of sitting beside the exe.
$assetsLink = Join-Path $run "fractalengine\assets"
$assetsSrc = Join-Path $QaRepoRoot "fractalengine\assets"
cmd /c mklink /J "$assetsLink" "$assetsSrc" | Out-Null
if (-not (Test-Path $assetsLink)) {
    Write-QaLog "Could not junction $assetsLink -> $assetsSrc"
    exit 1
}

$tokensPath = Join-Path $run "tokens.json"
$hexonDir = Join-Path $run "hexons"

# --- Optional offline step: install the one sample hexon that is a real
# TerrainTileset (if any — the three shipped under sample-hexons/dist/ are
# Terrain/Scene/GpxCollection as of this writing, so this currently always
# no-ops; kept so the fixture step below picks it up the moment a tileset
# IS shipped, without script changes). Runs before launch: it only touches
# files under $hexonDir, no running process needed.
$terrainHexonId = $null
$hexonInstaller = Join-Path $stageDir "install_sample_hexons.exe"
$sampleHexonsDir = Join-Path $QaRepoRoot "sample-hexons\dist"
if ((Test-Path $hexonInstaller) -and (Test-Path $sampleHexonsDir)) {
    $env:FE_HEXON_DIR = $hexonDir
    $installOut = & $hexonInstaller $sampleHexonsDir 2>&1
    $installOut | ForEach-Object { Write-QaLog "install_sample_hexons: $_" }
    $installed = $installOut | Select-String -Pattern "^(installed|refreshed) (\S+)"
    if ($installed) {
        $terrainHexonId = $installed[0].Matches[0].Groups[2].Value
    }
}

# --- Launch -------------------------------------------------------------
$env:FE_API_DEV_TOKEN_OUT = $tokensPath
$env:FE_API_INPUT_ALLOW = "1"
$env:FE_SYNC_RELAY = "disabled"
$env:FE_HEXON_DIR = $hexonDir
$env:RUST_LOG = "info"

$swHealth = [System.Diagnostics.Stopwatch]::StartNew()
$proc = Start-Process -FilePath $exe -WorkingDirectory $run -PassThru `
    -RedirectStandardOutput (Join-Path $run "stdout.log") `
    -RedirectStandardError (Join-Path $run "stderr.log") `
    -WindowStyle Hidden
$startedAt = (Get-Date).ToString("o")
Write-QaLog "Launched pid=$($proc.Id) run=$run"

# Every failure from here on must stop the already-launched process tree
# before exiting — a bare `exit 1` mid-fixture would otherwise strand a
# live fractalengine.exe (+ its webview children, §process-tree) that
# nothing else ever reaps.
function Fail {
    param([string]$Message)
    Write-QaLog $Message
    Get-QaProcessTree -RootId $proc.Id | ForEach-Object { Stop-Process -Id $_ -Force -ErrorAction SilentlyContinue }
    exit 1
}

if (-not (Wait-QaHttp -Url "$QaApiBase/api/v1/health" -TimeoutSec 120)) {
    Fail "API never answered /api/v1/health within 120s — stopping pid=$($proc.Id) and failing"
}
$healthMs = $swHealth.ElapsedMilliseconds
Write-QaLog "API healthy after ${healthMs}ms"

# --- Bootstrap token: the auto-seeded "Genesis Verse" (fe-database::handlers::
# seed::seed_default_data, fired automatically on an empty DB) is the ONLY
# way to get a first Owner token at all — see tools/qa/AGENTS.md §bootstrap
# for why there is no other door (every REST write needs a manager+ token
# already, and the GUI's own CreateVerse always mints the NEW verse's
# created_by from the node's own identity, never the caller's claims, which
# is what makes this bootstrap safe to chain).
$swToken = [System.Diagnostics.Stopwatch]::StartNew()
$bootstrap = Wait-QaOwnerToken -TokensPath $tokensPath -TimeoutSec 60
if (-not $bootstrap) {
    Fail "No owner token appeared in $tokensPath within 60s"
}
$tokenMs = $swToken.ElapsedMilliseconds
Write-QaLog "Bootstrap owner token ready (verse $($bootstrap.VerseId)) after ${tokenMs}ms"

$fixtureInfo = $null
$swFixture = [System.Diagnostics.Stopwatch]::StartNew()
if ($Fixture -eq "basic") {
    # 1 verse -> 1 fractal -> 2 petals, a few GLB nodes, terrain on one petal
    # IF a TerrainTileset hexon is available offline (tools/qa/AGENTS.md
    # §fixture-basic has the full shape + the REST calls this mirrors).
    $verseResp = Invoke-QaApi -Method POST -Path "/api/v1/verses" -Token $bootstrap.Token -JsonBody '{"name":"QA Basic"}'
    if (-not $verseResp.ok) { Fail "fixture: create verse failed: $($verseResp.error)" }
    $verseId = $verseResp.data.id

    $verseToken = Wait-QaOwnerToken -TokensPath $tokensPath -VerseId $verseId -TimeoutSec 30
    if (-not $verseToken) { Fail "fixture: QA Basic verse never got its own owner token" }
    $vt = $verseToken.Token

    $fractalResp = Invoke-QaApi -Method POST -Path "/api/v1/verses/$verseId/fractals" -Token $vt -JsonBody '{"name":"QA Fractal"}'
    if (-not $fractalResp.ok) { Fail "fixture: create fractal failed: $($fractalResp.error)" }
    $fractalId = $fractalResp.data.id

    $petalAResp = Invoke-QaApi -Method POST -Path "/api/v1/verses/$verseId/fractals/$fractalId/petals" -Token $vt -JsonBody '{"name":"Petal A"}'
    $petalBResp = Invoke-QaApi -Method POST -Path "/api/v1/verses/$verseId/fractals/$fractalId/petals" -Token $vt -JsonBody '{"name":"Petal B"}'
    if (-not $petalAResp.ok -or -not $petalBResp.ok) { Fail "fixture: create petal failed" }
    $petalAId = $petalAResp.data.id
    $petalBId = $petalBResp.data.id

    $glbPath = Join-Path $QaRepoRoot "fe-test-harness\src\fixtures\test_cube.glb"
    $uploadResp = Invoke-QaApiUpload -Path "/api/v1/petals/$petalAId/assets" -Token $vt -FilePath $glbPath
    if (-not $uploadResp.ok) { Fail "fixture: GLB upload failed: $($uploadResp.error)" }
    $assetId = $uploadResp.data.asset_id

    $nodeIds = @()
    $positions = @(@(0, 0, 0), @(2, 0, 0), @(-2, 0, 0))
    $names = @("Cube 1", "Cube 2", "Cube 3")
    for ($i = 0; $i -lt 3; $i++) {
        $body = @{ name = $names[$i]; asset_id = $assetId; position = $positions[$i] } | ConvertTo-Json -Compress
        $nodeResp = Invoke-QaApi -Method POST -Path "/api/v1/verses/$verseId/fractals/$fractalId/petals/$petalAId/nodes" -Token $vt -JsonBody $body
        if (-not $nodeResp.ok) { Fail "fixture: create node $($names[$i]) failed: $($nodeResp.error)" }
        $nodeIds += $nodeResp.data.id
    }

    $terrainBound = $false
    if ($terrainHexonId) {
        $terrainBody = @{
            enabled = $true
            origin = @{ origin_lat = 46.5; origin_lon = 8.0; origin_ele = 0.0 }
            tile_source_url = ""
            elevation_source = "none"
            tile_source_mode = "offline"
            tileset_hexon_uris = @($terrainHexonId)
        } | ConvertTo-Json -Compress
        $terrainResp = Invoke-QaApi -Method PUT -Path "/api/v1/petals/$petalBId/terrain" -Token $vt -JsonBody $terrainBody
        $terrainBound = [bool]$terrainResp.ok
        if (-not $terrainBound) { Write-QaLog "fixture: terrain PUT failed (continuing without it): $($terrainResp.error)" }
    } else {
        Write-QaLog "fixture: no TerrainTileset hexon available offline under sample-hexons/dist — skipping terrain binding (per spec: 'if a sample hexon is available offline')"
    }

    $fixtureInfo = @{
        name = "basic"
        verse_id = $verseId
        fractal_id = $fractalId
        petal_a_id = $petalAId
        petal_b_id = $petalBId
        asset_id = $assetId
        node_ids = $nodeIds
        terrain_petal_id = $(if ($terrainBound) { $petalBId } else { $null })
        terrain_bound = $terrainBound
    }
    Write-QaLog "Fixture 'basic' seeded: verse=$verseId fractal=$fractalId petals=[$petalAId,$petalBId] nodes=$($nodeIds.Count) terrain_bound=$terrainBound"
}
$fixtureMs = $swFixture.ElapsedMilliseconds

$current = @{
    pid = $proc.Id
    api_base = $QaApiBase
    tokens_path = $tokensPath
    stage = $stageDir
    fixture = $Fixture
    started_at = $startedAt
    run_dir = $run
    bootstrap_verse_id = $bootstrap.VerseId
    fixture_detail = $fixtureInfo
    timings_ms = @{ health = $healthMs; bootstrap_token = $tokenMs; fixture_seed = $fixtureMs }
}
$current | ConvertTo-Json -Depth 6 | Set-Content (Join-Path $run "current.json") -Encoding utf8
$run | Set-Content (Join-Path $QaRepoRoot ".qa\current") -Encoding ascii -NoNewline

Write-QaLog "Launch complete in $([math]::Round($swTotal.Elapsed.TotalSeconds, 1))s total. run=$run pid=$($proc.Id)"
Write-Output $run
