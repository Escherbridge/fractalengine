$ErrorActionPreference = 'Stop'
$repo = "C:\Users\atooz\Programming\fractalengine-workspace\fractalengine"
$research = "$repo\.omc\research"

try {
    & "$repo\tools\qa\launch.ps1" -Stage "9498869e-20261104204501360" -Fixture basic -DisableSyntheticInput -NativeObserver *> "$research\qa62-launch.log"
    $runDir = (Get-Content -LiteralPath "$repo\.qa\current" -Raw).Trim()
    Write-Output "LAUNCHED_RUN=$runDir"
    python "$research\qa62_native_drive.py"
} finally {
    $runDir = if (Test-Path -LiteralPath "$repo\.qa\current") { (Get-Content -LiteralPath "$repo\.qa\current" -Raw).Trim() } else { $null }
    if ($runDir) {
        & "$repo\tools\qa\stop.ps1" -Run $runDir *> "$research\qa62-stop.log"
    }
}

$context = Get-Content -LiteralPath "$research\qa62-owned-context.json" -Raw | ConvertFrom-Json
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
    kind = 'qa62-recorded-process-cleanup'
    checked_at = [DateTime]::UtcNow.ToString('o')
    all_recorded_identities_absent = (@($checks | Where-Object recorded_identity_present).Count -eq 0)
    checks = $checks
}
$result | ConvertTo-Json -Depth 8 | Set-Content -LiteralPath "$research\qa62-cleanup-identities.json" -Encoding UTF8
Write-Output ($result | ConvertTo-Json -Depth 8 -Compress)
if (-not $result.all_recorded_identities_absent) { exit 1 }
