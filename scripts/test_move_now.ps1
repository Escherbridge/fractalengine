# Quick transform test — pass token and node_id directly
# Usage: .\test_move_now.ps1 "TOKEN" "NODE_ID"
param(
    [Parameter(Mandatory=$true)][string]$Token,
    [Parameter(Mandatory=$true)][string]$NodeId
)

$BASE = "http://localhost:8765"
$headers = @{
    "Authorization" = "Bearer $Token"
    "Content-Type"  = "application/json"
}

Write-Host "Moving node $NodeId in a circle..." -ForegroundColor Cyan

for ($i = 0; $i -lt 20; $i++) {
    $angle = $i * 0.314
    $x = [math]::Round([math]::Sin($angle) * 3, 3)
    $z = [math]::Round([math]::Cos($angle) * 3, 3)
    $y = [math]::Round([math]::Sin($angle * 0.5) * 0.5 + 0.5, 3)
    $bodyObj = @{
        position = @($x, $y, $z)
        rotation = @(0.0, $angle, 0.0)
        scale    = @(1.0, 1.0, 1.0)
    }
    $body = $bodyObj | ConvertTo-Json -Compress

    try {
        $result = Invoke-RestMethod -Method Patch `
            -Uri "$BASE/api/v1/nodes/$NodeId/transform" `
            -Headers $headers `
            -Body $body `
            -ContentType "application/json"
        $status = $result | ConvertTo-Json -Compress
    } catch {
        $status = $_.Exception.Message
    }

    Write-Host "  [$i] x=$x y=$y z=$z  $status" -ForegroundColor Gray
    Start-Sleep -Milliseconds 250
}
Write-Host "Done!" -ForegroundColor Green
