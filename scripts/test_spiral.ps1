# Test: Spiral upward with rotation
$TOKEN = $env:FE_TOKEN; $BASE = $env:FE_BASE; $NODE = $env:FE_NODE_ID
if (-not $TOKEN -or -not $NODE) { Write-Host "Run api_test_setup.ps1 first!" -ForegroundColor Red; exit 1 }

Write-Host "Spiral on $NODE..." -ForegroundColor Cyan
for ($i = 0; $i -lt 40; $i++) {
    $t = $i * 0.15
    $r = 1.0 + $t * 0.5
    $x = [math]::Round([math]::Sin($t * 2) * $r, 3)
    $z = [math]::Round([math]::Cos($t * 2) * $r, 3)
    $y = [math]::Round($t * 0.3, 3)
    $body = "{""position"":[$x, $y, $z],""rotation"":[0.0, $([math]::Round($t * 2, 3)), 0.0],""scale"":[1.0, 1.0, 1.0]}"
    curl.exe -s -X PATCH "$BASE/api/v1/nodes/$NODE/transform" -H "Authorization: Bearer $TOKEN" -H "Content-Type: application/json" -d $body | Out-Null
    Write-Host "  [$i] x=$x y=$y z=$z" -ForegroundColor Gray
    Start-Sleep -Milliseconds 150
}
Write-Host "Done!" -ForegroundColor Green
