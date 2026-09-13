$ErrorActionPreference = 'Stop'

function Test-AssetHash([string]$Path, [string]$Sha256) {
    return (Test-Path -LiteralPath $Path -PathType Leaf) -and
        (Get-FileHash -LiteralPath $Path -Algorithm SHA256).Hash -eq $Sha256
}

function Get-AssetDownload([string]$Uri, [string]$Path, [string]$Sha256) {
    $partial = $Path + '.' + [guid]::NewGuid().ToString('N') + '.partial'
    [Net.ServicePointManager]::SecurityProtocol = [Net.ServicePointManager]::SecurityProtocol -bor [Net.SecurityProtocolType]::Tls12
    try {
        for ($attempt = 1; $attempt -le 3; $attempt++) {
            try {
                $ProgressPreference = 'SilentlyContinue'
                Invoke-WebRequest -UseBasicParsing -Uri $Uri -OutFile $partial -TimeoutSec 300
                if (!(Test-AssetHash $partial $Sha256)) { throw "Checksum mismatch: $Uri" }
                Move-Item -LiteralPath $partial -Destination $Path -Force
                return
            } catch {
                if ($attempt -eq 3) { throw }
            }
        }
    } finally {
        if (Test-Path -LiteralPath $partial) { Remove-Item -LiteralPath $partial -Force }
    }
}

function Get-AssetOras([string]$Root, [switch]$Offline) {
    $directory = Join-Path $Root 'target/tools/oras-1.3.0'
    $executable = Join-Path $directory 'oras.exe'
    $executableHash = '6ed8d5b18a88a2bea32f4a562bf4fb646931590de9152ed55b646a43ed970247'
    if (Test-AssetHash $executable $executableHash) { return $executable }
    if ($Offline) { throw 'ORAS is not cached. Run Setup.bat once with internet access.' }
    New-Item -ItemType Directory -Force -Path $directory | Out-Null
    $archive = Join-Path $directory 'oras_1.3.0_windows_amd64.zip'
    $archiveHash = 'b050e93aa0dc7a79a61fa8e4074dfa302c41d4af01b634fe393c5dd687536aee'
    if (!(Test-AssetHash $archive $archiveHash)) {
        Write-Host 'Downloading ORAS 1.3.0...'
        Get-AssetDownload 'https://github.com/oras-project/oras/releases/download/v1.3.0/oras_1.3.0_windows_amd64.zip' $archive $archiveHash
    }
    Expand-Archive -LiteralPath $archive -DestinationPath $directory -Force
    if (!(Test-AssetHash $executable $executableHash)) { throw 'ORAS executable checksum mismatch' }
    return $executable
}

function Get-AssetPath([string]$Root, [string]$Relative) {
    if ($Relative -notmatch '^content/(mesh/|texture/|zenith-assets\.json$)' -or
        $Relative -match '(^|/)\.\.?(/|$)|[\\:]') { throw "Invalid asset path: $Relative" }
    $rootPath = [IO.Path]::GetFullPath($Root).TrimEnd('\', '/')
    $path = [IO.Path]::GetFullPath((Join-Path $rootPath $Relative))
    if (!$path.StartsWith($rootPath + [IO.Path]::DirectorySeparatorChar, [StringComparison]::OrdinalIgnoreCase)) {
        throw "Asset path escapes destination: $Relative"
    }
    $parent = $path
    while ($parent -and $parent.Length -ge $rootPath.Length) {
        if ((Test-Path -LiteralPath $parent) -and
            ((Get-Item -LiteralPath $parent -Force).Attributes -band [IO.FileAttributes]::ReparsePoint)) {
            throw "Asset path contains a symbolic link or junction: $parent"
        }
        $parent = [IO.Path]::GetDirectoryName($parent)
    }
    return $path
}
