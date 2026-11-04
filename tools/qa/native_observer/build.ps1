# Fixed x64 SDK build recipe; see AGENTS.md. Never executed by the observer CLI.
[CmdletBinding()]
param([Parameter(Mandatory)][string]$OutputDirectory)
. "$PSScriptRoot\..\_lib.ps1"
$ErrorActionPreference = 'Stop'
$output = Resolve-QaChildPath -Path $OutputDirectory -Root $QaStageRoot
if (Test-Path -LiteralPath (Join-Path $output 'stage.json')) { throw 'Native helper cannot modify a published immutable stage' }
$vswhere = 'C:\Program Files (x86)\Microsoft Visual Studio\Installer\vswhere.exe'
if (-not (Test-Path -LiteralPath $vswhere)) { throw 'Visual Studio installer discovery unavailable' }
$instances = @(& $vswhere -latest -version '[17.0,18.0)' -products '*' `
    -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath |
    Where-Object { -not [string]::IsNullOrWhiteSpace($_) })
if ($LASTEXITCODE -ne 0 -or $instances.Count -ne 1) { throw 'Declared Visual Studio 2022 C++ toolchain unavailable' }
$vcvars = Join-Path -Path ($instances[0].Trim()) -ChildPath 'VC\Auxiliary\Build\vcvarsall.bat'
$sdk = 'C:\Program Files (x86)\Windows Kits\10\Include\10.0.22621.0\um\UIAutomationClient.h'
if (-not (Test-Path -LiteralPath $vcvars) -or -not (Test-Path -LiteralPath $sdk)) { throw 'Declared x64 MSVC/SDK22621 toolchain unavailable' }
foreach ($path in @($output, $PSScriptRoot, $vcvars)) {
    if ($path -match '["%!^&|<>\r\n]') { throw 'Native build path contains shell metacharacters' }
}
$exe = Join-Path $output 'native_observer.exe'
$object = Join-Path $output 'native_observer.obj'
$manifest = Join-Path $PSScriptRoot 'worker.manifest'
$source = Join-Path $PSScriptRoot 'worker.cpp'
$command = 'call "' + $vcvars + '" x64 10.0.22621.0 >nul && cl.exe /nologo /std:c++20 /MT /EHsc /W4 /utf-8 /permissive- /D_WIN32_WINNT=0x0A00 /DUNICODE /D_UNICODE "' + $source + '" /Fo"' + $object + '" /Fe"' + $exe + '" /link /SUBSYSTEM:CONSOLE /MANIFEST:EMBED /MANIFESTINPUT:"' + $manifest + '" ole32.lib oleaut32.lib uiautomationcore.lib user32.lib advapi32.lib bcrypt.lib dwmapi.lib'
& $env:ComSpec /d /s /c $command
if ($LASTEXITCODE -ne 0 -or -not (Test-Path -LiteralPath $exe)) { throw 'Native helper build failed' }
$sourceLines = foreach ($name in @('build.ps1', 'worker.cpp', 'worker.manifest', 'worker_protocol.hpp')) {
    $hash = (Get-FileHash -LiteralPath (Join-Path $PSScriptRoot $name) -Algorithm SHA256).Hash.ToLowerInvariant()
    "$name=$hash"
}
$sha = [Security.Cryptography.SHA256]::Create()
try { $sourceHash = [BitConverter]::ToString($sha.ComputeHash([Text.Encoding]::UTF8.GetBytes(($sourceLines -join "`n")))).Replace('-', '').ToLowerInvariant() }
finally { $sha.Dispose() }
return @{ schema = 'fe-native-helper-stage-v1'; helper_file = 'native_observer.exe'
    helper_sha256 = (Get-FileHash -LiteralPath $exe -Algorithm SHA256).Hash.ToLowerInvariant()
    source_sha256 = $sourceHash; sdk = '10.0.22621.0'; architecture = 'x64'; contract = 'fe-native-worker-v1' }
