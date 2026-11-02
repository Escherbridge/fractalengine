# Coordinate two isolated app roots through the existing launcher; see AGENTS.md.
[CmdletBinding()]
param(
    [Parameter(Mandatory)][ValidateSet('Start','Stop','Info')][string]$Action,
    [string]$OwnerRun,
    [string]$Stage,
    [ValidateRange(1024,65535)][int]$OwnerPort = 8882,
    [ValidateRange(1024,65535)][int]$ViewerPort = 8883,
    [switch]$AllowStaleStage
)
. "$PSScriptRoot\_lib.ps1"
$ErrorActionPreference = 'Stop'

function Get-PairMember {
    param([string]$Directory)
    $context = Get-QaResumeContext -Run $Directory
    return @{ run_dir = $context.directory; api_base = $context.current.api_base;
        process_identity = $context.current.process_identity;
        descendant_identities = @($context.current.descendant_identities);
        stage = $context.current.stage; stage_manifest = $context.current.stage_manifest }
}

function Get-PairPublicDid {
    param([string]$Directory)
    $context = Get-QaResumeContext -Run $Directory
    $credential = Wait-QaOwnerToken -TokensPath $context.current.tokens_path -VerseId $context.current.bootstrap_verse_id -TimeoutSec 3
    if (-not $credential -or $credential.Token -isnot [string]) { throw 'Owned bootstrap identity credential unavailable; preserve token ACLs' }
    try {
        $parts = $credential.Token.Split('.')
        if ($parts.Length -ne 3 -or $parts[1].Length -gt 8192) { throw 'Invalid owned identity credential format' }
        $encoded = $parts[1].Replace('-', '+').Replace('_', '/')
        $encoded = $encoded.PadRight($encoded.Length + ((4 - $encoded.Length % 4) % 4), '=')
        $claims = [Text.Encoding]::UTF8.GetString([Convert]::FromBase64String($encoded)) | ConvertFrom-Json
        if ($claims.sub -isnot [string] -or $claims.sub -cnotmatch '^did:key:z6Mk[1-9A-HJ-NP-Za-km-z]{40,60}$' -or
            $claims.token_type -cne 'api' -or $claims.max_role -cne 'owner' -or
            $claims.scope -cne "VERSE#$($context.current.bootstrap_verse_id)") { throw 'Owned credential does not describe the bootstrapped issuer' }
        return $claims.sub
    } catch { throw 'Could not extract public issuer identity from the protected owned credential' }
    finally { $credential = $null; $claims = $null; $parts = $null; $encoded = $null }
}

if ($Action -eq 'Start') {
    if ($OwnerRun) { throw 'Start always creates two fresh roots; do not supply OwnerRun' }
    if ($OwnerPort -eq $ViewerPort) { throw 'Pair requires distinct high loopback API ports' }
    $launched = New-Object System.Collections.Generic.List[string]
    $launchedMembers = @{}
    $complete = $false
    try {
        $options = @{ Fixture = 'basic'; ApiPort = $OwnerPort; AllowStaleStage = $AllowStaleStage }
        if ($Stage) { $options.Stage = $Stage }
        $ownerOutput = @(& "$PSScriptRoot\launch.ps1" @options)
        if (-not $ownerOutput.Count) { throw 'Owner launcher did not return an owned run path' }
        $ownerPath = Resolve-QaChildPath -Path ([string]$ownerOutput[-1]) -Root $QaRunRoot
        $launched.Add($ownerPath)
        $owner = Get-PairMember -Directory $ownerPath
        $launchedMembers[$ownerPath] = $owner
        # Pin both peers to the same verified immutable stage, even if another stage appears.
        $options.Stage = [IO.Path]::GetFileName($owner.stage)
        $options.Fixture = 'none'
        $options.ApiPort = $ViewerPort
        $viewerOutput = @(& "$PSScriptRoot\launch.ps1" @options)
        if (-not $viewerOutput.Count) { throw 'Viewer launcher did not return an owned run path' }
        $viewerPath = Resolve-QaChildPath -Path ([string]$viewerOutput[-1]) -Root $QaRunRoot
        $launched.Add($viewerPath)
        $viewer = Get-PairMember -Directory $viewerPath
        $launchedMembers[$viewerPath] = $viewer
        $publicationLease = Enter-QaLifecycleLease
        $pinned = New-Object System.Collections.Generic.List[object]
        try {
            foreach ($member in @($owner, $viewer)) {
                $actual = Get-PairMember -Directory $member.run_dir
                if ($actual.api_base -ne $member.api_base -or $actual.stage -ne $member.stage -or
                    $actual.stage_manifest.source_fingerprint -cne $member.stage_manifest.source_fingerprint -or
                    $actual.stage_manifest.exe_sha256 -cne $member.stage_manifest.exe_sha256 -or
                    $actual.process_identity.pid -ne $member.process_identity.pid -or
                    $actual.process_identity.start_ticks -cne $member.process_identity.start_ticks -or
                    $actual.process_identity.exe_path -ne $member.process_identity.exe_path) {
                    throw 'Pair member lifetime/provenance changed before publication'
                }
                $process = Get-QaOwnedProcess -Identity $member.process_identity
                if (-not $process) { throw 'An exact pair root exited before publication' }
                $pinned.Add($process)
                $tree = @(Get-QaOwnedProcessTree -Identity $member.process_identity)
                if (-not ($tree | Where-Object { $_.pid -eq $member.process_identity.pid -and $_.start_ticks -eq $member.process_identity.start_ticks })) {
                    throw 'An exact pair root exited during its final descendant snapshot'
                }
                $member.descendant_identities = @($tree | Where-Object { $_.pid -ne $member.process_identity.pid })
            }
            if ($owner.run_dir -eq $viewer.run_dir -or $owner.api_base -eq $viewer.api_base -or
                $owner.process_identity.pid -eq $viewer.process_identity.pid -or
                $owner.stage_manifest.exe_sha256 -cne $viewer.stage_manifest.exe_sha256 -or
                $owner.stage_manifest.source_fingerprint -cne $viewer.stage_manifest.source_fingerprint) {
                throw 'Pair roots, ports, process lifetimes or stage provenance are not distinct/consistent'
            }
            $ownerKey = Get-FileHash -LiteralPath (Join-Path $owner.run_dir 'identity\node_keypair.key') -Algorithm SHA256
            $viewerKey = Get-FileHash -LiteralPath (Join-Path $viewer.run_dir 'identity\node_keypair.key') -Algorithm SHA256
            if ($ownerKey.Hash -ceq $viewerKey.Hash) { throw 'Pair does not have distinct run-local identities' }
            $ownerKey = $null
            $viewerKey = $null
            $owner.public_did = Get-PairPublicDid -Directory $owner.run_dir
            $viewer.public_did = Get-PairPublicDid -Directory $viewer.run_dir
            if ($owner.public_did -ceq $viewer.public_did) { throw 'Actual bootstrapped public issuer identities are identical' }
            $pair = @{ schema = 'fe-qa-pair-v1'; owner = $owner; viewer = $viewer;
                distinct_run_local_identities = $true; created_at = [DateTime]::UtcNow.ToString('o');
                collaboration = 'Not joined: generate invite, select Viewer role and join through the two real GUIs';
                network = 'Existing launcher disables relay; direct peer connection must be verified independently';
                controlled_disconnect = 'Stop/restart the explicit Viewer run; global networking is never changed' }
            $pairPath = Resolve-QaChildPath -Path (Join-Path $owner.run_dir 'pair.json') -Root $QaRunRoot
            $pendingPath = Resolve-QaChildPath -Path (Join-Path $owner.run_dir 'pair.pending.json') -Root $QaRunRoot
            $pair | ConvertTo-Json -Depth 12 | Set-Content -LiteralPath $pendingPath -Encoding utf8
            if ($pinned | Where-Object { $_.HasExited }) { throw 'An exact pair root exited before final publication' }
            Move-Item -LiteralPath $pendingPath -Destination $pairPath -Force
            $complete = $true
            $pair | ConvertTo-Json -Depth 12
        } finally {
            try { foreach ($process in $pinned) { $process.Dispose() } }
            finally { $publicationLease.Dispose() }
        }
    } finally {
        if (-not $complete) {
            $failures = New-Object System.Collections.Generic.List[string]
            $cleanupLease = $null
            try {
                $cleanupLease = Enter-QaLifecycleLease
                foreach ($directory in $launched) {
                    try {
                        $member = $launchedMembers[$directory]
                        if (-not $member) { throw 'Original launch identity was not captured; retained run requires explicit inspection' }
                        Stop-QaOwnedProcessTree -Identity $member.process_identity -RecordedDescendants @($member.descendant_identities)
                    } catch { $failures.Add("$directory`: $_") }
                }
            } catch { $failures.Add('Cleanup lease unavailable: ' + $_) }
            finally { if ($cleanupLease) { $cleanupLease.Dispose() } }
            if ($failures.Count) { Write-Warning ('Pair startup cleanup incomplete: ' + ($failures -join '; ')) }
        }
    }
    return
}

if (-not $OwnerRun) { throw 'Info/Stop require the explicit owner run; .qa/current is never used for a pair' }
$lease = Enter-QaLifecycleLease
try {
    $ownerContext = Get-QaResumeContext -Run $OwnerRun
    $pairPath = Resolve-QaChildPath -Path (Join-Path $ownerContext.directory 'pair.json') -Root $QaRunRoot
    $pair = Get-Content -LiteralPath $pairPath -Raw | ConvertFrom-Json
    if ($pair.schema -cne 'fe-qa-pair-v1' -or $pair.owner.run_dir -ne $ownerContext.directory -or
        $pair.distinct_run_local_identities -isnot [bool] -or -not $pair.distinct_run_local_identities) { throw 'Invalid pair ownership record' }
    $members = @($pair.viewer, $pair.owner)
    $snapshots = @()
    foreach ($member in $members) {
        $actual = Get-PairMember -Directory $member.run_dir
        if ($actual.api_base -ne $member.api_base -or $actual.stage -ne $member.stage -or
            $actual.stage_manifest.exe_sha256 -cne $member.stage_manifest.exe_sha256 -or
            $actual.process_identity.pid -ne $member.process_identity.pid -or
            $actual.process_identity.start_ticks -cne $member.process_identity.start_ticks -or
            $actual.process_identity.exe_path -ne $member.process_identity.exe_path) {
            throw 'Pair member was restarted or its ownership record changed; use explicit stop.ps1 for each current run'
        }
        $snapshots += $actual
    }
    if ($snapshots[0].run_dir -eq $snapshots[1].run_dir -or $snapshots[0].api_base -eq $snapshots[1].api_base) {
        throw 'Pair members do not identify distinct roots/ports'
    }
    if ($Action -eq 'Info') {
        $pair | ConvertTo-Json -Depth 12
        return
    }
    $failures = New-Object System.Collections.Generic.List[string]
    foreach ($member in $snapshots) {
        try {
            $owned = Get-QaOwnedProcess -Identity $member.process_identity
            if ($owned) { $owned.Dispose() }
            else { Write-Warning 'Pair root already exited; unseen descendants after its last snapshot cannot be proven absent' }
            Stop-QaOwnedProcessTree -Identity $member.process_identity -RecordedDescendants @($member.descendant_identities)
        } catch { $failures.Add("$($member.run_dir)`: $_") }
    }
    if ($failures.Count) { throw ('Pair cleanup incomplete: ' + ($failures -join '; ')) }
    Write-QaLog 'Both exact recorded app process trees stopped or already absent; roots retained'
} finally { $lease.Dispose() }
