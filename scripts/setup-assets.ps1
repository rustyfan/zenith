param(
    [string]$Destination = (Join-Path $PSScriptRoot '..'),
    [string]$Archive,
    [switch]$Offline,
    [switch]$Force,
    [ValidateRange(1, 3600)][int]$DownloadTimeoutSeconds = 600
)

$ErrorActionPreference = 'Stop'
. (Join-Path $PSScriptRoot 'asset-tools.ps1')
Add-Type -AssemblyName System.IO.Compression.FileSystem
$repo = (Resolve-Path -LiteralPath (Join-Path $PSScriptRoot '..')).Path
$lockPath = Join-Path $repo 'assets.lock.json'
if (!(Test-Path -LiteralPath $lockPath)) { throw 'assets.lock.json is missing; the asset package has not been published for this revision.' }
$lock = Get-Content -LiteralPath $lockPath -Raw | ConvertFrom-Json
if ($lock.schemaVersion -ne 1 -or $lock.package -ne 'ghcr.io/rustyfan/zenith-assets' -or
    $lock.digest -notmatch '^sha256:[0-9a-f]{64}$' -or $lock.archive.name -ne 'zenith-assets.zip' -or
    $lock.archive.sha256 -notmatch '^[0-9a-f]{64}$' -or !$lock.files.Count) { throw 'Invalid assets.lock.json' }
New-Item -ItemType Directory -Force -Path $Destination | Out-Null
$root = (Resolve-Path -LiteralPath $Destination).Path
$missing = @()
$seen = @{}
foreach ($file in $lock.files) {
    if ($seen.ContainsKey($file.path) -or $file.sha256 -notmatch '^[0-9a-f]{64}$' -or $file.bytes -lt 0) { throw 'Invalid asset file manifest' }
    $seen[$file.path] = $true
    $path = Get-AssetPath $root $file.path
    if (!(Test-AssetHash $path $file.sha256)) {
        if ((Test-Path -LiteralPath $path) -and !$Force) { throw "Local asset differs: $($file.path). Preserving it; use Setup.bat -Force to replace package files." }
        $missing += $file
    }
}
if (!$missing.Count) { Write-Host "zenith-assets $($lock.version) is already installed and verified."; return }
$cache = Join-Path $root ('target/asset-downloads/' + $lock.archive.sha256)
New-Item -ItemType Directory -Force -Path $cache | Out-Null
if ($Archive) {
    $archivePath = (Resolve-Path -LiteralPath $Archive).Path
} else {
    $archivePath = Join-Path $cache $lock.archive.name
    if (!(Test-AssetHash $archivePath $lock.archive.sha256)) {
        if ($Offline) { throw 'Asset ZIP is not cached. Run Setup.bat with internet access or supply -Archive <zip>.' }
        Write-Host "Downloading zenith-assets $($lock.version)..."
        [Net.ServicePointManager]::SecurityProtocol = [Net.ServicePointManager]::SecurityProtocol -bor [Net.SecurityProtocolType]::Tls12
        $token = Invoke-RestMethod -UseBasicParsing -Uri 'https://ghcr.io/token?service=ghcr.io&scope=repository:rustyfan/zenith-assets:pull' -TimeoutSec $DownloadTimeoutSeconds
        $headers = @{ Authorization=('Bearer ' + $token.token); Accept='application/vnd.oci.image.manifest.v1+json' }
        $registry = 'https://ghcr.io/v2/rustyfan/zenith-assets'
        $manifestPath = Join-Path $cache 'manifest.json'
        Get-AssetDownload ($registry + '/manifests/' + $lock.digest) $manifestPath $lock.digest.Substring(7) $headers $DownloadTimeoutSeconds
        $manifest = Get-Content -LiteralPath $manifestPath -Raw | ConvertFrom-Json
        if ($manifest.layers.Count -ne 1 -or $manifest.layers[0].digest -ne ('sha256:' + $lock.archive.sha256) -or
            $manifest.layers[0].size -ne $lock.archive.bytes -or $manifest.layers[0].mediaType -ne 'application/zip') {
            throw 'Published package does not match assets.lock.json'
        }
        $headers.Remove('Accept')
        Get-AssetDownload ($registry + '/blobs/sha256:' + $lock.archive.sha256) $archivePath $lock.archive.sha256 $headers $DownloadTimeoutSeconds
    }
}
if (!(Test-AssetHash $archivePath $lock.archive.sha256) -or (Get-Item -LiteralPath $archivePath).Length -ne $lock.archive.bytes) {
    throw 'Asset ZIP checksum or size mismatch; no assets were installed.'
}
$stage = Join-Path $cache ('stage-' + [guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $stage | Out-Null
try {
    $zip = [IO.Compression.ZipFile]::OpenRead($archivePath)
    try {
        if ($zip.Entries.Count -ne $lock.files.Count) { throw 'Unexpected files in asset ZIP' }
        foreach ($file in $lock.files) {
            $entries = @($zip.Entries | Where-Object { $_.FullName -ceq $file.path })
            if ($entries.Count -ne 1 -or $entries[0].Length -ne $file.bytes) { throw "Invalid ZIP entry: $($file.path)" }
            $stagedPath = Get-AssetPath $stage $file.path
            New-Item -ItemType Directory -Force -Path (Split-Path $stagedPath) | Out-Null
            [IO.Compression.ZipFileExtensions]::ExtractToFile($entries[0], $stagedPath)
            if (!(Test-AssetHash $stagedPath $file.sha256)) { throw "Invalid asset checksum: $($file.path)" }
        }
    } finally { $zip.Dispose() }
    foreach ($file in $missing) {
        $path = Get-AssetPath $root $file.path
        if ((Test-Path -LiteralPath $path) -and !$Force -and !(Test-AssetHash $path $file.sha256)) {
            throw "Asset changed during setup; preserving $($file.path)"
        }
        New-Item -ItemType Directory -Force -Path (Split-Path $path) | Out-Null
        Move-Item -LiteralPath (Get-AssetPath $stage $file.path) -Destination $path -Force
    }
    Write-Host "Installed and verified zenith-assets $($lock.version) in $root"
} finally {
    $stagePath = [IO.Path]::GetFullPath($stage)
    $cachePrefix = [IO.Path]::GetFullPath($cache).TrimEnd('\', '/') + [IO.Path]::DirectorySeparatorChar
    if (!$stagePath.StartsWith($cachePrefix, [StringComparison]::OrdinalIgnoreCase)) { throw 'Invalid staging cleanup path' }
    Remove-Item -LiteralPath $stagePath -Recurse -Force
}
