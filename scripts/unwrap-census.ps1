# Unwrap/expect census: lists every production .unwrap() / .expect() site in the
# workspace (lib + bin targets only, so #[cfg(test)] code is excluded by
# construction). Read-only: never edits code. PowerShell twin of
# unwrap-census.sh; see docs/unwrap-audit.md.
#
# Usage: scripts/unwrap-census.ps1 [-OutputFile <path>] [-Toolchain 1.99.0]
param(
    [string]$OutputFile = "",
    [string]$Toolchain = ""
)
$ErrorActionPreference = "Stop"

$repoRoot = Split-Path -Parent $PSScriptRoot
$cargoArgs = @()
if ($Toolchain) { $cargoArgs += "+$Toolchain" }
$cargoArgs += @("clippy", "--workspace", "--lib", "--bins", "--message-format=short", "--",
    "-A", "clippy::all", "-W", "clippy::unwrap_used", "-W", "clippy::expect_used")

Push-Location $repoRoot
try {
    $previousPreference = $ErrorActionPreference
    $ErrorActionPreference = "Continue"
    $rawLines = & cargo @cargoArgs 2>&1 | ForEach-Object { "$_" }
    $exitCode = $LASTEXITCODE
    $ErrorActionPreference = $previousPreference
    if ($exitCode -ne 0) {
        Write-Error ("unwrap-census: cargo clippy failed; last lines:`n" + (($rawLines | Select-Object -Last 20) -join "`n"))
    }

    # `path:line:col: warning: used `unwrap()` on ...` -> `path:line:col kind`
    $pattern = '^(?<file>[^ :]+\.rs):(?<line>\d+):(?<col>\d+): warning: used `(?<kind>unwrap|expect)\(\)`'
    $sites = $rawLines |
        ForEach-Object { $_ -replace '\\', '/' } |
        Where-Object { $_ -match $pattern } |
        ForEach-Object { "{0}:{1}:{2} {3}" -f $Matches.file, $Matches.line, $Matches.col, $Matches.kind } |
        Sort-Object -Unique

    $commit = (git rev-parse --short HEAD).Trim()
    # Native stderr (e.g. CRLF warnings) must not trip $ErrorActionPreference = Stop.
    $ErrorActionPreference = "Continue"
    git diff --quiet 2>&1 | Out-Null
    $dirty = $LASTEXITCODE -ne 0
    $ErrorActionPreference = $previousPreference
    if ($dirty) { $commit += " (dirty)" }
    $unwrapCount = @($sites | Where-Object { $_ -like "* unwrap" }).Count
    $expectCount = @($sites | Where-Object { $_ -like "* expect" }).Count

    $report = New-Object System.Collections.Generic.List[string]
    $report.Add("# unwrap/expect census")
    $report.Add("date:    " + (Get-Date).ToUniversalTime().ToString("yyyy-MM-ddTHH:mmZ"))
    $report.Add("commit:  $commit")
    $report.Add("command: cargo clippy --workspace --lib --bins --message-format=short -- -A clippy::all -W clippy::unwrap_used -W clippy::expect_used")
    $report.Add("total:   $(@($sites).Count) ($unwrapCount unwrap, $expectCount expect)")
    $report.Add("")
    $report.Add("## per crate (count crate)")
    $sites | Group-Object { ($_ -split '/')[0] } | Sort-Object Count -Descending |
        ForEach-Object { $report.Add(("{0,7} {1}" -f $_.Count, $_.Name)) }
    $report.Add("")
    $report.Add("## per file (count file)")
    $sites | Group-Object { ($_ -split ':')[0] } | Sort-Object Count -Descending |
        ForEach-Object { $report.Add(("{0,7} {1}" -f $_.Count, $_.Name)) }
    $report.Add("")
    $report.Add("## sites (file:line:col kind)")
    $sites | ForEach-Object { $report.Add($_) }

    if ($OutputFile) {
        $report | Out-File -FilePath $OutputFile -Encoding utf8
        Write-Host "unwrap-census: wrote $OutputFile"
    } else {
        $report
    }
} finally {
    Pop-Location
}
exit 0
