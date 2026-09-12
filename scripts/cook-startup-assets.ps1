param(
    [string]$Source = $env:ZENITH_CONTENT,
    [string]$Cache = $env:ZENITH_ASSET_CACHE,
    [switch]$Offline
)

$ErrorActionPreference = 'Stop'
if (!$Source) { $Source = 'content' }
if (!$Cache) { $Cache = 'asset' }
[string[]]$offlineArgs = @()
if ($Offline) { $offlineArgs = @('--offline') }
Push-Location (Join-Path $PSScriptRoot '..')
try {
    & cargo run @offlineArgs -p zenith-asset --release --example assets -- cook $Source $Cache texture/minedump_flats_4k.hdr
    if ($LASTEXITCODE -ne 0) { throw 'Startup asset cooking failed' }
} finally {
    Pop-Location
}
