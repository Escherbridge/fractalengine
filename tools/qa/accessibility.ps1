# Read only static named buttons below one identity-verified owned FE HWND.
[CmdletBinding()]
param(
    [Parameter(Mandatory)][string]$Run,
    [ValidateRange(30,180)][int]$TimeoutSec = 120,
    [switch]$Worker
)

. "$PSScriptRoot\_lib.ps1"
$ErrorActionPreference = 'Stop'
$candidate = if ([IO.Path]::IsPathRooted($Run)) { $Run } else { Join-Path $QaRunRoot $Run }
$runDir = Resolve-QaChildPath -Path $candidate -Root $QaRunRoot

if (-not $Worker) {
    $start = New-Object Diagnostics.ProcessStartInfo
    $start.FileName = Join-Path $env:SystemRoot 'System32\WindowsPowerShell\v1.0\powershell.exe'
    $start.Arguments = '-NoProfile -NonInteractive -Mta -File "' + $PSCommandPath + '" -Run "' + $runDir + '" -Worker'
    $start.UseShellExecute = $false
    $start.CreateNoWindow = $true
    $start.RedirectStandardOutput = $true
    $start.RedirectStandardError = $true
    $child = New-Object Diagnostics.Process
    $child.StartInfo = $start
    $started = $false
    try {
        if (-not $child.Start()) { throw 'Could not start the owned read-only UIA helper' }
        $started = $true
        $null = $child.Handle
        $stdout = $child.StandardOutput.ReadToEndAsync()
        $stderr = $child.StandardError.ReadToEndAsync()
        if (-not $child.WaitForExit($TimeoutSec * 1000)) {
            $child.Kill()
            $null = $child.WaitForExit(5000)
            @{ status = 'incomplete'; reason = 'owned_uia_helper_timed_out' } | ConvertTo-Json -Compress
            exit 2
        }
        if ($child.ExitCode -ne 0) {
            @{ status = 'incomplete'; reason = 'owned_uia_probe_failed' } | ConvertTo-Json -Compress
            exit 2
        }
        Write-Output $stdout.GetAwaiter().GetResult()
    } finally {
        if ($started -and -not $child.HasExited) {
            $child.Kill()
            $null = $child.WaitForExit(5000)
        }
        $child.Dispose()
    }
    exit 0
}

$recordPath = Resolve-QaChildPath -Path (Join-Path $runDir 'current.json') -Root $QaRunRoot
$record = Get-Content -LiteralPath $recordPath -Raw | ConvertFrom-Json
if (-not $record.process_identity) { throw 'A recorded process identity is required' }
$process = Get-QaOwnedProcess -Identity $record.process_identity
if (-not $process) { throw 'The recorded FE process is not running with the recorded identity' }
try {
    $ownerPath = Resolve-QaChildPath -Path (Join-Path $runDir 'a11y-timing-owner.json') -Root $QaRunRoot
    $owner = Get-Content -LiteralPath $ownerPath -Raw | ConvertFrom-Json
    $ownerStarted = [long]$owner.started_unix_ms
    $processStarted = ([DateTimeOffset]$process.StartTime.ToUniversalTime()).ToUnixTimeMilliseconds()
    $now = [DateTimeOffset]::UtcNow.ToUnixTimeMilliseconds()
    if ($owner.process_id -ne $process.Id -or $ownerStarted -lt $processStarted -or $ownerStarted -gt $now) {
        throw 'The timing owner marker does not belong to this recorded process lifetime'
    }
    $clock = [Diagnostics.Stopwatch]::StartNew()
    $readyPath = Join-Path $runDir 'a11y-baseline-ready.json'
    while (-not (Test-Path -LiteralPath $readyPath)) {
        if ($process.HasExited -or $clock.Elapsed.TotalSeconds -ge 70) { throw 'No valid timing baseline became ready' }
        $reportPath = Resolve-QaChildPath -Path (Join-Path $runDir 'a11y-timing.json') -Root $QaRunRoot
        if (Test-Path -LiteralPath $reportPath) {
            $report = Get-Content -LiteralPath $reportPath -Raw | ConvertFrom-Json
            if ($report.process_id -ne $process.Id -or [long]$report.owner_started_unix_ms -ne $ownerStarted) { throw 'The timing report belongs to another process lifetime' }
            if ($report.phase -eq 'done') { throw 'The timing baseline was unavailable' }
        }
        Start-Sleep -Milliseconds 200
    }
    $readyPath = Resolve-QaChildPath -Path $readyPath -Root $QaRunRoot
    $ready = Get-Content -LiteralPath $readyPath -Raw | ConvertFrom-Json
    if ($ready.process_id -ne $process.Id -or [long]$ready.owner_started_unix_ms -ne $ownerStarted -or $ready.qa_input_gate -ne $false -or $ready.baseline_ready -ne $true) {
        throw 'The baseline marker does not identify this gate-off FE process'
    }
    $assemblyDir = Join-Path $env:SystemRoot 'Microsoft.NET\Framework64\v4.0.30319\WPF'
    Add-Type -Path (Join-Path $assemblyDir 'UIAutomationTypes.dll')
    Add-Type -Path (Join-Path $assemblyDir 'UIAutomationClient.dll')
    $process.Refresh()
    $handle = $process.MainWindowHandle
    if ($handle -eq [IntPtr]::Zero) { throw 'The owned FE process has no primary HWND' }
    $root = [Windows.Automation.AutomationElement]::FromHandle($handle)
    if ($root.Current.ProcessId -ne $process.Id) { throw 'The HWND provider does not belong to the owned FE process' }
    $expected = @('Close FractalEngine', 'Minimize', 'Maximize', 'Restore down', 'Select', 'Move', 'Rotate', 'Scale', 'Pen', 'Brush', 'Measure', 'Toggle Data panel')
    $names = [Windows.Automation.Condition[]]@($expected | ForEach-Object {
        [Windows.Automation.PropertyCondition]::new([Windows.Automation.AutomationElement]::NameProperty, $_)
    })
    $filter = [Windows.Automation.AndCondition]::new([Windows.Automation.Condition[]]@(
        [Windows.Automation.PropertyCondition]::new([Windows.Automation.AutomationElement]::ControlTypeProperty, [Windows.Automation.ControlType]::Button),
        [Windows.Automation.PropertyCondition]::new([Windows.Automation.AutomationElement]::ProcessIdProperty, $process.Id),
        [Windows.Automation.OrCondition]::new($names)
    ))
    $probe = [Diagnostics.Stopwatch]::StartNew()
    $probeMatches = @()
    $passed = $false
    do {
        if ($process.HasExited) { throw 'The owned FE process exited during the read-only probe' }
        $items = $root.FindAll([Windows.Automation.TreeScope]::Descendants, $filter)
        $probeMatches = @()
        for ($index = 0; $index -lt [Math]::Min($items.Count, 32); $index++) {
            $item = $items.Item($index).Current
            if ($item.ProcessId -ne $process.Id -or $item.ControlType -ne [Windows.Automation.ControlType]::Button -or $expected -notcontains $item.Name) { continue }
            $bounds = $item.BoundingRectangle
            $finite = $true
            foreach ($value in @($bounds.X, $bounds.Y, $bounds.Width, $bounds.Height)) {
                if ([Double]::IsNaN($value) -or [Double]::IsInfinity($value)) { $finite = $false }
            }
            if ($finite -and $bounds.Width -gt 0 -and $bounds.Height -gt 0) {
                $probeMatches += @{ name = $item.Name; role = 'Button'; bounds = @($bounds.X, $bounds.Y, $bounds.Width, $bounds.Height) }
            }
        }
        $found = @($probeMatches | ForEach-Object { $_.name })
        $caption = ($found -contains 'Close FractalEngine') -and ($found -contains 'Minimize') -and (($found -contains 'Maximize') -or ($found -contains 'Restore down'))
        $tool = @($found | Where-Object { $_ -in @('Select', 'Move', 'Rotate', 'Scale', 'Pen', 'Brush', 'Measure', 'Toggle Data panel') }).Count -gt 0
        $passed = $caption -and $tool
        if (-not $passed) { Start-Sleep -Milliseconds 200 }
    } while (-not $passed -and $probe.Elapsed.TotalSeconds -lt 15)
    @{ status = $(if ($passed) { 'pass' } else { 'incomplete' }); process_id = $process.Id; baseline_ready = $true; matches = $probeMatches } | ConvertTo-Json -Depth 5 -Compress
} finally {
    $process.Dispose()
}
