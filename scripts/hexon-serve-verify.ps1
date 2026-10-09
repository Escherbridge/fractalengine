<#
.SYNOPSIS
    F16 (A29) V2 end-to-end verification: a live fe-relay installs F15's
    produced `.hexon` tilesets and serves their tiles over the authenticated,
    petal-bound tile data plane (fe-api/src/terrain.rs).

.DESCRIPTION
    conductor/tracks/mission_prodready_continuation_20261008/m6-gis-regions-design.md.

    REST/MCP petal-terrain mutation is deliberately refused
    (fe-api/src/terrain.rs::TERRAIN_MUTATION_UNAVAILABLE, pending durable
    DbCommand-reply correlation -- see fe-api/src/terrain.rs:153-177), so
    there is NO authed path to bind a tileset to a petal on a live relay.
    This script works around that the same way
    fe-database/examples/seed_join_verse.rs already does for verse rows:
    stop the relay, write the binding directly into its SurrealKV store
    (same `UPDATE petal SET terrain = $config WHERE petal_id = $pid`
    statement as the disabled handler), restart it. That is the
    "binding path" this script documents in its own summary.

    Flow:
      1. Pre-install THREE real hexon tilesets into a fresh FE_HEXON_DIR via
         `cargo run -p fe-terrain --example install_sample_hexons` (BEFORE
         any relay starts -- the store is a plain directory, no DB lock).
         Two (switzerland, japan) will be bound to the test petal; the third
         (the pre-existing north-america-pacific-northwest.hexon) is left
         deliberately UNBOUND to any petal -- it is the negative fixture for
         "an unbound tileset's tile -> denied".
      2. Start relay #1, seed verse/fractal/petal (bound) + a second,
         unrelated foreign petal over REST, mint petal-scoped + foreign-scoped
         tokens, stop relay #1 (releases the SurrealKV file lock).
      3. Direct-DB-bind: `seed_join_verse bind-terrain` writes
         petal.terrain.tileset_hexon_uris = [switzerland_id, japan_id] on the
         bound petal only.
      4. Start relay #2 (same FE_DB_PATH + FE_HEXON_DIR + node-keypair seed,
         so the tokens minted against relay #1 are still valid) and drive the
         real authed HTTP surface:
           - GET /api/v1/tilesets?petal_id=<bound> -> lists exactly the two
             bound tilesets (not the unbound third).
           - GET one elevation + one satellite tile for the bound switzerland
             tileset -> 200, correct content-type, real decoded-image magic
             bytes (not a synthetic placeholder -- these are the real F15
             archives).
           - A token scoped to the FOREIGN petal -> 403 on the bound petal's
             tileset list AND tile routes (fe-policy scope check fails before
             the registry is ever touched).
           - The bound petal's OWN valid token against the UNBOUND third
             tileset's tile route -> 404, not 403 (authorize_tileset_read's
             `is_assigned_tileset` check fails after the scope check passes
             -- the "don't reveal unbound existence" contract, exercised
             against a tileset that genuinely exists in the local store).

    Never modifies gis-tile-etl (reads its dist/ dir only). Always tears
    down both relay processes by exact PID and removes the temp dir, even on
    failure (try/finally).
#>
param(
    [string]$DistDir = (Join-Path (Split-Path (Split-Path $PSScriptRoot -Parent) -Parent) 'gis-tile-etl\dist')
)

$ErrorActionPreference = 'Continue'
$script:Failures = @()
$script:Passed = 0
$script:Checked = 0
$script:Relay1 = $null
$script:Relay2 = $null
$script:WorkDir = $null

function Write-Section($title) {
    Write-Host ""
    Write-Host "=== $title ===" -ForegroundColor Cyan
}

function Assert-True($condition, $message) {
    $script:Checked++
    if ($condition) {
        $script:Passed++
        Write-Host "  [PASS] $message" -ForegroundColor Green
    } else {
        $script:Failures += $message
        Write-Host "  [FAIL] $message" -ForegroundColor Red
    }
}

function Assert-Equal($actual, $expected, $message) {
    Assert-True ($actual -eq $expected) "$message (expected [$expected], got [$actual])"
}

# ---------------------------------------------------------------------------
# Setup
# ---------------------------------------------------------------------------

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

Write-Section "Resolve F15/F16 hexon archives"
Write-Host "  DistDir: $DistDir"
$switzerlandPath = Join-Path $DistDir 'switzerland-zurich-alps.hexon'
$japanPath = Join-Path $DistDir 'japan-mount-fuji.hexon'
$pacificPath = Join-Path $DistDir 'north-america-pacific-northwest.hexon'
$switzerlandId = 'tileset-switzerland-zurich-alps'
$japanId = 'tileset-japan-mount-fuji'
$pacificId = 'tileset-north-america-pacific-northwest'
Assert-True (Test-Path $switzerlandPath) "found $switzerlandPath"
Assert-True (Test-Path $japanPath) "found $japanPath"
Assert-True (Test-Path $pacificPath) "found $pacificPath (unbound negative fixture)"
if ($script:Failures.Count -gt 0) {
    Write-Host "Cannot continue without all three dist archives." -ForegroundColor Red
    exit 1
}

$stamp = Get-Date -Format 'yyyyMMdd-HHmmss'
$script:WorkDir = Join-Path $env:TEMP "fe-hexon-serve-verify-$stamp"
New-Item -ItemType Directory -Path $script:WorkDir -Force | Out-Null
$dbPath = Join-Path $script:WorkDir 'db'
$p2pDir = Join-Path $script:WorkDir 'p2p'
$hexonDir = Join-Path $script:WorkDir 'hexons'
$logPath = Join-Path $PSScriptRoot 'hexon-serve-verify.log'
Write-Host "  Work dir: $script:WorkDir"
Write-Host "  Log: $logPath"
"" | Set-Content -Path $logPath
function Log-Line($line) { $line | Add-Content -Path $logPath }

$BindAddr = '127.0.0.1:18766'
$BaseUrl = "http://$BindAddr"

$seedBytes = New-Object byte[] 32
[System.Security.Cryptography.RandomNumberGenerator]::Create().GetBytes($seedBytes)
$SeedHex = -join ($seedBytes | ForEach-Object { $_.ToString('x2') })

$env:RUST_MIN_STACK = '134217728'
if (-not $env:CARGO_BUILD_JOBS) { $env:CARGO_BUILD_JOBS = '2' }

function Start-Relay($bindAddr, $dbPath, $p2pDir, $hexonDir, $seedHex, $outLog, $errLog) {
    $psi = New-Object System.Diagnostics.ProcessStartInfo
    $psi.FileName = (Join-Path $RepoRoot 'target\debug\fe-relay.exe')
    $psi.WorkingDirectory = $RepoRoot
    $psi.UseShellExecute = $false
    $psi.RedirectStandardOutput = $true
    $psi.RedirectStandardError = $true
    $psi.EnvironmentVariables['FE_BIND_ADDR'] = $bindAddr
    $psi.EnvironmentVariables['FE_DB_PATH'] = $dbPath
    $psi.EnvironmentVariables['FE_P2P_DIR'] = $p2pDir
    $psi.EnvironmentVariables['FE_HEXON_DIR'] = $hexonDir
    $psi.EnvironmentVariables['FE_SYNC_RELAY'] = 'disabled'
    $psi.EnvironmentVariables['FE_SHUTDOWN_AFTER_SECS'] = '600'
    $psi.EnvironmentVariables['RUST_MIN_STACK'] = '134217728'
    $psi.EnvironmentVariables['RUST_LOG'] = 'info,fe_api=debug'
    $psi.EnvironmentVariables['FE_SECRET_FRACTALENGINE_NODE_KEYPAIR'] = $seedHex

    $proc = [System.Diagnostics.Process]::Start($psi)
    Write-Host "  Started fe-relay.exe PID=$($proc.Id)"

    $outWriter = [System.IO.StreamWriter]::new($outLog, $false)
    $errWriter = [System.IO.StreamWriter]::new($errLog, $false)
    $outEvent = Register-ObjectEvent -InputObject $proc -EventName OutputDataReceived -Action {
        if ($EventArgs.Data -ne $null) { $Event.MessageData.WriteLine($EventArgs.Data); $Event.MessageData.Flush() }
    } -MessageData $outWriter
    $errEvent = Register-ObjectEvent -InputObject $proc -EventName ErrorDataReceived -Action {
        if ($EventArgs.Data -ne $null) { $Event.MessageData.WriteLine($EventArgs.Data); $Event.MessageData.Flush() }
    } -MessageData $errWriter
    $proc.BeginOutputReadLine()
    $proc.BeginErrorReadLine()

    $deadline = (Get-Date).AddSeconds(60)
    $ready = $false
    while ((Get-Date) -lt $deadline) {
        if ($proc.HasExited) {
            Start-Sleep -Milliseconds 300
            $errTail = (Get-Content $errLog -Tail 20 -ErrorAction SilentlyContinue) -join "`n"
            throw "fe-relay exited early (code $($proc.ExitCode)) before /ready:`n$errTail"
        }
        try {
            $resp = Invoke-WebRequest -Uri "http://$bindAddr/ready" -UseBasicParsing -TimeoutSec 2
            if ($resp.StatusCode -eq 200) { $ready = $true; break }
        } catch { Start-Sleep -Milliseconds 500 }
    }
    if (-not $ready) { throw "relay never became ready (log: $outLog / $errLog)" }

    return @{
        Process = $proc; OutWriter = $outWriter; ErrWriter = $errWriter
        OutEvent = $outEvent; ErrEvent = $errEvent; OutLog = $outLog; ErrLog = $errLog
    }
}

function Stop-Relay($handle) {
    if (-not $handle) { return }
    if ($handle.OutEvent) { Unregister-Event -SourceIdentifier $handle.OutEvent.Name -ErrorAction SilentlyContinue }
    if ($handle.ErrEvent) { Unregister-Event -SourceIdentifier $handle.ErrEvent.Name -ErrorAction SilentlyContinue }
    if ($handle.Process -and -not $handle.Process.HasExited) {
        Write-Host "  Stopping fe-relay.exe PID=$($handle.Process.Id)"
        try { $handle.Process.Kill() } catch { Write-Host "  (already exited)" }
        $handle.Process.WaitForExit(5000) | Out-Null
    }
    if ($handle.OutWriter) { $handle.OutWriter.Close() }
    if ($handle.ErrWriter) { $handle.ErrWriter.Close() }
    # Give Windows a brief moment to fully release the SurrealKV file handle
    # before the next process (bind-terrain tool, or relay #2) opens it.
    Start-Sleep -Milliseconds 500
}

function Invoke-Api($method, $path, $token, $bodyObj) {
    $headers = @{ Authorization = "Bearer $token" }
    $uri = "$BaseUrl$path"
    if ($null -ne $bodyObj) {
        $json = $bodyObj | ConvertTo-Json -Depth 10 -Compress
        return Invoke-RestMethod -Method $method -Uri $uri -Headers $headers -ContentType 'application/json' -Body $json
    }
    return Invoke-RestMethod -Method $method -Uri $uri -Headers $headers
}

function Get-ApiStatus($method, $path, $token, $bodyObj) {
    try {
        Invoke-Api $method $path $token $bodyObj | Out-Null
        return 200
    } catch {
        if ($_.Exception.Response) { return [int]$_.Exception.Response.StatusCode }
        return 0
    }
}

# Fetches a tile URL with binary safety (Windows PowerShell 5.1's
# -UseBasicParsing mangles binary bytes through $resp.Content as a string,
# so this always routes through -OutFile and re-reads raw bytes).
function Get-TileResponse($url, $token) {
    $outFile = Join-Path $script:WorkDir ("tile-{0}.bin" -f ([guid]::NewGuid()))
    try {
        $resp = Invoke-WebRequest -Uri $url -Headers @{ Authorization = "Bearer $token" } `
            -UseBasicParsing -OutFile $outFile -PassThru
        $bytes = [System.IO.File]::ReadAllBytes($outFile)
        return @{ Status = [int]$resp.StatusCode; ContentType = [string]$resp.Headers['Content-Type']; Bytes = $bytes }
    } catch {
        $status = 0
        if ($_.Exception.Response) { $status = [int]$_.Exception.Response.StatusCode }
        return @{ Status = $status; ContentType = $null; Bytes = @() }
    } finally {
        Remove-Item -Path $outFile -ErrorAction SilentlyContinue
    }
}

function Test-PngMagic($bytes) {
    return ($bytes.Length -ge 4 -and $bytes[0] -eq 0x89 -and $bytes[1] -eq 0x50 -and $bytes[2] -eq 0x4E -and $bytes[3] -eq 0x47)
}

function Test-JpegMagic($bytes) {
    return ($bytes.Length -ge 3 -and $bytes[0] -eq 0xFF -and $bytes[1] -eq 0xD8 -and $bytes[2] -eq 0xFF)
}

function Mint-Token($mintExe, $seedHex, $scope, $role, $jti) {
    $out = & $mintExe --seed-hex $seedHex --scope $scope --role $role --ttl-secs 3600 --jti $jti
    $line = $out | Where-Object { $_ -like 'token:*' }
    if (-not $line) { throw "mint_api_token produced no token line for jti=$jti : $out" }
    return $line.Substring('token: '.Length).Trim()
}

try {
    # -----------------------------------------------------------------------
    # Build (serial -- this script owns the build lock)
    # -----------------------------------------------------------------------
    Write-Section "Build (incremental, strictly serial)"
    Write-Host "  cargo build -p fractalengine-relay ..."
    cargo build -q -p fractalengine-relay 2>&1 | ForEach-Object { Write-Host "    $_" }
    if ($LASTEXITCODE -ne 0) { throw "cargo build -p fractalengine-relay failed" }
    Write-Host "  cargo build -p fe-identity --example mint_api_token ..."
    cargo build -q -p fe-identity --example mint_api_token 2>&1 | ForEach-Object { Write-Host "    $_" }
    if ($LASTEXITCODE -ne 0) { throw "cargo build -p fe-identity --example mint_api_token failed" }
    $mintExe = Join-Path $RepoRoot 'target\debug\examples\mint_api_token.exe'

    # -----------------------------------------------------------------------
    # Port pre-check
    # -----------------------------------------------------------------------
    Write-Section "Port pre-check"
    $probe = New-Object System.Net.Sockets.TcpListener([System.Net.IPAddress]::Loopback, 18766)
    try { $probe.Start(); $probe.Stop() }
    catch { throw "port $BindAddr is already in use -- stop the other listener first" }
    Write-Host "  port 18766 is free"

    # -----------------------------------------------------------------------
    # Pre-install: 3 real hexon tilesets into FE_HEXON_DIR (no relay running
    # yet -- the store is a plain directory, no DB file-lock contention).
    # -----------------------------------------------------------------------
    Write-Section "Pre-install 3 hexon tilesets (2 to be bound, 1 deliberately unbound)"
    $env:FE_HEXON_DIR = $hexonDir
    Log-Line "=== install_sample_hexons (pre-install) ==="
    $installOut = & cargo run -q -p fe-terrain --example install_sample_hexons -- $switzerlandPath $japanPath $pacificPath 2>&1
    $installOut | Add-Content -Path $logPath
    $installOut | ForEach-Object { Write-Host "    $_" }
    $installOutText = ($installOut -join "`n")
    Assert-True ($installOutText -match "installed $switzerlandId") "pre-installed switzerland tileset"
    Assert-True ($installOutText -match "installed $japanId") "pre-installed japan tileset"
    Assert-True ($installOutText -match "installed $pacificId") "pre-installed pacific-northwest tileset (unbound fixture)"
    Assert-True ($installOutText -match '3 tileset\(s\) now installed') "registry reports 3 tilesets"
    # Explicit FE_HEXON_DIR was set by install_sample_hexons via the env var
    # below for the `cargo run` call itself (set before, cleared never --
    # see env assignment immediately preceding); re-confirm on disk too.
    Assert-True (Test-Path (Join-Path $hexonDir 'registry.json')) "registry.json present in FE_HEXON_DIR"

    # -----------------------------------------------------------------------
    # Relay #1: seed hierarchy, mint tokens, then stop (release DB lock)
    # -----------------------------------------------------------------------
    Write-Section "Starting fe-relay #1 (seed phase)"
    $env:FE_HEXON_DIR = $hexonDir
    $relay1OutLog = Join-Path $script:WorkDir 'relay1.log'
    $relay1ErrLog = Join-Path $script:WorkDir 'relay1-err.log'
    $script:Relay1 = Start-Relay $BindAddr $dbPath $p2pDir $hexonDir $SeedHex $relay1OutLog $relay1ErrLog
    Assert-True $true "relay #1 /ready within 60s"

    Write-Section "Seeding hierarchy over REST (bound petal + unrelated foreign petal)"
    $bootToken = Mint-Token $mintExe $SeedHex 'VERSE#bootstrap' 'owner' 'boot'
    Assert-True ($bootToken.Length -gt 0) "minted a bootstrap token offline with the relay's seed"

    $verse = Invoke-Api POST '/api/v1/verses' $bootToken @{ name = 'HexonServeVerify' }
    Assert-True $verse.ok "created verse"
    $verseId = $verse.data.id
    $verseToken = Mint-Token $mintExe $SeedHex "VERSE#$verseId" 'owner' 'verse1'

    $fractal = Invoke-Api POST "/api/v1/verses/$verseId/fractals" $verseToken @{ name = 'F1' }
    Assert-True $fractal.ok "created fractal"
    $fractalId = $fractal.data.id

    $petal = Invoke-Api POST "/api/v1/verses/$verseId/fractals/$fractalId/petals" $verseToken @{ name = 'BoundPetal' }
    Assert-True $petal.ok "created bound petal"
    $petalId = $petal.data.id
    $petalScope = "VERSE#$verseId-FRACTAL#$fractalId-PETAL#$petalId"

    $foreignPetal = Invoke-Api POST "/api/v1/verses/$verseId/fractals/$fractalId/petals" $verseToken @{ name = 'ForeignPetal' }
    Assert-True $foreignPetal.ok "created foreign (unrelated) petal"
    $foreignPetalId = $foreignPetal.data.id
    $foreignScope = "VERSE#$verseId-FRACTAL#$fractalId-PETAL#$foreignPetalId"

    $petalToken = Mint-Token $mintExe $SeedHex $petalScope 'viewer' 'petal-viewer'
    $foreignToken = Mint-Token $mintExe $SeedHex $foreignScope 'viewer' 'foreign-viewer'
    Write-Host "  verseId=$verseId fractalId=$fractalId petalId=$petalId (bound) foreignPetalId=$foreignPetalId"

    Write-Section "Stopping fe-relay #1 (release SurrealKV lock before direct DB write)"
    Stop-Relay $script:Relay1
    $script:Relay1 = $null

    # -----------------------------------------------------------------------
    # Direct-DB bind: the only available path (REST/MCP terrain mutation is
    # refused -- see script header).
    # -----------------------------------------------------------------------
    Write-Section "Binding terrain directly (seed_join_verse bind-terrain -- relay stopped)"
    Log-Line "=== seed_join_verse bind-terrain ==="
    $bindOut = & cargo run -q -p fe-database --example seed_join_verse -- bind-terrain `
        --db $dbPath --petal-id $petalId --tileset-ids "$switzerlandId,$japanId" 2>&1
    $bindOut | Add-Content -Path $logPath
    $bindOut | ForEach-Object { Write-Host "    $_" }
    $bindOutText = ($bindOut -join "`n")
    Assert-True ($bindOutText -match "petal_id: $petalId") "bind-terrain targeted the correct petal"
    Assert-True ($bindOutText -match [regex]::Escape($switzerlandId)) "bind-terrain wrote the switzerland tileset id"
    Assert-True ($bindOutText -match [regex]::Escape($japanId)) "bind-terrain wrote the japan tileset id"
    Assert-True (-not ($bindOutText -match [regex]::Escape($pacificId))) "bind-terrain did NOT bind the pacific-northwest tileset (negative fixture stays unbound)"

    # -----------------------------------------------------------------------
    # Relay #2: same DB + hexon dir + node-keypair seed -> drive the real
    # authed tile-serving surface.
    # -----------------------------------------------------------------------
    Write-Section "Starting fe-relay #2 (serve phase)"
    $relay2OutLog = Join-Path $script:WorkDir 'relay2.log'
    $relay2ErrLog = Join-Path $script:WorkDir 'relay2-err.log'
    $script:Relay2 = Start-Relay $BindAddr $dbPath $p2pDir $hexonDir $SeedHex $relay2OutLog $relay2ErrLog
    Assert-True $true "relay #2 /ready within 60s"

    # The async stdout-drain event (Register-ObjectEvent) can lag a beat
    # behind the /ready poll succeeding, so this re-checks briefly instead of
    # reading the log exactly once immediately after Start-Relay returns.
    $sawTilesetLoad = $false
    $logDeadline = (Get-Date).AddSeconds(5)
    while ((Get-Date) -lt $logDeadline) {
        $relay2Log = Get-Content $relay2OutLog -Raw -ErrorAction SilentlyContinue
        if ($relay2Log -match 'Loaded hexon tilesets') { $sawTilesetLoad = $true; break }
        Start-Sleep -Milliseconds 300
    }
    Assert-True $sawTilesetLoad "relay #2 startup log shows the hexon tileset registry loading"

    Write-Section "GET /api/v1/tilesets?petal_id=<bound> -- lists exactly the two bound tilesets"
    $listResp = Invoke-Api GET "/api/v1/tilesets?petal_id=$petalId" $petalToken $null
    $listedIds = @($listResp.data | ForEach-Object { $_.tileset_id })
    Write-Host "  listed: $($listedIds -join ', ')"
    Assert-True ($listedIds -contains $switzerlandId) "list includes switzerland (bound)"
    Assert-True ($listedIds -contains $japanId) "list includes japan (bound)"
    Assert-True (-not ($listedIds -contains $pacificId)) "list EXCLUDES pacific-northwest (unbound)"
    Assert-Equal $listedIds.Count 2 "list contains exactly the 2 bound tilesets, nothing more"

    Write-Section "GET elevation + satellite tiles for the bound switzerland tileset (real archive bytes)"
    # z/x/y pinned from `gis-tile-etl verify dist/switzerland-zurich-alps.hexon`
    # 2026-10-09: "sample elevation tile: 10/535/358" / "sample satellite tile:
    # 10/535/358" -- both layers have a tile at this exact coordinate.
    $elevUrl = "$BaseUrl/api/v1/tiles/elevation/$switzerlandId/10/535/358.png?petal_id=$petalId"
    $elevResp = Get-TileResponse $elevUrl $petalToken
    Assert-Equal $elevResp.Status 200 "elevation tile: 200"
    Assert-True ($elevResp.ContentType -match 'image/png') "elevation tile: Content-Type image/png"
    Assert-True ($elevResp.Bytes.Length -gt 1000) "elevation tile: non-trivial byte length ($($elevResp.Bytes.Length) bytes)"
    Assert-True (Test-PngMagic $elevResp.Bytes) "elevation tile: real PNG magic bytes (89 50 4E 47)"

    $satUrl = "$BaseUrl/api/v1/tiles/satellite/$switzerlandId/10/535/358.jpg?petal_id=$petalId"
    $satResp = Get-TileResponse $satUrl $petalToken
    Assert-Equal $satResp.Status 200 "satellite tile: 200"
    Assert-True ($satResp.ContentType -match 'image/jpeg') "satellite tile: Content-Type image/jpeg"
    Assert-True ($satResp.Bytes.Length -gt 1000) "satellite tile: non-trivial byte length ($($satResp.Bytes.Length) bytes)"
    Assert-True (Test-JpegMagic $satResp.Bytes) "satellite tile: real JPEG magic bytes (FF D8 FF)"

    Write-Section "Foreign-petal token denied on the bound petal's tileset surface (403)"
    $foreignListStatus = Get-ApiStatus GET "/api/v1/tilesets?petal_id=$petalId" $foreignToken $null
    Assert-Equal $foreignListStatus 403 "foreign-petal token: list tilesets on bound petal -> 403"
    $foreignTileResp = Get-TileResponse $elevUrl.Replace("petal_id=$petalId", "petal_id=$foreignPetalId") $foreignToken
    # The foreign token's OWN scope IS the foreign petal (role/scope checks
    # pass), but that petal was never bound to any tileset at all --
    # deterministically 404 via is_assigned_tileset on an empty list, not 403.
    Assert-Equal $foreignTileResp.Status 404 "foreign petal's own valid token, requesting switzerland's tile under its own (unbound) petal_id -> 404"
    $crossTileResp = Get-TileResponse $elevUrl $foreignToken
    Assert-Equal $crossTileResp.Status 403 "foreign-SCOPED token requesting the BOUND petal's tile (petal_id=bound) -> 403 (scope check fails before registry lookup)"

    Write-Section "Unbound tileset (real, installed, never bound) -- denied without revealing existence (404)"
    $unboundUrl = "$BaseUrl/api/v1/tiles/elevation/$pacificId/8/0/0.png?petal_id=$petalId"
    $unboundResp = Get-TileResponse $unboundUrl $petalToken
    Assert-Equal $unboundResp.Status 404 "bound petal's OWN valid token on the unbound pacific-northwest tile -> 404 (not 403 -- scope/role passed, binding check failed)"
    $unboundMetaStatus = Get-ApiStatus GET "/api/v1/tilesets/$pacificId/meta?petal_id=$petalId" $petalToken $null
    Assert-Equal $unboundMetaStatus 404 "bound petal's OWN valid token on the unbound pacific-northwest meta route -> 404"

    Write-Host ""
    Write-Host "Full relay #2 log: $relay2OutLog"
}
catch {
    $script:Failures += "UNHANDLED EXCEPTION: $_"
    Write-Host "UNHANDLED EXCEPTION: $_" -ForegroundColor Red
    Write-Host $_.ScriptStackTrace -ForegroundColor Red
}
finally {
    Write-Section "Teardown"
    Stop-Relay $script:Relay1
    Stop-Relay $script:Relay2

    foreach ($candidate in @('relay1.log','relay1-err.log','relay2.log','relay2-err.log')) {
        $p = Join-Path $script:WorkDir $candidate
        if (Test-Path $p) {
            Log-Line "=== $candidate ==="
            Get-Content $p -Raw | Add-Content -Path $logPath
        }
    }

    Remove-Item Env:\FE_HEXON_DIR -ErrorAction SilentlyContinue
    if ($script:WorkDir -and (Test-Path $script:WorkDir)) {
        Write-Host "  Removing temp dir $script:WorkDir"
        Remove-Item -Recurse -Force $script:WorkDir -ErrorAction SilentlyContinue
    }

    Write-Host ""
    Write-Host "=== SUMMARY ===" -ForegroundColor Cyan
    Write-Host "Checks: $script:Passed / $script:Checked passed"
    if ($script:Failures.Count -eq 0) {
        Write-Host "PASS: hexon serve e2e (F16/A29 V2) - all checks passed" -ForegroundColor Green
    } else {
        Write-Host "FAIL: hexon serve e2e (F16/A29 V2) - $($script:Failures.Count) check(s) failed:" -ForegroundColor Red
        $script:Failures | ForEach-Object { Write-Host "  - $_" -ForegroundColor Red }
    }
    Write-Host "Full log: $logPath"
}

if ($script:Failures.Count -gt 0) { exit 1 } else { exit 0 }
