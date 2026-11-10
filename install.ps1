# Installs Switchyard on Windows.
#
#   irm https://raw.githubusercontent.com/NathanielInman/switchyard/main/install.ps1 | iex
#
# Downloads the latest release into %LOCALAPPDATA%\Programs\switchyard, adds
# it to your PATH and runs `yard setup`, which starts the tray app at login.
# Set $env:YARD_VERSION = "v0.2.0" first to pick a release.

$ErrorActionPreference = "Stop"
$repo = "NathanielInman/switchyard"
$dir = if ($env:YARD_INSTALL_DIR) { $env:YARD_INSTALL_DIR } else { Join-Path $env:LOCALAPPDATA "Programs\switchyard" }
$arch = if ($env:PROCESSOR_ARCHITECTURE -eq "ARM64") { "aarch64" } else { "x86_64" }
$target = "$arch-pc-windows-msvc"
$version = if ($env:YARD_VERSION) { $env:YARD_VERSION } else { "latest" }
$url = if ($version -eq "latest") {
    "https://github.com/$repo/releases/latest/download/switchyard-$target.zip"
} else {
    "https://github.com/$repo/releases/download/$version/switchyard-$target.zip"
}

Write-Host "==> downloading $url" -ForegroundColor Cyan
$tmp = Join-Path ([IO.Path]::GetTempPath()) ("switchyard-" + [guid]::NewGuid())
New-Item -ItemType Directory -Path $tmp | Out-Null
try {
    $zip = Join-Path $tmp "switchyard.zip"
    Invoke-WebRequest -Uri $url -OutFile $zip -UseBasicParsing
    Expand-Archive -Path $zip -DestinationPath $tmp -Force

    # A running tray app holds its exe open; stop it so the new one can land.
    Get-Process yard-tray -ErrorAction SilentlyContinue | Stop-Process -Force
    New-Item -ItemType Directory -Path $dir -Force | Out-Null
    foreach ($b in "yard.exe", "yard-tray.exe") {
        Copy-Item (Join-Path $tmp $b) (Join-Path $dir $b) -Force
    }
} finally {
    Remove-Item -Recurse -Force $tmp -ErrorAction SilentlyContinue
}
Write-Host "==> installed yard and yard-tray to $dir" -ForegroundColor Cyan

$userPath = [Environment]::GetEnvironmentVariable("Path", "User")
if (-not ($userPath -split ";" | Where-Object { $_ -eq $dir })) {
    [Environment]::SetEnvironmentVariable("Path", "$userPath;$dir", "User")
    $env:Path = "$env:Path;$dir"
    Write-Host "==> added $dir to your PATH (new terminals pick it up)" -ForegroundColor Cyan
}

& (Join-Path $dir "yard.exe") setup
