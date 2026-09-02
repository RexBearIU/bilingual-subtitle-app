# Fetch the uv release the installer bundles.
#
# `src-tauri/binaries/` is gitignored — a 46 MB binary does not belong in the
# history — so a fresh clone has to pull it before `npm run tauri build`, or
# the bundle ships without it and first-run setup can only tell the user to do
# it by hand.
#
# Pinned rather than "latest": the installer's job is to reproduce the
# environment bench/ was measured against, and that starts with the resolver.

$ErrorActionPreference = "Stop"
$version = "0.12.2"
$dest    = Join-Path $PSScriptRoot "..\src-tauri\binaries"
$exe     = Join-Path $dest "uv.exe"

if (Test-Path $exe) {
    $have = (& $exe --version) -replace '^uv (\S+).*', '$1'
    if ($have -eq $version) { Write-Host "uv $version already present"; exit 0 }
    Write-Host "replacing uv $have with $version"
}

New-Item -ItemType Directory -Force -Path $dest | Out-Null
$url = "https://github.com/astral-sh/uv/releases/download/$version/uv-x86_64-pc-windows-msvc.zip"
$tmp = Join-Path $env:TEMP "uv-$version.zip"

Write-Host "downloading $url"
Invoke-WebRequest -Uri $url -OutFile $tmp
Expand-Archive -Path $tmp -DestinationPath $dest -Force
Remove-Item $tmp

& $exe --version
