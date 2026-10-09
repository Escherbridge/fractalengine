# BI Egress: Connect Your Analytics Tool

Export nodes and IoT readings from FractalEngine as Parquet or CSV over signed HTTP URLs, then query them with DuckDB, PowerBI, or spreadsheets. No database credentials required—the signature on the share URL is the only credential.

Every command below is taken from `scripts/bi-egress-verify.ps1`, the live acceptance run (transcript: `scripts/bi-egress-verify.log`). Steps marked **derived** are not exercised by that script.

## 1. Generate the Node Seed (before starting the relay)

FractalEngine mints API tokens offline—there is no HTTP mint endpoint. The relay and the mint tool must share one 32-byte node-keypair seed, so generate it first and export it into the shell that will start the relay.

```powershell
$seedBytes = New-Object byte[] 32
[System.Security.Cryptography.RandomNumberGenerator]::Create().GetBytes($seedBytes)
$SeedHex = -join ($seedBytes | ForEach-Object { $_.ToString('x2') })
$env:FE_SECRET_FRACTALENGINE_NODE_KEYPAIR = $SeedHex
```

Optional but recommended: export a share-signer seed too, so share links survive a relay restart (see [Semantics & Limits](#semantics--limits)). Generate it the same way and set `$env:FE_SECRET_FRACTALENGINE_SHARE_SIGNER`.

## 2. Start a Relay

The relay is a headless FractalEngine server. Build it, then start it from the SAME shell that holds the seed variable(s):

```powershell
cargo build -p fractalengine-relay
cargo build -p fe-identity --example mint_api_token

$env:FE_BIND_ADDR = '127.0.0.1:18765'
$env:FE_DB_PATH = 'C:\temp\fe-relay-db'
$env:FE_P2P_DIR = 'C:\temp\fe-relay-p2p'
$env:FE_SYNC_RELAY = 'disabled'
$env:FE_SHUTDOWN_AFTER_SECS = '600'  # auto-exit after 10 minutes for testing

Start-Process .\target\debug\fe-relay.exe
```

The relay listens on `FE_BIND_ADDR` (default `0.0.0.0:8765`). It is ready when `GET /ready` returns 200—poll it:

```powershell
$baseUrl = 'http://127.0.0.1:18765'
do {
  Start-Sleep -Milliseconds 500
  try { $ok = (Invoke-WebRequest "$baseUrl/ready" -UseBasicParsing -TimeoutSec 2).StatusCode -eq 200 } catch { $ok = $false }
} until ($ok)
```

If you did not export `FE_SECRET_FRACTALENGINE_SHARE_SIGNER`, the relay logs a warning at startup: share links it mints stop working after a restart.

## 3. Mint Tokens

`mint_api_token` prints several lines; the token is the one starting with `token: `. This helper extracts it:

```powershell
function Mint-Token($scope, $role, $jti) {
  $out = & .\target\debug\examples\mint_api_token.exe --seed-hex $SeedHex --scope $scope --role $role --ttl-secs 3600 --jti $jti
  $line = $out | Where-Object { $_ -like 'token:*' }
  return $line.Substring('token: '.Length).Trim()
}

# Bootstrap token (scope VERSE#bootstrap) — good for creating a verse
$bootToken = Mint-Token 'VERSE#bootstrap' 'owner' 'boot'
```

## 4. Seed Data & Mint a Share

Create a verse, fractal, petal, and nodes over REST, then mint a parquet or CSV share. Parquet/CSV share links require a **petal-scoped** token.

```powershell
function Invoke-Api($method, $path, $token, $bodyObj) {
  $json = $bodyObj | ConvertTo-Json -Depth 10 -Compress
  Invoke-RestMethod -Method $method -Uri "$baseUrl$path" -Headers @{ Authorization = "Bearer $token" } `
    -ContentType 'application/json' -Body $json
}

$verseId = (Invoke-Api POST '/api/v1/verses' $bootToken @{ name = 'MyVerse' }).data.id
$verseToken = Mint-Token "VERSE#$verseId" 'owner' 'verse1'

$fractalId = (Invoke-Api POST "/api/v1/verses/$verseId/fractals" $verseToken @{ name = 'MyFractal' }).data.id
$petalId = (Invoke-Api POST "/api/v1/verses/$verseId/fractals/$fractalId/petals" $verseToken @{ name = 'MyPetal' }).data.id
$petalToken = Mint-Token "VERSE#$verseId-FRACTAL#$fractalId-PETAL#$petalId" 'owner' 'petal1'

# A node at [x, y, z] (y = elevation)
Invoke-Api POST "/api/v1/verses/$verseId/fractals/$fractalId/petals/$petalId/nodes" $verseToken `
  @{ name = 'Node1'; position = @(1.0, 2.0, 3.0) } | Out-Null

# IoT readings anchored to a node
# Invoke-Api POST "/api/v1/petals/$petalId/iot/readings" $verseToken @{ readings = @(@{ node_id = '<node id>'; metric = 'temperature_c'; value = 21.5; units = 'C' }) }

# Mint a node parquet share (signed, shareable URL)
$nodeShare = Invoke-Api POST '/api/v1/query/share' $petalToken @{
  sql = "SELECT * FROM node WHERE petal_id = '$petalId'"; format = 'parquet'; ttl_secs = 3600
}
$nodeShareUrl = "$baseUrl/api/v1/shared/$($nodeShare.data.token)"

# Mint a readings CSV share
$readingsCsvShare = Invoke-Api POST '/api/v1/query/share' $petalToken @{
  sql = "SELECT * FROM iot_reading WHERE petal_id = '$petalId'"; format = 'csv'; ttl_secs = 3600
}
$readingsCsvUrl = "$baseUrl/api/v1/shared/$($readingsCsvShare.data.token)"
```

Share/export SQL must be ONE flat `SELECT` over `node` or `iot_reading`: comments (`--`, `#`, `//`, `/*` — even inside string literals, so no URLs), nested `SELECT`s, and multi-table `FROM` lists are rejected with 400. The `SELECT` list is ignored for parquet/CSV (the output columns are fixed); use `WHERE`/`ORDER BY`/`LIMIT` to shape rows.

## 5. DuckDB

DuckDB is not bundled with the repo. Fetch the CLI from <https://duckdb.org/docs/installation/> (the acceptance run uses v1.5.6) and place it at `tools\duckdb\duckdb.exe` (`tools/` is gitignored), or use any `duckdb` on your PATH.

```powershell
$duckdb = '.\tools\duckdb\duckdb.exe'

# Nodes (parquet) — live-verified
& $duckdb -c "INSTALL httpfs; LOAD httpfs; SELECT node_id, rotation_x FROM read_parquet('$nodeShareUrl');"

# Readings (CSV) — live-verified. skip=1 drops the leading '# crs=' comment line.
& $duckdb -c "INSTALL httpfs; LOAD httpfs; SELECT node_id, metric, value FROM read_csv('$readingsCsvUrl', skip=1, header=true);"
```

**Schema notes:**
- Nodes parquet: `node_id`, `petal_id` (VARCHAR); `position` (GeoParquet **WKB** `Point Z` geometry — local frame `x, elevation, z` or, with `coords=latlon`, `lon, lat, ele`); `rotation_x/y/z`, `scale_x/y/z` (FLOAT); `properties` (JSON text); `updated_at_ms` (UBIGINT). There are no `position_x/y/z` columns in parquet.
- Nodes CSV: `node_id,petal_id,x_m,y_m,z_m` (or `lon,lat,ele_m`), then rotation/scale/properties/updated_at_ms.
- Readings: `reading_id, node_id, petal_id, metric, value (DOUBLE), units, recorded_at, recorded_at_ms (BIGINT)` plus the anchor node's position (`position` WKB in parquet; `anchor_x_m/anchor_y_m/anchor_z_m` or `anchor_lon/anchor_lat/anchor_ele_m` in CSV).
- GeoParquet `geo` metadata: petal-local exports carry `crs: null` (unspecified — local meters have no standard CRS); `coords=latlon` exports omit `crs` (= OGC:CRS84, lon/lat). The human-readable label is in the custom `fe:crs` key. Stock DuckDB reads both without settings; only parquet produced before 2026-10-09 needs `SET enable_geoparquet_conversion=false;`.

Range requests work: DuckDB's httpfs issues ranged GETs and the relay answers 206 Partial Content. Every export carries a strong `ETag` and honors `If-Range` (a stale validator gets the full body, never a mismatched slice).

## PowerBI

**Derived — not script-verified.** Follow this pattern on the same share-URL surface:

1. Open Power Query.
2. Select **Get Data > Web**.
3. Paste your CSV share URL (or parquet URL if your version supports it).
4. Load and transform the data (the first CSV line is a `# crs=` comment—skip one row).

## Spreadsheets

**Derived — not script-verified.**

**Excel:**
- **Data > From Web** with the CSV share URL.
- Accepts `http://` (relay must be reachable from your machine).

**Google Sheets:**
- `=IMPORTDATA(csv_share_url)` formula.
- **Important:** Google Sheets runs server-side, so the relay must be on a public/tunneled address, not `localhost:18765`.

## Semantics & Limits

- **Scope filtering:** A share URL carries a frozen scope ceiling (the issuer's token scope at signing time). The server substitutes that scope into the table source itself (`FROM node` runs as `FROM (SELECT * FROM node WHERE petal_id = …)`) and re-checks every exported row's `petal_id` before it leaves; the SQL you write only ever sees rows inside the ceiling. Verse- and fractal-scoped tokens on `/api/v1/query` are likewise limited to the petals under that verse/fractal.
- **Row cap:** Exports return max 500,000 rows / 128 MiB response body. Parquet/CSV shares have the same limit. `/api/v1/query` allows 10,000 rows / 8 MiB. Exceeding a cap is an error, never a truncated result.
- **Statement timeout:** every guarded query runs with a 5 s database-side timeout (504 on expiry).
- **Caching:** export/share responses are live data: `Cache-Control: private, no-store`.
- **Share TTL:** Default 1 hour, max 24 hours. Expired shares return HTTP 410.
- **Share-signer persistence:** The GUI app stores its share-signing key in the OS keystore, so its links survive restarts. The relay reads it from `FE_SECRET_FRACTALENGINE_SHARE_SIGNER` (32-byte hex seed, same format as the node keypair); if that variable is unset the relay generates an ephemeral key (and warns at startup), and every link it minted returns 401 after a restart.
- **CRS (Coordinate Reference System):** Exports include an `x-fe-crs` header and CSV `# crs=` comment line. Local meters are labeled `PETAL-LOCAL:meters`. Use `coords=latlon` (e.g. `<share url>?coords=latlon`) to convert to WGS84 lon/lat/elevation; it answers 400 when the petal has no terrain origin.
- **Readings geometry:** Each reading row includes its anchor node's position. Anchors that were hard-deleted (or are missing) export a **null** geometry cell / empty CSV anchor cells—never a fabricated `0,0,0`. REST delete tombstones nodes; tombstoned anchors currently still export their last position. Readings' `value` column is always Float64 for bit-exact sensor data.

## Troubleshooting

**401 Unauthorized**
- Share token expired or invalid signature.
- After a relay restart: the share-signer key was ephemeral because `FE_SECRET_FRACTALENGINE_SHARE_SIGNER` was not exported. Export a seed before starting the relay: `$env:FE_SECRET_FRACTALENGINE_SHARE_SIGNER = '<32-byte hex seed>'`. Links minted under the old ephemeral key cannot be recovered—mint new ones.

**400 Bad Request**
- The share/export SQL uses a comment, a nested `SELECT`, more than one table, or a table other than `node`/`iot_reading`.
- `coords=latlon` on a petal without a terrain origin.

**403 Forbidden**
- Your token's scope does not cover the requested petal/node.
- Use a petal-scoped token for parquet/CSV share minting and a verse-scoped (or broader) token for data operations.

**503 Service Unavailable**
- `"shared query not available (no db_reader)"` / `"query endpoint not available (no db_reader)"`: on Windows the relay's direct DB reader is routinely unavailable (SurrealKV file lock), and `fmt=json` shares and `/api/v1/query` still require it. Use `parquet`/`csv` shares or the export routes, which fall back to the DB-thread channel.

**Invalid CRS error from DuckDB**
- Parquet produced before 2026-10-09 has a spec-violating `crs` field in its GeoParquet metadata. Run `SET enable_geoparquet_conversion=false;` before reading it.
