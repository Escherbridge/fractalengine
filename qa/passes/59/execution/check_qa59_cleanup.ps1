$ErrorActionPreference = 'Stop'
$context = Get-Content -LiteralPath "$PSScriptRoot/qa59-owned-context.json" -Raw | ConvertFrom-Json
$records = @($context.owner.process_identity) + @($context.owner.descendant_identities)
$checks = @($records | ForEach-Object {
    $record = $_
    $process = Get-Process -Id $record.pid -ErrorAction SilentlyContinue
    $matches = $false
    if ($process) {
        $matches = ($process.StartTime.ToUniversalTime().Ticks.ToString() -eq $record.start_ticks) -and ($process.Path -eq $record.exe_path)
    }
    [pscustomobject]@{ identity = $record; pid_present = [bool]$process; recorded_identity_present = $matches }
})
$result = [pscustomobject]@{
    kind = 'qa59-recorded-process-cleanup'
    checked_at = [DateTime]::UtcNow.ToString('o')
    all_recorded_identities_absent = (@($checks | Where-Object recorded_identity_present).Count -eq 0)
    checks = $checks
}
$result | ConvertTo-Json -Depth 8 | Set-Content -LiteralPath "$PSScriptRoot/qa59-cleanup-identities.json" -Encoding UTF8
Write-Output ($result | ConvertTo-Json -Depth 8 -Compress)
if (-not $result.all_recorded_identities_absent) { exit 1 }

