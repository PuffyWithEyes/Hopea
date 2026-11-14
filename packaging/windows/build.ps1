# SPDX-FileCopyrightText: 2026 Daniil Klimov
# SPDX-License-Identifier: GPL-3.0-or-later
[CmdletBinding()]
param(
    [string]$BundlePath,
    [string]$Repository = $env:HOPEA_REPOSITORY,
    [string]$IsccPath
)
$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest
if (-not $IsWindows) { throw 'Run the installer build in PowerShell 7 on Windows.' }
$root = (Resolve-Path (Join-Path $PSScriptRoot '../..')).Path
$versionMatch = [regex]::Match((Get-Content "$root/meson.build" -Raw), "version: '([0-9]+(?:\.[0-9]+){1,3})'")
if (-not $versionMatch.Success) { throw 'Invalid Meson release version.' }
$version = $versionMatch.Groups[1].Value
if (@($version.Split('.') | Where-Object { [long]$_ -gt 65535 }).Count -gt 0) {
    throw 'Windows version components must fit in 16 bits.'
}
if ($env:HOPEA_VERSION -and $env:HOPEA_VERSION -ne $version) { throw 'HOPEA_VERSION differs from Meson.' }
if ($Repository -notmatch '^[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+$') { throw 'Set HOPEA_REPOSITORY to owner/repository.' }
if (-not $BundlePath) { $BundlePath = Join-Path $root '_windows/Hopea' }
$bundle = (Resolve-Path -LiteralPath $BundlePath).Path
$required = @('bin/hopea.exe', 'bin/hopea-spotify.exe', 'bin/ffmpeg.exe',
    'bin/ffprobe.exe', 'lib/hopea-import/python.exe',
    'share/glib-2.0/schemas/gschemas.compiled', 'lib/gstreamer-1.0', 'LICENSES', 'ADDITIONAL-TERMS.md')
foreach ($file in $required) {
    if (-not (Test-Path -LiteralPath (Join-Path $bundle $file))) { throw "Incomplete Windows bundle: $file" }
}
# Do not silently package symlinks/junctions pointing outside the staged runtime.
if (Get-ChildItem -LiteralPath $bundle -Recurse -Force | Where-Object { $_.Attributes -band [IO.FileAttributes]::ReparsePoint }) {
    throw 'Windows bundle contains a reparse point; stage ordinary files instead.'
}
$stage = Join-Path $root '_windows/setup'
$output = Join-Path $root 'dist'
New-Item -ItemType Directory -Force $stage, $output | Out-Null

# Put the existing PNG sizes into an ICO container without changing pixels.
$sizes = @(16, 24, 32, 48, 64, 128, 256)
$images = @($sizes | ForEach-Object { ,([IO.File]::ReadAllBytes("$root/data/icons/cat-kz/$_.png")) })
$icon = Join-Path $stage 'hopea.ico'
$writer = [IO.BinaryWriter]::new([IO.File]::Create($icon))
try {
    $writer.Write([uint16]0); $writer.Write([uint16]1); $writer.Write([uint16]$sizes.Count)
    $offset = 6 + 16 * $sizes.Count
    for ($i = 0; $i -lt $sizes.Count; $i++) {
        $dimension = if ($sizes[$i] -eq 256) { 0 } else { $sizes[$i] }
        $writer.Write([byte]$dimension); $writer.Write([byte]$dimension)
        $writer.Write([byte]0); $writer.Write([byte]0)
        $writer.Write([uint16]1); $writer.Write([uint16]32)
        $writer.Write([uint32]$images[$i].Length); $writer.Write([uint32]$offset)
        $offset += $images[$i].Length
    }
    foreach ($png in $images) { $writer.Write([byte[]]$png) }
} finally { $writer.Dispose() }

if (-not $IsccPath) {
    # Upstream release, pinned hash: no unversioned installer or third-party action.
    $download = Join-Path $stage 'innosetup-7.1.1-x64.exe'
    $expected = '8a5a6711c3766a6e5fb74f280ba0e3551e719ce94538d901fdbd1f8f9920b3f0'
    if (-not (Test-Path -LiteralPath $download)) {
        Invoke-WebRequest 'https://github.com/jrsoftware/issrc/releases/download/is-7_1_1/innosetup-7.1.1-x64.exe' -OutFile $download
    }
    if ((Get-FileHash -Algorithm SHA256 -LiteralPath $download).Hash -ne $expected) {
        throw 'Inno Setup checksum mismatch; compiler was not executed.'
    }
    $compilerDir = Join-Path $stage 'inno'
    $install = Start-Process -FilePath $download -Wait -PassThru -ArgumentList @(
        '/VERYSILENT', '/SUPPRESSMSGBOXES', '/NORESTART', '/SP-', '/CURRENTUSER',
        "/DIR=`"$compilerDir`"", "/LOG=`"$stage/compiler-install.log`"")
    if ($install.ExitCode -ne 0) { throw "Inno Setup installation failed: $($install.ExitCode)" }
    $IsccPath = Join-Path $compilerDir 'ISCC.exe'
}
$IsccPath = (Resolve-Path -LiteralPath $IsccPath).Path
$compilerVersion = (Get-Item -LiteralPath $IsccPath).VersionInfo.FileVersion
Write-Host "Compiler: $IsccPath ($compilerVersion)"
& $IsccPath "/DAppVersion=$version" "/DBundleDir=$bundle" "/DOutputPath=$output" "/DIconPath=$icon" "/DRepository=$Repository" "$PSScriptRoot/hopea.iss"
if ($LASTEXITCODE -ne 0) { throw "Installer compilation failed: $LASTEXITCODE" }
$installer = Join-Path $output "hopea-$version-windows-x86_64-setup.exe"
if (-not (Test-Path -LiteralPath $installer)) { throw 'Compiler did not produce the installer.' }
$digest = (Get-FileHash -Algorithm SHA256 -LiteralPath $installer).Hash.ToLowerInvariant()
"$digest  $([IO.Path]::GetFileName($installer))" | Set-Content -Encoding utf8NoBOM "$installer.sha256"
Write-Host "Built $installer"
