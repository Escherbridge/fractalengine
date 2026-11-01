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

# BFS over Win32_Process.ParentProcessId starting at $RootId. Returns every
# pid in the tree (root included), so Stop-Process can take out a spawned
# WebView2 host/renderer/GPU helper the GUI never cleans up on SIGKILL —
# verified empirically: the Tauri backend spawns a child `msedgewebview2.exe`
# under this exact pid (tools/qa/AGENTS.md §process-tree).
function Get-QaProcessTree {
    param([Parameter(Mandatory)] [int]$RootId)
    $ids = [System.Collections.Generic.List[int]]::new()
    $frontier = [System.Collections.Generic.Queue[int]]::new()
    $frontier.Enqueue($RootId)
    while ($frontier.Count -gt 0) {
        $id = $frontier.Dequeue()
        if ($ids.Contains($id)) { continue }
        $ids.Add($id)
        Get-CimInstance Win32_Process -Filter "ParentProcessId=$id" -ErrorAction SilentlyContinue |
            ForEach-Object { $frontier.Enqueue([int]$_.ProcessId) }
    }
    return $ids
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
