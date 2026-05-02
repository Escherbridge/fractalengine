# Test: Bounce node up and down with scale pulse
$TOKEN = $env:FE_TOKEN; $BASE = $env:FE_BASE; $NODE = $env:FE_NODE_ID
if (-not $TOKEN -or -not $NODE) { Write-Host "Run api_test_setup.ps1 first!" -ForegroundColor Red; exit 1 }

Write-Host "Bounce on $NODE..." -ForegroundColor Cyan
for ($i = 0; $i -lt 30; $i++) {
    $t = $i * 0.2
    $y = [math]::Round([math]::Abs([math]::Sin($t)) * 4, 3)
    $s = [math]::Round(1.0 + [math]::Sin($t * 2) * 0.3, 3)
    $body = "{""position"":[0.0, $y, 0.0],""rotation"":[0.0, 0.0, 0.0],""scale"":[$s, $s, $s]}"
    curl.exe -s -X PATCH "$BASE/api/v1/nodes/$NODE/transform" -H "Authorization: Bearer $TOKEN" -H "Content-Type: application/json" -d $body | Out-Null
    Write-Host "  [$i] y=$y scale=$s" -ForegroundColor Gray
    Start-Sleep -Milliseconds 200
}
Write-Host "Done!" -ForegroundColor Green
