# Chunked test-batch runner: one cargo command at a time, link outputs pruned after each chunk, disk and build-gate
# guards. Never `cargo test --workspace` on this box. Rationale and usage: tools/qa/AGENTS.md §test-batches.
[CmdletBinding()]
param(
    [string[]]$Batches = @(),      # batch ids from conductor/test-batches.md: R1..R11, Harness, Misc, Slow, or All
    [switch]$ListOnly,             # print the selected chunks and exit (runs no cargo)
    [switch]$KeepArtifacts,        # keep the test executables and PDBs a chunk linked (default: prune them)
    [int]$Jobs = 2,                # cargo -j for every chunk (6 cores shared with the other sessions)
    [int]$MinFreeGb = 60,          # refuse to start a chunk with less free disk than this on the target drive
    [string]$LogRoot = '',         # default: .omc/logs/tests/<yyyyMMdd-HHmmss>
    [string]$BatchFile = ''        # default: conductor/test-batches.md
)
$ErrorActionPreference = 'Continue'
$repoRoot = (Resolve-Path (Join-Path $PSScriptRoot '..\..')).Path
if (-not $BatchFile) { $BatchFile = Join-Path $repoRoot 'conductor\test-batches.md' }

# A section heading names a batch; every `cargo test` line inside its fenced blocks is one chunk.
function Get-BatchId([string]$heading) {
    if ($heading -match '^## (R\d+)\b') { return $Matches[1] }
    if ($heading -match '^## Harness') { return 'Harness' }
    if ($heading -match '^## Misc batch') { return 'Misc' }
    if ($heading -match '^## Slow sub-batches') { return 'Slow' }
    if ($heading -match '^## Full-suite') { return 'Full' }
    return $null
}
$chunks = New-Object System.Collections.Generic.List[object]
$currentId = $null; $inFence = $false; $index = 0
foreach ($line in Get-Content $BatchFile) {
    if ($line -match '^## ') { $currentId = Get-BatchId $line; $inFence = $false; $index = 0; continue }
    if ($line -match '^```') { $inFence = -not $inFence; continue }
    if ($inFence -and $currentId -and $line -match '^\s*cargo test\b') {
        $index++
        $chunks.Add([pscustomobject]@{ Batch = $currentId; Index = $index; Command = ($line -replace '\s+#.*$', '').Trim() })
    }
}
$known = @($chunks | Select-Object -ExpandProperty Batch -Unique)

$requested = @($Batches | ForEach-Object { $_ -split ',' } | ForEach-Object { $_.Trim() } | Where-Object { $_ })
if ($requested.Count -eq 0) {
    "Batches in $BatchFile`:"
    $known | ForEach-Object { $id = $_; "  {0,-8} {1,2} chunk(s)" -f $id, @($chunks | Where-Object Batch -eq $id).Count }
    ""
    "Usage: tools\qa\test_batches.ps1 -Batches R1,R8 [-ListOnly] [-KeepArtifacts] [-Jobs 2] [-MinFreeGb 60]"
    '       All = every batch except Full, one chunk at a time. Post "building <SESSION> tests <batches>" on the'
    '       session bus before running and close it after (one build at a time on this machine).'
    exit 0
}
$wanted = @()
foreach ($id in $requested) { if ($id -eq 'All') { $wanted += @($known | Where-Object { $_ -ne 'Full' }) } else { $wanted += $id } }
if ($wanted -contains 'Full') {
    Write-Error "Refusing 'Full': cargo test --workspace links every test binary at once (2026-11-08: 258 GB of link outputs filled the disk and the build died of commit exhaustion). Use -Batches All, which runs the same tests one chunk at a time."
    exit 2
}
$unknown = @($wanted | Where-Object { $known -notcontains $_ })
if ($unknown.Count) { Write-Error "Unknown batch id(s): $($unknown -join ', '). Known: $($known -join ', ')"; exit 2 }
$selected = @($chunks | Where-Object { $wanted -contains $_.Batch })

if ($ListOnly) { $selected | ForEach-Object { "{0,-8} #{1,-2} {2}" -f $_.Batch, $_.Index, $_.Command }; exit 0 }

# Guards: the session build gate, then the target drive's free space before every chunk.
$gatePath = Join-Path $repoRoot '.omc\state\build-gate.json'
if (Test-Path $gatePath) {
    $gate = Get-Content $gatePath -Raw | ConvertFrom-Json
    if (-not $gate.cargo_allowed) { Write-Error "Build gate closed ($gatePath): $($gate.reason)"; exit 3 }
}
Push-Location $repoRoot
$targetDir = (& cargo metadata --format-version 1 --no-deps | ConvertFrom-Json).target_directory
Pop-Location
if (-not $targetDir) { $targetDir = Join-Path $repoRoot 'target' }
$depsDir = Join-Path $targetDir 'debug\deps'
$drive = (Split-Path -Qualifier $targetDir).TrimEnd(':')
if (-not $LogRoot) { $LogRoot = Join-Path $repoRoot ('.omc\logs\tests\' + (Get-Date -Format 'yyyyMMdd-HHmmss')) }
New-Item -ItemType Directory -Force -Path $LogRoot | Out-Null
if (-not $env:RUST_MIN_STACK) { $env:RUST_MIN_STACK = '67108864' }   # surrealdb-core needs a deep type-checker stack

"target: $targetDir | log: $LogRoot | jobs: $Jobs | chunks: $($selected.Count) | prune: $(-not $KeepArtifacts)"
$results = @()
foreach ($chunk in $selected) {
    $freeGb = (Get-PSDrive $drive).Free / 1GB
    if ($freeGb -lt $MinFreeGb) {
        Write-Warning ("Stopping before {0} #{1}: {2:N1} GB free on {3}: is below the {4} GB floor." -f $chunk.Batch, $chunk.Index, $freeGb, $drive, $MinFreeGb)
        $results += [pscustomobject]@{ Batch = $chunk.Batch; Index = $chunk.Index; Status = 'skipped_disk'; Passed = 0; Failed = 0; Ignored = 0; Binaries = 0; Seconds = 0; PrunedGb = 0; Log = ''; FailedTests = '' }
        break
    }
    $started = Get-Date
    $log = Join-Path $LogRoot ("{0}-{1}.log" -f $chunk.Batch, $chunk.Index)
    $command = $chunk.Command -replace '^cargo test', "cargo test -j $Jobs"
    "[{0:HH:mm:ss}] {1} #{2}: {3}" -f $started, $chunk.Batch, $chunk.Index, $command
    Push-Location $repoRoot
    & cmd /c "$command > `"$log`" 2>&1"
    $exitCode = $LASTEXITCODE
    Pop-Location
    $text = if (Test-Path $log) { Get-Content $log -Raw } else { '' }
    $passed = 0; $failed = 0; $ignored = 0; $binaries = 0
    foreach ($m in [regex]::Matches($text, 'test result: (\w+)\. (\d+) passed; (\d+) failed; (\d+) ignored')) {
        $binaries++; $passed += [int]$m.Groups[2].Value; $failed += [int]$m.Groups[3].Value; $ignored += [int]$m.Groups[4].Value
    }
    $status = if ($exitCode -eq 0) { 'ok' } elseif ($binaries -eq 0) { 'build_failed' } else { 'FAILED' }
    $failedTests = @([regex]::Matches($text, '(?m)^test (\S+) \.\.\. FAILED\s*$') | ForEach-Object { $_.Groups[1].Value })
    $prunedGb = 0
    if (-not $KeepArtifacts -and (Test-Path $depsDir)) {
        $linked = @(Get-ChildItem (Join-Path $depsDir '*') -Include *.exe, *.pdb -File | Where-Object { $_.LastWriteTime -ge $started })
        $prunedGb = (($linked | Measure-Object Length -Sum).Sum) / 1GB
        $linked | Remove-Item -Force -ErrorAction SilentlyContinue
    }
    $seconds = [int]((Get-Date) - $started).TotalSeconds
    $results += [pscustomobject]@{ Batch = $chunk.Batch; Index = $chunk.Index; Status = $status; Passed = $passed; Failed = $failed; Ignored = $ignored; Binaries = $binaries; Seconds = $seconds; PrunedGb = [math]::Round($prunedGb, 1); Log = $log; FailedTests = ($failedTests -join ' ') }
    "    -> {0}: {1} passed, {2} failed, {3} ignored across {4} binaries; {5}s; pruned {6:N1} GB" -f $status, $passed, $failed, $ignored, $binaries, $seconds, $prunedGb
    if ($failedTests.Count) { $failedTests | ForEach-Object { "       FAILED $_" } }
}

""
"Summary ($LogRoot):"
$results | Format-Table Batch, Index, Status, Passed, Failed, Ignored, Binaries, Seconds, PrunedGb -AutoSize | Out-String -Width 200
$results | ConvertTo-Json -Depth 3 | Set-Content (Join-Path $LogRoot 'summary.json') -Encoding utf8
$bad = @($results | Where-Object { $_.Status -ne 'ok' })
if ($bad.Count) {
    "Not clean: " + (($bad | ForEach-Object { "$($_.Batch)#$($_.Index) [$($_.Status)] $($_.FailedTests)".Trim() }) -join '; ')
    exit 1
}
"All $($results.Count) chunk(s) ok."
exit 0
