# Shared helpers for tools/qa/*.ps1. Dot-source this file: `. "$PSScriptRoot\_lib.ps1"`.
# See tools/qa/AGENTS.md for the design rationale (stage/launch/stop/clean split,
# the CWD-isolation gotcha, the Genesis-verse bootstrap token).

$QaRepoRoot = (Resolve-Path (Join-Path $PSScriptRoot "..\..")).Path
$QaStageRoot = Join-Path $QaRepoRoot ".qa\stage"
$QaRunRoot = Join-Path $QaRepoRoot ".qa\run"
$QaApiBase = "http://127.0.0.1:8765"

function Write-QaLog {
    param([string]$Message)
    $ts = (Get-Date).ToString("HH:mm:ss.fff")
    Write-Output "[$ts] $Message"
}

# Polls a URL (GET) until it returns any HTTP response (2xx-5xx all count as
# "the server answered") or the timeout elapses. Returns $true/$false.
function Wait-QaHttp {
    param(
        [Parameter(Mandatory)] [string]$Url,
        [int]$TimeoutSec = 120,
        [int]$PollMs = 400
    )
    $deadline = (Get-Date).AddSeconds($TimeoutSec)
    while ((Get-Date) -lt $deadline) {
        try {
            $r = Invoke-WebRequest -Uri $Url -UseBasicParsing -TimeoutSec 3
            if ($r.StatusCode -ge 200) { return $true }
        } catch {
            # Connection refused / reset while the listener isn't up yet — keep polling.
        }
        Start-Sleep -Milliseconds $PollMs
    }
    return $false
}

# Polls tokens.json (fe-api/AGENTS.md §qa-dev-token) until it holds an
# "owner"-role entry for ANY verse (the bootstrap / Genesis verse on a fresh
# DB) or, with -VerseId, for that specific verse. Returns the token string,
# or $null on timeout.
function Wait-QaOwnerToken {
    param(
        [Parameter(Mandatory)] [string]$TokensPath,
        [string]$VerseId,
        [int]$TimeoutSec = 60,
        [int]$PollMs = 400
    )
    $deadline = (Get-Date).AddSeconds($TimeoutSec)
    while ((Get-Date) -lt $deadline) {
        if (Test-Path $TokensPath) {
            try {
                $j = Get-Content $TokensPath -Raw | ConvertFrom-Json
                if ($VerseId) {
                    if ($j.roles.$VerseId -eq "owner") {
                        return @{ VerseId = $VerseId; Token = $j.tokens.$VerseId }
                    }
                } else {
                    $owner = $j.roles.PSObject.Properties | Where-Object { $_.Value -eq "owner" } | Select-Object -First 1
                    if ($owner) {
                        return @{ VerseId = $owner.Name; Token = $j.tokens.($owner.Name) }
                    }
                }
            } catch {
                # tokens.json mid-write (the GUI replaces it atomically, but a
                # read can still race a rename on some filesystems) — retry.
            }
        }
        Start-Sleep -Milliseconds $PollMs
    }
    return $null
}

# curl.exe (the real Windows 10+ binary, NOT the Invoke-WebRequest alias)
# against the local API. Returns the parsed JSON body (ApiResponse<T> shape:
# {ok, data, error}).
function Invoke-QaApi {
    param(
        [Parameter(Mandatory)] [string]$Method,
        [Parameter(Mandatory)] [string]$Path,
        [string]$Token,
        [string]$JsonBody
    )
    # NOTE: deliberately NOT named $args — that's PowerShell's reserved
    # automatic variable for a function's unbound positional parameters;
    # shadowing it here made `& curl.exe @args` splat something other than
    # this array and curl printed its own usage/error text as the "response"
    # (verified: ConvertFrom-Json choked on that text, not on real JSON).
    $curlArgs = @("-s", "-X", $Method, "$QaApiBase$Path")
    if ($Token) { $curlArgs += @("-H", "Authorization: Bearer $Token") }
    $bodyFile = $null
    if ($JsonBody) {
        # `-d <literal>` passes the embedded double-quotes through PowerShell's
        # own native-argument encoding, which can mangle them en route to
        # curl.exe (verified: the server received `{name:QA Basic}` with the
        # quotes stripped and rejected it as invalid JSON). `-d @file` reads
        # the body as raw bytes instead, sidestepping argv quoting entirely.
        $bodyFile = [System.IO.Path]::GetTempFileName()
        # `Encoding.UTF8` (the static property) writes a BOM, which the
        # server's JSON parser then rejects ("expected value at line 1
        # column 1" — the 3 BOM bytes before "{") — verified. Build a
        # BOM-less UTF8Encoding explicitly instead.
        $noBomUtf8 = New-Object System.Text.UTF8Encoding($false)
        [System.IO.File]::WriteAllText($bodyFile, $JsonBody, $noBomUtf8)
        $curlArgs += @("-H", "Content-Type: application/json", "-d", "@$bodyFile")
    }
    try {
        $raw = & curl.exe @curlArgs
        return $raw | ConvertFrom-Json
    } finally {
        if ($bodyFile) { Remove-Item $bodyFile -Force -ErrorAction SilentlyContinue }
    }
}

# Multipart file upload (asset ingest, fe-api/AGENTS.md §assets) — curl.exe's
# native -F, which Invoke-RestMethod cannot do on PowerShell 5.1 without a
# hand-rolled MultipartFormDataContent.
function Invoke-QaApiUpload {
    param(
        [Parameter(Mandatory)] [string]$Path,
        [Parameter(Mandatory)] [string]$Token,
        [Parameter(Mandatory)] [string]$FilePath,
        [string]$FileFieldName = "file"
    )
    $raw = & curl.exe -s -X POST "$QaApiBase$Path" `
        -H "Authorization: Bearer $Token" `
        -F "${FileFieldName}=@${FilePath};type=model/gltf-binary"
    return $raw | ConvertFrom-Json
}

# Newest <sha> directory under .qa/stage (by LastWriteTime), or $null if
# none staged yet.
function Get-QaLatestStageDir {
    if (-not (Test-Path $QaStageRoot)) { return $null }
    $dirs = Get-ChildItem $QaStageRoot -Directory -ErrorAction SilentlyContinue |
        Sort-Object LastWriteTime -Descending
    if ($dirs.Count -eq 0) { return $null }
    return $dirs[0].FullName
}

# Resolve a child of a QA root without traversing a reparse point.
function Resolve-QaChildPath {
    param([Parameter(Mandatory)][string]$Path, [Parameter(Mandatory)][string]$Root)
    $full = [IO.Path]::GetFullPath($Path)
    $boundary = [IO.Path]::GetFullPath($Root).TrimEnd('\') + '\'
    if (-not $full.StartsWith($boundary, [StringComparison]::OrdinalIgnoreCase)) {
        throw "QA path is outside $Root`: $full"
    }
    $cursor = $full
    while ($cursor -and $cursor.Length -ge $QaRepoRoot.Length) {
        if (Test-Path -LiteralPath $cursor) {
            $item = Get-Item -LiteralPath $cursor -Force -ErrorAction Stop
            if ($item.Attributes -band [IO.FileAttributes]::ReparsePoint) { throw "QA path traverses a reparse point: $cursor" }
        }
        $cursor = [IO.Path]::GetDirectoryName($cursor)
    }
    return $full
}

function Get-QaProcessIdentity {
    param([Parameter(Mandatory)][int]$ProcessId)
    $process = Get-Process -Id $ProcessId -ErrorAction SilentlyContinue
    if (-not $process) { return $null }
    try {
        $null = $process.Handle
        return [pscustomobject]@{ pid = $process.Id; start_ticks = $process.StartTime.ToUniversalTime().Ticks.ToString(); exe_path = $process.MainModule.FileName }
    } finally { $process.Dispose() }
}

function Get-QaOwnedProcess {
    param([Parameter(Mandatory)]$Identity)
    if (-not $Identity.start_ticks -or -not $Identity.exe_path) { throw 'Missing process identity; refusing PID-only cleanup' }
    $process = Get-Process -Id ([int]$Identity.pid) -ErrorAction SilentlyContinue
    if (-not $process) { return $null }
    try {
        $null = $process.Handle
        if ($process.StartTime.ToUniversalTime().Ticks.ToString() -ne [string]$Identity.start_ticks -or
            -not [string]::Equals($process.MainModule.FileName, $Identity.exe_path, [StringComparison]::OrdinalIgnoreCase)) {
            $process.Dispose()
            return $null
        }
        return $process
    } catch { $process.Dispose(); throw }
}

# Toolhelp avoids WMI permissions while retaining parent PID information.
function Get-QaOwnedProcessTree {
    param([Parameter(Mandatory)]$Identity)
    $root = Get-QaOwnedProcess -Identity $Identity
    if (-not $root) { return @() }
    try {
    if (-not ('QaProcessSnapshot' -as [type])) {
        Add-Type -TypeDefinition @'
using System;
using System.Collections.Generic;
using System.Runtime.InteropServices;
public static class QaProcessSnapshot {
    [StructLayout(LayoutKind.Sequential, CharSet = CharSet.Unicode)]
    struct Entry {
        public uint size, usage, id; public UIntPtr heap; public uint module, threads, parent;
        public int priority; public uint flags;
        [MarshalAs(UnmanagedType.ByValTStr, SizeConst = 260)] public string name;
    }
    [DllImport("kernel32.dll", SetLastError = true)] static extern IntPtr CreateToolhelp32Snapshot(uint flags, uint id);
    [DllImport("kernel32.dll", CharSet = CharSet.Unicode, SetLastError = true)] static extern bool Process32FirstW(IntPtr snapshot, ref Entry entry);
    [DllImport("kernel32.dll", CharSet = CharSet.Unicode, SetLastError = true)] static extern bool Process32NextW(IntPtr snapshot, ref Entry entry);
    [DllImport("kernel32.dll")] static extern bool CloseHandle(IntPtr handle);
    public static int[][] Read() {
        IntPtr snapshot = CreateToolhelp32Snapshot(2, 0);
        if (snapshot == new IntPtr(-1)) throw new System.ComponentModel.Win32Exception();
        try {
            var rows = new List<int[]>(); var entry = new Entry(); entry.size = (uint)Marshal.SizeOf(typeof(Entry));
            if (!Process32FirstW(snapshot, ref entry)) throw new System.ComponentModel.Win32Exception();
            do { rows.Add(new int[] { (int)entry.id, (int)entry.parent }); } while (Process32NextW(snapshot, ref entry));
            return rows.ToArray();
        } finally { CloseHandle(snapshot); }
    }
}
'@
    }
    $snapshotStarted = [DateTime]::UtcNow.Ticks
    $rows = [QaProcessSnapshot]::Read()
    $tree = New-Object System.Collections.Generic.List[object]
    $tree.Add($Identity)
    for ($i = 0; $i -lt $tree.Count; $i++) {
        $parent = $tree[$i]
        $parentProcess = Get-QaOwnedProcess -Identity $parent
        if (-not $parentProcess) { continue }
        try {
        foreach ($row in $rows) {
            if ($row[1] -ne [int]$parent.pid -or $row[0] -eq [int]$parent.pid) { continue }
            if ($tree | Where-Object { [int]$_.pid -eq $row[0] }) { continue }
            $child = Get-QaProcessIdentity -ProcessId $row[0]
            if ($child -and [long]$child.start_ticks -ge [long]$parent.start_ticks -and
                [long]$child.start_ticks -le $snapshotStarted) { $tree.Add($child) }
        }
        } finally { $parentProcess.Dispose() }
    }
    return $tree.ToArray()
    } finally { $root.Dispose() }
}

function Stop-QaOwnedProcessTree {
    param([Parameter(Mandatory)]$Identity, [object[]]$RecordedDescendants = @())
    $enumerationError = $null
    try { $tree = @(Get-QaOwnedProcessTree -Identity $Identity) }
    catch { $enumerationError = $_; $tree = @($Identity) }
    $known = @{}
    foreach ($entry in (@($tree) + @($RecordedDescendants) + @($Identity))) {
        if (-not $entry) { continue }
        $known["$($entry.pid):$($entry.start_ticks)"] = $entry
    }
    $tree = @($known.Values | Sort-Object { [long]$_.start_ticks } -Descending)
    $pinned = New-Object System.Collections.Generic.List[object]
    $failures = New-Object System.Collections.Generic.List[string]
    try {
        foreach ($entry in $tree) {
            try {
                $process = Get-QaOwnedProcess -Identity $entry
                if (-not $process) { continue }
                $pinned.Add(@{ process = $process; identity = $entry })
                $process.Kill()
            } catch { $failures.Add("PID $($entry.pid): $_") }
        }
        $deadline = [Diagnostics.Stopwatch]::StartNew()
        foreach ($item in $pinned) {
            $remaining = [math]::Max(0, 15000 - [int]$deadline.ElapsedMilliseconds)
            try {
                if (-not $item.process.WaitForExit($remaining)) { $failures.Add("PID $($item.identity.pid) did not exit within the cleanup deadline") }
            } catch { $failures.Add("PID $($item.identity.pid): $_") }
        }
    } finally { foreach ($item in $pinned) { $item.process.Dispose() } }
    if ($failures.Count -gt 0) { throw ($failures -join '; ') }

    if ($enumerationError) { throw "Root stopped, but descendant cleanup could not be verified: $enumerationError" }
}

function Get-QaStageManifest {
    param([Parameter(Mandatory)][string]$StageDir)
    $stage = Resolve-QaChildPath -Path $StageDir -Root $QaStageRoot
    $manifest = Get-Content -LiteralPath (Join-Path $stage 'stage.json') -Raw -ErrorAction Stop | ConvertFrom-Json
    $exe = Join-Path $stage 'fractalengine.exe'
    if ($manifest.kind -ne 'fractalengine-qa-stage-v1' -or
        (Get-FileHash -LiteralPath $exe -Algorithm SHA256).Hash -ne $manifest.exe_sha256) {
        throw "Invalid or modified QA stage: $stage"
    }
    if (Test-Path -LiteralPath (Join-Path $stage 'assets')) { throw 'QA stage must not contain packaged assets' }
    return $manifest
}

function Get-QaSourceFingerprint {
    $paths = @(& git -C $QaRepoRoot ls-files --cached --others --exclude-standard -- '*.rs' '*.toml' 'Cargo.lock' '*.wgsl' 'fractalengine/assets')
    if ($LASTEXITCODE -ne 0) { throw 'Could not enumerate QA build inputs' }
    $lines = foreach ($relative in ($paths | Sort-Object -Unique)) {
        $path = Join-Path $QaRepoRoot $relative
        $hash = if (Test-Path -LiteralPath $path -PathType Leaf) { (Get-FileHash -LiteralPath $path -Algorithm SHA256).Hash } else { 'MISSING' }
        "$relative=$hash"
    }
    $sha = [Security.Cryptography.SHA256]::Create()
    try { return [BitConverter]::ToString($sha.ComputeHash([Text.Encoding]::UTF8.GetBytes(($lines -join "`n")))).Replace('-', '') }
    finally { $sha.Dispose() }
}
