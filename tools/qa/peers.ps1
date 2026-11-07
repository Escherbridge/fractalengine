# Coordinate N isolated app roots (owner/editor/viewer...) through the existing launcher; see AGENTS.md §peers.
[CmdletBinding()]
param(
    [Parameter(Mandatory)][ValidateSet('Start','Stop','Info','Restart','Focus')][string]$Action,
    [ValidateRange(2,8)][int]$Count = 3,
    [string]$OwnerRun,
    [string]$Peer,
    [string]$Stage,
    [ValidateSet('basic','terrain')][string]$Fixture = 'basic',
    [ValidateRange(1024,65520)][int]$BasePort = 8882,
    [int[]]$Ports,
    [string[]]$Names,
    [switch]$AllowStaleStage,
    [switch]$NativeObserver
)
. "$PSScriptRoot\_lib.ps1"
$ErrorActionPreference = 'Stop'
$PeersPointer = Join-Path $QaRepoRoot '.qa\peers-current'

function Get-PeerRoleNames {
    param([int]$Total)
    # Count 2 keeps the historical pair vocabulary (owner/viewer).
    if ($Total -eq 2) { return @('owner','viewer') }
    $names = @('owner','editor','viewer')
    for ($index = 3; $index -lt $Total; $index++) { $names += ('viewer' + ($index - 1)) }
    return $names
}

function Get-PeerMember {
    param([string]$Directory)
    $context = Get-QaResumeContext -Run $Directory
    return Get-QaPairMemberRecord $context.current
}

function Assert-PeerMemberMatches {
    param($Recorded, $Actual)
    if ($Actual.api_base -ne $Recorded.api_base -or $Actual.stage -ne $Recorded.stage -or
        $Actual.stage_manifest.exe_sha256 -cne $Recorded.stage_manifest.exe_sha256 -or
        $Actual.process_identity.pid -ne $Recorded.process_identity.pid -or
        $Actual.process_identity.start_ticks -cne $Recorded.process_identity.start_ticks -or
        $Actual.process_identity.exe_path -ne $Recorded.process_identity.exe_path) {
        throw "Peer $($Recorded.name) lifetime/provenance changed outside peers.ps1; stop it explicitly with stop.ps1 -Run"
    }
}

function Read-PeersRecord {
    param([string]$Run)
    if (-not $Run) {
        if (-not (Test-Path -LiteralPath $PeersPointer)) { throw 'No -OwnerRun given and no .qa/peers-current pointer' }
        $Run = (Get-Content -LiteralPath $PeersPointer -Raw).Trim()
    }
    $ownerContext = Get-QaResumeContext -Run $Run
    $path = Resolve-QaChildPath -Path (Join-Path $ownerContext.directory 'peers.json') -Root $QaRunRoot
    $record = Get-Content -LiteralPath $path -Raw | ConvertFrom-Json
    if ($record.schema -cne 'fe-qa-peers-v1' -or $record.members[0].run_dir -ne $ownerContext.directory -or
        $record.members[0].name -cne 'owner') { throw 'Invalid peers ownership record' }
    return @{ path = $path; record = $record; owner_dir = $ownerContext.directory }
}

function Write-PeersRecord {
    param([string]$Path, $Record)
    $pending = [IO.Path]::ChangeExtension($Path, '.pending.json')
    $Record | ConvertTo-Json -Depth 14 | Set-Content -LiteralPath $pending -Encoding utf8
    Move-Item -LiteralPath $pending -Destination $Path -Force
}

if ($Action -eq 'Start') {
    if ($OwnerRun -or $Peer) { throw 'Start always creates fresh roots; do not supply OwnerRun/Peer' }
    if (-not $Ports) { $Ports = @(0..($Count - 1) | ForEach-Object { $BasePort + $_ }) }
    if ($Ports.Count -ne $Count -or (@($Ports | Select-Object -Unique)).Count -ne $Count) { throw 'Peers require one distinct loopback API port each' }
    $memberNames = @(if ($Names) { $Names } else { Get-PeerRoleNames $Count })
    if ($memberNames.Count -ne $Count -or $memberNames[0] -cne 'owner' -or (@($memberNames | Select-Object -Unique)).Count -ne $Count -or
        @($memberNames | Where-Object { $_ -cnotmatch '^[a-z][a-z0-9]{0,15}$' }).Count) {
        throw '-Names needs Count distinct lowercase names starting with owner'
    }
    $launched = New-Object System.Collections.Generic.List[object]
    $complete = $false
    try {
        $options = @{ Fixture = $Fixture; ApiPort = $Ports[0]; AllowStaleStage = $AllowStaleStage }
        if ($NativeObserver) { $options.NativeObserver = $true; $options.DisableSyntheticInput = $true }
        if ($Stage) { $options.Stage = $Stage }
        for ($index = 0; $index -lt $Count; $index++) {
            if ($index -gt 0) {
                # Pin every peer to the owner's verified immutable stage; only the owner seeds a fixture.
                $options.Stage = [IO.Path]::GetFileName($launched[0].stage)
                $options.Fixture = 'none'
                $options.ApiPort = $Ports[$index]
            }
            $output = @(& "$PSScriptRoot\launch.ps1" @options)
            if (-not $output.Count) { throw "Launcher did not return an owned run path for $($memberNames[$index])" }
            $path = Resolve-QaChildPath -Path ([string]$output[-1]) -Root $QaRunRoot
            $member = Get-PeerMember -Directory $path
            $member.name = $memberNames[$index]
            $member.role = if ($index -eq 0) { 'owner' } elseif ($memberNames[$index] -clike 'editor*') { 'editor' } else { 'viewer' }
            $launched.Add($member)
        }
        $publicationLease = Enter-QaLifecycleLease
        $pinned = New-Object System.Collections.Generic.List[object]
        try {
            $keys = @{}
            foreach ($member in $launched) {
                $actual = Get-PeerMember -Directory $member.run_dir
                Assert-PeerMemberMatches $member ([pscustomobject]$actual)
                if ($actual.stage_manifest.source_fingerprint -cne $launched[0].stage_manifest.source_fingerprint -or
                    $actual.stage_manifest.exe_sha256 -cne $launched[0].stage_manifest.exe_sha256) { throw 'Peers do not share one stage' }
                $process = Get-QaOwnedProcess -Identity $member.process_identity
                if (-not $process) { throw "Peer $($member.name) exited before publication" }
                $pinned.Add($process)
                $tree = @(Get-QaOwnedProcessTree -Identity $member.process_identity)
                $member.descendant_identities = @($tree | Where-Object { $_.pid -ne $member.process_identity.pid })
                # Identity-file hashes are compared in memory only and discarded.
                $hash = (Get-FileHash -LiteralPath (Join-Path $member.run_dir 'identity\node_keypair.key') -Algorithm SHA256).Hash
                if ($keys.ContainsKey($hash)) { throw 'Peers do not have distinct run-local identities' }
                $keys[$hash] = $true
                $member.public_did = Get-QaPairPublicDid -Directory $member.run_dir
            }
            $keys = $null
            if ((@($launched | ForEach-Object { $_.public_did } | Select-Object -Unique)).Count -ne $Count -or
                (@($launched | ForEach-Object { $_.api_base } | Select-Object -Unique)).Count -ne $Count -or
                (@($launched | ForEach-Object { $_.run_dir } | Select-Object -Unique)).Count -ne $Count) {
                throw 'Peer roots, ports or issuer identities are not distinct'
            }
            $ownerDir = $launched[0].run_dir
            $peers = @{ schema = 'fe-qa-peers-v1'; revision = 1; count = $Count; members = $launched.ToArray()
                distinct_run_local_identities = $true; created_at = [DateTime]::UtcNow.ToString('o')
                transport = 'FE_SYNC_RELAY=disabled (launcher): loopback direct addresses carried by the native join ticket'
                collaboration = 'Not joined: owner generates addressed invites per peer DID with the role in the Access panel; transfer-invite drives each masked Join' }
            Write-PeersRecord -Path (Join-Path $ownerDir 'peers.json') -Record $peers
            if ($Count -eq 2) {
                # Historical pair-v1 publication so pair.ps1/native-observer restart lineage stay valid.
                $pair = @{ schema = 'fe-qa-pair-v1'; revision = 1; owner = $launched[0]; viewer = $launched[1];
                    distinct_run_local_identities = $true; created_at = $peers.created_at
                    collaboration = 'Not joined: generate invite, select Viewer role and join through the two real GUIs'
                    network = 'Existing launcher disables relay; direct peer connection must be verified independently'
                    controlled_disconnect = 'Stop/restart the explicit Viewer run; global networking is never changed' }
                $pairPath = Join-Path $ownerDir 'pair.json'
                $pair | ConvertTo-Json -Depth 12 | Set-Content -LiteralPath (Join-Path $ownerDir 'pair.pending.json') -Encoding utf8
                if ($pinned | Where-Object { $_.HasExited }) { throw 'A pair root exited before final publication' }
                Move-Item -LiteralPath (Join-Path $ownerDir 'pair.pending.json') -Destination $pairPath -Force
                foreach ($member in $launched) {
                    $memberPath = Join-Path $member.run_dir 'current.json'
                    $memberCurrent = Get-Content -LiteralPath $memberPath -Raw | ConvertFrom-Json
                    $memberCurrent | Add-Member -Force -NotePropertyName native_pair -NotePropertyValue @{pair_path=$pairPath;member=$member.name}
                    $memberPending = Join-Path $member.run_dir 'native-pair.pending.json'
                    $memberCurrent | ConvertTo-Json -Depth 16 | Set-Content -LiteralPath $memberPending -Encoding utf8
                    Move-Item -LiteralPath $memberPending -Destination $memberPath -Force
                }
            }
            Set-Content -LiteralPath $PeersPointer -Value $ownerDir -Encoding utf8
            $complete = $true
            $peers | ConvertTo-Json -Depth 14
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
                foreach ($member in $launched) {
                    try { Stop-QaOwnedProcessTree -Identity $member.process_identity -RecordedDescendants @($member.descendant_identities) }
                    catch { $failures.Add("$($member.run_dir)`: $_") }
                }
            } catch { $failures.Add('Cleanup lease unavailable: ' + $_) }
            finally { if ($cleanupLease) { $cleanupLease.Dispose() } }
            if ($failures.Count) { Write-Warning ('Peers startup cleanup incomplete: ' + ($failures -join '; ')) }
        }
    }
    return
}

$loaded = Read-PeersRecord -Run $OwnerRun
$peers = $loaded.record

if ($Action -eq 'Focus') {
    # The guarded Join lease needs OS focus; activate only the exact recorded peer identity.
    $target = @($peers.members | Where-Object { $_.name -ceq $Peer })
    if ($target.Count -ne 1) { throw "Unknown peer '$Peer'" }
    Assert-PeerMemberMatches $target[0] ([pscustomobject](Get-PeerMember -Directory $target[0].run_dir))
    $process = Get-QaOwnedProcess -Identity $target[0].process_identity
    if (-not $process) { throw "Peer $Peer is not running" }
    try {
        Add-Type -AssemblyName Microsoft.VisualBasic
        [Microsoft.VisualBasic.Interaction]::AppActivate([int]$process.Id)
    } finally { $process.Dispose() }
    Write-QaLog "Activated peer $Peer window"
    return
}

if ($Action -eq 'Restart') {
    $target = @($peers.members | Where-Object { $_.name -ceq $Peer })
    if ($target.Count -ne 1) { throw "Unknown peer '$Peer'" }
    $target = $target[0]
    Assert-PeerMemberMatches $target ([pscustomobject](Get-PeerMember -Directory $target.run_dir))
    # Relay is disabled, so a restarted peer has no address lookup: hand it the other live
    # peers' current dialable addresses (the documented FE_SYNC_BOOTSTRAP operator path).
    $bootstrap = @(foreach ($member in @($peers.members | Where-Object { $_.name -cne $Peer })) {
        $log = Get-ChildItem -LiteralPath $member.run_dir -Filter stdout.log -Recurse -File |
            Sort-Object LastWriteTimeUtc -Descending | Select-Object -First 1
        if (-not $log) { continue }
        $line = Select-String -LiteralPath $log.FullName -Pattern 'node_addr=(\{.*\})' | Select-Object -Last 1
        if ($line) { $line.Matches[0].Groups[1].Value }
    })
    $priorBootstrap = $env:FE_SYNC_BOOTSTRAP
    try {
        if ($bootstrap.Count) { $env:FE_SYNC_BOOTSTRAP = ($bootstrap -join ';') }
        # Same root, port and stage; launch -ResumeRun stops the exact prior identity first.
        $null = @(& "$PSScriptRoot\launch.ps1" -ResumeRun $target.run_dir)
    } finally { $env:FE_SYNC_BOOTSTRAP = $priorBootstrap }
    $fresh = Get-PeerMember -Directory $target.run_dir
    $did = Get-QaPairPublicDid -Directory $target.run_dir
    if ($did -cne $target.public_did) { throw 'Restarted peer changed its issuer identity' }
    foreach ($key in @('process_identity','descendant_identities','stage','stage_manifest','api_base')) { $target.$key = $fresh.$key }
    $peers.revision = [int]$peers.revision + 1
    $peers | Add-Member -Force -NotePropertyName restarted_at -NotePropertyValue ([DateTime]::UtcNow.ToString('o'))
    Write-PeersRecord -Path $loaded.path -Record $peers
    $target | ConvertTo-Json -Depth 10
    return
}

if ($Action -eq 'Stop') {
    foreach ($member in $peers.members) {
        Stop-QaNativeObserverWorkers -Run (Resolve-QaChildPath -Path $member.run_dir -Root $QaRunRoot)
    }
}
$lease = Enter-QaLifecycleLease
try {
    foreach ($member in $peers.members) { Assert-PeerMemberMatches $member ([pscustomobject](Get-PeerMember -Directory $member.run_dir)) }
    if ($Action -eq 'Info') {
        $peers | ConvertTo-Json -Depth 14
        return
    }
    $failures = New-Object System.Collections.Generic.List[string]
    # Non-owner peers first, owner last, each children-first through the shared exact-identity helper.
    foreach ($member in @($peers.members | Select-Object -Skip 1) + @($peers.members[0])) {
        try {
            $owned = Get-QaOwnedProcess -Identity $member.process_identity
            if ($owned) { $owned.Dispose() }
            else { Write-Warning "Peer $($member.name) root already exited; unseen descendants after its last snapshot cannot be proven absent" }
            Stop-QaOwnedProcessTree -Identity $member.process_identity -RecordedDescendants @($member.descendant_identities)
        } catch { $failures.Add("$($member.name) $($member.run_dir)`: $_") }
    }
    if ($failures.Count) { throw ('Peers cleanup incomplete: ' + ($failures -join '; ')) }
    Write-QaLog "All $($peers.members.Count) exact recorded peer process trees stopped or already absent; roots retained"
} finally { $lease.Dispose() }
