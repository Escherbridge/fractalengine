# Exact paired restart membership transition; see native_observer/AGENTS.md.

function Get-QaPairIssuerSubject {
    param([Parameter(Mandatory)][string]$Token, [Parameter(Mandatory)][string]$VerseId)
    try {
        $parts = $Token.Split('.')
        if ($parts.Length -ne 3 -or $parts[1].Length -gt 8192) { throw 'Invalid owned identity credential format' }
        $encoded = $parts[1].Replace('-', '+').Replace('_', '/')
        $encoded = $encoded.PadRight($encoded.Length + ((4 - $encoded.Length % 4) % 4), '=')
        $claims = [Text.Encoding]::UTF8.GetString([Convert]::FromBase64String($encoded)) | ConvertFrom-Json
        if ($claims.sub -isnot [string] -or $claims.sub -cnotmatch '^did:key:z6Mk[1-9A-HJ-NP-Za-km-z]{40,60}$' -or
            $claims.token_type -cne 'api' -or $claims.max_role -cne 'owner' -or $claims.scope -cne "VERSE#$VerseId") {
            throw 'Owned credential does not describe the bootstrapped issuer'
        }
        return $claims.sub
    } catch { throw 'Could not extract public issuer identity from the protected owned credential' }
    finally { $Token=$null; $claims=$null; $parts=$null; $encoded=$null }
}

function Get-QaPairPublicDid {
    param([string]$Directory)
    $context = Get-QaResumeContext -Run $Directory
    $credential = Wait-QaOwnerToken -TokensPath $context.current.tokens_path -VerseId $context.current.bootstrap_verse_id -TimeoutSec 3
    if (-not $credential -or $credential.Token -isnot [string]) { throw 'Owned bootstrap identity credential unavailable; preserve token ACLs' }
    try { return Get-QaPairIssuerSubject -Token $credential.Token -VerseId $context.current.bootstrap_verse_id }
    finally { $credential=$null }
}

function Get-QaPairMemberRecord {
    param($Current)
    return @{run_dir=$Current.run_dir; api_base=$Current.api_base; process_identity=$Current.process_identity
        descendant_identities=@($Current.descendant_identities); stage=$Current.stage
        stage_manifest=$Current.stage_manifest; native_observer=$Current.native_observer}
}

function Assert-QaPairMemberCurrent {
    param($Member, $Current)
    foreach ($key in @('run_dir','api_base','process_identity','stage','stage_manifest','native_observer')) {
        if ((ConvertTo-QaNativeCanonical $Member.$key) -cne (ConvertTo-QaNativeCanonical $Current.$key)) {
            throw 'native_pair_member_changed'
        }
    }
}

function Assert-QaPairLink {
    param($Current, [string]$PairPath, [string]$Member)
    Assert-QaNativeKeys $Current.native_pair @('pair_path','member')
    if ($Current.native_pair.pair_path -cne $PairPath -or $Current.native_pair.member -cne $Member) {
        throw 'native_pair_link_changed'
    }
}

function Get-QaPairRevision {
    param($Pair)
    # Pre-reseal pair-v1 publications have implicit revision one.
    if (-not $Pair.PSObject.Properties['revision']) { return 1 }
    if ($Pair.revision -isnot [int] -or $Pair.revision -lt 1 -or $Pair.revision -ge [int]::MaxValue) {
        throw 'native_pair_revision_refused'
    }
    return $Pair.revision
}

function Open-QaPairRestart {
    param($Resume, [string]$Stage, [int]$Port, [bool]$NativeObserver, [bool]$SyntheticInput)
    $linked = $Resume.current.native_pair
    $ownPair = Join-Path $Resume.directory 'pair.json'
    if (-not $linked) {
        if (Test-Path -LiteralPath $ownPair) { throw 'native_pair_link_missing' }
        return $null
    }
    Assert-QaNativeKeys $linked @('pair_path','member')
    if ($linked.member -cnotin @('owner','viewer')) { throw 'native_pair_member_refused' }
    $pairPath = Resolve-QaChildPath -Path $linked.pair_path -Root $QaRunRoot
    if ([IO.Path]::GetFileName($pairPath) -cne 'pair.json') { throw 'native_pair_path_refused' }
    $file = Get-Item -LiteralPath $pairPath
    if ($file.Length -le 0 -or $file.Length -gt 65536) { throw 'native_pair_record_bound' }
    $pair = [IO.File]::ReadAllText($pairPath) | ConvertFrom-Json
    if ($pair.schema -cne 'fe-qa-pair-v1' -or $pair.distinct_run_local_identities -isnot [bool] -or
        -not $pair.distinct_run_local_identities -or
        $pairPath -cne (Join-Path $pair.owner.run_dir 'pair.json')) { throw 'native_pair_record_refused' }
    $revision = Get-QaPairRevision $pair
    $role = $linked.member
    $other = if ($role -ceq 'owner') { 'viewer' } else { 'owner' }
    Assert-QaPairLink $Resume.current $pairPath $role
    Assert-QaPairMemberCurrent $pair.$role $Resume.current
    if ($Stage -cne $Resume.current.stage -or $Port -ne $Resume.port -or
        $NativeObserver -ne [bool]$Resume.current.qa_options.native_observer -or
        $SyntheticInput -ne [bool]$Resume.current.qa_options.synthetic_input) { throw 'native_pair_restart_settings_changed' }
    if ($NativeObserver -and (-not $Resume.current.native_observer -or $SyntheticInput)) { throw 'native_pair_bootstrap_required' }
    $counterpart = Get-QaResumeContext -Run $pair.$other.run_dir
    Assert-QaPairLink $counterpart.current $pairPath $other
    Assert-QaPairMemberCurrent $pair.$other $counterpart.current
    if ($counterpart.directory -ceq $Resume.directory -or $counterpart.port -eq $Port -or
        $counterpart.current.stage -cne $Stage -or
        (ConvertTo-QaNativeCanonical $counterpart.current.stage_manifest) -cne (ConvertTo-QaNativeCanonical $Resume.current.stage_manifest)) {
        throw 'native_pair_counterpart_refused'
    }
    $process = Get-QaOwnedProcess -Identity $counterpart.current.process_identity
    if (-not $process) { throw 'native_pair_counterpart_not_live' }
    return @{pair=$pair; path=$pairPath; revision=$revision; role=$role; other=$other; process=$process
        old_current=$Resume.current; counterpart=$counterpart.current
        counterpart_sha256=(Get-FileHash -LiteralPath (Join-Path $counterpart.directory 'current.json') -Algorithm SHA256).Hash
        pair_sha256=(Get-FileHash -LiteralPath $pairPath -Algorithm SHA256).Hash}
}

function Save-QaPairRestartArchive {
    param($Transition, [string]$AttemptDirectory)
    if (-not $Transition) { return }
    $archive = Join-Path $AttemptDirectory 'previous-pair.json'
    if (Test-Path -LiteralPath $archive) { throw 'native_pair_archive_exists' }
    [IO.File]::Copy($Transition.path, $archive, $false)
    $archiveFile = [IO.File]::Open($archive, [IO.FileMode]::Open, [IO.FileAccess]::ReadWrite, [IO.FileShare]::Read)
    try { $archiveFile.Flush($true) } finally { $archiveFile.Dispose() }
    if ((Get-FileHash -LiteralPath $archive -Algorithm SHA256).Hash -cne $Transition.pair_sha256) { throw 'native_pair_archive_changed' }
    $Transition.archive = $archive
    $Transition.intent = Join-Path $AttemptDirectory 'pair-reseal.json'
    @{schema='fe-native-pair-reseal-v1'; status='prepared'; member=$Transition.role
        previous_pair=$archive; previous_pair_sha256=$Transition.pair_sha256; previous_revision=$Transition.revision
        old_process=$Transition.old_current.process_identity; old_bootstrap=$Transition.old_current.native_observer
        counterpart_process=$Transition.counterpart.process_identity} |
        ConvertTo-Json -Depth 16 | Set-Content -LiteralPath $Transition.intent -Encoding utf8
}

function New-QaPairRestartRevision {
    param($Transition, $Current, [string]$PublicDid)
    $old = $Transition.old_current
    if ($PublicDid -cnotmatch '^did:key:z6Mk[1-9A-HJ-NP-Za-km-z]{40,60}$' -or
        $PublicDid -cne $Transition.pair.($Transition.role).public_did -or
        $PublicDid -ceq $Transition.pair.($Transition.other).public_did) { throw 'native_pair_issuer_changed' }
    if ($Current.run_dir -cne $old.run_dir -or $Current.api_base -cne $old.api_base -or $Current.stage -cne $old.stage -or
        (ConvertTo-QaNativeCanonical $Current.stage_manifest) -cne (ConvertTo-QaNativeCanonical $old.stage_manifest) -or
        [long]$Current.process_identity.start_ticks -le [long]$old.process_identity.start_ticks -or
        $Current.process_identity.exe_path -cne $old.process_identity.exe_path) { throw 'native_pair_new_incarnation_refused' }
    Assert-QaPairLink $Current $Transition.path $Transition.role
    if ($old.native_observer) {
        $fresh = $Current.native_observer
        if (-not $fresh -or (ConvertTo-QaNativeCanonical $fresh.process_identity) -cne (ConvertTo-QaNativeCanonical $Current.process_identity)) {
            throw 'native_pair_new_bootstrap_required'
        }
        foreach ($key in @('run_id','ledger_id','ledger_file_id','observer_file_id','ledger_path','observer_path')) {
            if (-not $fresh.$key -or $fresh.$key -ceq $old.native_observer.$key) { throw 'native_pair_bootstrap_reused' }
        }
        if ($fresh.helper_sha256 -cne $old.native_observer.helper_sha256) { throw 'native_pair_helper_changed' }
    } elseif ($Current.native_observer) { throw 'native_pair_mode_changed' }
    $next = $Transition.pair | ConvertTo-Json -Depth 16 | ConvertFrom-Json
    $replacement = Get-QaPairMemberRecord $Current
    $replacement.public_did = $PublicDid
    $next.($Transition.role) = [pscustomobject]$replacement
    $next | Add-Member -Force -NotePropertyName revision -NotePropertyValue ([int]($Transition.revision + 1))
    $next | Add-Member -Force -NotePropertyName previous_pair -NotePropertyValue $Transition.archive
    $next | Add-Member -Force -NotePropertyName previous_pair_sha256 -NotePropertyValue $Transition.pair_sha256
    $next | Add-Member -Force -NotePropertyName resealed_at -NotePropertyValue ([DateTime]::UtcNow.ToString('o'))
    return $next
}

function Assert-QaFreshPairBootstrap {
    param($Current)
    $fresh = $Current.native_observer
    if (-not $fresh) { return }
    $expectedDirectory = Join-Path $Current.run_dir ("native-observer\" + ([long]$Current.process_identity.start_ticks - [long]504911232000000000) + '-' + $fresh.run_id)
    if ($fresh.schema -cne 'fe-qa-native-bootstrap-v1' -or $fresh.run_id -cnotmatch '^[a-f0-9]{32}$' -or
        $fresh.ledger_id -cnotmatch '^[a-f0-9]{32}$' -or $fresh.ledger_path -cne (Join-Path $expectedDirectory 'attempt.jsonl') -or
        $fresh.observer_path -cne (Join-Path $expectedDirectory 'observer.lock')) { throw 'native_pair_fresh_bootstrap_refused' }
    $expected = [pscustomobject]@{schema='fe-native-attempt-v1';kind='bootstrap';run_id=$fresh.run_id;ledger_id=$fresh.ledger_id
        pid=[int]$Current.process_identity.pid;birth_filetime=([long]$Current.process_identity.start_ticks - [long]504911232000000000).ToString()
        exe_sha256=$Current.stage_manifest.exe_sha256.ToLowerInvariant();helper_sha256=$fresh.helper_sha256}
    foreach ($entry in @(@($fresh.ledger_path,$fresh.ledger_file_id), @($fresh.observer_path,$fresh.observer_file_id))) {
        $path = Resolve-QaChildPath -Path $entry[0] -Root $QaRunRoot
        $file = [IO.File]::Open($path, [IO.FileMode]::Open, [IO.FileAccess]::ReadWrite, [IO.FileShare]::None)
        try {
            if ([QaNativeFileIdentity]::Read($file, $path) -cne $entry[1]) { throw 'native_pair_fresh_file_replaced' }
            if ($path -ceq $fresh.ledger_path) {
                $bytes = [Text.Encoding]::UTF8.GetBytes((ConvertTo-QaNativeCanonical $expected) + "`n")
                $actual = New-Object byte[] $bytes.Length
                if ($file.Length -ne $bytes.Length -or $file.Read($actual,0,$actual.Length) -ne $actual.Length -or
                    [Convert]::ToBase64String($actual) -cne [Convert]::ToBase64String($bytes)) { throw 'native_pair_fresh_ledger_changed' }
            } elseif ($file.Length -ne 0) { throw 'native_pair_fresh_observer_changed' }
        } finally { $file.Dispose() }
    }
}

function Complete-QaPairRestart {
    param($Transition, $Current, [string]$PublicDid)
    if (-not $Transition) { return }
    if ($Transition.process.HasExited -or
        (Get-FileHash -LiteralPath $Transition.path -Algorithm SHA256).Hash -cne $Transition.pair_sha256) { throw 'native_pair_lineage_changed' }
    $counterpart = Get-QaResumeContext -Run $Transition.counterpart.run_dir
    if ((Get-FileHash -LiteralPath (Join-Path $counterpart.directory 'current.json') -Algorithm SHA256).Hash -cne $Transition.counterpart_sha256) {
        throw 'native_pair_counterpart_changed'
    }
    Assert-QaPairMemberCurrent $Transition.pair.($Transition.other) $counterpart.current
    Assert-QaPairLink $counterpart.current $Transition.path $Transition.other
    $next = New-QaPairRestartRevision $Transition $Current $PublicDid
    Assert-QaFreshPairBootstrap $Current
    $currentProcess = Get-QaOwnedProcess -Identity $Current.process_identity
    if (-not $currentProcess) { throw 'native_pair_new_process_not_live' }
    try {
        $pending = Join-Path ([IO.Path]::GetDirectoryName($Transition.path)) 'pair.pending.json'
        $next | ConvertTo-Json -Depth 16 | Set-Content -LiteralPath $pending -Encoding utf8
        if ($Transition.process.HasExited -or $currentProcess.HasExited) { throw 'native_pair_process_exited' }
        [IO.File]::Replace($pending, $Transition.path, $null)
        @{schema='fe-native-pair-reseal-v1'; status='committed'; revision=$next.revision; member=$Transition.role
            previous_pair=$Transition.archive; previous_pair_sha256=$Transition.pair_sha256
            new_process=$Current.process_identity; new_bootstrap=$Current.native_observer} |
            ConvertTo-Json -Depth 16 | Set-Content -LiteralPath $Transition.intent -Encoding utf8
    } finally { $currentProcess.Dispose() }
}
