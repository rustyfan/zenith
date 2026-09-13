param(
    [string]$SourceRoot = (Join-Path $PSScriptRoot '..'),
    [string]$OutputDirectory = (Join-Path $PSScriptRoot '../target/zenith-assets'),
    [ValidatePattern('^\d+\.\d+\.\d+$')][string]$Version = '1.0.0'
)

$ErrorActionPreference = 'Stop'
. (Join-Path $PSScriptRoot 'asset-tools.ps1')
Add-Type -AssemblyName System.IO.Compression.FileSystem
$sources = Get-Content -LiteralPath (Join-Path $PSScriptRoot '../assets.sources.json') -Raw | ConvertFrom-Json
$sourcePath = (Resolve-Path -LiteralPath $SourceRoot).Path
New-Item -ItemType Directory -Force -Path $OutputDirectory | Out-Null
$outputPath = (Resolve-Path -LiteralPath $OutputDirectory).Path
$archivePath = Join-Path $outputPath 'zenith-assets.zip'
$records = @()
foreach ($relative in $sources.files) {
    $path = Get-AssetPath $sourcePath $relative
    if (!(Test-Path -LiteralPath $path -PathType Leaf)) { throw "Required asset missing: $path" }
    $records += [ordered]@{ path=$relative; bytes=(Get-Item -LiteralPath $path).Length; sha256=(Get-FileHash -LiteralPath $path -Algorithm SHA256).Hash.ToLowerInvariant() }
}
$metadata = [ordered]@{ name='zenith-assets'; version=$Version; attributions=$sources.attributions }
$metadataBytes = [Text.UTF8Encoding]::new($false).GetBytes(($metadata | ConvertTo-Json -Depth 8) + "`n")
$sha = [Security.Cryptography.SHA256]::Create()
try { $metadataHash = [BitConverter]::ToString($sha.ComputeHash($metadataBytes)).Replace('-', '').ToLowerInvariant() } finally { $sha.Dispose() }
$records += [ordered]@{ path='content/zenith-assets.json'; bytes=$metadataBytes.Length; sha256=$metadataHash }
$stream = [IO.File]::Open($archivePath, [IO.FileMode]::Create, [IO.FileAccess]::Write)
try {
    $zip = [IO.Compression.ZipArchive]::new($stream, [IO.Compression.ZipArchiveMode]::Create, $true)
    try {
        foreach ($record in $records) {
            $entry = $zip.CreateEntry($record.path, [IO.Compression.CompressionLevel]::Optimal)
            $entry.LastWriteTime = [DateTimeOffset]::new(2026, 1, 1, 0, 0, 0, [TimeSpan]::Zero)
            $entryStream = $entry.Open()
            try {
                if ($record.path -eq 'content/zenith-assets.json') {
                    $entryStream.Write($metadataBytes, 0, $metadataBytes.Length)
                } else {
                    $inputStream = [IO.File]::OpenRead((Get-AssetPath $sourcePath $record.path))
                    try { $inputStream.CopyTo($entryStream) } finally { $inputStream.Dispose() }
                }
            } finally { $entryStream.Dispose() }
        }
    } finally { $zip.Dispose() }
} finally { $stream.Dispose() }
$lock = [ordered]@{
    schemaVersion=1; package=$sources.package; version=$Version; digest=$null
    archive=[ordered]@{ name='zenith-assets.zip'; bytes=(Get-Item -LiteralPath $archivePath).Length; sha256=(Get-FileHash -LiteralPath $archivePath -Algorithm SHA256).Hash.ToLowerInvariant() }
    files=$records
}
[IO.File]::WriteAllText((Join-Path $outputPath 'assets.lock.json'), ($lock | ConvertTo-Json -Depth 8) + "`n", [Text.UTF8Encoding]::new($false))
Write-Host "Packed $($records.Count) files into $archivePath ($($lock.archive.bytes) bytes)"
