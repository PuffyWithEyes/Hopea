# SPDX-FileCopyrightText: 2026 Daniil Klimov
# SPDX-License-Identifier: GPL-3.0-or-later
$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest
if (-not $IsWindows) { throw 'This test requires Windows Shell COM.' }
Add-Type -Path "$PSScriptRoot/windows-shell-link.cs"
$work = Join-Path ([IO.Path]::GetTempPath()) "hopea-shortcuts-$([guid]::NewGuid())"
try {
    $directory = Join-Path $work 'Hopea путь 日本語 with spaces'
    New-Item -ItemType Directory -Path $directory | Out-Null
    $target = Join-Path $directory 'player.exe'
    Copy-Item -LiteralPath "$env:SystemRoot/System32/cmd.exe" -Destination $target
    $shortcut = Join-Path $work 'ярлык.lnk'
    [WindowsShellLink]::CreateFixture($shortcut, $target, $directory, $target, 0)
    $before = (Get-FileHash -LiteralPath $shortcut).Hash
    $link = [WindowsShellLink]::Read($shortcut)
    if ($link.TargetPath -cne $target -or $link.WorkingDirectory -cne $directory -or
        $link.IconPath -cne $target -or $link.IconIndex -ne 0) {
        throw "Unicode paths did not survive reading: $($link | ConvertTo-Json -Compress)"
    }
    if (-not (Test-Path -LiteralPath $link.TargetPath)) { throw 'Target is missing.' }
    if ((Get-FileHash -LiteralPath $shortcut).Hash -ne $before) { throw 'Reading changed the shortcut.' }

    # The reader must preserve a genuinely wrong target, never substitute the
    # expected path or repair the link to make the installer assertion pass.
    $missing = Join-Path $directory 'missing.exe'
    [WindowsShellLink]::CreateFixture($shortcut, $missing, $directory, $target, 0)
    $broken = [WindowsShellLink]::Read($shortcut)
    if ($broken.TargetPath -cne $missing -or (Test-Path -LiteralPath $broken.TargetPath)) {
        throw 'Missing target was changed or concealed.'
    }
    Write-Host 'Unicode shortcut paths, missing targets and read-only access: passed.'
} finally {
    Remove-Item -LiteralPath $work -Recurse -Force -ErrorAction SilentlyContinue
}
