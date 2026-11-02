# Own one bounded WEB/HTTP registry fixture; see tools/qa/AGENTS.md.
[CmdletBinding()]
param(
    [Parameter(Mandatory)][ValidateSet('Start','Stop')][string]$Action,
    [Parameter(Mandatory)][ValidateNotNullOrEmpty()][string]$Run,
    [ValidateRange(1024,65535)][int]$Port = 8890,
    [ValidateRange(30,3600)][int]$LifetimeSeconds = 900,
    [ValidateSet('normal','slow','corrupt')][string]$Mode = 'normal',
    [string]$PythonExe
)
. "$PSScriptRoot\_lib.ps1"
$ErrorActionPreference = 'Stop'
$lease = Enter-QaLifecycleLease
$server = $null
$serverIdentity = $null
$handedOff = $false
try {
    $context = Get-QaResumeContext -Run $Run
    $runDir = $context.directory
    $recordPath = Resolve-QaChildPath -Path (Join-Path $runDir 'controlled-fixtures.json') -Root $QaRunRoot
    if (Test-Path -LiteralPath $recordPath) {
        $prior = Get-Content -LiteralPath $recordPath -Raw | ConvertFrom-Json
        if ($prior.schema -cne 'fe-qa-controlled-fixtures-v1' -or $prior.run_dir -ne $runDir -or
            -not $prior.process_identity -or [string]$prior.process_identity.start_ticks -notmatch '^\d+$' -or
            [int]$prior.process_identity.pid -le 0) { throw 'Fixture ownership record is malformed' }
        $instanceDir = Resolve-QaChildPath -Path $prior.instance_dir -Root (Join-Path $runDir 'controlled-fixtures')
        $config = Get-Content -LiteralPath (Join-Path $instanceDir 'config.json') -Raw | ConvertFrom-Json
        if ($config.schema -cne $prior.schema -or $config.instance -cne $prior.instance -or
            $prior.python_exe -ne $prior.process_identity.exe_path -or $prior.origin -cne "http://127.0.0.1:$($config.port)") {
            throw 'Fixture ownership/configuration identity is ambiguous'
        }
        $owned = Get-QaOwnedProcess -Identity $prior.process_identity
        if ($Action -eq 'Start') {
            if ($owned) { $owned.Dispose(); throw 'An owned fixture is already running; stop it explicitly first' }
        } else {
            if ($owned) { $owned.Dispose() }
            Stop-QaOwnedProcessTree -Identity $prior.process_identity -RecordedDescendants @($prior.descendant_identities)
            Write-QaLog 'Exact recorded fixture process stopped or already absent; artifacts retained'
            return
        }
    } elseif ($Action -eq 'Stop') { throw 'No exact fixture ownership record exists; refusing PID/name cleanup' }

    if ($Port -eq $context.port) { throw 'Fixture port must differ from the app API port' }
    $app = Get-QaOwnedProcess -Identity $context.current.process_identity
    if (-not $app) { throw 'Start requires the exact owned app lifetime to be running' }
    $app.Dispose()
    if (-not $PythonExe) { $PythonExe = (Get-Command python.exe -CommandType Application -ErrorAction Stop).Source }
    if (-not [IO.Path]::IsPathRooted($PythonExe) -or -not (Test-Path -LiteralPath $PythonExe -PathType Leaf) -or
        [IO.Path]::GetFileName($PythonExe) -notmatch '^python(?:\d+(?:\.\d+)?)?\.exe$') { throw 'PythonExe must be an absolute native Python executable' }
    $PythonExe = (Get-Item -LiteralPath $PythonExe).FullName
    foreach ($path in @($PythonExe, $runDir, $PSScriptRoot)) {
        if ($path -match '["\r\n]') { throw 'Fixture process paths cannot contain quotes or line breaks' }
    }
    $stage = Resolve-QaChildPath -Path $context.current.stage -Root $QaStageRoot
    $stageManifest = Get-QaStageManifest -StageDir $stage
    $sealer = Join-Path $stage 'seal_hexon.exe'
    if (-not $stageManifest.seal_hexon_sha256 -or -not (Test-Path -LiteralPath $sealer) -or
        (Get-FileHash -LiteralPath $sealer -Algorithm SHA256).Hash -cne $stageManifest.seal_hexon_sha256) {
        throw 'Rebuild the immutable stage with the verified seal_hexon helper before starting fixtures'
    }
    $source = Resolve-QaChildPath -Path (Join-Path $runDir 'terrain-fixture\source.hexon') -Root $QaRunRoot
    $sourceInfo = Get-Item -LiteralPath $source -ErrorAction Stop
    if ($sourceInfo.Length -le 0 -or $sourceInfo.Length -gt 8MB) { throw 'Fixture requires the bounded generated QA TerrainTileset source' }
    $instance = [Guid]::NewGuid().ToString('N')
    $instanceDir = Resolve-QaChildPath -Path (Join-Path $runDir "controlled-fixtures\$instance") -Root $QaRunRoot
    New-Item -ItemType Directory -Path $instanceDir | Out-Null
    $archivePath = Join-Path $instanceDir 'registry.hexon'
    $seedBytes = New-Object byte[] 32
    $rng = [Security.Cryptography.RandomNumberGenerator]::Create()
    $sealProcess = New-Object Diagnostics.Process
    try {
        $rng.GetBytes($seedBytes)
        $sealProcess.StartInfo = New-Object Diagnostics.ProcessStartInfo
        $sealProcess.StartInfo.FileName = $sealer
        $sealProcess.StartInfo.Arguments = '"' + $source + '" "' + $archivePath + '"'
        $sealProcess.StartInfo.UseShellExecute = $false
        $sealProcess.StartInfo.CreateNoWindow = $true
        $sealProcess.StartInfo.RedirectStandardOutput = $true
        $sealProcess.StartInfo.RedirectStandardError = $true
        $sealProcess.StartInfo.EnvironmentVariables['FE_SECRET_HEXON_SEAL_KEY'] = ([BitConverter]::ToString($seedBytes)).Replace('-', '').ToLowerInvariant()
        if (-not $sealProcess.Start()) { throw 'Fixture sealer did not start' }
        $sealProcess.StartInfo.EnvironmentVariables.Remove('FE_SECRET_HEXON_SEAL_KEY')
        if (-not $sealProcess.WaitForExit(30000)) {
            $cleanup = @{ status = 'termination_requested'; process_identity = $null; identity_capture = 'not_attempted' }
            try {
                $cleanup.process_identity = Get-QaProcessIdentity -ProcessId $sealProcess.Id
                $cleanup.identity_capture = if ($cleanup.process_identity) { 'captured' } else { 'already_absent' }
            } catch { $cleanup.identity_capture = 'unavailable' }
            $failures = New-Object System.Collections.Generic.List[string]
            try { $sealProcess.Kill() }
            catch { $failures.Add('termination_' + $_.Exception.GetType().Name) }
            try {
                if (-not $sealProcess.WaitForExit(5000)) { $failures.Add('exit_deadline_exceeded') }
            } catch { $failures.Add('wait_' + $_.Exception.GetType().Name) }
            $cleanup.status = if ($failures.Count) { 'incomplete' } else { 'verified_exit' }
            $cleanup.failures = @($failures)
            $cleanup | ConvertTo-Json -Depth 5 | Set-Content -LiteralPath (Join-Path $instanceDir 'sealer-cleanup.json') -Encoding utf8
            if ($failures.Count) { throw ('Fixture sealer timeout cleanup incomplete: ' + ($failures -join ', ') + '; see the owned instance sealer-cleanup.json') }
            throw 'Fixture seal/RequireSealed verification exceeded 30 seconds'
        }
        if ($sealProcess.ExitCode -ne 0) { throw 'Fixture seal/RequireSealed verification failed; no provider published' }
    } finally {
        [Array]::Clear($seedBytes, 0, $seedBytes.Length)
        $rng.Dispose()
        $sealProcess.Dispose()
    }
    $archiveHash = (Get-FileHash -LiteralPath $archivePath -Algorithm SHA256).Hash.ToLowerInvariant()
    $configPath = Join-Path $instanceDir 'config.json'
    @{ schema = 'fe-qa-controlled-fixtures-v1'; instance = $instance; port = $Port; mode = $Mode;
        lifetime_seconds = $LifetimeSeconds; archive_path = $archivePath; archive_sha256 = $archiveHash } |
        ConvertTo-Json | Set-Content -LiteralPath $configPath -Encoding utf8
    $server = Start-Process -FilePath $PythonExe -ArgumentList @('-I', ('"' + (Join-Path $PSScriptRoot 'controlled_fixtures.py') + '"'), '--config', ('"' + $configPath + '"')) `
        -WorkingDirectory $instanceDir -WindowStyle Hidden -PassThru `
        -RedirectStandardOutput (Join-Path $instanceDir 'stdout.log') -RedirectStandardError (Join-Path $instanceDir 'stderr.log')
    $null = $server.Handle
    $serverIdentity = Get-QaProcessIdentity -ProcessId $server.Id
    if (-not $serverIdentity -or $serverIdentity.exe_path -ne $PythonExe) { throw 'Fixture process executable/lifetime could not be identified' }
    $readyPath = Join-Path $instanceDir 'ready.json'
    $deadline = [Diagnostics.Stopwatch]::StartNew()
    while (-not (Test-Path -LiteralPath $readyPath) -and $deadline.Elapsed.TotalSeconds -lt 10) {
        if ($server.HasExited) { throw 'Fixture server exited before readiness; inspect its confined stderr.log' }
        Start-Sleep -Milliseconds 100
    }
    if (-not (Test-Path -LiteralPath $readyPath)) { throw 'Fixture readiness file did not appear within ten seconds' }
    $ready = Get-Content -LiteralPath $readyPath -Raw | ConvertFrom-Json
    if ($ready.schema -cne 'fe-qa-controlled-fixtures-v1' -or $ready.instance -cne $instance -or
        $ready.pid -ne $server.Id -or $ready.origin -cne "http://127.0.0.1:$Port" -or $ready.archive_sha256 -cne $archiveHash) {
        throw 'Fixture readiness does not identify this owned launch'
    }
    $tree = @(Get-QaOwnedProcessTree -Identity $serverIdentity)
    if (-not ($tree | Where-Object { $_.pid -eq $serverIdentity.pid -and $_.start_ticks -eq $serverIdentity.start_ticks })) {
        throw 'Fixture process exited before ownership handoff'
    }
    $record = @{ schema = $ready.schema; instance = $instance; instance_dir = $instanceDir; run_dir = $runDir;
        python_exe = $PythonExe; process_identity = $serverIdentity; descendant_identities = @($tree | Where-Object { $_.pid -ne $serverIdentity.pid });
        origin = $ready.origin; web_url = "$($ready.origin)/web/one"; registry_uri = $ready.uri; mode = $Mode;
        archive_path = $archivePath; archive_sha256 = $archiveHash; lifetime_seconds = $LifetimeSeconds;
        prepared_with = @{ helper_sha256 = $stageManifest.seal_hexon_sha256; verification = 'RequireSealed with publisher pin'; stage = $stage };
        web_policy = 'Loopback remains refused by production Portal policy; requires separately reviewed exact-origin QA allowance';
        available_downloads = 'P2P advertisement fixture required; HTTP registry is a separate route' }
    $record | ConvertTo-Json -Depth 8 | Set-Content -LiteralPath $recordPath -Encoding utf8
    $handedOff = $true
    $record | ConvertTo-Json -Depth 8
} finally {
    try {
        if (-not $handedOff -and $serverIdentity) { Stop-QaOwnedProcessTree -Identity $serverIdentity }
        elseif (-not $handedOff -and $server) {
            if (-not $server.HasExited) { $server.Kill(); $null = $server.WaitForExit(5000) }
        }
    } finally {
        if ($server) { $server.Dispose() }
        $lease.Dispose()
    }
}
