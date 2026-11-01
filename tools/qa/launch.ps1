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
    [string]$Fixture = "basic",
    [switch]$AllowStaleStage
)

. "$PSScriptRoot\_lib.ps1"
$ErrorActionPreference = 'Stop'

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
$stageDir = Resolve-QaChildPath -Path $stageDir -Root $QaStageRoot
$stageManifest = Get-QaStageManifest -StageDir $stageDir
$stageMatchesSource = $stageManifest.source_fingerprint -eq (Get-QaSourceFingerprint)
if (-not $stageMatchesSource -and -not $AllowStaleStage) { throw 'Stage build inputs differ from current source; rebuild or explicitly use -AllowStaleStage and report limited provenance' }
if (-not $stageMatchesSource) { Write-Warning 'Using a stale QA stage; results do not verify current source' }
$portProbe = New-Object Net.Sockets.TcpListener([Net.IPAddress]::Loopback, 8765)
try { $portProbe.Start() } finally { $portProbe.Stop() }

# --- Isolated run dir --------------------------------------------------------
$ts = (Get-Date).ToString("yyyyMMdd-HHmmss-fff")
$run = Resolve-QaChildPath -Path (Join-Path $QaRunRoot $ts) -Root $QaRunRoot
New-Item -ItemType Directory -Force -Path $run | Out-Null
New-Item -ItemType Directory -Force -Path (Join-Path $run "data") | Out-Null
New-Item -ItemType Directory -Force -Path (Join-Path $run "fractalengine") | Out-Null

# Copy the fixture assets into the run-local workspace layout.
$assetsLink = Join-Path $run "fractalengine\assets"
$assetsSrc = Join-Path $QaRepoRoot "fractalengine\assets"
New-Item -ItemType Directory -Path $assetsLink | Out-Null
foreach ($name in @('branding', 'samples')) {
    $source = Join-Path $assetsSrc $name
    if (Test-Path -LiteralPath $source) { Copy-Item -LiteralPath $source -Destination $assetsLink -Recurse }
}
Get-ChildItem -LiteralPath $assetsSrc -File | ForEach-Object { Copy-Item -LiteralPath $_.FullName -Destination $assetsLink }

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
    $installOut = & $hexonInstaller $sampleHexonsDir
    if ($LASTEXITCODE -ne 0) { throw "Sample hexon installation failed: $LASTEXITCODE" }
    $installOut | ForEach-Object { Write-QaLog "install_sample_hexons: $_" }
    $installed = $installOut | Select-String -Pattern "^(installed|refreshed) (\S+)"
    if ($installed) {
        $terrainHexonId = $installed[0].Matches[0].Groups[2].Value
    }
}

# --- Launch -------------------------------------------------------------
$env:FE_API_DEV_TOKEN_OUT = $tokensPath
$env:FE_QA_STORAGE_ROOT = $run
$env:FE_API_INPUT_ALLOW = "1"
$env:FE_SYNC_RELAY = "disabled"
$env:FE_HEXON_DIR = $hexonDir
$env:RUST_LOG = "info"

$swHealth = [System.Diagnostics.Stopwatch]::StartNew()
$proc = $null
$identity = $null
$handedOff = $false
try {
$proc = Start-Process -FilePath $exe -WorkingDirectory $run -PassThru `
    -RedirectStandardOutput (Join-Path $run "stdout.log") `
    -RedirectStandardError (Join-Path $run "stderr.log") `
    -WindowStyle Hidden
$null = $proc.Handle
$identity = Get-QaProcessIdentity -ProcessId $proc.Id
if (-not $identity) { throw 'QA process exited before its identity could be recorded' }
$startedAt = (Get-Date).ToString("o")
Write-QaLog "Launched pid=$($proc.Id) run=$run"

# Throw into the enclosing lifecycle cleanup on every startup failure.
function Fail {
    param([string]$Message)
    throw $Message
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

    # A GLB with a real scene + mesh: fe-test-harness's 48-byte test_cube.glb
    # has no `scenes`, so its nodes rendered nothing (tools/qa/AGENTS.md §fixture-basic).
    $glbPath = Join-Path $QaRepoRoot "fractalengine\assets\samples\info_panel.glb"
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
    process_identity = $identity
    stage_manifest = $stageManifest
    stage_matches_source = $stageMatchesSource
    run_dir = $run
    bootstrap_verse_id = $bootstrap.VerseId
    fixture_detail = $fixtureInfo
    timings_ms = @{ health = $healthMs; bootstrap_token = $tokenMs; fixture_seed = $fixtureMs }
}
$current | ConvertTo-Json -Depth 6 | Set-Content (Join-Path $run "current.json") -Encoding utf8
$run | Set-Content (Join-Path $QaRepoRoot ".qa\current") -Encoding ascii -NoNewline
$handedOff = $true

Write-QaLog "Launch complete in $([math]::Round($swTotal.Elapsed.TotalSeconds, 1))s total. run=$run pid=$($proc.Id)"
Write-Output $run
} finally {
    if (-not $handedOff -and $proc) {
        if ($identity) { Stop-QaOwnedProcessTree -Identity $identity }
        elseif (-not $proc.HasExited) { $proc.Kill(); $null = $proc.WaitForExit(10000) }
    }
    if ($proc) { $proc.Dispose() }
}
