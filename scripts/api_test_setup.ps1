# API Test Setup
# Usage: . .\api_test_setup.ps1 "YOUR_EDITOR_TOKEN" "NODE_ID"
# If no node ID given, you can set $env:FE_NODE_ID manually.
param(
    [string]$TokenArg,
    [string]$NodeIdArg
)

if (-not $TokenArg) {
    Write-Host "Usage: . .\api_test_setup.ps1 `"<editor-token>`" `"<node-id>`"" -ForegroundColor Red
    Write-Host ""
    Write-Host "1. In the Inspector > API Access tab, set Role to Editor, click Generate" -ForegroundColor Gray
    Write-Host "2. Copy the token" -ForegroundColor Gray
    Write-Host "3. Get a node ID from the Inspector > Properties tab (the ID field)" -ForegroundColor Gray
    return
}

$BASE = "http://localhost:8765"

Write-Host "=== FractalEngine API Test ===" -ForegroundColor Cyan
Write-Host ""

# Health check (no auth)
Write-Host "1. Health check..." -ForegroundColor Yellow
try {
    $health = Invoke-RestMethod -Uri "$BASE/api/v1/health" -TimeoutSec 3
    Write-Host "   OK" -ForegroundColor Green
} catch {
    Write-Host "   FAILED: $($_.Exception.Message)" -ForegroundColor Red
    return
}

# Auth check — use a fast endpoint (transform GET on a fake node returns error but not 401)
Write-Host "2. Checking token..." -ForegroundColor Yellow
$HEADERS = @{ Authorization = "Bearer $TokenArg" }
try {
    $r = Invoke-WebRequest -Uri "$BASE/api/v1/nodes/00000000000000000000000000/transform" -Headers $HEADERS -TimeoutSec 3
    Write-Host "   OK: token accepted" -ForegroundColor Green
} catch {
    $code = $_.Exception.Response.StatusCode.value__
    if ($code -eq 401) {
        Write-Host "   FAILED: 401 Unauthorized — token invalid or from a previous session" -ForegroundColor Red
        return
    }
    # Any other error (404, 500, etc) means auth passed
    Write-Host "   OK: token accepted (status $code)" -ForegroundColor Green
}

$env:FE_TOKEN = $TokenArg
$env:FE_BASE = $BASE

if ($NodeIdArg) {
    $env:FE_NODE_ID = $NodeIdArg
}

if (-not $env:FE_NODE_ID) {
    Write-Host ""
    Write-Host "No node ID set. Find one in the Inspector (Properties tab, ID field)" -ForegroundColor Yellow
    Write-Host "Then run:  `$env:FE_NODE_ID = `"paste-id-here`"" -ForegroundColor Gray
    return
}

Write-Host ""
Write-Host "Ready! Node: $($env:FE_NODE_ID)" -ForegroundColor Cyan
Write-Host "Run: .\test_circle.ps1  |  .\test_bounce.ps1  |  .\test_spiral.ps1" -ForegroundColor Gray
