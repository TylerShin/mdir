param(
    [switch]$Local,
    [string]$Repo = $env:MDIR_REPO,
    [string]$Version = 'latest',
    [string]$InstallDir = "$env:LOCALAPPDATA\mDir\bin",
    [switch]$PrintTarget
)
$ErrorActionPreference = 'Stop'
$arch = [System.Runtime.InteropServices.RuntimeInformation]::OSArchitecture.ToString()
$target = switch ($arch) {
    'X64' { 'x86_64-pc-windows-msvc' }
    'Arm64' { 'aarch64-pc-windows-msvc' }
    default { throw "Unsupported Windows architecture: $arch" }
}
if ($PrintTarget) { Write-Output $target; exit 0 }
$temp = Join-Path ([System.IO.Path]::GetTempPath()) ([Guid]::NewGuid().ToString())
New-Item -ItemType Directory -Path $temp | Out-Null
try {
    $download = Join-Path $temp 'mdir.exe'
    if ($Local) {
        $source = Join-Path $PSScriptRoot '..\target\release\mdir.exe'
        if (!(Test-Path -LiteralPath $source)) { throw 'Build first: mise run build' }
        Copy-Item -LiteralPath $source -Destination $download
    } else {
        if ($Repo -notmatch '^[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+$') { throw 'Supply -Repo OWNER/REPO for a published mDir release.' }
        if ($Version -eq 'latest') {
            $base = "https://github.com/$Repo/releases/latest/download"
        } elseif ($Version -match '^v[0-9][A-Za-z0-9._-]*$') {
            $base = "https://github.com/$Repo/releases/download/$Version"
        } else { throw 'Version must be latest or vVERSION' }
        $asset = "mdir-$target.exe"
        Invoke-WebRequest -Uri "$base/$asset" -OutFile $download
        $checksums = Join-Path $temp 'SHA256SUMS'
        Invoke-WebRequest -Uri "$base/SHA256SUMS" -OutFile $checksums
        $lines = @(Get-Content -LiteralPath $checksums | Where-Object { $_ -match ("^[a-fA-F0-9]{64}\s+" + [regex]::Escape($asset) + '$') })
        if ($lines.Count -ne 1) { throw 'Missing or ambiguous release checksum' }
        $expected = ($lines[0] -split '\s+')[0]
        if ((Get-FileHash -LiteralPath $download -Algorithm SHA256).Hash -ne $expected) { throw 'Checksum mismatch; nothing installed' }
    }
    New-Item -ItemType Directory -Path $InstallDir -Force | Out-Null
    $destination = Join-Path $InstallDir 'mdir.exe'
    if (Test-Path -LiteralPath $destination -PathType Container) { throw 'Install destination is a directory; nothing installed' }
    Copy-Item -LiteralPath $download -Destination $destination -Force
    Write-Output "Installed: $destination"
    Write-Output "Run: & '$destination'"
    Write-Output 'Add the install directory to your user PATH to use mdir anywhere.'
} finally {
    Remove-Item -LiteralPath $temp -Recurse -Force
}
