# SPDX-FileCopyrightText: 2026 Daniil Klimov
# SPDX-License-Identifier: GPL-3.0-or-later
# Integration check for an isolated Windows CI account, never the user's profile.
[CmdletBinding()]
param([string]$InstallerPath)
$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest
if (-not $IsWindows -or $env:CI -ne 'true') { throw 'Run only in a disposable Windows CI account.' }
Add-Type -Path "$PSScriptRoot/windows-shell-link.cs"
$root = (Resolve-Path "$PSScriptRoot/..").Path
if (-not $InstallerPath) {
    $installers = @(Get-ChildItem "$root/dist/*-windows-x86_64-setup.exe")
    if ($installers.Count -ne 1) { throw 'Expected exactly one Windows installer.' }
    $InstallerPath = $installers[0].FullName
}
$InstallerPath = (Resolve-Path -LiteralPath $InstallerPath).Path
$registry = 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Uninstall\io.hopea.Hopea_is1'
$shortcut = Join-Path ([Environment]::GetFolderPath('Programs')) 'Hopea.lnk'
if ((Test-Path $registry) -or (Test-Path -LiteralPath $shortcut)) {
    throw 'Hopea is already installed in this account; refusing to overwrite it.'
}
$work = Join-Path ([IO.Path]::GetTempPath()) "hopea-installer-$([guid]::NewGuid())"
$destination = Join-Path $work 'Hopea путь with spaces'
$logs = Join-Path $root '_windows/setup/test-logs'
New-Item -ItemType Directory -Force $work, $logs | Out-Null
$dataDir = Join-Path $env:LOCALAPPDATA 'hopea'
$dataExisted = Test-Path -LiteralPath $dataDir
New-Item -ItemType Directory -Force $dataDir | Out-Null
$sentinel = Join-Path $dataDir "installer-test-$([guid]::NewGuid()).txt"
'Preserve the user library' | Set-Content -LiteralPath $sentinel
$expectedData = (Get-FileHash -LiteralPath $sentinel).Hash
$uninstallCompleted = $false

function Invoke-CheckedProcess([string]$File, [string[]]$Arguments) {
    $process = Start-Process -FilePath $File -ArgumentList $Arguments -PassThru
    try {
        if (-not $process.WaitForExit(180000)) {
            $process.Kill()
            throw "Process timed out: $File"
        }
        if ($process.ExitCode -ne 0) { throw "Process failed ($($process.ExitCode)): $File" }
    } finally { $process.Dispose() }
}
function Assert-LibraryPreserved {
    if ((Get-FileHash -LiteralPath $sentinel).Hash -ne $expectedData) { throw 'User data changed.' }
}
# Shell links may expose an 8.3 path for names outside the ANSI code page.
# Compare existing long Unicode paths, including extended-length path syntax.
Add-Type @'
using System;
using System.ComponentModel;
using System.Runtime.InteropServices;
using System.Text;
public static class InstallerPaths {
    [DllImport("kernel32.dll", CharSet = CharSet.Unicode, SetLastError = true)]
    private static extern uint GetLongPathNameW(string path, StringBuilder buffer, uint size);
    public static string LongPath(string path) {
        var buffer = new StringBuilder(32768);
        uint length = GetLongPathNameW(path, buffer, (uint)buffer.Capacity);
        if (length == 0) throw new Win32Exception(Marshal.GetLastWin32Error());
        if (length >= buffer.Capacity) throw new InvalidOperationException("Path exceeds buffer size.");
        string result = buffer.ToString();
        if (result.StartsWith(@"\\?\UNC\", StringComparison.OrdinalIgnoreCase))
            return @"\\" + result.Substring(8);
        if (result.StartsWith(@"\\?\", StringComparison.Ordinal)) return result.Substring(4);
        return result;
    }
}
'@
function Assert-SameWindowsPath([string]$Actual, [string]$Expected, [string]$Description) {
    if ([string]::IsNullOrWhiteSpace($Actual) -or -not (Test-Path -LiteralPath $Actual)) {
        throw "${Description}: missing path '$Actual'; expected '$Expected'."
    }
    $actualPath = [InstallerPaths]::LongPath([IO.Path]::GetFullPath($Actual)).TrimEnd('\')
    $expectedPath = [InstallerPaths]::LongPath([IO.Path]::GetFullPath($Expected)).TrimEnd('\')
    if (-not [string]::Equals($actualPath, $expectedPath, [StringComparison]::OrdinalIgnoreCase)) {
        throw "${Description}: got '$Actual' ('$actualPath'); expected '$Expected' ('$expectedPath')."
    }
}
try {
    $quiet = @('/VERYSILENT', '/SUPPRESSMSGBOXES', '/NORESTART', '/SP-')
    Invoke-CheckedProcess $InstallerPath ($quiet + @("/DIR=`"$destination`"", "/LOG=`"$logs/install.log`""))
    if (-not (Test-Path $registry)) { throw 'Uninstall registration is missing.' }
    if (-not (Test-Path -LiteralPath $shortcut)) { throw 'Start menu shortcut is missing.' }
    Copy-Item -LiteralPath $shortcut -Destination "$logs/Hopea.lnk"
    $link = [WindowsShellLink]::Read($shortcut)
    @{
        Destination = $destination
        Target = $link.TargetPath
        Icon = $link.IconPath
        IconIndex = $link.IconIndex
        WorkingDirectory = $link.WorkingDirectory
    } | ConvertTo-Json | Set-Content -LiteralPath "$logs/shortcut.log" -Encoding utf8
    Assert-SameWindowsPath $link.TargetPath "$destination\bin\hopea.exe" 'Shortcut target'
    Assert-SameWindowsPath $link.IconPath "$destination\hopea.ico" 'Shortcut icon'
    Assert-SameWindowsPath $link.WorkingDirectory $destination 'Shortcut working directory'
    Assert-LibraryPreserved

    # No MSYS2, developer Python, or GTK DLLs may leak in from the runner's PATH.
    $oldPath = $env:PATH
    $xdgKeys = @('XDG_DATA_HOME', 'XDG_CACHE_HOME', 'XDG_CONFIG_HOME')
    $oldXdg = @{}
    try {
        $env:PATH = "$env:SystemRoot\System32;$env:SystemRoot"
        foreach ($key in $xdgKeys) {
            $oldXdg[$key] = [Environment]::GetEnvironmentVariable($key)
            [Environment]::SetEnvironmentVariable($key, (Join-Path $work $key))
        }
        Invoke-CheckedProcess "$destination/bin/hopea.exe" @('--help')
        Invoke-CheckedProcess "$destination/lib/hopea-import/python.exe" @('-I', '-c', '"import vk_api, bs4, yandex_music, ytmusicapi, yt_dlp, Cryptodome, spotipy, ssl, sqlite3"')
    } finally {
        $env:PATH = $oldPath
        foreach ($key in $xdgKeys) { [Environment]::SetEnvironmentVariable($key, $oldXdg[$key]) }
    }

    # Install over the existing copy without /DIR: stable AppId must reuse it.
    'A file not owned by the installer' | Set-Content "$destination/keep-user-file.txt"
    Invoke-CheckedProcess $InstallerPath ($quiet + @("/LOG=`"$logs/reinstall.log`""))
    $location = (Get-ItemProperty $registry).InstallLocation.TrimEnd('\')
    Assert-SameWindowsPath $location $destination 'Reinstall directory'
    $uninstallers = @(Get-ChildItem "$destination/unins*.exe")
    if ($uninstallers.Count -ne 1) { throw 'Reinstall created duplicate uninstallers.' }
    Assert-LibraryPreserved
    Invoke-CheckedProcess $uninstallers[0].FullName ($quiet + @("/LOG=`"$logs/uninstall.log`""))
    $uninstallCompleted = $true
    if (Test-Path "$destination/bin/hopea.exe") { throw 'Uninstaller left the player installed.' }
    if ((Test-Path $registry) -or (Test-Path -LiteralPath $shortcut)) { throw 'Uninstall registration or shortcut remained.' }
    if (-not (Test-Path "$destination/keep-user-file.txt")) { throw 'Uninstaller deleted an unowned file.' }
    Assert-LibraryPreserved
    Write-Host 'Installer: install, relocated runtime, reinstall and uninstall checks passed.'
} finally {
    # Best effort cleanup of this test's installation if an assertion failed.
    $uninstaller = Get-ChildItem "$destination/unins*.exe" -ErrorAction SilentlyContinue | Select-Object -First 1
    # Inno may leave its executable briefly while its self-delete helper runs.
    # A successful uninstall must never be invoked a second time.
    if (-not $uninstallCompleted -and $uninstaller) {
        try {
            Invoke-CheckedProcess $uninstaller.FullName @('/VERYSILENT', '/SUPPRESSMSGBOXES', '/NORESTART')
        } catch { Write-Warning "Installer cleanup failed: $_" }
    }
    Remove-Item -LiteralPath $sentinel -Force -ErrorAction SilentlyContinue
    if (-not $dataExisted -and @(Get-ChildItem -LiteralPath $dataDir -Force).Count -eq 0) {
        Remove-Item -LiteralPath $dataDir
    }
    # Allow the self-delete helper to release the executable. Cleanup must not
    # replace an earlier assertion failure with a secondary removal error.
    for ($attempt = 0; $attempt -lt 20; $attempt++) {
        try {
            if (Test-Path -LiteralPath $work) { Remove-Item -LiteralPath $work -Recurse -Force }
            break
        } catch {
            if ($attempt -eq 19) { Write-Warning "Temporary directory cleanup failed: $_" }
            else { Start-Sleep -Milliseconds 250 }
        }
    }
}
