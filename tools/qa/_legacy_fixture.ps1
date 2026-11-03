# Explicit disposable legacy setup and stopped-store reads; see AGENTS.md.

function Invoke-QaLegacyHelper {
    param([Parameter(Mandatory)]$Request, [Parameter(Mandatory)][string]$Stage,
          [Parameter(Mandatory)][ValidateSet('generate','seal','discovery','baseline','adopted','resume')][string]$Mode)
    $helper = Resolve-QaChildPath -Path (Join-Path $Stage 'legacy_adoption_fixture.exe') -Root $QaStageRoot
    $pin = $null; $process = $null
    try {
        $pin = [IO.File]::Open($helper, [IO.FileMode]::Open, [IO.FileAccess]::Read, [IO.FileShare]::Read)
        if ((Get-FileHash -LiteralPath $helper -Algorithm SHA256).Hash -cne $Request.legacy_fixture_sha256) { throw 'helper_mismatch' }
        $json = $Request | ConvertTo-Json -Depth 16 -Compress
        if ([Text.Encoding]::UTF8.GetByteCount($json) -gt 65536) { throw 'request_overflow' }
        $start = New-Object Diagnostics.ProcessStartInfo
        $start.FileName = $helper; $start.Arguments = $Mode
        $start.WorkingDirectory = $QaRepoRoot; $start.UseShellExecute = $false
        $start.CreateNoWindow = $true
        $start.RedirectStandardInput = $true; $start.RedirectStandardOutput = $true; $start.RedirectStandardError = $true
        $process = New-Object Diagnostics.Process; $process.StartInfo = $start
        if (-not $process.Start()) { throw 'helper_start_failed' }
        $output = $process.StandardOutput.ReadToEndAsync(); $errors = $process.StandardError.ReadToEndAsync()
        $process.StandardInput.Write($json); $process.StandardInput.Close()
        if (-not $process.WaitForExit(35000)) { $process.Kill(); $null = $process.WaitForExit(5000); throw 'helper_deadline' }
        $text = $output.GetAwaiter().GetResult(); $stderr = $errors.GetAwaiter().GetResult()
        if ([Text.Encoding]::UTF8.GetByteCount($text) -gt 4194304 -or $stderr.Length -ne 0) { throw 'helper_output_refused' }
        $result = $text | ConvertFrom-Json
        if ($process.ExitCode -ne 0 -or $result.ok -ne $true -or $null -eq $result.data) {
            $code = if ([string]$result.code -cmatch '^[a-z_]{1,80}$') { [string]$result.code } else { 'helper_refused' }
            throw $code
        }
        return $result.data
    } catch {
        # Only static helper codes cross this boundary; no native/DB exception text.
        $message = [string]$_.Exception.Message
        if ($message -cnotmatch '^[a-z_]{1,80}$') { $message = 'legacy_fixture_helper_unavailable' }
        throw $message
    } finally { if ($process) { $process.Dispose() }; if ($pin) { $pin.Dispose() } }
}

function New-QaLegacyRequest {
    param([Parameter(Mandatory)][string]$Run, [Parameter(Mandatory)][string]$Stage,
          [Parameter(Mandatory)][string]$PreparationId, [Parameter(Mandatory)]$Lease)
    $manifest = Get-QaStageManifest -StageDir $Stage
    if ([string]$manifest.legacy_fixture_sha256 -cnotmatch '^[0-9A-Fa-f]{64}$') { throw 'Legacy fixture helper is not sealed in this stage' }
    return @{
        run_dir = $Run; stage_dir = $Stage; preparation_id = $PreparationId
        source_commit = $manifest.source_commit; source_fingerprint = $manifest.source_fingerprint
        exe_sha256 = $manifest.exe_sha256; legacy_fixture_sha256 = $manifest.legacy_fixture_sha256
        lease = @{ process_id = $PID; handle = $Lease.SafeFileHandle.DangerousGetHandle().ToInt64() }
    }
}

function New-QaLegacyPreparation {
    param([Parameter(Mandatory)][string]$Run, [Parameter(Mandatory)][string]$Stage, [Parameter(Mandatory)]$Lease)
    if (Test-Path -LiteralPath $Run) { throw 'Legacy preparation requires a new run directory' }
    $request = New-QaLegacyRequest -Run $Run -Stage $Stage -PreparationId ([Guid]::NewGuid().ToString('N')) -Lease $Lease
    $null = Invoke-QaLegacyHelper -Request $request -Stage $Stage -Mode generate
    $source = Resolve-QaChildPath -Path (Join-Path $Run 'assets/models/qa_legacy_asymmetric_v1.glb') -Root $QaRunRoot
    $pin = [IO.File]::Open($source, [IO.FileMode]::Open, [IO.FileAccess]::Read, [IO.FileShare]::Read)
    try {
        $request.input_sha256 = (Get-FileHash -LiteralPath $source -Algorithm SHA256).Hash
        return Invoke-QaLegacyHelper -Request $request -Stage $Stage -Mode seal
    } finally { $pin.Dispose() }
}

function Get-QaLegacyRuntimeDid {
    param([Parameter(Mandatory)][string]$Token, [Parameter(Mandatory)][string]$VerseId)
    try {
        if ($Token.Length -gt 16384) { throw 'invalid_runtime_identity' }
        $parts = $Token.Split('.')
        if ($parts.Count -ne 3) { throw 'invalid_runtime_identity' }
        $payload = $parts[1].Replace('-', '+').Replace('_', '/')
        $payload += '=' * ((4 - $payload.Length % 4) % 4)
        $claims = [Text.Encoding]::UTF8.GetString([Convert]::FromBase64String($payload)) | ConvertFrom-Json
        if ($claims.token_type -cne 'api' -or $claims.max_role -cne 'owner' -or $claims.scope -cne "VERSE#$VerseId") { throw 'invalid_runtime_identity' }
        if ($claims.sub -isnot [string] -or $claims.sub -cnotmatch '^did:key:z[1-9A-HJ-NP-Za-km-z]{20,120}$') { throw 'invalid_runtime_identity' }
        return [string]$claims.sub
    } catch { throw 'Legacy fixture runtime identity could not be recorded' }
}

function Read-QaLegacyFixture {
    param([Parameter(Mandatory)]$Context, [Parameter(Mandatory)][string]$Stage,
          [Parameter(Mandatory)][ValidateSet('discovery','baseline','adopted','resume')][string]$Phase,
          [Parameter(Mandatory)]$Lease)
    $run = $Context.directory; $current = $Context.current
    if ($current.fixture -cne 'legacy-adoption' -or $current.fixture_detail.preparation_id -cnotmatch '^[0-9a-f]{32}$') { throw 'Not an explicitly prepared legacy fixture' }
    $request = New-QaLegacyRequest -Run $run -Stage $Stage -PreparationId $current.fixture_detail.preparation_id -Lease $Lease
    $preparationPath = Resolve-QaChildPath -Path (Join-Path $run 'legacy-fixture.json') -Root $QaRunRoot
    $preparationFile = Get-Item -LiteralPath $preparationPath -Force -ErrorAction Stop
    if ($preparationFile.PSIsContainer -or $preparationFile.Length -le 0 -or $preparationFile.Length -gt 65536) { throw 'legacy_preparation_size_refused' }
    $source = Resolve-QaChildPath -Path (Join-Path $run 'assets/models/qa_legacy_asymmetric_v1.glb') -Root $QaRunRoot
    $sourcePin = $null; $currentPin = $null; $preparationPin = $null
    try {
        $preparationPin = [IO.File]::Open($preparationPath, [IO.FileMode]::Open, [IO.FileAccess]::Read, [IO.FileShare]::Read)
        $preparation = Get-Content -LiteralPath $preparationPath -Raw | ConvertFrom-Json
        $sourcePin = [IO.File]::Open($source, [IO.FileMode]::Open, [IO.FileAccess]::Read, [IO.FileShare]::Read)
        $currentPin = [IO.File]::Open((Join-Path $run 'current.json'), [IO.FileMode]::Open, [IO.FileAccess]::Read, [IO.FileShare]::Read)
        if ((Get-FileHash -LiteralPath $source -Algorithm SHA256).Hash -cne $preparation.input_sha256) { throw 'Legacy fixture input bytes changed' }
        foreach ($identity in (@($current.process_identity) + @($current.descendant_identities))) {
            $owned = Get-QaOwnedProcess -Identity $identity
            if ($owned) { $owned.Dispose(); throw 'Legacy inspection requires the recorded lifetime to be stopped' }
        }
        foreach ($key in @('process_identity','descendant_identities','started_at','bootstrap_verse_id')) { $request[$key] = $current.$key }
        return Invoke-QaLegacyHelper -Request $request -Stage $Stage -Mode $Phase
    } finally { if ($currentPin) { $currentPin.Dispose() }; if ($sourcePin) { $sourcePin.Dispose() }; if ($preparationPin) { $preparationPin.Dispose() } }
}
