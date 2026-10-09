# BI Egress: Connect Your Analytics Tool

Export nodes and IoT readings from FractalEngine as Parquet or CSV over signed HTTP URLs, then query them with DuckDB, PowerBI, or spreadsheets. No database credentials required—the signature on the share URL is the only credential.

## Start a Relay

The relay is a headless FractalEngine server. It requires a temporary directory for the database and P2P data.

```powershell
# Windows (PowerShell)
$env:FE_BIND_ADDR = '127.0.0.1:18765'
$env:FE_DB_PATH = 'C:\temp\fe-relay-db'
$env:FE_P2P_DIR = 'C:\temp\fe-relay-p2p'
$env:FE_SYNC_RELAY = 'disabled'
$env:FE_SHUTDOWN_AFTER_SECS = '600'  # auto-exit after 10 minutes for testing

.\target\debug\fe-relay.exe
```

The relay listens on the address you set in `FE_BIND_ADDR` (default `0.0.0.0:8765`). For local testing, use `127.0.0.1:18765` as shown above. Once it logs `SyncStatus updated`, it is ready.

## Mint Tokens

FractalEngine uses offline JWT minting—there is no HTTP mint endpoint. Generate a token by running the identity example with a shared 32-byte hex seed.

```powershell
# Generate a random 32-byte seed (hex)
$seedBytes = New-Object byte[] 32
[System.Security.Cryptography.RandomNumberGenerator]::Create().GetBytes($seedBytes)
$SeedHex = -join ($seedBytes | ForEach-Object { $_.ToString('x2') })

# Export the seed so the relay and the mint tool use the same keypair
$env:FE_SECRET_FRACTALENGINE_NODE_KEYPAIR = $SeedHex

# Start the relay (in another terminal) with that env var set

# Mint a bootstrap token (scope VERSE#bootstrap, good for creating a verse)
$bootToken = & .\target\debug\examples\mint_api_token.exe `
  --seed-hex $SeedHex `
  --scope 'VERSE#bootstrap' `
  --role 'owner' `
  --ttl-secs 3600 `
  --jti 'boot'
# Extract the token from the output line "token: ..."
```

Once you have a verse ID, mint a verse-scoped token and a petal-scoped token for share minting:

```powershell
# After creating a verse with ID $verseId and fractal with ID $fractalId and petal with ID $petalId:
$verseToken = & .\target\debug\examples\mint_api_token.exe `
  --seed-hex $SeedHex `
  --scope "VERSE#$verseId" `
  --role 'owner' `
  --ttl-secs 3600 `
  --jti 'verse1'

$petalToken = & .\target\debug\examples\mint_api_token.exe `
  --seed-hex $SeedHex `
  --scope "VERSE#$verseId-FRACTAL#$fractalId-PETAL#$petalId" `
  --role 'owner' `
  --ttl-secs 3600 `
  --jti 'petal1'
```

## Seed Data & Mint a Share

Create a verse, fractal, petal, and nodes over REST, then mint a parquet or CSV share.

```powershell
$baseUrl = 'http://127.0.0.1:18765'

# Create a verse
$verse = Invoke-RestMethod -Method POST -Uri "$baseUrl/api/v1/verses" `
  -Headers @{ Authorization = "Bearer $bootToken" } `
  -ContentType 'application/json' `
  -Body (@{ name = 'MyVerse' } | ConvertTo-Json)
$verseId = $verse.data.id

# Create a fractal
$fractal = Invoke-RestMethod -Method POST -Uri "$baseUrl/api/v1/verses/$verseId/fractals" `
  -Headers @{ Authorization = "Bearer $verseToken" } `
  -ContentType 'application/json' `
  -Body (@{ name = 'MyFractal' } | ConvertTo-Json)
$fractalId = $fractal.data.id

# Create a petal
$petal = Invoke-RestMethod -Method POST -Uri "$baseUrl/api/v1/verses/$verseId/fractals/$fractalId/petals" `
  -Headers @{ Authorization = "Bearer $verseToken" } `
  -ContentType 'application/json' `
  -Body (@{ name = 'MyPetal' } | ConvertTo-Json)
$petalId = $petal.data.id

# Create a node
$node = Invoke-RestMethod -Method POST `
  -Uri "$baseUrl/api/v1/verses/$verseId/fractals/$fractalId/petals/$petalId/nodes" `
  -Headers @{ Authorization = "Bearer $verseToken" } `
  -ContentType 'application/json' `
  -Body (@{ name = 'Node1'; position = @(0, 0, 0) } | ConvertTo-Json)

# Mint a node parquet share (signed, shareable URL)
$nodeShare = Invoke-RestMethod -Method POST -Uri "$baseUrl/api/v1/query/share" `
  -Headers @{ Authorization = "Bearer $petalToken" } `
  -ContentType 'application/json' `
  -Body (@{
    sql = "SELECT * FROM node WHERE petal_id = '$petalId'"
    format = 'parquet'
    ttl_secs = 3600
  } | ConvertTo-Json)
$nodeShareUrl = "$baseUrl/api/v1/shared/$($nodeShare.data.token)"
```

## DuckDB

Download [DuckDB](https://duckdb.org/docs/installation/) or use the bundled `tools\duckdb\duckdb.exe` from the repo.

```powershell
# Install the httpfs extension and load it
& duckdb.exe -c "INSTALL httpfs; LOAD httpfs;"

# Read the parquet share
& duckdb.exe -c "SELECT * FROM read_parquet('$nodeShareUrl');"

# For readings (CSV format)
$readingsCsvShare = Invoke-RestMethod -Method POST -Uri "$baseUrl/api/v1/query/share" `
  -Headers @{ Authorization = "Bearer $petalToken" } `
  -ContentType 'application/json' `
  -Body (@{
    sql = "SELECT * FROM iot_reading WHERE petal_id = '$petalId'"
    format = 'csv'
    ttl_secs = 3600
  } | ConvertTo-Json)
$readingsCsvUrl = "$baseUrl/api/v1/shared/$($readingsCsvShare.data.token)"

& duckdb.exe -c "SELECT * FROM read_csv('$readingsCsvUrl');"
```

**Schema notes:**
- Nodes: `node_id (VARCHAR)`, `position_x/y/z (FLOAT)`, and other columns as defined in your schema.
- Readings: `reading_id, node_id, petal_id, metric, value (DOUBLE), units, recorded_at, recorded_at_ms (BIGINT)`.
- Both carry `position` geometry (WKB format in parquet; `x,y,z` or `lon,lat,ele` CSV columns).
- All archives produced after 2026-10-09 have spec-legal GeoParquet metadata. Older archives may need: `SET enable_geoparquet_conversion=false;` before reading.

Range requests work: DuckDB's httpfs will issue ranged GETs for large files, and the relay honors them with HTTP 206 Partial Content.

## PowerBI

**Not script-verified in the acceptance run.** Follow this pattern derived from the same share-URL surface:

1. Open Power Query.
2. Select **Get Data > Web**.
3. Paste your CSV share URL (or parquet URL if your version supports it).
4. Load and transform the data.

Alternatively, for CSV: **Data > From Web > paste the CSV share URL > Load**.

## Spreadsheets

**Not script-verified; derived from the same share-URL surface.**

**Excel:**
- **Data > From Web** with the CSV share URL.
- Accepts `http://` (relay must be reachable from your machine).

**Google Sheets:**
- `=IMPORTDATA(csv_share_url)` formula.
- **Important:** Google Sheets runs server-side, so the relay must be on a public/tunneled address, not `localhost:18765`.

## Semantics & Limits

- **Scope filtering:** A share URL carries a frozen scope ceiling (the issuer's token scope at signing time). Results are filtered to that scope; you cannot widen it by changing the SQL.
- **Row cap:** Exports return max 500,000 rows / 128 MiB response body. Parquet/CSV shares are the same limit. `/query` endpoint allows 10,000 rows / 8 MiB.
- **Share TTL:** Default 1 hour, max 24 hours. Expired shares return HTTP 410.
- **Share-signer persistence:** The relay signs shares with a key stored in the environment variable `FE_SECRET_FRACTALENGINE_SHARE_SIGNER` (32-byte hex seed, same format as the node keypair). If you restart the relay without exporting this variable, all previously issued share URLs will fail with 401. Export it before every run if you want shares to survive restarts.
- **CRS (Coordinate Reference System):** Exports include an `x-fe-crs` header and CSV `# crs=` comment line. Local meters are labeled `PETAL-LOCAL:meters`. Use `coords=latlon` to convert to `EPSG:4326` (lon/lat/elevation).
- **Readings geometry:** Each reading row includes its anchor node's position (nullable if the anchor was deleted). Readings' `value` column is always `Float64` (64-bit double) for bit-exact sensor data.

## Troubleshooting

**401 Unauthorized**
- Share token expired or invalid signature.
- On relay restart: the share-signer key was not persistent (you didn't export `FE_SECRET_FRACTALENGINE_SHARE_SIGNER` before restart).

**403 Forbidden**
- Your token's scope does not cover the requested petal/node.
- Ensure you are using a petal-scoped token for share minting and a verse-scoped (or broader) token for data operations.

**503 Service Unavailable**
- `"export endpoint not available"`: likely a pre-F10 relay build lacking the Windows DB-reader fallback. Upgrade to a build from 2026-10-09 or later.

**Invalid CRS error from DuckDB**
- Hexon archives created before 2026-10-09 have a spec-violating `crs` field in GeoParquet metadata. Run: `SET enable_geoparquet_conversion=false;` before reading.

**Share URL returns 404 after relay restart**
- The relay lost its share-signer key because `FE_SECRET_FRACTALENGINE_SHARE_SIGNER` was not exported. Before starting the relay, export it: `$env:FE_SECRET_FRACTALENGINE_SHARE_SIGNER = '<32-byte hex seed>'`.
