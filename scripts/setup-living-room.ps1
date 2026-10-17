param(
    [string]$Destination = (Join-Path $PSScriptRoot '..')
)

$ErrorActionPreference = 'Stop'
. (Join-Path $PSScriptRoot 'asset-tools.ps1')
$sources = Get-Content -LiteralPath (Join-Path $PSScriptRoot '../assets.sources.json') -Raw | ConvertFrom-Json
New-Item -ItemType Directory -Force -Path $Destination | Out-Null
$root = (Resolve-Path -LiteralPath $Destination).Path
foreach ($download in $sources.downloads) {
    $path = Get-AssetPath $root $download.path
    if (Test-AssetHash $path $download.sha256) { continue }
    if (Test-Path -LiteralPath $path) { throw "Local asset differs: $($download.path)" }
    New-Item -ItemType Directory -Force -Path (Split-Path $path) | Out-Null
    Get-AssetDownload $download.uri $path $download.sha256
}
$notice = Get-AssetPath $root 'content/mesh/living-room-2/license.txt'
New-Item -ItemType Directory -Force -Path (Split-Path $notice) | Out-Null
Copy-Item -LiteralPath (Join-Path $PSScriptRoot '../content/licenses/living-room-2.txt') -Destination $notice -Force
Write-Host 'Living Room 2 source files verified.'
