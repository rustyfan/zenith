param(
    [Parameter(Mandatory)][string[]]$Binaries,
    [int]$Rounds = 3,
    [int]$Warmup = 3000,
    [int]$Frames = 6000,
    [switch]$GpuTimings,
    [string]$Output = 'target/cpu-bench/results',
    [string]$SlangDir = 'D:\Software\slang-2026.17-windows-x86_64'
)

$ErrorActionPreference = 'Stop'
$repo = (Resolve-Path (Join-Path $PSScriptRoot '..')).Path
$outputPath = [IO.Path]::GetFullPath((Join-Path $repo $Output))
New-Item -ItemType Directory -Force $outputPath | Out-Null
$saved = @{}
$names = @('SLANG_DIR', 'RUST_LOG', 'ZENITH_BENCH_WARMUP', 'ZENITH_BENCH_FRAMES', 'ZENITH_BENCH_OUTPUT', 'ZENITH_VALIDATION', 'VK_INSTANCE_LAYERS', 'ZENITH_TEST_RESIZE', 'ZENITH_TEST_GPU_TIMINGS', 'ZENITH_PROFILE', 'ZENITH_STARTUP_PROFILE')
foreach ($name in $names) { $saved[$name] = [Environment]::GetEnvironmentVariable($name, 'Process') }
try {
    $env:SLANG_DIR = $SlangDir
    $env:RUST_LOG = 'info'
    $env:ZENITH_BENCH_WARMUP = "$Warmup"
    $env:ZENITH_BENCH_FRAMES = "$Frames"
    if ($GpuTimings) {
        $env:ZENITH_TEST_GPU_TIMINGS = '1'
    } else {
        Remove-Item -LiteralPath Env:ZENITH_TEST_GPU_TIMINGS -ErrorAction SilentlyContinue
    }
    foreach ($name in @('ZENITH_VALIDATION', 'VK_INSTANCE_LAYERS', 'ZENITH_TEST_RESIZE', 'ZENITH_PROFILE', 'ZENITH_STARTUP_PROFILE')) {
        Remove-Item -LiteralPath "Env:$name" -ErrorAction SilentlyContinue
    }
    for ($round = 1; $round -le $Rounds; $round++) {
        $order = @($Binaries)
        if ($round % 2 -eq 0) { [Array]::Reverse($order) }
        foreach ($binary in $order) {
            $executable = (Resolve-Path (Join-Path $repo $binary)).Path
            $label = [IO.Path]::GetFileNameWithoutExtension($executable)
            $prefix = Join-Path $outputPath "$label-$round"
            $env:ZENITH_BENCH_OUTPUT = "$prefix.csv"
            $process = Start-Process -FilePath $executable -WorkingDirectory $repo -WindowStyle Hidden -RedirectStandardOutput "$prefix.stdout.log" -RedirectStandardError "$prefix.stderr.log" -PassThru
            if (-not $process.WaitForExit(60000)) { $process.Kill(); throw "Benchmark timed out: $label" }
            if ($process.ExitCode -ne 0) { throw "Benchmark failed: $label ($($process.ExitCode)); see $prefix.stderr.log" }
            if (-not (Test-Path "$prefix.csv")) { throw "Benchmark did not write measurements: $label" }
            $count = ([IO.File]::ReadAllLines("$prefix.csv")).Length - 1
            if ($count -ne $Frames) { throw "Expected $Frames samples, received $count from $label" }
            Write-Output "Completed $label round $round"
        }
    }
} finally {
    foreach ($name in $names) {
        if ($null -eq $saved[$name]) {
            Remove-Item -LiteralPath "Env:$name" -ErrorAction SilentlyContinue
        } else {
            [Environment]::SetEnvironmentVariable($name, $saved[$name], 'Process')
        }
    }
}
