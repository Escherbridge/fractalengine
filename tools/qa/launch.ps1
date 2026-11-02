# tools/qa/launch.ps1 — start a staged build in a fresh, isolated run dir
# and seed checked basic/terrain scenes over REST before
# handing back control. See tools/qa/AGENTS.md for the full design and the
# non-obvious gotchas this script works around.
#
# Usage: powershell -File tools\qa\launch.ps1 [-Stage <sha>] [-Fixture basic|terrain|none]
[CmdletBinding()]
param(
    [string]$Stage,
    [ValidateNotNullOrEmpty()][string]$ResumeRun,
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

$lifecycleLease = Enter-QaLifecycleLease
$resume = $null
$runLock = $null
$attemptPath = $null
try {
if ($ResumeRun) {
    $resume = Get-QaResumeContext -Run $ResumeRun
    $run = $resume.directory
    $runLock = [IO.File]::Open((Join-Path $run 'restart.lock'), [IO.FileMode]::OpenOrCreate, [IO.FileAccess]::ReadWrite, [IO.FileShare]::None)
    $resume = Get-QaResumeContext -Run $run
    if ($PSBoundParameters.ContainsKey('Fixture')) { throw 'ResumeRun preserves the existing fixture; do not supply Fixture' }
    $Fixture = $resume.current.fixture
    if (-not $PSBoundParameters.ContainsKey('ApiPort')) { $ApiPort = $resume.port }
    foreach ($option in @(@('DisableSyntheticInput','synthetic_input'), @('A11yTiming','a11y_timing'), @('A11yContinuous','a11y_continuous'))) {
        if (-not $PSBoundParameters.ContainsKey($option[0])) {
            $value = [bool]$resume.current.qa_options.($option[1])
            if ($option[0] -eq 'DisableSyntheticInput') { $value = -not $value }
            Set-Variable -Name $option[0] -Value ([switch]$value)
        }
    }
    if ($PSBoundParameters.ContainsKey('A11yTiming') -and -not $A11yTiming -and -not $PSBoundParameters.ContainsKey('A11yContinuous')) {
        $A11yContinuous = [switch]$false
    }
    if ($A11yTiming) {
        foreach ($marker in @('a11y-timing-owner.json','a11y-baseline-ready.json','a11y-timing.json')) {
            if (Test-Path -LiteralPath (Join-Path $run $marker)) {
                throw 'Resume timing requires fresh exclusive markers. Existing proof is preserved; explicitly use -A11yTiming:$false or create a fresh timing run.'
            }
        }
    }
}
if ($A11yTiming -and -not $DisableSyntheticInput) { throw 'A11yTiming requires DisableSyntheticInput' }
if ($A11yContinuous -and -not $A11yTiming) { throw 'A11yContinuous requires A11yTiming' }
$QaApiBase = "http://127.0.0.1:$ApiPort"
$swTotal = [System.Diagnostics.Stopwatch]::StartNew()

# --- Resolve the stage dir -------------------------------------------------
if ($Stage) {
    $stageDir = Join-Path $QaStageRoot $Stage
} elseif ($resume) {
    $stageDir = $resume.current.stage
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
if ($resume) {
    $attemptId = (Get-Date).ToString('yyyyMMdd-HHmmss-fff') + '-' + [Guid]::NewGuid().ToString('N')
    $attemptDir = Resolve-QaChildPath -Path (Join-Path $run "starts\$attemptId") -Root $QaRunRoot
    New-Item -ItemType Directory -Path $attemptDir -Force | Out-Null
    $attemptPath = Join-Path $attemptDir 'start.json'
    $prior = $resume.current
    $previousStart = @{}
    foreach ($key in @('pid','api_base','stage','started_at','process_identity','descendant_identities','descendants_captured_at','stage_manifest','stage_matches_source','stale_stage_allowed','qa_options','restart')) {
        $previousStart[$key] = $prior.$key
    }
    $previousStart | ConvertTo-Json -Depth 16 | Set-Content -LiteralPath (Join-Path $attemptDir 'previous-start.json') -Encoding utf8
    @{ status = 'stopping_previous'; run_dir = $run; stage = $stageDir; stage_manifest = $stageManifest; stage_matches_source = $stageMatchesSource; stale_stage_allowed = [bool]$AllowStaleStage } |
        ConvertTo-Json -Depth 8 | Set-Content -LiteralPath $attemptPath -Encoding utf8
    $owned = Get-QaOwnedProcess -Identity $prior.process_identity
    if ($owned) { $owned.Dispose() }
    else { Write-Warning 'Root already exited; only recorded descendants can be checked, not children born after its last snapshot' }
    Stop-QaOwnedProcessTree -Identity $prior.process_identity -RecordedDescendants @($prior.descendant_identities)
}
$portProbe = New-Object Net.Sockets.TcpListener([Net.IPAddress]::Loopback, $ApiPort)
try { $portProbe.Start() } finally { $portProbe.Stop() }

# --- Isolated run dir --------------------------------------------------------
if (-not $resume) {
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

}
$tokensPath = Resolve-QaChildPath -Path (Join-Path $run "tokens.json") -Root $QaRunRoot
$hexonDir = Join-Path $run "hexons"

# Generate and install the requested offline fixture before the GUI opens its store.
$terrainDescriptor = $null
if (-not $resume -and $Fixture -ne 'none') {
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
if ($resume) {
    Assert-QaStorageTree -Directory $run
    if (Test-Path -LiteralPath $tokensPath) { Remove-Item -LiteralPath $tokensPath -Force }
}
$stdoutPath = Join-Path $run 'stdout.log'
$stderrPath = Join-Path $run 'stderr.log'
if ($resume) {
    $stdoutPath = Join-Path $attemptDir 'stdout.log'
    $stderrPath = Join-Path $attemptDir 'stderr.log'
}

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
$recordedDescendants = @()
try {
$priorEnvironment = @{}
foreach ($key in $appEnvironment.Keys) { $priorEnvironment[$key] = [Environment]::GetEnvironmentVariable($key, 'Process') }
try {
    foreach ($key in $appEnvironment.Keys) { [Environment]::SetEnvironmentVariable($key, $appEnvironment[$key], 'Process') }
    $proc = Start-Process -FilePath $exe -WorkingDirectory $run -PassThru `
        -RedirectStandardOutput $stdoutPath `
        -RedirectStandardError $stderrPath `
        -WindowStyle Hidden
} finally {
    foreach ($key in $appEnvironment.Keys) { [Environment]::SetEnvironmentVariable($key, $priorEnvironment[$key], 'Process') }
}
$null = $proc.Handle
$identity = Get-QaProcessIdentity -ProcessId $proc.Id
if (-not $identity) { throw 'QA process exited before its identity could be recorded' }
$startedAt = (Get-Date).ToString("o")
if ($attemptPath) {
    @{ status = 'starting'; run_dir = $run; process_identity = $identity; started_at = $startedAt; stage = $stageDir; stage_manifest = $stageManifest; stage_matches_source = $stageMatchesSource; stale_stage_allowed = [bool]$AllowStaleStage } |
        ConvertTo-Json -Depth 8 | Set-Content -LiteralPath $attemptPath -Encoding utf8
}
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
$bootstrapVerse = if ($resume) { $resume.current.bootstrap_verse_id } else { $null }
$bootstrap = Wait-QaOwnerToken -TokensPath $tokensPath -VerseId $bootstrapVerse -TimeoutSec 60
if (-not $bootstrap) {
    Fail "No owner token appeared in $tokensPath within 60s"
}
$tokenMs = $swToken.ElapsedMilliseconds
Write-QaLog "Bootstrap owner token ready (verse $($bootstrap.VerseId)) after ${tokenMs}ms"

$liveProcess = Get-QaOwnedProcess -Identity $identity
if (-not $liveProcess) { Fail 'New QA process exited before authenticated readiness' }
try {
    $ready = Invoke-WebRequest -Uri "$QaApiBase/api/v1/hierarchy" -UseBasicParsing -MaximumRedirection 0 -TimeoutSec 10 -Headers @{ Authorization = "Bearer $($bootstrap.Token)" }
    $readyBody = $ready.Content | ConvertFrom-Json
    if ($readyBody.ok -ne $true) { Fail 'New API did not confirm authenticated readiness' }
} finally { $liveProcess.Dispose() }
$fixtureInfo = if ($resume) { $resume.current.fixture_detail } else { $null }
$swFixture = [System.Diagnostics.Stopwatch]::StartNew()
if (-not $resume -and $Fixture -ne 'none') {
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
    stdout_path = $stdoutPath
    stderr_path = $stderrPath
    qa_options = @{ synthetic_input = -not [bool]$DisableSyntheticInput; a11y_timing = [bool]$A11yTiming; a11y_continuous = [bool]$A11yContinuous }
    started_at = $startedAt
    process_identity = $identity
    descendant_identities = $recordedDescendants
    descendants_captured_at = [DateTime]::UtcNow.ToString('o')
    stage_manifest = $stageManifest
    stage_matches_source = $stageMatchesSource
    stale_stage_allowed = [bool]$AllowStaleStage
    run_dir = $run
    bootstrap_verse_id = $bootstrap.VerseId
    fixture_detail = $fixtureInfo
    timings_ms = @{ health = $healthMs; bootstrap_token = $tokenMs; fixture_seed = $fixtureMs }
}
if ($resume) {
    $current.restart = @{ previous_start = (Join-Path $attemptDir 'previous-start.json'); attempt = $attemptPath; preserved_root = $run; reseeded = $false; port_changed = ($ApiPort -ne $resume.port); stale_stage_allowed = [bool]$AllowStaleStage }
    @{ status = 'ready'; run_dir = $run; process_identity = $identity; started_at = $startedAt; stage = $stageDir; stage_manifest = $stageManifest; stage_matches_source = $stageMatchesSource; restart = $current.restart } |
        ConvertTo-Json -Depth 8 | Set-Content -LiteralPath $attemptPath -Encoding utf8
}
$current | ConvertTo-Json -Depth 8 | Set-Content -LiteralPath (Join-Path $run 'current.pending.json') -Encoding utf8
Move-Item -LiteralPath (Join-Path $run 'current.pending.json') -Destination (Join-Path $run 'current.json') -Force
$run | Set-Content (Join-Path $QaRepoRoot ".qa\current") -Encoding ascii -NoNewline
$handedOff = $true

Write-QaLog "Launch complete in $([math]::Round($swTotal.Elapsed.TotalSeconds, 1))s total. run=$run pid=$($proc.Id)"
Write-Output $run
} finally {
    try {
        if (-not $handedOff -and $attemptPath) {
            @{ status = 'failed'; run_dir = $run; process_identity = $identity; stage = $stageDir; stage_manifest = $stageManifest; stage_matches_source = $stageMatchesSource; stale_stage_allowed = [bool]$AllowStaleStage } |
                ConvertTo-Json -Depth 8 | Set-Content -LiteralPath $attemptPath -Encoding utf8
        }
    } finally {
        try {
            if (-not $handedOff -and $proc) {
                if ($identity) { Stop-QaOwnedProcessTree -Identity $identity -RecordedDescendants @($recordedDescendants) }
                elseif (-not $proc.HasExited) {
                    $proc.Kill()
                    if (-not $proc.WaitForExit(15000)) { throw 'New start did not exit within rollback deadline' }
                }
            }
        } finally { if ($proc) { $proc.Dispose() } }
    }
}

} finally {
    try { if ($runLock) { $runLock.Dispose() } }
    finally { $lifecycleLease.Dispose() }
}
