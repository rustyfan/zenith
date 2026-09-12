param(
    [string]$SlangDir = $env:SLANG_DIR,
    [string]$ValidationDir = $env:VK_LAYER_PATH,
    [switch]$GraphOnly,
    [switch]$WindowTests,
    [switch]$Offline
)

$ErrorActionPreference = 'Stop'
$originalEnvironment = @{}
foreach ($name in @('SLANG_DIR','VK_LAYER_PATH','VK_LAYER_VALIDATE_SYNC','ZENITH_VALIDATION','ZENITH_TEST_TIME','ZENITH_TEST_FRAMES','ZENITH_PROFILE','ZENITH_TEST_RESIZE')) {
    $originalEnvironment[$name] = [Environment]::GetEnvironmentVariable($name, 'Process')
}
$repo = (Resolve-Path (Join-Path $PSScriptRoot '..')).Path
if (-not $SlangDir) { $SlangDir = 'D:\Software\slang-2026.17-windows-x86_64' }
if (-not $ValidationDir) { $ValidationDir = Join-Path $repo 'target\vulkan-sdk\Bin' }
if (-not $env:ZENITH_SLANGC -and -not (Test-Path (Join-Path $SlangDir 'bin\slangc.exe'))) { throw 'Set -SlangDir to the Slang SDK directory or set ZENITH_SLANGC' }
if (-not (@($ValidationDir -split ';' | Where-Object { $_ -and (Test-Path (Join-Path $_ 'VkLayer_khronos_validation.json')) }).Count)) {
    throw 'Set -ValidationDir to a Vulkan SDK Bin directory containing VkLayer_khronos_validation.json'
}
$env:SLANG_DIR = $SlangDir
$env:VK_LAYER_PATH = $ValidationDir
$env:VK_LAYER_VALIDATE_SYNC = '1'
$env:ZENITH_VALIDATION = '1'
$env:ZENITH_TEST_TIME = '0'
$env:ZENITH_TEST_FRAMES = '1000'
$env:ZENITH_PROFILE = '1'
Remove-Item Env:ZENITH_TEST_RESIZE -ErrorAction SilentlyContinue
$logs = Join-Path $repo 'target\validation'
New-Item -ItemType Directory -Force $logs | Out-Null
$offlineArgs = @()
if ($Offline) { $offlineArgs = @('--offline') }

function Invoke-CargoCheck([string]$Name, [string[]]$CargoArgs) {
    $log = Join-Path $logs "$Name.log"
    & cargo @offlineArgs @CargoArgs *> $log
    if ($LASTEXITCODE -ne 0) { Get-Content $log -Tail 40; throw "$Name failed: $log" }
    Write-Host "PASS $Name"
}

function Invoke-WindowCheck([string]$Name, [string]$Binary) {
    $stdout = Join-Path $logs "$Name.log"
    $stderr = Join-Path $logs "$Name-errors.log"
    $process = Start-Process -FilePath $Binary -WorkingDirectory $repo -WindowStyle Hidden -RedirectStandardOutput $stdout -RedirectStandardError $stderr -PassThru
    if (-not $process.WaitForExit(60000)) { $process.Kill(); throw "$Name exceeded 60 seconds" }
    if ($process.ExitCode -ne 0) { Get-Content $stderr -Tail 20; throw "$Name failed" }
    Write-Host "PASS $Name"
}

Push-Location $repo
try {
    if ($GraphOnly) {
        Invoke-CargoCheck 'debug-graph' @('run', '-p', 'zenith-rendergraph', '--example', 'graph_smoke')
        Invoke-CargoCheck 'release-graph' @('run', '-p', 'zenith-rendergraph', '--example', 'graph_smoke', '--release')
        return
    }
    Invoke-CargoCheck 'workspace-tests' @('test', '--workspace', '--all-targets')
    Invoke-CargoCheck 'shader-compiler-tests' @('test', '-p', 'zenith-rhi', '--lib', '--', '--ignored')
    Invoke-CargoCheck 'asset-runtime-tests' @('test', '-p', 'zenith-asset', '--no-default-features')
    Invoke-CargoCheck 'asset-parallel-runtime-tests' @('test', '-p', 'zenith-asset', '--no-default-features', '--features', 'parallel')
    Invoke-CargoCheck 'asset-gpu-tests' @('test', '-p', 'zenith-renderer', '--lib', '--', '--ignored')
    Invoke-CargoCheck 'asset-import-tests' @('test', '-p', 'zenith-asset', '--lib', '--', '--ignored')
    Invoke-CargoCheck 'workspace-doc-tests' @('test', '--workspace', '--doc')
    Invoke-CargoCheck 'optional-features' @('check', '--workspace', '--all-targets', '--all-features')
    Invoke-CargoCheck 'capabilities' @('run', '-p', 'zenith-rhi', '--example', 'capabilities')
    foreach ($profile in @('debug', 'release')) {
        $releaseArgs = if ($profile -eq 'release') { @('--release') } else { @() }
        Invoke-CargoCheck "$profile-build" (@('build', '--workspace', '--all-targets') + $releaseArgs)
        Invoke-CargoCheck "$profile-api" (@('run', '-p', 'zenith-rhi', '--example', 'minimal_smoke') + $releaseArgs)
        Invoke-CargoCheck "$profile-graph" (@('run', '-p', 'zenith-rendergraph', '--example', 'graph_smoke') + $releaseArgs)
        if ($WindowTests) {
            $bin = Join-Path $repo "target\$profile"
            Invoke-WindowCheck "$profile-window-api" (Join-Path $bin 'examples\minimal_window.exe')
            Invoke-WindowCheck "$profile-clear" (Join-Path $bin 'zenith-sandbox.exe')
            Invoke-WindowCheck "$profile-triangle" (Join-Path $bin 'examples\triangle.exe')
            Invoke-WindowCheck "$profile-world" (Join-Path $bin 'examples\world.exe')
            $env:ZENITH_TEST_RESIZE = '1'
            Invoke-WindowCheck "$profile-world-resize" (Join-Path $bin 'examples\world.exe')
            Remove-Item Env:ZENITH_TEST_RESIZE
        }
    }
} finally {
    Pop-Location
    foreach ($name in $originalEnvironment.Keys) { [Environment]::SetEnvironmentVariable($name, $originalEnvironment[$name], 'Process') }
}
