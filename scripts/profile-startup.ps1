param(
    [string]$Label = 'world',
    [string]$Executable = 'target/debug/examples/world.exe',
    [string]$ShaderCache = 'asset/shaders',
    [int]$Runs = 7,
    [switch]$ColdShaders,
    [switch]$NoShaderCache,
    [string]$SlangDir = $env:SLANG_DIR
)

$ErrorActionPreference = 'Stop'
if ($Runs -lt 1 -or $Label -notmatch '^[a-zA-Z0-9_-]+$') { throw 'Use a simple label and at least one measured run' }
$repo = (Resolve-Path (Join-Path $PSScriptRoot '..')).Path
$names = @('ZENITH_STARTUP_PROFILE','ZENITH_SHADER_CACHE','ZENITH_SHADER_DEBUG','ZENITH_TEST_FRAMES','ZENITH_TEST_RESIZE','ZENITH_VALIDATION','VK_LAYER_VALIDATE_SYNC','ZENITH_PROFILE','RUST_LOG','PATH','SLANG_DIR')
$saved = @{}
foreach ($name in $names) { $saved[$name] = [Environment]::GetEnvironmentVariable($name, 'Process') }
Push-Location $repo
try {
    if ($SlangDir) { $env:SLANG_DIR = $SlangDir }
    $env:ZENITH_TEST_FRAMES = '1'
    $env:ZENITH_VALIDATION = '1'
    $env:VK_LAYER_VALIDATE_SYNC = '1'
    $env:ZENITH_SHADER_DEBUG = '1'
    Remove-Item Env:ZENITH_TEST_RESIZE -ErrorAction SilentlyContinue
    $env:ZENITH_PROFILE = '1'
    $env:RUST_LOG = 'debug'
    $env:PATH = (Resolve-Path (Join-Path (Split-Path (Split-Path $Executable)) 'deps')).Path + ';' + $env:PATH
    $output = Join-Path $repo 'target/startup-measurements'
    New-Item -ItemType Directory -Force $output | Out-Null
    $records = @()
    for ($run = 0; $run -le $Runs; $run++) {
        $stem = Join-Path $output "$Label-$run"
        $env:ZENITH_STARTUP_PROFILE = "$stem.csv"
        $env:ZENITH_SHADER_CACHE = if ($NoShaderCache) { '0' } elseif ($ColdShaders) { "$stem-shaders" } else { $ShaderCache }
        if ($ColdShaders -and (Test-Path -LiteralPath $env:ZENITH_SHADER_CACHE)) { throw "Cold shader directory exists; choose another label: $env:ZENITH_SHADER_CACHE" }
        $process = Start-Process -FilePath (Resolve-Path $Executable).Path -WorkingDirectory $repo -WindowStyle Hidden -RedirectStandardOutput "$stem.stdout.log" -RedirectStandardError "$stem.stderr.log" -PassThru
        $created = ([DateTimeOffset]$process.StartTime.ToUniversalTime()).ToUnixTimeMilliseconds()
        if (!$process.WaitForExit(60000)) { $process.Kill(); throw 'Startup exceeded 60 seconds' }
        $process.Refresh()
        if ($process.ExitCode -ne 0) { throw "Startup failed: $(Get-Content "$stem.stderr.log" -Raw)" }
        $events = @(Import-Csv "$stem.csv")
        $epoch = [double]($events | Where-Object name -eq 'entry_epoch_ms').start_ms
        $presented = [double]($events | Where-Object name -eq 'first_present_return').start_ms
        if (!($events | Where-Object name -eq 'present_accepted')) { throw 'No accepted presentation recorded' }
        $log = Get-Content "$stem.stderr.log" -Raw
        $records += [pscustomobject]@{
            run = $run; process_to_present_ms = $epoch - $created + $presented; entry_to_present_ms = $presented
            gpu_completion_after_present_ms = [double]($events | Where-Object name -eq 'first_gpu_complete').start_ms - $presented
            shader_compiles = ([regex]::Matches($log, 'Shader compiler:')).Count
            shader_cache_hits = ([regex]::Matches($log, 'Shader cache hit:')).Count
        }
        Write-Output "$Label run=$run first_present=$([math]::Round($records[-1].process_to_present_ms,3)) ms compiles=$($records[-1].shader_compiles) hits=$($records[-1].shader_cache_hits)"
    }
    ConvertTo-Json -InputObject @($records) | Set-Content -Encoding UTF8 (Join-Path $output "$Label.json")
    $sorted = @($records | Where-Object run -gt 0 | Select-Object -ExpandProperty process_to_present_ms | Sort-Object)
    $median = ($sorted[[int][math]::Floor(($Runs-1)/2)] + $sorted[[int][math]::Floor($Runs/2)])/2
    Write-Output "Median: $([math]::Round($median,3)) ms; range $([math]::Round($sorted[0],3))-$([math]::Round($sorted[-1],3)) ms"
} finally {
    foreach ($name in $names) { [Environment]::SetEnvironmentVariable($name, $saved[$name], 'Process') }
    Pop-Location
}
