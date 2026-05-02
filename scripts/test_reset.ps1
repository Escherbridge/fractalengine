# Reset node to origin
$TOKEN = $env:FE_TOKEN; $BASE = $env:FE_BASE; $NODE = $env:FE_NODE_ID
if (-not $TOKEN -or -not $NODE) { Write-Host "Run api_test_setup.ps1 first!" -ForegroundColor Red; exit 1 }

curl.exe -s -X PATCH "$BASE/api/v1/nodes/$NODE/transform" -H "Authorization: Bearer $TOKEN" -H "Content-Type: application/json" -d "{""position"":[0,0,0],""rotation"":[0,0,0],""scale"":[1,1,1]}" | Out-Null
Write-Host "Reset $NODE to origin." -ForegroundColor Green
