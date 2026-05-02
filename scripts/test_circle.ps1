# Test: Move node in a circle (20 steps)
$TOKEN = $env:FE_TOKEN; $BASE = $env:FE_BASE; $NODE = $env:FE_NODE_ID
if (-not $TOKEN -or -not $NODE) { Write-Host "Run api_test_setup.ps1 first!" -ForegroundColor Red; exit 1 }

Write-Host "Circle movement on $NODE..." -ForegroundColor Cyan
for ($i = 0; $i -lt 20; $i++) {
    $a = $i * 0.314
    $x = [math]::Round([math]::Sin($a) * 3, 3)
    $z = [math]::Round([math]::Cos($a) * 3, 3)
    $body = "{""position"":[$x, 0.5, $z],""rotation"":[0.0, $a, 0.0],""scale"":[1.0, 1.0, 1.0]}"
    curl.exe -s -X PATCH "$BASE/api/v1/nodes/$NODE/transform" -H "Authorization: Bearer $TOKEN" -H "Content-Type: application/json" -d $body | Out-Null
    Write-Host "  [$i] x=$x z=$z" -ForegroundColor Gray
    Start-Sleep -Milliseconds 250
}
Write-Host "Done!" -ForegroundColor Green
