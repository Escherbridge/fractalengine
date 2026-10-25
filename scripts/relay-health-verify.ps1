<#
.SYNOPSIS
    PAS-P1V: headless verification that `fe-relay` resolves, logs, and
    transitions relay health correctly under all three `FE_SYNC_RELAY` modes
    (campaign plan `p2p_asset_streaming_20260718`, S4 step 4).

.DESCRIPTION
    Boots `fe-relay.exe` three times, once per relay posture:
      - `default`                   -- iroh's n0-hosted public relays
      - `http://127.0.0.1:3340`     -- an operator-supplied Custom URL (no
                                      local iroh-relay 1.x container is
                                      assumed to be up for this run -- R6: the
                                      relay host is still undecided, so this
                                      run only proves mode logging + a
                                      Degraded/Unknown health transition, not
                                      a reachable custom relay)
      - `disabled`                  -- no relay, direct/LAN-only

    Each run gets a fresh `FE_P2P_DIR`/`FE_DB_PATH`, boots with
    `FE_SYNC_RELAY=disabled`'s sibling values substituted per mode, waits for
    `/ready` (bounded ~20s), captures stdout/stderr, and is torn down by its
    OWN PID before the next mode starts -- never a blind process-name kill.

    Asserts, per mode, from the captured log:
      1. the resolved relay config line names the right mode
         (`fe-sync/src/sync_thread.rs`'s "Resolved relay config for sync
         thread", `relay_config=<Mode>`);
      2. the one-time default-relay warning ("Using iroh's default public n0
         relay servers") appears ONLY for `default`;
      3. a relay-health line ("SyncStatus updated: relay health changed")
         appears for every mode (`fe-sync/src/status.rs::drain_sync_events`,
         already unconditional since S3 -- this run is what captures it for
         the record, not a new log line).

    Output: this script's own PASS/FAIL to the console, and every mode's
    captured log appended to `scripts/relay-health-verify.log` (committed --
    campaign S9 pastes its PAS-P1V run excerpt from here into evidence.md S6).
#>

$ErrorActionPreference = 'Continue'
$script:Failures = @()
$script:Passed = 0
$script:Checked = 0
$script:Relay = $null
$script:WorkDir = $null

function Write-Section($title) {
    Write-Host ""
    Write-Host "=== $title ===" -ForegroundColor Cyan
}

function Assert-True($condition, $message) {
    $script:Checked++
    if ($condition) {
        $script:Passed++
        $line = "  [PASS] $message"
        Write-Host $line -ForegroundColor Green
    } else {
        $script:Failures += $message
        $line = "  [FAIL] $message"
        Write-Host $line -ForegroundColor Red
    }
    if ($logPath) { Add-Content -Path $logPath -Value $line }
}

$RepoRoot = (Resolve-Path (Join-Path $PSScriptRoot '..')).Path
Set-Location $RepoRoot

Write-Section "Disk guard"
$drive = Get-PSDrive -Name C
$freeGb = [math]::Round($drive.Free / 1GB, 1)
Write-Host "  C: free = $freeGb GB"
if ($freeGb -lt 8) {
    Write-Host "STOP: less than 8 GB free on C:. Aborting before any build." -ForegroundColor Red
    exit 1
}

$stamp = Get-Date -Format 'yyyyMMdd-HHmmss'
$script:WorkDir = Join-Path $env:TEMP "fe-relay-health-verify-$stamp"
New-Item -ItemType Directory -Path $script:WorkDir -Force | Out-Null
$logPath = Join-Path $PSScriptRoot 'relay-health-verify.log'
Write-Host "  Work dir: $script:WorkDir"
Write-Host "  Log: $logPath"
"PAS-P1V relay-health-verify -- $(Get-Date -Format o)" | Set-Content -Path $logPath
function Log-Line($line) { $line | Add-Content -Path $logPath }

$env:RUST_MIN_STACK = '134217728'
if (-not $env:CARGO_BUILD_JOBS) { $env:CARGO_BUILD_JOBS = '2' }

function Start-RelayMode($relaySpec, $bindAddr, $dbPath, $p2pDir, $outLog, $errLog) {
    $psi = New-Object System.Diagnostics.ProcessStartInfo
    $psi.FileName = (Join-Path $RepoRoot 'target\debug\fe-relay.exe')
    $psi.WorkingDirectory = $RepoRoot
    $psi.UseShellExecute = $false
    $psi.RedirectStandardOutput = $true
    $psi.RedirectStandardError = $true
    $psi.EnvironmentVariables['FE_BIND_ADDR'] = $bindAddr
    $psi.EnvironmentVariables['FE_DB_PATH'] = $dbPath
    $psi.EnvironmentVariables['FE_P2P_DIR'] = $p2pDir
    $psi.EnvironmentVariables['FE_HEXON_DIR'] = (Join-Path $p2pDir 'hexons')
    $psi.EnvironmentVariables['FE_SYNC_RELAY'] = $relaySpec
    $psi.EnvironmentVariables['FE_SHUTDOWN_AFTER_SECS'] = '60'
    $psi.EnvironmentVariables['RUST_MIN_STACK'] = '134217728'
    $psi.EnvironmentVariables['RUST_LOG'] = 'info,fe_sync=debug'

    $proc = [System.Diagnostics.Process]::Start($psi)
    Write-Host "  Started fe-relay.exe PID=$($proc.Id) (FE_SYNC_RELAY=$relaySpec)"

    $outWriter = [System.IO.StreamWriter]::new($outLog, $false)
    $errWriter = [System.IO.StreamWriter]::new($errLog, $false)
    $outEvent = Register-ObjectEvent -InputObject $proc -EventName OutputDataReceived -Action {
        if ($EventArgs.Data -ne $null) { $Event.MessageData.WriteLine($EventArgs.Data); $Event.MessageData.Flush() }
    } -MessageData $outWriter
    $errEvent = Register-ObjectEvent -InputObject $proc -EventName ErrorDataReceived -Action {
        if ($EventArgs.Data -ne $null) { $Event.MessageData.WriteLine($EventArgs.Data); $Event.MessageData.Flush() }
    } -MessageData $errWriter
    $proc.BeginOutputReadLine()
    $proc.BeginErrorReadLine()

    $deadline = (Get-Date).AddSeconds(20)
    $ready = $false
    while ((Get-Date) -lt $deadline) {
        if ($proc.HasExited) {
            Start-Sleep -Milliseconds 300
            $errTail = (Get-Content $errLog -Tail 20 -ErrorAction SilentlyContinue) -join "`n"
            throw "fe-relay (FE_SYNC_RELAY=$relaySpec) exited early (code $($proc.ExitCode)) before /ready:`n$errTail"
        }
        try {
            $resp = Invoke-WebRequest -Uri "http://$bindAddr/ready" -UseBasicParsing -TimeoutSec 2
            if ($resp.StatusCode -eq 200) { $ready = $true; break }
        } catch { Start-Sleep -Milliseconds 400 }
    }
    if (-not $ready) { throw "relay (FE_SYNC_RELAY=$relaySpec) never became ready within 20s (log: $outLog / $errLog)" }

    # Wait (bounded) for the relay-health watcher's first transition to land in
    # the log before teardown. The sync thread emits it right after
    # `SyncEndpoint::node_addr`, which with a relay configured waits up to
    # iroh's NET_REPORT_TIMEOUT for a home relay (`endpoint.rs`
    # HOME_RELAY_WAIT) -- so an UNREACHABLE custom relay logs it only after
    # that bound, well past a fixed short grace (R5 sweep).
    $healthDeadline = (Get-Date).AddSeconds(20)
    while ((Get-Date) -lt $healthDeadline) {
        $seen = ((Get-Content $outLog -Raw -ErrorAction SilentlyContinue) + "`n" +
                 (Get-Content $errLog -Raw -ErrorAction SilentlyContinue))
        if ($seen -match 'SyncStatus updated: relay health changed') { break }
        Start-Sleep -Milliseconds 250
    }

    return @{
        Process = $proc; OutWriter = $outWriter; ErrWriter = $errWriter
        OutEvent = $outEvent; ErrEvent = $errEvent; OutLog = $outLog; ErrLog = $errLog
    }
}

function Stop-RelayMode($handle) {
    if (-not $handle) { return }
    if ($handle.OutEvent) { Unregister-Event -SourceIdentifier $handle.OutEvent.Name -ErrorAction SilentlyContinue }
    if ($handle.ErrEvent) { Unregister-Event -SourceIdentifier $handle.ErrEvent.Name -ErrorAction SilentlyContinue }
    if ($handle.Process -and -not $handle.Process.HasExited) {
        Write-Host "  Stopping fe-relay.exe PID=$($handle.Process.Id)"
        try { $handle.Process.Kill() } catch { Write-Host "  (already exited)" }
        $handle.Process.WaitForExit(5000) | Out-Null
    }
    if ($handle.OutWriter) { $handle.OutWriter.Close() }
    if ($handle.ErrWriter) { $handle.ErrWriter.Close() }
    Start-Sleep -Milliseconds 300
}

# Mode spec -> (FE_SYNC_RELAY value, expected `relay_config=` debug tag,
# expect the one-time default-relay warning).
$Modes = @(
    @{ Spec = 'default'; Tag = 'Default'; ExpectWarning = $true },
    @{ Spec = 'http://127.0.0.1:3340'; Tag = 'Custom'; ExpectWarning = $false },
    @{ Spec = 'disabled'; Tag = 'Disabled'; ExpectWarning = $false }
)

try {
    Write-Section "Build (incremental, strictly serial)"
    Write-Host "  cargo build -p fractalengine-relay ..."
    cargo build -q -p fractalengine-relay 2>&1 | ForEach-Object { Write-Host "    $_" }
    if ($LASTEXITCODE -ne 0) { throw "cargo build -p fractalengine-relay failed" }

    $BindAddr = '127.0.0.1:18777'
    $probe = New-Object System.Net.Sockets.TcpListener([System.Net.IPAddress]::Loopback, 18777)
    try { $probe.Start(); $probe.Stop() }
    catch { throw "port $BindAddr is already in use -- stop the other listener first" }

    foreach ($mode in $Modes) {
        Write-Section "FE_SYNC_RELAY=$($mode.Spec)"
        $modeDir = Join-Path $script:WorkDir ($mode.Tag)
        New-Item -ItemType Directory -Path $modeDir -Force | Out-Null
        $dbPath = Join-Path $modeDir 'db'
        $p2pDir = Join-Path $modeDir 'p2p'
        $outLog = Join-Path $modeDir 'relay.log'
        $errLog = Join-Path $modeDir 'relay-err.log'

        $script:Relay = Start-RelayMode $mode.Spec $BindAddr $dbPath $p2pDir $outLog $errLog
        Assert-True $true "relay (FE_SYNC_RELAY=$($mode.Spec)) reached /ready within 20s"

        Stop-RelayMode $script:Relay
        $script:Relay = $null

        $combined = ((Get-Content $outLog -Raw -ErrorAction SilentlyContinue) + "`n" +
                     (Get-Content $errLog -Raw -ErrorAction SilentlyContinue))

        Log-Line "=== FE_SYNC_RELAY=$($mode.Spec) ==="
        Log-Line $combined

        Assert-True (
            $combined -match 'Resolved relay config for sync thread' -and
            $combined -match "relay_config=$([regex]::Escape($mode.Tag))"
        ) "resolved relay config line names relay_config=$($mode.Tag)"

        $hasWarning = $combined -match "Using iroh's default public n0 relay servers"
        if ($mode.ExpectWarning) {
            Assert-True $hasWarning "default-relay rate-limit warning present (Default mode)"
        } else {
            Assert-True (-not $hasWarning) "default-relay rate-limit warning ABSENT ($($mode.Tag) mode)"
        }

        Assert-True (
            $combined -match 'SyncStatus updated: relay health changed'
        ) "a relay-health line was captured for $($mode.Tag) mode"
    }

    Write-Host ""
    Write-Host "Full combined log: $logPath"
}
catch {
    $script:Failures += "UNHANDLED EXCEPTION: $_"
    Write-Host "UNHANDLED EXCEPTION: $_" -ForegroundColor Red
    Write-Host $_.ScriptStackTrace -ForegroundColor Red
}
finally {
    Write-Section "Teardown"
    Stop-RelayMode $script:Relay

    if ($script:WorkDir -and (Test-Path $script:WorkDir)) {
        Write-Host "  Removing temp dir $script:WorkDir"
        Remove-Item -Recurse -Force $script:WorkDir -ErrorAction SilentlyContinue
    }

    Write-Host ""
    Write-Host "=== SUMMARY ===" -ForegroundColor Cyan
    Write-Host "Checks: $script:Passed / $script:Checked passed"
    if ($logPath) {
        Add-Content -Path $logPath -Value ""
        Add-Content -Path $logPath -Value "=== SUMMARY ==="
        Add-Content -Path $logPath -Value "Checks: $script:Passed / $script:Checked passed"
    }
    if ($script:Failures.Count -eq 0) {
        Write-Host "PASS: relay-health-verify (PAS-P1V) - all checks passed" -ForegroundColor Green
        if ($logPath) { Add-Content -Path $logPath -Value "PASS: relay-health-verify (PAS-P1V) - all checks passed" }
    } else {
        Write-Host "FAIL: relay-health-verify (PAS-P1V) - $($script:Failures.Count) check(s) failed:" -ForegroundColor Red
        $script:Failures | ForEach-Object { Write-Host "  - $_" -ForegroundColor Red }
        if ($logPath) {
            Add-Content -Path $logPath -Value "FAIL: relay-health-verify (PAS-P1V) - $($script:Failures.Count) check(s) failed:"
            $script:Failures | ForEach-Object { Add-Content -Path $logPath -Value "  - $_" }
        }
    }
    Write-Host "Full log: $logPath"
}

if ($script:Failures.Count -gt 0) { exit 1 } else { exit 0 }
