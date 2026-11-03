# Inspect an explicit stopped disposable legacy fixture; see AGENTS.md.
[CmdletBinding()]
param([Parameter(Mandatory)][ValidateNotNullOrEmpty()][string]$Run,
      [Parameter(Mandatory)][ValidateSet('discovery','baseline','adopted')][string]$Phase)
. "$PSScriptRoot\_lib.ps1"
. "$PSScriptRoot\_legacy_fixture.ps1"
$ErrorActionPreference = 'Stop'
$lease = $null; $restartLock = $null
try {
    $lease = Enter-QaLifecycleLease
    $context = Get-QaResumeContext -Run $Run
    $restartLock = [IO.File]::Open((Join-Path $context.directory 'restart.lock'), [IO.FileMode]::OpenOrCreate, [IO.FileAccess]::ReadWrite, [IO.FileShare]::None)
    $report = Read-QaLegacyFixture -Context $context -Stage $context.current.stage -Phase $Phase -Lease $lease
    $report | ConvertTo-Json -Depth 20 -Compress
} catch {
    $code = [string]$_.Exception.Message
    if ($code -cnotmatch '^[a-z_]{1,80}$') { $code = 'legacy_fixture_inspection_refused' }
    @{ ok = $false; code = $code } | ConvertTo-Json -Compress
    exit 1
} finally { if ($restartLock) { $restartLock.Dispose() }; if ($lease) { $lease.Dispose() } }
