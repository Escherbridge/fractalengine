<#
.SYNOPSIS
    F10 (A22) end-to-end verification: live fe-relay + real DuckDB CLI reading
    node and IoT-reading parquet/CSV exports over signed share URLs.

.DESCRIPTION
    Self-contained, re-runnable smoke test for the DuckDB-first BI egress
    path (conductor/tracks/mission_prodready_continuation_20261008/
    m4-bi-egress-design.md). Builds (if needed) and starts a disposable
    fe-relay on a non-default port with a fresh temp DB/P2P dir and a known
    node-keypair seed, mints Bearer JWTs offline with that same seed (there
    is no HTTP mint route -- see fe-api/AGENTS.md section "export"'s "Windows
    db_reader fallback" note), seeds a verse/fractal/petal/nodes/readings tree over REST,
    mints node + readings parquet (and a readings CSV) share URLs, drives
    tools/duckdb/duckdb.exe against them, and probes Range/header behavior
    for DEC-C10 evidence.

    M4 fix pass (DEC-C17/C18, 2026-10-09) adds live checks for: scope-bypass
    vectors against a second (foreign) petal, DuckDB read_csv on the readings
    CSV, non-null node + anchor geometry, ETag / Cache-Control / If-Range,
    coords=latlon honesty (400 without a terrain origin -- the relay has no
    live path to SET an origin; the known lat/lon round-trip is pinned by
    fe-api export_csv_local_vs_latlon_landmine), and the relay's startup
    warning when FE_SECRET_FRACTALENGINE_SHARE_SIGNER is unset. It always
    (incrementally) rebuilds the relay + mint example so a stale binary can
    never pass, refuses to start if the port is taken, and fails fast if the
    relay exits early.

    Always tears down the relay it started (by PID, never by process name)
    and the temp dirs, even on failure (try/finally).

.NOTES
    Run from the repo root or anywhere; the script resolves its own paths.
#>

## 'Continue' at the top level: Invoke-RestMethod / Invoke-WebRequest already
## throw terminating errors on a non-2xx HTTP response regardless of this
## preference, so real API failures still abort into the catch/finally
## below. 'Stop' at this scope would ALSO promote ordinary native-command
## stderr chatter (e.g. duckdb.exe's one-time httpfs extension install
## progress text under `2>&1`) into a script-terminating exception, which
## is not what we want -- see Invoke-DuckDb's own local override.
$ErrorActionPreference = 'Continue'
$script:Failures = @()
$script:RelayProcess = $null
$script:WorkDir = $null
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

$stamp = Get-Date -Format 'yyyyMMdd-HHmmss'
$script:WorkDir = Join-Path $env:TEMP "fe-bi-egress-verify-$stamp"
New-Item -ItemType Directory -Path $script:WorkDir -Force | Out-Null
$dbPath = Join-Path $script:WorkDir 'db'
$p2pDir = Join-Path $script:WorkDir 'p2p'
$relayLog = Join-Path $script:WorkDir 'relay.log'
$relayErrLog = Join-Path $script:WorkDir 'relay-err.log'
Write-Host "  Work dir: $script:WorkDir"

$BindAddr = '127.0.0.1:18765'
$BaseUrl = "http://$BindAddr"

# A fresh, known 32-byte hex seed for the relay's node keypair -- this is
# what lets us mint matching Bearer JWTs offline (see mint path below).
$seedBytes = New-Object byte[] 32
[System.Security.Cryptography.RandomNumberGenerator]::Create().GetBytes($seedBytes)
$SeedHex = -join ($seedBytes | ForEach-Object { $_.ToString('x2') })

try {
    # -----------------------------------------------------------------------
    # Build
    # -----------------------------------------------------------------------
    Write-Section "Build (incremental -- never trust a stale binary)"
    $relayExe = Join-Path $RepoRoot 'target\debug\fe-relay.exe'
    $mintExe = Join-Path $RepoRoot 'target\debug\examples\mint_api_token.exe'
    $env:RUST_MIN_STACK = '134217728'
    if (-not $env:CARGO_BUILD_JOBS) { $env:CARGO_BUILD_JOBS = '2' }

    Write-Host "  cargo build -p fractalengine-relay ..."
    cargo build -q -p fractalengine-relay 2>&1 | ForEach-Object { Write-Host "    $_" }
    if ($LASTEXITCODE -ne 0) { throw "cargo build -p fractalengine-relay failed" }
    Write-Host "  cargo build -p fe-identity --example mint_api_token ..."
    cargo build -q -p fe-identity --example mint_api_token 2>&1 | ForEach-Object { Write-Host "    $_" }
    if ($LASTEXITCODE -ne 0) { throw "cargo build -p fe-identity --example mint_api_token failed" }

    $duckdbExe = Join-Path $RepoRoot 'tools\duckdb\duckdb.exe'
    if (-not (Test-Path $duckdbExe)) { throw "duckdb.exe not found at $duckdbExe" }
    Write-Host "  duckdb.exe: $(& $duckdbExe -version)"

    # -----------------------------------------------------------------------
    # Start relay
    # -----------------------------------------------------------------------
    Write-Section "Starting fe-relay"

    # Port pre-check: a leftover relay (or anything else) on the port would
    # answer our requests and silently invalidate every check below.
    $probe = New-Object System.Net.Sockets.TcpListener([System.Net.IPAddress]::Loopback, 18765)
    try { $probe.Start(); $probe.Stop() }
    catch { throw "port $BindAddr is already in use -- stop the other listener first" }

    $psi = New-Object System.Diagnostics.ProcessStartInfo
    $psi.FileName = $relayExe
    $psi.WorkingDirectory = $RepoRoot
    $psi.UseShellExecute = $false
    $psi.RedirectStandardOutput = $true
    $psi.RedirectStandardError = $true
    $psi.EnvironmentVariables['FE_BIND_ADDR'] = $BindAddr
    $psi.EnvironmentVariables['FE_DB_PATH'] = $dbPath
    $psi.EnvironmentVariables['FE_P2P_DIR'] = $p2pDir
    $psi.EnvironmentVariables['FE_SYNC_RELAY'] = 'disabled'
    $psi.EnvironmentVariables['FE_SHUTDOWN_AFTER_SECS'] = '600'
    $psi.EnvironmentVariables['RUST_MIN_STACK'] = '134217728'
    # debug: the body_response range-decision line + tower_http request/response
    # spans are the server-side evidence for DEC-C10's "did DuckDB send a Range
    # header" question (no client-side httpfs request log exists in 1.5.6 CLI).
    $psi.EnvironmentVariables['RUST_LOG'] = 'info,fe_api=debug,tower_http=debug'
    $psi.EnvironmentVariables['FE_SECRET_FRACTALENGINE_NODE_KEYPAIR'] = $SeedHex

    $proc = [System.Diagnostics.Process]::Start($psi)
    $script:RelayProcess = $proc
    Write-Host "  Started fe-relay.exe PID=$($proc.Id)"

    # Drain stdout/stderr asynchronously to files so the relay never blocks
    # on a full pipe buffer, and we get the full transcript for the log.
    $outWriter = [System.IO.StreamWriter]::new($relayLog, $false)
    $errWriter = [System.IO.StreamWriter]::new($relayErrLog, $false)
    $outEvent = Register-ObjectEvent -InputObject $proc -EventName OutputDataReceived -Action {
        if ($EventArgs.Data -ne $null) { $Event.MessageData.WriteLine($EventArgs.Data); $Event.MessageData.Flush() }
    } -MessageData $outWriter
    $errEvent = Register-ObjectEvent -InputObject $proc -EventName ErrorDataReceived -Action {
        if ($EventArgs.Data -ne $null) { $Event.MessageData.WriteLine($EventArgs.Data); $Event.MessageData.Flush() }
    } -MessageData $errWriter
    $proc.BeginOutputReadLine()
    $proc.BeginErrorReadLine()

    # Poll /ready
    $deadline = (Get-Date).AddSeconds(60)
    $ready = $false
    while ((Get-Date) -lt $deadline) {
        if ($proc.HasExited) {
            Start-Sleep -Milliseconds 300
            $errTail = (Get-Content $relayErrLog -Tail 20 -ErrorAction SilentlyContinue) -join "`n"
            throw "fe-relay exited early (code $($proc.ExitCode)) before /ready:`n$errTail"
        }
        try {
            $resp = Invoke-WebRequest -Uri "$BaseUrl/ready" -UseBasicParsing -TimeoutSec 2
            if ($resp.StatusCode -eq 200) { $ready = $true; break }
        } catch { Start-Sleep -Milliseconds 500 }
    }
    Assert-True $ready "relay /ready returned 200 within 60s"
    if (-not $ready) { throw "relay never became ready" }

    # -----------------------------------------------------------------------
    # Mint path
    # -----------------------------------------------------------------------
    Write-Section "Mint path (offline JWT, matching the relay's node-keypair seed)"

    function Mint-Token($scope, $role, $jti) {
        $out = & $mintExe --seed-hex $SeedHex --scope $scope --role $role --ttl-secs 3600 --jti $jti
        $line = $out | Where-Object { $_ -like 'token:*' }
        if (-not $line) { throw "mint_api_token produced no token line for jti=$jti : $out" }
        return $line.Substring('token: '.Length).Trim()
    }

    $bootToken = Mint-Token 'VERSE#bootstrap' 'owner' 'boot'
    Assert-True ($bootToken.Length -gt 0) "minted a bootstrap token offline with the relay's seed"

    # -----------------------------------------------------------------------
    # Seed: verse -> fractal -> petal -> >=3 nodes -> >=6 readings / 2 anchors
    # -----------------------------------------------------------------------
    Write-Section "Seeding hierarchy over REST"

    function Invoke-Api($method, $path, $token, $bodyObj) {
        $headers = @{ Authorization = "Bearer $token" }
        $uri = "$BaseUrl$path"
        if ($null -ne $bodyObj) {
            $json = $bodyObj | ConvertTo-Json -Depth 10 -Compress
            return Invoke-RestMethod -Method $method -Uri $uri -Headers $headers -ContentType 'application/json' -Body $json
        }
        return Invoke-RestMethod -Method $method -Uri $uri -Headers $headers
    }

    $verse = Invoke-Api POST '/api/v1/verses' $bootToken @{ name = 'BiEgressVerify' }
    Assert-True $verse.ok "created verse"
    $verseId = $verse.data.id

    $verseToken = Mint-Token "VERSE#$verseId" 'owner' 'verse1'

    $fractal = Invoke-Api POST "/api/v1/verses/$verseId/fractals" $verseToken @{ name = 'F1' }
    Assert-True $fractal.ok "created fractal"
    $fractalId = $fractal.data.id

    $petal = Invoke-Api POST "/api/v1/verses/$verseId/fractals/$fractalId/petals" $verseToken @{ name = 'P1' }
    Assert-True $petal.ok "created petal"
    $petalId = $petal.data.id

    # A petal-scoped token is required to MINT parquet/csv share URLs
    # (fe-api/src/share.rs issue_share_url's petal_scoped check) even though
    # the verse-scoped token above already covers writes into this petal.
    $petalToken = Mint-Token "VERSE#$verseId-FRACTAL#$fractalId-PETAL#$petalId" 'owner' 'petal1'

    $nodePath = "/api/v1/verses/$verseId/fractals/$fractalId/petals/$petalId/nodes"
    $nodeSpecs = @(
        @{ name = 'AnchorAlpha'; position = @(1.0, 2.0, 3.0) },
        @{ name = 'AnchorBeta';  position = @(10.0, 0.0, -5.0) },
        @{ name = 'PlainNode';   position = @(-3.5, 1.5, 0.0) }
    )
    $nodeIds = @()
    foreach ($spec in $nodeSpecs) {
        $resp = Invoke-Api POST $nodePath $verseToken $spec
        Assert-True $resp.ok "created node '$($spec.name)'"
        $nodeIds += $resp.data.id
    }
    $anchorAlpha = $nodeIds[0]
    $anchorBeta = $nodeIds[1]

    # A SECOND petal in the same verse with its own node: the foreign rows the
    # scope-bypass vectors below must never surface through petal P1's links.
    $foreignPetal = Invoke-Api POST "/api/v1/verses/$verseId/fractals/$fractalId/petals" $verseToken @{ name = 'P2-foreign' }
    Assert-True $foreignPetal.ok "created foreign petal P2"
    $foreignPetalId = $foreignPetal.data.id
    $foreignNode = Invoke-Api POST "/api/v1/verses/$verseId/fractals/$fractalId/petals/$foreignPetalId/nodes" $verseToken @{ name = 'ForeignNode'; position = @(99.0, 99.0, 99.0) }
    Assert-True $foreignNode.ok "created node in foreign petal P2"
    $foreignNodeId = $foreignNode.data.id

    # >=6 readings across >=2 anchors, with a handful of known f64 values we
    # will assert bit-exact after the parquet round-trip. Deliberately
    # irrational-looking decimals so a float32 downcast (node positions) vs
    # float64 (readings) mismatch would be visible.
    $knownValueAlpha1 = 21.123456789012345
    $knownValueBeta1 = -0.000001234567891
    $readingSpecs = @(
        @{ node_id = $anchorAlpha; metric = 'temperature_c'; value = $knownValueAlpha1; units = 'C'; recorded_at = '2026-10-09T08:00:00Z' },
        @{ node_id = $anchorAlpha; metric = 'temperature_c'; value = 22.5;               units = 'C'; recorded_at = '2026-10-09T08:05:00Z' },
        @{ node_id = $anchorAlpha; metric = 'humidity_pct';  value = 55.0;               units = '%'; recorded_at = '2026-10-09T08:05:00Z' },
        @{ node_id = $anchorBeta;  metric = 'co2_ppm';        value = $knownValueBeta1;    units = 'ppm'; recorded_at = '2026-10-09T08:00:00Z' },
        @{ node_id = $anchorBeta;  metric = 'co2_ppm';        value = 415.0;              units = 'ppm'; recorded_at = '2026-10-09T08:05:00Z' },
        @{ node_id = $anchorBeta;  metric = 'pressure_hpa';   value = 1013.25;            units = 'hPa'; recorded_at = '2026-10-09T08:05:00Z' }
    )
    $ingest = Invoke-Api POST "/api/v1/petals/$petalId/iot/readings" $verseToken @{ readings = $readingSpecs }
    Assert-True $ingest.ok "ingested $($readingSpecs.Count) IoT readings across 2 anchors"
    Assert-Equal $ingest.accepted $readingSpecs.Count "ingest accepted count matches batch size"

    # Note on the "nullable anchor" optional sub-check (E1 step 5): REST
    # DELETE /api/v1/nodes/{id} ALWAYS tombstones (fe-api/AGENTS.md
    # section "endpoint-surface" FR-3, N-4) and export.rs's anchor join deliberately
    # does NOT filter by tombstone (fe-api/AGENTS.md section "export"), so a
    # REST-tombstoned anchor still resolves its last-known position -- a
    # null geometry cell can only come from a HARD-deleted node, which no
    # REST surface exposes. Skipping this sub-check; documented here rather
    # than silently omitted.
    Write-Host "  [SKIP] nullable-anchor geometry cell: REST delete is tombstone-only (never a hard drop), so no REST path produces an unresolvable anchor -- see AGENTS.md note above." -ForegroundColor Yellow

    # -----------------------------------------------------------------------
    # Mint share URLs
    # -----------------------------------------------------------------------
    Write-Section "Minting share URLs"

    $nodeShare = Invoke-Api POST '/api/v1/query/share' $petalToken @{
        sql = "SELECT * FROM node WHERE petal_id = '$petalId'"; format = 'parquet'; ttl_secs = 3600
    }
    Assert-True $nodeShare.ok "minted node parquet share"
    $nodeShareToken = $nodeShare.data.token

    $readingsShare = Invoke-Api POST '/api/v1/query/share' $petalToken @{
        sql = "SELECT * FROM iot_reading WHERE petal_id = '$petalId'"; format = 'parquet'; ttl_secs = 3600
    }
    Assert-True $readingsShare.ok "minted readings parquet share"
    $readingsShareToken = $readingsShare.data.token

    $readingsCsvShare = Invoke-Api POST '/api/v1/query/share' $petalToken @{
        sql = "SELECT * FROM iot_reading WHERE petal_id = '$petalId'"; format = 'csv'; ttl_secs = 3600
    }
    Assert-True $readingsCsvShare.ok "minted readings csv share"
    $readingsCsvShareToken = $readingsCsvShare.data.token

    $nodeShareUrl = "$BaseUrl/api/v1/shared/$nodeShareToken"
    $readingsShareUrl = "$BaseUrl/api/v1/shared/$readingsShareToken"
    $readingsCsvShareUrl = "$BaseUrl/api/v1/shared/$readingsCsvShareToken"

    # -----------------------------------------------------------------------
    # DuckDB reads
    # -----------------------------------------------------------------------
    Write-Section "DuckDB httpfs reads"

    # DEC-C16: the GeoParquet "geo" key-value metadata's "crs" field is now
    # spec-legal null (the honest free-text label moved to the custom
    # "fe:crs" key), so stock DuckDB's enable_geoparquet_conversion (default
    # ON) no longer rejects the file -- `SET enable_geoparquet_conversion=
    # false;` is only needed for hexon archives produced before DEC-C16.
    $prelude = "INSTALL httpfs; LOAD httpfs;"

    # -json mode gives structured, precision-preserving output (doubles are
    # rendered as their shortest round-trip decimal string) instead of the
    # box-drawn table text -- parsing that reliably for bit-exact float
    # comparison is what regex-on-table-text cannot do safely.
    function Invoke-DuckDbJson($sql) {
        $out = & $duckdbExe -json -c $sql 2>&1
        $text = ($out -join "`n")
        try {
            $parsed = ConvertFrom-Json $text -ErrorAction Stop
            return @($parsed)
        } catch {
            Write-Host "  (duckdb output did not parse as JSON)" -ForegroundColor Yellow
            Write-Host "  $text"
            return @()
        }
    }

    Write-Host "  --- nodes parquet ---"
    $nodeRows = Invoke-DuckDbJson "$prelude SELECT node_id, rotation_x FROM read_parquet('$nodeShareUrl') ORDER BY node_id;"
    Write-Host "  $(($nodeRows | ConvertTo-Json -Compress))"
    Assert-Equal $nodeRows.Count $nodeSpecs.Count "node parquet row count"
    foreach ($nid in $nodeIds) {
        $found = @($nodeRows | Where-Object { $_.node_id -eq $nid })
        Assert-True ($found.Count -gt 0) "node parquet contains seeded node_id $nid"
    }

    Write-Host "  --- readings parquet: rows + schema ---"
    $readingRows = Invoke-DuckDbJson "$prelude SELECT * FROM read_parquet('$readingsShareUrl') ORDER BY recorded_at_ms;"
    Write-Host "  $(($readingRows | ConvertTo-Json -Compress))"
    Assert-Equal $readingRows.Count $readingSpecs.Count "readings parquet row count"

    $describeRows = Invoke-DuckDbJson "$prelude DESCRIBE SELECT * FROM read_parquet('$readingsShareUrl');"
    Write-Host "  $(($describeRows | ConvertTo-Json -Compress))"
    $valueCol = $describeRows | Where-Object { $_.column_name -eq 'value' } | Select-Object -First 1
    Assert-Equal $valueCol.column_type 'DOUBLE' "readings schema: value is DOUBLE (Float64)"
    $recMsCol = $describeRows | Where-Object { $_.column_name -eq 'recorded_at_ms' } | Select-Object -First 1
    Assert-Equal $recMsCol.column_type 'BIGINT' "readings schema: recorded_at_ms is BIGINT (Int64)"

    Write-Host "  --- readings parquet: known f64 values bit-exact ---"
    $ic = [System.Globalization.CultureInfo]::InvariantCulture
    $alphaRow = $readingRows | Where-Object { $_.node_id -eq $anchorAlpha -and $_.metric -eq 'temperature_c' } | Sort-Object recorded_at_ms | Select-Object -First 1
    $alphaValue = [double]::Parse([string]$alphaRow.value, $ic)
    Assert-Equal $alphaValue $knownValueAlpha1 "known reading value for anchorAlpha round-trips bit-exact (f64 equality, not string match)"

    $betaRow = $readingRows | Where-Object { $_.node_id -eq $anchorBeta -and $_.metric -eq 'co2_ppm' } | Sort-Object recorded_at_ms | Select-Object -First 1
    $betaValue = [double]::Parse([string]$betaRow.value, $ic)
    Assert-Equal $betaValue $knownValueBeta1 "known reading value for anchorBeta round-trips bit-exact (f64 equality, not string match)"

    Write-Host "  --- geometry is real, not null (nodes + live anchors) ---"
    $nodeGeom = Invoke-DuckDbJson "$prelude SELECT count(*) AS n FROM read_parquet('$nodeShareUrl') WHERE position IS NOT NULL;"
    Assert-Equal ([int]$nodeGeom[0].n) $nodeSpecs.Count "node parquet: every position geometry is non-null"
    $anchorGeom = Invoke-DuckDbJson "$prelude SELECT count(*) AS n FROM read_parquet('$readingsShareUrl') WHERE position IS NOT NULL;"
    Assert-Equal ([int]$anchorGeom[0].n) $readingSpecs.Count "readings parquet: every live-anchor geometry is non-null"

    Write-Host "  --- readings CSV via DuckDB read_csv ---"
    # skip=1 drops the leading '# crs=' comment line; the next line is the header.
    $csvRows = Invoke-DuckDbJson "$prelude SELECT node_id, metric, value, recorded_at_ms FROM read_csv('$readingsCsvShareUrl', skip=1, header=true) ORDER BY recorded_at_ms;"
    Write-Host "  $(($csvRows | ConvertTo-Json -Compress))"
    Assert-Equal $csvRows.Count $readingSpecs.Count "DuckDB read_csv: readings CSV row count"
    $csvDuckAlpha = $csvRows | Where-Object { $_.node_id -eq $anchorAlpha -and $_.metric -eq 'temperature_c' } | Sort-Object recorded_at_ms | Select-Object -First 1
    Assert-Equal ([double]::Parse([string]$csvDuckAlpha.value, $ic)) $knownValueAlpha1 "DuckDB read_csv: known value bit-exact (f64 equality)"

    # -----------------------------------------------------------------------
    # Scope-bypass vectors (M4 fix B1) -- live, against a real foreign petal
    # -----------------------------------------------------------------------
    Write-Section "Scope-bypass vectors (M4 fix B1)"
    $servedVectors = @(
        @{ name = 'double-space FROM'; sql = 'SELECT * FROM  node' },
        @{ name = 'tab FROM'; sql = "SELECT * FROM`tnode" },
        @{ name = 'OR precedence'; sql = 'SELECT * FROM node WHERE true OR true' }
    )
    foreach ($v in $servedVectors) {
        $share = Invoke-Api POST '/api/v1/query/share' $petalToken @{ sql = $v.sql; format = 'parquet'; ttl_secs = 600 }
        $url = "$BaseUrl/api/v1/shared/$($share.data.token)"
        $rows = Invoke-DuckDbJson "$prelude SELECT node_id, petal_id FROM read_parquet('$url');"
        $foreign = @($rows | Where-Object { $_.petal_id -ne $petalId -or $_.node_id -eq $foreignNodeId })
        Assert-True ($rows.Count -eq $nodeSpecs.Count -and $foreign.Count -eq 0) "$($v.name): served scoped -- $($rows.Count) P1 rows, 0 foreign"
    }
    $rejectedVectors = @(
        @{ name = 'trailing comment'; sql = 'SELECT * FROM node --' },
        @{ name = 'projection subquery'; sql = 'SELECT *, (SELECT * FROM iot_reading) AS x FROM node' }
    )
    foreach ($v in $rejectedVectors) {
        $status = 0
        try {
            Invoke-Api POST '/api/v1/query/share' $petalToken @{ sql = $v.sql; format = 'parquet'; ttl_secs = 600 } | Out-Null
            $status = 200
        } catch { $status = [int]$_.Exception.Response.StatusCode }
        Assert-Equal $status 400 "$($v.name): rejected at share mint"
    }

    # -----------------------------------------------------------------------
    # coords=latlon honesty
    # -----------------------------------------------------------------------
    Write-Section "coords=latlon (no terrain origin on this petal)"
    # REST/MCP terrain mutation is deliberately refused (fe-api terrain.rs), so
    # a live relay petal can never carry an origin here; the known lat/lon
    # value round-trip is pinned in fe-api's export_csv_local_vs_latlon_landmine.
    $latlonStatus = 0
    try { Invoke-WebRequest -Uri "$($nodeShareUrl)?coords=latlon" -UseBasicParsing | Out-Null; $latlonStatus = 200 }
    catch { $latlonStatus = [int]$_.Exception.Response.StatusCode }
    Assert-Equal $latlonStatus 400 "coords=latlon without a terrain origin answers 400 (never a mislabeled export)"

    # -----------------------------------------------------------------------
    # CSV spot-check
    # -----------------------------------------------------------------------
    Write-Section "CSV spot-check"
    $csvResp = Invoke-WebRequest -Uri $readingsCsvShareUrl -UseBasicParsing
    $csvLines = $csvResp.Content -split "`r`n"
    Write-Host "  CSV first 3 lines:"
    $csvLines[0..2] | ForEach-Object { Write-Host "    $_" }
    Assert-True ($csvLines[0] -match '^# crs=') "CSV has leading # crs= comment line"
    Assert-True ($csvLines[1] -match '^reading_id,node_id,petal_id,metric,value,units,recorded_at,recorded_at_ms,') "CSV header row matches documented shape"
    $csvHeaderCols = $csvLines[1] -split ','
    $csvValueIdx = [array]::IndexOf($csvHeaderCols, 'value')
    # Disambiguate from anchorAlpha's OTHER temperature_c reading (08:05) by
    # matching the known value's specific recorded_at timestamp, not just
    # node_id+metric.
    $csvAlphaLine = $csvLines | Where-Object { $_ -match [regex]::Escape($anchorAlpha) -and $_ -match 'temperature_c' -and $_ -match '08:00:00' } | Select-Object -First 1
    $csvAlphaValue = [double]::Parse(($csvAlphaLine -split ',')[$csvValueIdx], $ic)
    Assert-Equal $csvAlphaValue $knownValueAlpha1 "CSV body contains the known bit-exact value (f64 equality)"

    # -----------------------------------------------------------------------
    # Header + Range checks (DEC-C10 evidence)
    # -----------------------------------------------------------------------
    Write-Section "Header + Range checks (DEC-C10)"

    try {
        $headResp = Invoke-WebRequest -Uri $nodeShareUrl -Method Head -UseBasicParsing
        Write-Host "  HEAD status: $($headResp.StatusCode)"
        Write-Host "  HEAD headers: $($headResp.Headers | ConvertTo-Json -Compress)"
        Assert-True ($headResp.Headers['Content-Length']) "HEAD response carries Content-Length"
        Assert-True ($headResp.Headers['Content-Type'] -match 'parquet') "HEAD response Content-Type is parquet"
        Assert-True ($headResp.Headers['x-fe-crs']) "HEAD response carries x-fe-crs"
        Assert-True ($headResp.Headers['Content-Disposition'] -match 'attachment') "HEAD response carries Content-Disposition: attachment"
        $script:NodeEtag = $headResp.Headers['ETag']
        Assert-True ($script:NodeEtag -match '^"[0-9a-f]{64}"$') "HEAD response carries a strong blake3 ETag (DEC-C18)"
        Assert-Equal $headResp.Headers['Cache-Control'] 'private, no-store' "export Cache-Control is private, no-store -- live data, never immutable (DEC-C18)"
    } catch {
        Write-Host "  HEAD request failed/unsupported: $_" -ForegroundColor Yellow
        $script:Failures += "HEAD request to share URL failed: $_"
    }

    # Explicit Range probe: GET with Range: bytes=0-99. Invoke-WebRequest
    # cannot set the `Range` header directly in Windows PowerShell 5.1 (its
    # underlying HttpWebRequest treats Range as a "restricted" header that
    # must go through a dedicated method, not the generic Headers
    # dictionary -- "The 'Range' header must be modified using the
    # appropriate property or method"), so this probe uses the real
    # curl.exe (not the `curl` alias for Invoke-WebRequest) exactly as the
    # task's own evidence-gathering suggestion names.
    $curlExe = 'C:\Windows\System32\curl.exe'
    $rangeHeadersFile = Join-Path $script:WorkDir 'range-headers.txt'
    $rangeBodyFile = Join-Path $script:WorkDir 'range-body.bin'
    & $curlExe -s -D $rangeHeadersFile -o $rangeBodyFile -H 'Range: bytes=0-99' $nodeShareUrl
    $rangeHeaderText = Get-Content $rangeHeadersFile -Raw
    $rangeBodyLen = (Get-Item $rangeBodyFile).Length
    $statusLine = ($rangeHeaderText -split "`r`n")[0]
    Write-Host "  GET with Range: bytes=0-99 -> $statusLine"
    Write-Host "  Headers:`n$rangeHeaderText"
    Write-Host "  Body length: $rangeBodyLen bytes"
    if ($statusLine -match ' 206 ') {
        Write-Host "  Server returned 206 Partial Content with a sliced body (DEC-C10 fix confirmed live)." -ForegroundColor Green
        Assert-Equal $rangeBodyLen 100 "ranged GET returned exactly 100 bytes"
        Assert-True ($rangeHeaderText -match 'Content-Range:\s*bytes 0-99/\d+') "Content-Range header well-formed"
    } elseif ($statusLine -match ' 200 ') {
        Write-Host "  Server returned 200 with the FULL body for a Range request (pre-fix behavior / unexpected)." -ForegroundColor Yellow
        $script:Failures += "ranged GET returned 200 full body instead of 206 -- DEC-C10 fix not active"
    } else {
        Write-Host "  Server returned unexpected status line '$statusLine' for a Range request." -ForegroundColor Yellow
        $script:Failures += "ranged GET returned unexpected status line '$statusLine'"
    }

    # If-Range (DEC-C18): current ETag -> the range (206); stale -> full body (200).
    # Request headers go through a curl `-H @file`: Windows PowerShell 5.1
    # strips embedded double quotes from native-command arguments, which
    # would send an unquoted (invalid) entity-tag and mangle the Range header.
    function Get-StatusWithIfRange($validator) {
        $reqHdrFile = Join-Path $script:WorkDir 'ifrange-request-headers.txt'
        $respHdrFile = Join-Path $script:WorkDir 'ifrange-headers.txt'
        Set-Content -Path $reqHdrFile -Encoding Ascii -Value @('Range: bytes=0-99', "If-Range: $validator")
        & $curlExe -s -D $respHdrFile -o NUL -H "@$reqHdrFile" $nodeShareUrl
        return ((Get-Content $respHdrFile -Raw) -split "`r`n")[0]
    }
    $ifRangeHit = Get-StatusWithIfRange $script:NodeEtag
    Assert-True ($ifRangeHit -match ' 206 ') "If-Range with the current ETag serves the range ($ifRangeHit)"
    $ifRangeMiss = Get-StatusWithIfRange '"0000000000000000000000000000000000000000000000000000000000000000"'
    Assert-True ($ifRangeMiss -match ' 200 ') "If-Range with a stale ETag serves the full body ($ifRangeMiss)"

    # -----------------------------------------------------------------------
    # DuckDB httpfs Range-issuance evidence (server-side request log)
    # -----------------------------------------------------------------------
    Write-Section "Did DuckDB httpfs itself issue ranged GETs? (server-side log evidence)"
    Start-Sleep -Milliseconds 300  # let the async log writers flush
    $logContent = Get-Content $relayLog -Raw -ErrorAction SilentlyContinue
    $rangeEvidence = ($logContent -split "`n") | Select-String -Pattern 'range' -SimpleMatch:$false
    if ($rangeEvidence) {
        Write-Host "  Relay log lines mentioning 'range' (case-insensitive):"
        $rangeEvidence | ForEach-Object { Write-Host "    $_" }
    } else {
        Write-Host "  No 'range' mentions found in the relay log for the DuckDB read_parquet() calls above." -ForegroundColor Yellow
        Write-Host "  Interpretation: DuckDB 1.5.6 httpfs served these small single-row-group files via full GET (or HEAD + full GET), not ranged GETs -- consistent with duckdb's httpfs preferring a single-request path for files under its prefetch/footer-read threshold." -ForegroundColor Yellow
    }

    # A24 honesty (M4 fix M3): this run deliberately does NOT export the
    # share-signer slot, so the relay must say its links are ephemeral.
    Write-Section "Relay share-signer warning (A24)"
    $allLogs = (Get-Content $relayLog -Raw -ErrorAction SilentlyContinue) + (Get-Content $relayErrLog -Raw -ErrorAction SilentlyContinue)
    Assert-True ($allLogs -match 'FE_SECRET_FRACTALENGINE_SHARE_SIGNER is not set') "relay warns at startup that share links are ephemeral when the signer env is unset"

    Write-Host ""
    Write-Host "Full relay log: $relayLog"
}
catch {
    $script:Failures += "UNHANDLED EXCEPTION: $_"
    Write-Host "UNHANDLED EXCEPTION: $_" -ForegroundColor Red
    Write-Host $_.ScriptStackTrace -ForegroundColor Red
}
finally {
    # ---------------------------------------------------------------------
    # Teardown (always runs, even on failure)
    # ---------------------------------------------------------------------
    Write-Section "Teardown"
    if ($outEvent) { Unregister-Event -SourceIdentifier $outEvent.Name -ErrorAction SilentlyContinue }
    if ($errEvent) { Unregister-Event -SourceIdentifier $errEvent.Name -ErrorAction SilentlyContinue }
    if ($script:RelayProcess -and -not $script:RelayProcess.HasExited) {
        Write-Host "  Stopping fe-relay.exe PID=$($script:RelayProcess.Id)"
        try { $script:RelayProcess.Kill() } catch { Write-Host "  (already exited)" }
        $script:RelayProcess.WaitForExit(5000) | Out-Null
    }
    if ($outWriter) { $outWriter.Close() }
    if ($errWriter) { $errWriter.Close() }

    # Copy the transcript out before the temp dir is removed so the caller
    # (this script's invoker) can preserve it as scripts/bi-egress-verify.log.
    $script:PreservedLog = $null
    if ($relayLog -and (Test-Path $relayLog)) {
        $script:PreservedLog = Get-Content $relayLog -Raw
    }

    if ($script:WorkDir -and (Test-Path $script:WorkDir)) {
        Write-Host "  Removing temp dir $script:WorkDir"
        Remove-Item -Recurse -Force $script:WorkDir -ErrorAction SilentlyContinue
    }

    Write-Host ""
    Write-Host "=== SUMMARY ===" -ForegroundColor Cyan
    Write-Host "Checks: $script:Passed / $script:Checked passed"
    if ($script:Failures.Count -eq 0) {
        Write-Host "PASS: BI egress e2e (F10/A22) - all checks passed" -ForegroundColor Green
    } else {
        Write-Host "FAIL: BI egress e2e (F10/A22) - $($script:Failures.Count) check(s) failed:" -ForegroundColor Red
        $script:Failures | ForEach-Object { Write-Host "  - $_" -ForegroundColor Red }
    }
}

if ($script:Failures.Count -gt 0) { exit 1 } else { exit 0 }
