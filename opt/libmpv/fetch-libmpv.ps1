# Fetch libmpv-2.dll for Windows x64 (mpv-winbuild-cmake dev build).
# Usage: powershell -File fetch-libmpv.ps1
$ErrorActionPreference = "Stop"
$out = Join-Path $PSScriptRoot "libmpv-2.dll"
if (Test-Path $out) {
    Write-Host "libmpv-2.dll already present: $out"
    exit 0
}

$release = "20260828"
$commit = "git-182fa6ca49"
$url = "https://github.com/shinchiro/mpv-winbuild-cmake/releases/download/$release/mpv-dev-x86_64-$release-$commit.7z"
$archive = Join-Path $PSScriptRoot "mpv-dev.7z"

Write-Host "Downloading $url"
Invoke-WebRequest -Uri $url -OutFile $archive

# Extract just libmpv-2.dll + headers using 7-Zip (expected at Program Files\7-Zip)
$sevenZip = "C:\Program Files\7-Zip\7z.exe"
if (-not (Test-Path $sevenZip)) {
    throw "7-Zip not found at $sevenZip. Install it: winget install 7zip.7zip"
}
$tmp = Join-Path $PSScriptRoot "extracted"
if (Test-Path $tmp) { Remove-Item $tmp -Recurse -Force }
& $sevenZip x -y $archive "-o$tmp" | Out-Null
Copy-Item (Join-Path $tmp "libmpv-2.dll") $out -Force
Write-Host "Done: $out"
