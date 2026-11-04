# Existing evidence-boundary journey support; no app/process/native authority credit.
param([Parameter(Mandatory)][string]$Scratch)
$ErrorActionPreference = 'Stop'
. (Join-Path $PSScriptRoot '..\_lib.ps1')
$QaRepoRoot = [IO.Path]::GetFullPath($Scratch)
$QaRunRoot = Join-Path $QaRepoRoot 'run'
$QaStageRoot = Join-Path $QaRepoRoot 'stage'
New-Item -ItemType Directory -Path $QaRunRoot,$QaStageRoot -Force | Out-Null
$results = New-Object System.Collections.Generic.List[object]
function Record-Case([string]$Name, [bool]$Passed) {
    $results.Add(@{case="native-lifecycle-$Name";expected_accepted=$true;accepted=$Passed;passed=$Passed
        boundary='offline production inventory/reseal with controlled process/bootstrap seams; no native/application coverage'})
}
function Write-Json($Path, $Value) { $Value | ConvertTo-Json -Depth 32 | Set-Content -LiteralPath $Path -Encoding utf8 }
function Clone($Value) { return $Value | ConvertTo-Json -Depth 32 | ConvertFrom-Json }
function New-ScenarioStage([string]$Name) {
    $directory = Join-Path $QaStageRoot $Name
    New-Item -ItemType Directory -Path $directory -Force | Out-Null
    [IO.File]::WriteAllText((Join-Path $directory 'fractalengine.exe'), "Offline image fixture $Name; never executed")
    [IO.File]::WriteAllText((Join-Path $directory 'native_observer.exe'), 'Offline helper image fixture; never executed')
    $manifest = [pscustomobject]@{kind='fractalengine-qa-stage-v1'
        exe_sha256=(Get-FileHash -LiteralPath (Join-Path $directory 'fractalengine.exe') -Algorithm SHA256).Hash.ToLowerInvariant()
        native_observer=@{schema='fe-native-helper-stage-v1';helper_file='native_observer.exe'
            helper_sha256=(Get-FileHash -LiteralPath (Join-Path $directory 'native_observer.exe') -Algorithm SHA256).Hash.ToLowerInvariant()}}
    Write-Json (Join-Path $directory 'stage.json') $manifest
    return @{directory=$directory;manifest=$manifest}
}

$run = Join-Path $QaRunRoot '20261104-000000-000'
$fixtureStage = New-ScenarioStage 'fixture'
$stage = $fixtureStage.directory
New-Item -ItemType Directory -Path $run -Force | Out-Null
$helper = Join-Path $stage 'native_observer.exe'
$stageManifest = $fixtureStage.manifest
$helperSha = $stageManifest.native_observer.helper_sha256
$identity = [pscustomobject]@{pid=1234;start_ticks='504911232123456789';exe_path=(Join-Path $stage 'fractalengine.exe')}
$native = Join-Path $run ('native-observer\123456789-' + ('1'*32))
$workers = Join-Path $native 'workers'
New-Item -ItemType Directory -Path $workers -Force | Out-Null
$bootstrap = [pscustomobject]@{schema='fe-qa-native-bootstrap-v1';run_id=('1'*32);ledger_id=('2'*32);ledger_file_id=('b'*24)
    ledger_path=(Join-Path $native 'attempt.jsonl');helper_sha256=$helperSha}
[IO.File]::WriteAllText($bootstrap.ledger_path, 'retained spent/unknown ledger fixture')
$current = [pscustomobject]@{run_dir=$run;process_identity=$identity;stage=$stage;stage_manifest=$stageManifest
    qa_options=@{synthetic_input=$false;native_observer=$true};native_observer=$bootstrap}
Write-Json (Join-Path $run 'current.json') $current
$prefix = [pscustomobject]@{schema='fe-native-worker-owner-v1';phase='reserved';operation='';controller_identity=$identity
    app_identity=$identity;helper_path=$helper;helper_sha256=$helperSha;run_id=('1'*32);ledger_id=('2'*32)
    ledger_file_id=('b'*24);mode='observe'}
$utf8 = New-Object Text.UTF8Encoding($false)
for ($index=1; $index -le 254; $index++) {
    $prefix.operation = $index.ToString('x32')
    [IO.File]::WriteAllText((Join-Path $workers ($prefix.operation+'.jsonl')), (ConvertTo-QaNativeCanonical $prefix)+"`n", $utf8)
}
Assert-QaNativeAllocation -Run $run -Jsonl 1
$prefix.operation = (255).ToString('x32')
$lastReceipt = Join-Path $workers ($prefix.operation+'.jsonl')
[IO.File]::WriteAllText($lastReceipt, (ConvertTo-QaNativeCanonical $prefix)+"`n", $utf8)
$before = Get-FileHash -LiteralPath $bootstrap.ledger_path,$lastReceipt -Algorithm SHA256
$owned = @(Get-QaNativeWorkerRecords -Run $run)
Record-Case 'last-receipt-is-completely-owned' ($owned.Count -eq 255 -and -not @($owned | Where-Object {$_.confirmed}).Count)
$refused = $false
try { Assert-QaNativeAllocation -Run $run -Jsonl 1 } catch { $refused = $_.Exception.Message -ceq 'native_history_capacity' }
$after = Get-FileHash -LiteralPath $bootstrap.ledger_path,$lastReceipt -Algorithm SHA256
Record-Case 'first-excess-receipt-refused-before-spend' ($refused -and ($before.Hash -join '|') -ceq ($after.Hash -join '|') -and @(Get-QaNativeWorkerRecords -Run $run).Count -eq 255)
$history = Join-Path $run 'starts'
New-Item -ItemType Directory -Path $history -Force | Out-Null
for ($index=1; $index -le 255; $index++) { New-Item -ItemType Directory -Path (Join-Path $history $index) | Out-Null }
Assert-QaNativeAllocation -Run $run -Starts 1
New-Item -ItemType Directory -Path (Join-Path $history '256') | Out-Null
$refused = $false
try { Assert-QaNativeAllocation -Run $run -Starts 1 } catch { $refused = $_.Exception.Message -ceq 'native_history_capacity' }
Record-Case 'last-start-inspectable-next-refused-before-stop' ($refused -and @(Get-QaNativeWorkerRecords -Run $run).Count -eq 255 -and @(Get-ChildItem -LiteralPath $history -Directory).Count -eq 256)

$accumulated = Join-Path $QaRunRoot '20261104-000001-000'
New-Item -ItemType Directory -Path $accumulated -Force | Out-Null
for ($index=1; $index -le 255; $index++) {
    $directory = Join-Path $accumulated ("native-observer\$index")
    New-Item -ItemType Directory -Path $directory -Force | Out-Null
    [IO.File]::WriteAllText((Join-Path $directory 'attempt.jsonl'), 'retained historical outcome')
}
Assert-QaNativeAllocation -Run $accumulated -Jsonl 1 -Incarnations 1
$observedCurrent = Clone $current
$observedCurrent.run_dir=$accumulated; $observedCurrent.native_observer=$null
Write-Json (Join-Path $accumulated 'current.json') $observedCurrent
$hash = [Security.Cryptography.SHA256]::Create()
try { $observedRun = [BitConverter]::ToString($hash.ComputeHash([Text.Encoding]::UTF8.GetBytes((ConvertTo-QaNativeCanonical $identity)))).Replace('-','').ToLowerInvariant().Substring(0,32) }
finally { $hash.Dispose() }
$directory = Join-Path $accumulated ("native-observer-observe\123456789-$observedRun\workers")
New-Item -ItemType Directory -Path $directory -Force | Out-Null
$observedPrefix = Clone $prefix
$observedPrefix.run_id=$observedRun; $observedPrefix.ledger_id=''; $observedPrefix.ledger_file_id=''
[IO.File]::WriteAllText((Join-Path $directory ($observedPrefix.operation+'.jsonl')), (ConvertTo-QaNativeCanonical $observedPrefix)+"`n", $utf8)
$refused = $false
try { Assert-QaNativeAllocation -Run $accumulated -Jsonl 1 -Incarnations 1 } catch { $refused = $_.Exception.Message -ceq 'native_history_capacity' }
$inventory = Get-QaNativeHistoryInventory -Run $accumulated
Record-Case 'both-roots-bootstrap-budget-retained' ($refused -and $inventory.counts.jsonl_files -eq 256 -and $inventory.counts.incarnation_directories -eq 256 -and @(Get-QaNativeWorkerRecords -Run $accumulated).Count -eq 1)

# Real stage files exercise prune/parse dependency; only process ownership is controlled.
$historicalRun = Join-Path $QaRunRoot '20261104-000004-000'
$stageA = New-ScenarioStage 'historical-a'
$stageB = New-ScenarioStage 'current-b'
$oldCurrent = Clone $current
$oldCurrent.run_dir=$historicalRun; $oldCurrent.stage=$stageA.directory; $oldCurrent.stage_manifest=$stageA.manifest
$oldCurrent.process_identity.exe_path=Join-Path $stageA.directory 'fractalengine.exe'
$oldDirectory = Join-Path $historicalRun ('native-observer\123456789-'+('1'*32))
$oldWorkers = Join-Path $oldDirectory 'workers'
New-Item -ItemType Directory -Path $oldWorkers -Force | Out-Null
$oldCurrent.native_observer.ledger_path=Join-Path $oldDirectory 'attempt.jsonl'
[IO.File]::WriteAllText($oldCurrent.native_observer.ledger_path, 'retained spent historical ledger')
$historicalPrefix = Clone $prefix
$historicalPrefix.operation='f'*32; $historicalPrefix.app_identity=Clone $oldCurrent.process_identity
$historicalPrefix.controller_identity=[pscustomobject]@{pid=2345;start_ticks='504911232123456788';exe_path=(Join-Path $QaRepoRoot 'controller.exe')}
$historicalPrefix.helper_path=Join-Path $stageA.directory 'native_observer.exe'
$historicalPrefix.helper_sha256=$stageA.manifest.native_observer.helper_sha256
$workerIdentity = [pscustomobject]@{pid=3456;start_ticks='504911232123456790';exe_path=$historicalPrefix.helper_path}
$workerStarted = [pscustomobject]@{schema=$historicalPrefix.schema;phase='worker_started';operation=$historicalPrefix.operation;worker_identity=$workerIdentity}
$exitConfirmed = [pscustomobject]@{schema=$historicalPrefix.schema;phase='exit_confirmed';operation=$historicalPrefix.operation
    worker_identity=$workerIdentity;exit_code=0;code='worker_exited'}
$receipt = Join-Path $oldWorkers ($historicalPrefix.operation+'.jsonl')
function Write-HistoricalReceipt($Entries) {
    $text = (@($Entries | ForEach-Object { ConvertTo-QaNativeCanonical $_ }) -join "`n")+"`n"
    [IO.File]::WriteAllText($receipt, $text, $utf8)
}
Write-Json (Join-Path $historicalRun 'current.json') $oldCurrent
Write-HistoricalReceipt @($historicalPrefix,$workerStarted,$exitConfirmed)
Record-Case 'confirmed-original-stage-owned' (@(Get-QaNativeWorkerRecords -Run $historicalRun | Where-Object {$_.confirmed}).Count -eq 1)
$archive = Join-Path $historicalRun 'starts\upgrade'
New-Item -ItemType Directory -Path $archive -Force | Out-Null
Write-Json (Join-Path $archive 'previous-start.json') $oldCurrent
$newCurrent = Clone $oldCurrent
$newCurrent.stage=$stageB.directory; $newCurrent.stage_manifest=$stageB.manifest
$newCurrent.process_identity.pid=4567; $newCurrent.process_identity.start_ticks='504911232223456789'
$newCurrent.process_identity.exe_path=Join-Path $stageB.directory 'fractalengine.exe'
$newDirectory = Join-Path $historicalRun ('native-observer\223456789-'+('3'*32))
New-Item -ItemType Directory -Path (Join-Path $newDirectory 'workers') -Force | Out-Null
$newCurrent.native_observer.run_id='3'*32; $newCurrent.native_observer.ledger_id='4'*32
$newCurrent.native_observer.ledger_file_id='d'*24
$newCurrent.native_observer.ledger_path=Join-Path $newDirectory 'attempt.jsonl'
[IO.File]::WriteAllText($newCurrent.native_observer.ledger_path, 'fresh new-incarnation bootstrap fixture')
Write-Json (Join-Path $historicalRun 'current.json') $newCurrent
Write-HistoricalReceipt @($historicalPrefix,$workerStarted)
$unknownRetained = @(Get-QaNativeRetainedStages -Run $historicalRun)
Record-Case 'unconfirmed-real-stage-retained-before-prune' ($unknownRetained.Count -eq 1 -and $unknownRetained[0] -ceq $stageA.directory)
Write-HistoricalReceipt @($historicalPrefix,$workerStarted,$exitConfirmed)
$retained = @(Get-QaNativeRetainedStages -Run $historicalRun)
Record-Case 'confirmed-old-stage-not-retained-forever' (-not $retained.Count)
$prunable = Resolve-QaChildPath -Path $stageA.directory -Root $QaStageRoot
if ($prunable -cne $stageA.directory -or -not $prunable.StartsWith($QaStageRoot+'\', [StringComparison]::OrdinalIgnoreCase)) { throw 'scratch_stage_prune_boundary' }
Remove-Item -LiteralPath $prunable -Recurse -Force
$processCalls=0
function Get-QaOwnedProcess { param($Identity); $script:processCalls++; throw 'Unexpected process API seam in confirmed-history cleanup' }
$stopped=$true
try { Stop-QaNativeObserverWorkers -Run $historicalRun } catch { $stopped=$false }
$confirmed = @(Get-QaNativeWorkerRecords -Run $historicalRun)
Record-Case 'confirmed-pruned-stage-inspects-and-stops' ($stopped -and $processCalls -eq 0 -and $confirmed.Count -eq 1 -and $confirmed[0].confirmed -and -not (Test-Path -LiteralPath $stageA.directory) -and (Test-Path -LiteralPath (Join-Path $stageB.directory 'fractalengine.exe')))
foreach ($case in @('unstarted','reapable','unknown','lone-terminal','duplicated-terminal','mismatched-identity')) {
    $entries = switch ($case) {
        'unstarted' { @($historicalPrefix) }
        'reapable' { @($historicalPrefix,$workerStarted) }
        'unknown' { $unknown=[pscustomobject]@{schema=$historicalPrefix.schema;phase='lifetime_unknown';operation=$historicalPrefix.operation
            worker_identity=$workerIdentity;code='worker_lifetime_unknown'}; @($historicalPrefix,$workerStarted,$unknown) }
        'lone-terminal' { @($historicalPrefix,$exitConfirmed) }
        'duplicated-terminal' { @($historicalPrefix,$exitConfirmed,$exitConfirmed) }
        'mismatched-identity' { $changed=Clone $exitConfirmed; $changed.worker_identity.pid=3457; @($historicalPrefix,$workerStarted,$changed) }
    }
    Write-HistoricalReceipt $entries
    $refused=$false
    try { Stop-QaNativeObserverWorkers -Run $historicalRun } catch { $refused=$true }
    Record-Case "pruned-stage-$case-never-confirmed" ($refused -and $processCalls -eq 0)
}
Write-HistoricalReceipt @($historicalPrefix,$workerStarted,$exitConfirmed)
$foreignStart=Clone $oldCurrent
$foreignStart.run_dir=$run
Write-Json (Join-Path $archive 'previous-start.json') $foreignStart
$refused=$false
try { $null = Get-QaNativeWorkerRecords -Run $historicalRun } catch { $refused=$_.Exception.Message -ceq 'native_worker_not_owned_by_run_start' }
Record-Case 'confirmed-pruned-stage-still-requires-own-archived-start' $refused
Write-Json (Join-Path $archive 'previous-start.json') $oldCurrent

# The production transition/publication is real; process and native file-ID proofs are controlled seams.
$owner = Join-Path $QaRunRoot '20261104-000002-000'
$viewer = Join-Path $QaRunRoot '20261104-000003-000'
New-Item -ItemType Directory -Path $owner,$viewer -Force | Out-Null
$pairPath = Join-Path $owner 'pair.json'
$viewerDid = 'did:key:z6Mk' + ('A'*44)
$ownerDid = 'did:key:z6Mk' + ('B'*44)
function Fixture-Issuer([string]$Subject) {
    $claims = @{sub=$Subject;token_type='api';max_role='owner';scope=('VERSE#'+('0'*26))} | ConvertTo-Json -Compress
    $encoded = [Convert]::ToBase64String([Text.Encoding]::UTF8.GetBytes($claims)).TrimEnd('=').Replace('+','-').Replace('/','_')
    return Get-QaPairIssuerSubject -Token ("fixture.$encoded.fixture") -VerseId ('0'*26)
}
$old = Clone $current
$old.run_dir = $viewer
$old | Add-Member -NotePropertyName api_base -NotePropertyValue 'http://127.0.0.1:8883'
$old | Add-Member -NotePropertyName native_pair -NotePropertyValue @{pair_path=$pairPath;member='viewer'}
$peer = Clone $old
$peer.run_dir=$owner; $peer.api_base='http://127.0.0.1:8882'; $peer.process_identity.pid=1233; $peer.native_pair.member='owner'
$peer.native_observer=$null
$contexts = @{}
$contexts[$owner] = @{directory=$owner;current=$peer;port=8882}
function Get-QaResumeContext { param($Run); return $contexts[$Run] }
function Get-QaOwnedProcess {
    param($Identity)
    $process = [pscustomobject]@{HasExited=$false;Identity=(Clone $Identity)}
    $process | Add-Member -MemberType ScriptMethod -Name Dispose -Value {}
    return $process
}
function Assert-QaFreshPairBootstrap { param($Current); if ($Current.native_observer -and $Current.native_observer.ledger_id -cne ('4'*32)) { throw 'controlled_fresh_file_refusal' } }
foreach ($mode in @('native','ordinary')) {
    $prior = Clone $old
    $prior.qa_options | Add-Member -Force -NotePropertyName native_observer -NotePropertyValue ($mode -eq 'native')
    if ($mode -eq 'ordinary') { $prior.native_observer=$null }
    $fresh = Clone $prior
    $fresh.process_identity.pid=2234; $fresh.process_identity.start_ticks='504911232223456789'
    if ($fresh.native_observer) {
        $fresh.native_observer.run_id='3'*32; $fresh.native_observer.ledger_id='4'*32
        $fresh.native_observer.ledger_file_id='d'*24
        $fresh.native_observer | Add-Member -Force -NotePropertyName observer_file_id -NotePropertyValue ('e'*24)
        $fresh.native_observer.ledger_path='new-ledger'
        $fresh.native_observer | Add-Member -Force -NotePropertyName observer_path -NotePropertyValue 'new-observer'
        $fresh.native_observer | Add-Member -Force -NotePropertyName process_identity -NotePropertyValue (Clone $fresh.process_identity)
    }
    foreach ($case in @('success','old-birth','changed-stage','changed-counterpart','changed-pair','reused-ledger','changed-issuer','counterpart-issuer')) {
        if ($mode -eq 'ordinary' -and $case -eq 'reused-ledger') { continue }
        $member = Get-QaPairMemberRecord $prior; $member.public_did=$viewerDid
        $peerMember = Get-QaPairMemberRecord $peer; $peerMember.public_did=$ownerDid
        $pair = [pscustomobject]@{schema='fe-qa-pair-v1';revision=1;owner=$peerMember;viewer=$member;distinct_run_local_identities=$true}
        Write-Json $pairPath $pair
        Write-Json (Join-Path $owner 'current.json') $peer
        $resume = @{directory=$viewer;current=$prior;port=8883}
        $transition = Open-QaPairRestart -Resume $resume -Stage $stage -Port 8883 -NativeObserver ($mode -eq 'native') -SyntheticInput $false
        $attempt = Join-Path $viewer "$mode-$case"
        New-Item -ItemType Directory -Path $attempt | Out-Null
        Save-QaPairRestartArchive $transition $attempt
        $candidate = Clone $fresh
        $issuer = Fixture-Issuer $viewerDid
        switch ($case) {
            'old-birth' { $candidate.process_identity.start_ticks=$prior.process_identity.start_ticks }
            'changed-stage' { $candidate.stage='foreign-stage' }
            'changed-counterpart' { [IO.File]::AppendAllText((Join-Path $owner 'current.json'), ' ') }
            'changed-pair' { [IO.File]::AppendAllText($pairPath, ' ') }
            'reused-ledger' { $candidate.native_observer.ledger_id=$prior.native_observer.ledger_id }
            'changed-issuer' { $issuer=Fixture-Issuer ('did:key:z6Mk'+('C'*44)) }
            'counterpart-issuer' { $issuer=Fixture-Issuer $ownerDid }
        }
        Write-Json (Join-Path $viewer 'current.json') $candidate
        $accepted=$true
        try { Complete-QaPairRestart $transition $candidate $issuer } catch { $accepted=$false }
        $published = Get-Content -LiteralPath $pairPath -Raw | ConvertFrom-Json
        $archived = (Get-FileHash -LiteralPath $transition.archive -Algorithm SHA256).Hash -ceq $transition.pair_sha256
        $passed = if ($case -eq 'success') {
            $accepted -and $archived -and $published.revision -eq 2 -and
                $published.viewer.process_identity.start_ticks -ceq $candidate.process_identity.start_ticks -and
                (ConvertTo-QaNativeCanonical $published.owner) -ceq (ConvertTo-QaNativeCanonical $pair.owner) -and
                (ConvertTo-QaNativeCanonical $published.viewer.native_observer) -ceq (ConvertTo-QaNativeCanonical $candidate.native_observer)
        } else { -not $accepted -and $archived -and $published.revision -eq 1 }
        Record-Case "$mode-reseal-$case" $passed
    }
    $sameIssuerPair = Clone $transition.pair
    $sameIssuerPair.viewer.public_did=$ownerDid
    $sameIssuerTransition = $transition.Clone()
    $sameIssuerTransition.pair=$sameIssuerPair
    $refused=$false
    try { $null = New-QaPairRestartRevision $sameIssuerTransition $fresh (Fixture-Issuer $ownerDid) }
    catch { $refused=$_.Exception.Message -ceq 'native_pair_issuer_changed' }
    Record-Case "$mode-preserved-subject-still-distinct" $refused
}
ConvertTo-Json -InputObject @($results.ToArray()) -Depth 8 -Compress
