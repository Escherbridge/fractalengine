# Observer bootstrap and exact-owned helper cleanup; see native_observer/AGENTS.md.

function ConvertTo-QaNativeCanonical {
    param($Value)
    function Ordered-Value($Item) {
        if ($Item -is [pscustomobject] -or $Item -is [System.Collections.IDictionary]) {
            $ordered = [ordered]@{}
            $names = if ($Item -is [System.Collections.IDictionary]) { $Item.Keys } else { $Item.PSObject.Properties.Name }
            foreach ($name in @($names | Sort-Object -CaseSensitive)) { $ordered[$name] = Ordered-Value $Item.$name }
            return $ordered
        }
        if ($Item -is [array]) { return ,@($Item | ForEach-Object { Ordered-Value $_ }) }
        return $Item
    }
    return (ConvertTo-Json -InputObject (Ordered-Value $Value) -Depth 32 -Compress)
}

function Assert-QaNativeKeys {
    param($Value, [string[]]$Keys)
    if ($Value -isnot [pscustomobject] -or
        ((@($Value.PSObject.Properties.Name | Sort-Object -CaseSensitive)) -join '|') -cne (($Keys | Sort-Object -CaseSensitive) -join '|')) {
        throw 'native_record_fields_refused'
    }
}

function Assert-QaNativeProcessIdentity {
    param($Identity)
    Assert-QaNativeKeys $Identity @('pid','start_ticks','exe_path')
    if ($Identity.pid -isnot [int] -or $Identity.pid -le 0 -or $Identity.start_ticks -isnot [string] -or
        $Identity.start_ticks -cnotmatch '^[1-9][0-9]{0,18}$' -or $Identity.exe_path -isnot [string] -or
        -not [IO.Path]::IsPathRooted($Identity.exe_path)) { throw 'native_record_identity_refused' }
}

function Get-QaNativeHistoryLimits {
    $value = Get-Content -LiteralPath (Join-Path $PSScriptRoot 'native_observer\limits.json') -Raw | ConvertFrom-Json
    Assert-QaNativeKeys $value @('schema','jsonl_files','incarnation_directories','start_directories')
    if ($value.schema -cne 'fe-native-history-limits-v1') { throw 'native_history_limits_refused' }
    foreach ($key in @('jsonl_files','incarnation_directories','start_directories')) {
        if ($value.$key -isnot [int] -or $value.$key -le 0 -or $value.$key -gt 256) { throw 'native_history_limits_refused' }
    }
    return $value
}

function Assert-QaNativeHistoryCapacity {
    param($Counts, $Additions = @{}, $Limits = (Get-QaNativeHistoryLimits))
    foreach ($key in @('jsonl_files','incarnation_directories','start_directories')) {
        if ([int]$Counts.$key -lt 0 -or [int]$Additions.$key -lt 0 -or
            ([int]$Counts.$key + [int]$Additions.$key) -gt $Limits.$key) { throw 'native_history_capacity' }
    }
}

function Get-QaNativeHistoryInventory {
    param([Parameter(Mandatory)][string]$Run)
    $limits = Get-QaNativeHistoryLimits
    $counts = @{jsonl_files=0; incarnation_directories=0; start_directories=0}
    $files = New-Object System.Collections.Generic.List[object]
    $starts = New-Object System.Collections.Generic.List[object]
    foreach ($relative in @('native-observer','native-observer-observe','starts')) {
        $root = Resolve-QaChildPath -Path (Join-Path $Run $relative) -Root $QaRunRoot
        if (-not (Test-Path -LiteralPath $root)) { continue }
        foreach ($directory in Get-ChildItem -LiteralPath $root -Force) {
            $null = Resolve-QaChildPath -Path $directory.FullName -Root $QaRunRoot
            if (-not $directory.PSIsContainer) { throw 'native_history_layout_refused' }
            if ($relative -eq 'starts') { $counts.start_directories++; $starts.Add($directory) }
            else { $counts.incarnation_directories++ }
            Assert-QaNativeHistoryCapacity $counts @{} $limits
            if ($relative -eq 'starts') { continue }
            foreach ($child in Get-ChildItem -LiteralPath $directory.FullName -Force) {
                $null = Resolve-QaChildPath -Path $child.FullName -Root $QaRunRoot
                if ($child.PSIsContainer) {
                    if ($child.Name -cne 'workers') { throw 'native_history_layout_refused' }
                    foreach ($receipt in Get-ChildItem -LiteralPath $child.FullName -Force) {
                        $null = Resolve-QaChildPath -Path $receipt.FullName -Root $QaRunRoot
                        if ($receipt.PSIsContainer -or $receipt.Extension -cne '.jsonl') { throw 'native_history_layout_refused' }
                        $counts.jsonl_files++; $files.Add($receipt)
                        Assert-QaNativeHistoryCapacity $counts @{} $limits
                    }
                } else {
                    if ($child.Name -cnotin @('attempt.jsonl','observer.lock')) { throw 'native_history_layout_refused' }
                    if ($child.Name -ceq 'attempt.jsonl') { $counts.jsonl_files++ }
                    Assert-QaNativeHistoryCapacity $counts @{} $limits
                }
            }
        }
    }
    return @{counts=$counts; limits=$limits; receipts=$files.ToArray(); starts=$starts.ToArray()}
}

function Assert-QaNativeAllocation {
    param([Parameter(Mandatory)][string]$Run, [int]$Jsonl = 0, [int]$Incarnations = 0, [int]$Starts = 0)
    $inventory = Get-QaNativeHistoryInventory -Run $Run
    Assert-QaNativeHistoryCapacity $inventory.counts @{jsonl_files=$Jsonl;incarnation_directories=$Incarnations;start_directories=$Starts} $inventory.limits
}

function New-QaNativeObserverBootstrap {
    param([Parameter(Mandatory)][string]$Run, [Parameter(Mandatory)]$Identity,
          [Parameter(Mandatory)][string]$Stage, [Parameter(Mandatory)]$StageManifest)
    Assert-QaNativeAllocation -Run $Run -Jsonl 1 -Incarnations 1
    if (-not ('QaNativeFileIdentity' -as [type])) {
        Add-Type -Path (Join-Path $PSScriptRoot '_native_identity.cs') -ErrorAction Stop
    }
    $helper = $StageManifest.native_observer
    if ($helper.schema -cne 'fe-native-helper-stage-v1' -or $helper.helper_file -cne 'native_observer.exe' -or
        $helper.helper_sha256 -cnotmatch '^[a-f0-9]{64}$') { throw 'native_helper_stage_required' }
    $helperPath = Resolve-QaChildPath -Path (Join-Path $Stage 'native_observer.exe') -Root $QaStageRoot
    if ((Get-FileHash -LiteralPath $helperPath -Algorithm SHA256).Hash.ToLowerInvariant() -cne $helper.helper_sha256) {
        throw 'native_helper_hash_changed'
    }
    $born = ([long]$Identity.start_ticks - [long]504911232000000000).ToString()
    if ($born -cnotmatch '^[1-9][0-9]{0,19}$') { throw 'native_process_time_refused' }
    $runId = [Guid]::NewGuid().ToString('N')
    $ledgerId = [Guid]::NewGuid().ToString('N')
    $native = Resolve-QaChildPath -Path (Join-Path $Run "native-observer\$born-$runId") -Root $QaRunRoot
    if (Test-Path -LiteralPath $native) { throw 'native_bootstrap_already_exists' }
    New-Item -ItemType Directory -Path (Join-Path $native 'workers') | Out-Null
    $ledgerPath = Resolve-QaChildPath -Path (Join-Path $native 'attempt.jsonl') -Root $QaRunRoot
    $observerPath = Resolve-QaChildPath -Path (Join-Path $native 'observer.lock') -Root $QaRunRoot
    $record = [ordered]@{ birth_filetime = $born; exe_sha256 = $StageManifest.exe_sha256.ToLowerInvariant()
        helper_sha256 = $helper.helper_sha256; kind = 'bootstrap'; ledger_id = $ledgerId
        pid = [int]$Identity.pid; run_id = $runId; schema = 'fe-native-attempt-v1' }
    $utf8 = New-Object Text.UTF8Encoding($false, $true)
    $bytes = $utf8.GetBytes(($record | ConvertTo-Json -Compress) + "`n")
    $ledger = $null; $observer = $null
    try {
        $ledger = [IO.File]::Open($ledgerPath, [IO.FileMode]::CreateNew, [IO.FileAccess]::ReadWrite, [IO.FileShare]::None)
        $ledger.Write($bytes, 0, $bytes.Length)
        $ledger.Flush($true)
        $ledger.Position = 0
        $copy = New-Object byte[] $bytes.Length
        if ($ledger.Read($copy, 0, $copy.Length) -ne $bytes.Length -or
            [Convert]::ToBase64String($copy) -cne [Convert]::ToBase64String($bytes)) { throw 'native_bootstrap_unconfirmed' }
        $ledgerFileId = [QaNativeFileIdentity]::Read($ledger, $ledgerPath)
        $observer = [IO.File]::Open($observerPath, [IO.FileMode]::CreateNew, [IO.FileAccess]::ReadWrite, [IO.FileShare]::None)
        $observer.Flush($true)
        $observerFileId = [QaNativeFileIdentity]::Read($observer, $observerPath)
        return @{ schema = 'fe-qa-native-bootstrap-v1'; run_id = $runId; ledger_id = $ledgerId
            ledger_path = $ledgerPath; ledger_file_id = $ledgerFileId; observer_path = $observerPath
            observer_file_id = $observerFileId; process_identity = $Identity; helper_sha256 = $helper.helper_sha256 }
    } finally {
        if ($observer) { $observer.Dispose() }
        if ($ledger) { $ledger.Dispose() }
    }
}

function Get-QaNativeWorkerRecords {
    param([Parameter(Mandatory)][string]$Run)
    $inventory = Get-QaNativeHistoryInventory -Run $Run
    $knownStarts = New-Object System.Collections.Generic.List[object]
    $currentPath = Resolve-QaChildPath -Path (Join-Path $Run 'current.json') -Root $QaRunRoot
    $startPaths = New-Object System.Collections.Generic.List[string]
    $startPaths.Add($currentPath)
    $history = Resolve-QaChildPath -Path (Join-Path $Run 'starts') -Root $QaRunRoot
    if (Test-Path -LiteralPath $history) {
        $directories = @($inventory.starts)
        foreach ($directory in $directories) {
            $path = Resolve-QaChildPath -Path (Join-Path $directory.FullName 'previous-start.json') -Root $QaRunRoot
            if (Test-Path -LiteralPath $path) { $startPaths.Add($path) }
        }
    }
    foreach ($path in $startPaths) {
        if (-not (Test-Path -LiteralPath $path)) { continue }
        $file = Get-Item -LiteralPath $path
        if ($file.Length -le 0 -or $file.Length -gt 65536) { throw 'native_start_provenance_refused' }
        $known = [IO.File]::ReadAllText($path) | ConvertFrom-Json
        if (-not [string]::Equals([string]$known.run_dir, $Run, [StringComparison]::OrdinalIgnoreCase)) { continue }
        if ($known.qa_options.synthetic_input -isnot [bool] -or $known.qa_options.synthetic_input) { continue }
        if ($known.process_identity -and $known.stage_manifest) { $knownStarts.Add($known) }
    }
    $records = New-Object System.Collections.Generic.List[object]
    foreach ($relative in @('native-observer', 'native-observer-observe')) {
        $root = Resolve-QaChildPath -Path (Join-Path $Run $relative) -Root $QaRunRoot
        if (-not (Test-Path -LiteralPath $root)) { continue }
        $boundary = $root.TrimEnd('\') + '\'
        $files = @($inventory.receipts | Where-Object { $_.FullName.StartsWith($boundary, [StringComparison]::OrdinalIgnoreCase) })
        foreach ($file in $files) {
            if ($file.Name -eq 'attempt.jsonl') { continue }
            $path = Resolve-QaChildPath -Path $file.FullName -Root $QaRunRoot
            if ($file.Length -le 0 -or $file.Length -gt 65536 -or $file.BaseName -cnotmatch '^[a-f0-9]{32}$') {
                throw 'native_worker_record_refused'
            }
            $raw = [IO.File]::ReadAllText($path)
            if (-not $raw.EndsWith("`n")) { throw 'native_worker_record_partial' }
            $lines = @($raw.TrimEnd("`n").Split("`n"))
            if ($lines.Count -lt 1 -or $lines.Count -gt 3) { throw 'native_worker_record_refused' }
            $values = @($lines | ForEach-Object { $_ | ConvertFrom-Json })
            for ($index = 0; $index -lt $values.Count; $index++) {
                if ((ConvertTo-QaNativeCanonical $values[$index]) -cne $lines[$index]) { throw 'native_worker_record_noncanonical' }
            }
            $start = $values[0]
            Assert-QaNativeKeys $start @('schema','phase','operation','controller_identity','app_identity','helper_path','helper_sha256','run_id','ledger_id','ledger_file_id','mode')
            foreach ($identity in @($start.controller_identity,$start.app_identity)) {
                Assert-QaNativeProcessIdentity $identity
            }
            if ($start.schema -cne 'fe-native-worker-owner-v1' -or $start.phase -cne 'reserved' -or
                $start.operation -cne $file.BaseName -or $start.helper_sha256 -cnotmatch '^[a-f0-9]{64}$' -or
                $start.run_id -cnotmatch '^[a-f0-9]{32}$' -or $start.mode -notin @('observe','invoke_visible_petal_v1')) {
                throw 'native_worker_record_refused'
            }
            $helper = Resolve-QaChildPath -Path $start.helper_path -Root $QaStageRoot
            if ([IO.Path]::GetFileName($helper) -cne 'native_observer.exe') { throw 'native_worker_image_refused' }
            $app = $start.app_identity
            if ($app.pid -isnot [int] -or $app.pid -le 0 -or $app.start_ticks -cnotmatch '^[1-9][0-9]{0,18}$' -or
                -not [string]::Equals($app.exe_path, (Join-Path ([IO.Path]::GetDirectoryName($helper)) 'fractalengine.exe'), [StringComparison]::OrdinalIgnoreCase)) { throw 'native_worker_app_binding_refused' }
            $born = ([long]$app.start_ticks - [long]504911232000000000).ToString()
            $expectedParent = Join-Path (Join-Path $root "$born-$($start.run_id)") 'workers'
            if (-not [string]::Equals([IO.Path]::GetDirectoryName($path), $expectedParent, [StringComparison]::OrdinalIgnoreCase)) { throw 'native_worker_run_binding_refused' }
            $matchingStarts = @($knownStarts | Where-Object {
                $_.process_identity.pid -eq $app.pid -and $_.process_identity.start_ticks -ceq $app.start_ticks -and
                [string]::Equals([string]$_.process_identity.exe_path, [string]$app.exe_path, [StringComparison]::OrdinalIgnoreCase) -and
                [string]::Equals([string]$_.stage, [IO.Path]::GetDirectoryName($helper), [StringComparison]::OrdinalIgnoreCase) -and
                $_.stage_manifest.kind -ceq 'fractalengine-qa-stage-v1' -and
                $_.stage_manifest.exe_sha256 -is [string] -and
                $_.stage_manifest.exe_sha256 -cmatch '^[a-fA-F0-9]{64}$' -and
                $_.stage_manifest.native_observer.schema -ceq 'fe-native-helper-stage-v1' -and
                $_.stage_manifest.native_observer.helper_file -ceq 'native_observer.exe' -and
                $_.stage_manifest.native_observer.helper_sha256 -ceq $start.helper_sha256
            })
            if (-not $matchingStarts.Count) { throw 'native_worker_not_owned_by_run_start' }
            if ($relative -eq 'native-observer') {
                $ledgerPath = Resolve-QaChildPath -Path (Join-Path ([IO.Path]::GetDirectoryName($expectedParent)) 'attempt.jsonl') -Root $QaRunRoot
                $owned = @($matchingStarts | Where-Object {
                    $_.native_observer.schema -ceq 'fe-qa-native-bootstrap-v1' -and
                    $_.native_observer.run_id -ceq $start.run_id -and $_.native_observer.ledger_id -ceq $start.ledger_id -and
                    $_.native_observer.ledger_file_id -ceq $start.ledger_file_id -and
                    $_.native_observer.helper_sha256 -ceq $start.helper_sha256 -and
                    [string]::Equals([string]$_.native_observer.ledger_path, $ledgerPath, [StringComparison]::OrdinalIgnoreCase)
                })
                if (-not $owned.Count -or $start.ledger_id -cnotmatch '^[a-f0-9]{32}$' -or
                    $start.ledger_file_id -cnotmatch '^[a-f0-9]{24}$') { throw 'native_worker_bootstrap_refused' }
            } else {
                if ($start.ledger_id -cne '' -or $start.ledger_file_id -cne '' -or
                    -not @($matchingStarts | Where-Object { -not $_.native_observer }).Count) { throw 'native_observe_start_binding_refused' }
                $identityJson = [ordered]@{exe_path = $app.exe_path; pid = [int]$app.pid; start_ticks = $app.start_ticks} | ConvertTo-Json -Compress
                $hash = [Security.Cryptography.SHA256]::Create()
                try { $expectedRun = [BitConverter]::ToString($hash.ComputeHash([Text.Encoding]::UTF8.GetBytes($identityJson))).Replace('-', '').ToLowerInvariant().Substring(0,32) }
                finally { $hash.Dispose() }
                if ($start.run_id -cne $expectedRun -or $start.mode -cne 'observe') { throw 'native_worker_observe_binding_refused' }
            }
            foreach ($value in $values) {
                if ($value.schema -cne $start.schema -or $value.operation -cne $start.operation) { throw 'native_worker_record_mismatch' }
                switch -CaseSensitive ($value.phase) {
                    'reserved' { Assert-QaNativeKeys $value @('schema','phase','operation','controller_identity','app_identity','helper_path','helper_sha256','run_id','ledger_id','ledger_file_id','mode') }
                    'worker_started' { Assert-QaNativeKeys $value @('schema','phase','operation','worker_identity') }
                    'exit_confirmed' { Assert-QaNativeKeys $value @('schema','phase','operation','worker_identity','exit_code','code') }
                    'lifetime_unknown' { Assert-QaNativeKeys $value @('schema','phase','operation','worker_identity','code') }
                    default { throw 'native_worker_phase_refused' }
                }
                if ($value.phase -cne 'reserved') {
                    if ($value.worker_identity) {
                        Assert-QaNativeProcessIdentity $value.worker_identity
                        if (-not [string]::Equals($value.worker_identity.exe_path, $helper, [StringComparison]::OrdinalIgnoreCase) -or
                            [long]$value.worker_identity.start_ticks -lt [long]$start.controller_identity.start_ticks -or
                            [long]$value.worker_identity.start_ticks -lt [long]$app.start_ticks) { throw 'native_worker_identity_refused' }
                    } elseif ($value.phase -ceq 'worker_started') { throw 'native_worker_identity_refused' }
                }
                if ($value.phase -cin @('exit_confirmed','lifetime_unknown') -and
                    ($value.code -isnot [string] -or $value.code -cnotmatch '^[a-z][a-z0-9_]{0,63}$')) { throw 'native_worker_terminal_refused' }
                if ($value.phase -ceq 'exit_confirmed' -and
                    (($value.exit_code -isnot [int] -and $value.exit_code -isnot [long]) -or
                     $value.exit_code -lt -2147483648 -or $value.exit_code -gt 4294967295)) { throw 'native_worker_terminal_refused' }
            }
            if (($values.Count -eq 2 -and $values[1].phase -cnotin @('worker_started','exit_confirmed','lifetime_unknown')) -or
                ($values.Count -eq 3 -and ($values[1].phase -cne 'worker_started' -or
                    $values[2].phase -cnotin @('exit_confirmed','lifetime_unknown')))) { throw 'native_worker_phase_order_refused' }
            $started = if ($values.Count -ge 2 -and $values[1].phase -ceq 'worker_started') { $values[1].worker_identity } else { $null }
            $last = $values[-1]
            if ($values.Count -eq 3 -and $last.worker_identity -and
                (ConvertTo-QaNativeCanonical $last.worker_identity) -cne (ConvertTo-QaNativeCanonical $started)) { throw 'native_worker_record_mismatch' }
            $confirmed = $values.Count -eq 3 -and $last.phase -ceq 'exit_confirmed' -and $last.worker_identity -and $started
            if (-not $confirmed) {
                $stage = Get-QaStageManifest -StageDir ([IO.Path]::GetDirectoryName($helper))
                if ($stage.native_observer.helper_sha256 -cne $start.helper_sha256 -or
                    -not @($matchingStarts | Where-Object {
                        (ConvertTo-QaNativeCanonical $_.stage_manifest) -ceq (ConvertTo-QaNativeCanonical $stage)
                    }).Count) { throw 'native_worker_stage_binding_refused' }
            }
            $records.Add(@{ path = $path; start = $start; worker = $started; confirmed = [bool]$confirmed; helper = $helper })
        }
    }
    return $records.ToArray()
}

function Stop-QaNativeObserverWorkers {
    param([Parameter(Mandatory)][string]$Run)
    foreach ($record in @(Get-QaNativeWorkerRecords -Run $Run)) {
        if ($record.confirmed) { continue }
        if (-not $record.worker) {
            Write-Warning 'Native helper lifetime remains unknown; the attempt is retained and cannot be replayed.'
            continue
        }
        $controller = Get-QaOwnedProcess -Identity $record.start.controller_identity
        if ($controller) { $controller.Dispose(); throw 'native_controller_still_owned' }
        $helperPin = [IO.File]::Open($record.helper, [IO.FileMode]::Open, [IO.FileAccess]::Read, [IO.FileShare]::Read)
        try {
            if ((Get-FileHash -LiteralPath $record.helper -Algorithm SHA256).Hash.ToLowerInvariant() -cne $record.start.helper_sha256) {
                throw 'native_worker_hash_changed'
            }
            $worker = Get-QaOwnedProcess -Identity $record.worker
            if ($worker) {
                try {
                    $worker.Kill()
                    if (-not $worker.WaitForExit(5000)) { throw 'native_worker_exit_unconfirmed' }
                } finally { $worker.Dispose() }
            }
        } finally { $helperPin.Dispose() }
    }
}

function Get-QaNativeRetainedStages {
    param([Parameter(Mandatory)][string]$Run)
    foreach ($record in @(Get-QaNativeWorkerRecords -Run $Run)) {
        if (-not $record.confirmed) { [IO.Path]::GetDirectoryName($record.helper) }
    }
}
