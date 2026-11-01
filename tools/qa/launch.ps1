# tools/qa/launch.ps1 — start a staged build in a fresh, isolated run dir
# and seed checked basic/terrain scenes over REST before
# handing back control. See tools/qa/AGENTS.md for the full design and the
# non-obvious gotchas this script works around.
#
# Usage: powershell -File tools\qa\launch.ps1 [-Stage <sha>] [-Fixture basic|terrain|none]
[CmdletBinding()]
param(
    [string]$Stage,
    [ValidateSet("basic", "terrain", "none")]
    [string]$Fixture = "basic",
    [ValidateRange(1,65535)][int]$ApiPort = 8765,
    [switch]$DisableSyntheticInput,
    [switch]$A11yTiming,
    [switch]$A11yContinuous,
    [switch]$AllowStaleStage
)

. "$PSScriptRoot\_lib.ps1"
. "$PSScriptRoot\_fixture.ps1"
$QaApiBase = "http://127.0.0.1:$ApiPort"
$ErrorActionPreference = 'Stop'
if ($A11yTiming -and -not $DisableSyntheticInput) { throw 'A11yTiming requires DisableSyntheticInput for a normal OS comparison' }
if ($A11yContinuous -and -not $A11yTiming) { throw 'A11yContinuous requires A11yTiming' }

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
$portProbe = New-Object Net.Sockets.TcpListener([Net.IPAddress]::Loopback, $ApiPort)
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

# Generate and install the requested offline fixture before the GUI opens its store.
$terrainDescriptor = $null
if ($Fixture -ne 'none') {
    $terrainBuilder = Join-Path $stageDir 'build_qa_terrain.exe'
    if (-not (Test-Path -LiteralPath $terrainBuilder)) { throw 'Requested fixture requires a staged build_qa_terrain.exe' }
    $builderOutput = & $terrainBuilder $run
    if ($LASTEXITCODE -ne 0) { throw "Offline QA terrain generation failed: $LASTEXITCODE" }
    $descriptorPath = Join-Path $run 'terrain-fixture/descriptor.json'
    $terrainDescriptor = Get-Content -LiteralPath $descriptorPath -Raw | ConvertFrom-Json
    if ($terrainDescriptor.schema -cne 'fe-qa-terrain-v1' -or $terrainDescriptor.tile_count -ne 9 -or
        $terrainDescriptor.flat_pixel_samples -le 0 -or $terrainDescriptor.encoded_peak_m -le 7 -or
        $terrainDescriptor.terrain_config.elevation_source -cne 'terrain_rgb' -or
        $terrainDescriptor.terrain_config.tile_source_mode -cne 'offline' -or
        $terrainDescriptor.terrain_config.world_scale -ne 1) { throw 'Offline QA terrain descriptor is invalid' }
    Write-QaLog "Installed deterministic offline terrain: $($terrainDescriptor.tileset_id)"
}

# --- Launch -------------------------------------------------------------
$appEnvironment = @{
    FE_API_DEV_TOKEN_OUT = $tokensPath
    FE_QA_STORAGE_ROOT = $run
    FE_QA_API_PORT = $ApiPort.ToString()
    FE_API_INPUT_ALLOW = $(if ($DisableSyntheticInput) { '0' } else { '1' })
    FE_QA_A11Y_TIMING = $(if ($A11yTiming) { '1' } else { '0' })
    FE_QA_A11Y_CONTINUOUS = $(if ($A11yContinuous) { '1' } else { '0' })
    FE_SYNC_RELAY = 'disabled'
    FE_HEXON_DIR = $hexonDir
    RUST_LOG = 'info'
}

$swHealth = [System.Diagnostics.Stopwatch]::StartNew()
$proc = $null
$identity = $null
$handedOff = $false
try {
$priorEnvironment = @{}
foreach ($key in $appEnvironment.Keys) { $priorEnvironment[$key] = [Environment]::GetEnvironmentVariable($key, 'Process') }
try {
    foreach ($key in $appEnvironment.Keys) { [Environment]::SetEnvironmentVariable($key, $appEnvironment[$key], 'Process') }
    $proc = Start-Process -FilePath $exe -WorkingDirectory $run -PassThru `
        -RedirectStandardOutput (Join-Path $run "stdout.log") `
        -RedirectStandardError (Join-Path $run "stderr.log") `
        -WindowStyle Hidden
} finally {
    foreach ($key in $appEnvironment.Keys) { [Environment]::SetEnvironmentVariable($key, $priorEnvironment[$key], 'Process') }
}
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
if ($Fixture -ne 'none') {
    $fixtureInfo = New-QaSceneFixture -Name $Fixture -BootstrapToken $bootstrap.Token -TokensPath $tokensPath -Run $run -TerrainDescriptor $terrainDescriptor
    Write-QaLog "Fixture '$Fixture' verified: 3 values sum to 60; terrain petals=$($fixtureInfo.terrain_petal_ids -join ',')"
}
$fixtureMs = $swFixture.ElapsedMilliseconds
$capturedTree = @(Get-QaOwnedProcessTree -Identity $identity)
if (-not ($capturedTree | Where-Object { [int]$_.pid -eq [int]$identity.pid })) { throw 'QA process exited before successful handoff' }
$recordedDescendants = @($capturedTree | Where-Object { [int]$_.pid -ne [int]$identity.pid })

$current = @{
    pid = $proc.Id
    api_base = $QaApiBase
    tokens_path = $tokensPath
    stage = $stageDir
    fixture = $Fixture
    qa_options = @{ synthetic_input = -not [bool]$DisableSyntheticInput; a11y_timing = [bool]$A11yTiming; a11y_continuous = [bool]$A11yContinuous }
    started_at = $startedAt
    process_identity = $identity
    descendant_identities = $recordedDescendants
    descendants_captured_at = [DateTime]::UtcNow.ToString('o')
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
