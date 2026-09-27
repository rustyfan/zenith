param(
    [string]$Blender,
    [string]$Archive
)

$ErrorActionPreference = 'Stop'
$repo = (Resolve-Path -LiteralPath (Join-Path $PSScriptRoot '..')).Path
$cache = Join-Path $repo 'target/einar-source'
$output = Join-Path $repo 'content/mesh/einar/einar.bin'
$expectedHash = '3CBB5A1DD9FFD3CA39EDAB420A83EA7F7DB337B435A78EF27A0C87FF3E6977F7'
if (!$Blender) {
    $portable = Join-Path $repo 'target/tools/blender-3.6.23-windows-x64/blender.exe'
    if (Test-Path -LiteralPath $portable) { $Blender = $portable }
    else { $Blender = (Get-Command blender -ErrorAction Stop).Source }
}
if (!$Archive) {
    $Archive = Join-Path $repo 'target/einar-complete.zip'
    if (!(Test-Path -LiteralPath $Archive)) {
        New-Item -ItemType Directory -Force -Path (Split-Path $Archive) | Out-Null
        $temporary = $Archive + '.download'
        & curl.exe --location --fail --retry 3 --output $temporary 'https://studio.blender.org/download-source/files/41/419e83fb75f30b989adf7920dec7a21c/419e83fb75f30b989adf7920dec7a21c.zip'
        if ($LASTEXITCODE -ne 0) { throw 'Einar download failed' }
        if ((Get-FileHash -LiteralPath $temporary -Algorithm SHA256).Hash -ne $expectedHash) { throw 'Einar archive checksum mismatch' }
        Move-Item -LiteralPath $temporary -Destination $Archive
    }
}
if ((Get-FileHash -LiteralPath $Archive -Algorithm SHA256).Hash -ne $expectedHash) { throw 'Einar archive checksum mismatch' }
Expand-Archive -LiteralPath $Archive -DestinationPath $cache -Force
& $Blender --background --disable-autoexec (Join-Path $cache 'einar_release_v1.blend') --python-exit-code 1 --python (Join-Path $PSScriptRoot 'export-einar.py') -- --output $output
if ($LASTEXITCODE -ne 0 -or !(Test-Path -LiteralPath $output)) { throw 'Einar export failed' }
Copy-Item -LiteralPath (Join-Path $repo 'content/licenses/einar.txt') -Destination (Join-Path (Split-Path $output) 'LICENSE.txt')
Write-Host 'Run: cargo run -p zenith-sandbox --example bxdf_lab -- einar'
