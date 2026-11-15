# SPDX-FileCopyrightText: 2026 Daniil Klimov
# SPDX-License-Identifier: GPL-3.0-or-later
# Exercise the real finally block without installing into the current account.
$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest
$tokens = $null
$errors = $null
$ast = [System.Management.Automation.Language.Parser]::ParseFile(
    "$PSScriptRoot/test_windows_installer.ps1", [ref]$tokens, [ref]$errors)
if ($errors.Count) { throw "Installer test has parse errors: $errors" }
$statement = @($ast.EndBlock.Statements | Where-Object { $_ -is [System.Management.Automation.Language.TryStatementAst] })
if ($statement.Count -ne 1) { throw 'Expected one installer lifecycle try/finally block.' }
$cleanup = [scriptblock]::Create(($statement[0].Finally.Statements.Extent.Text -join "`n"))
function Invoke-CheckedProcess([string]$File, [string[]]$Arguments) {
    $script:calls++
    throw 'Simulated failed uninstall during cleanup'
}
foreach ($completed in @($true, $false)) {
    $work = Join-Path ([IO.Path]::GetTempPath()) "hopea-cleanup-$([guid]::NewGuid())"
    try {
        $destination = Join-Path $work 'Hopea путь with spaces'
        $dataDir = Join-Path $work 'data'
        New-Item -ItemType Directory $destination, $dataDir | Out-Null
        $sentinel = Join-Path $dataDir 'sentinel.txt'
        'fixture' | Set-Content -LiteralPath $sentinel
        'pending self-delete' | Set-Content -LiteralPath "$destination/unins000.exe"
        $dataExisted = $true
        $uninstallCompleted = $completed
        $script:calls = 0
        . $cleanup
        $expectedCalls = if ($completed) { 0 } else { 1 }
        if ($script:calls -ne $expectedCalls) { throw "Unexpected uninstall calls: $script:calls" }
        if (Test-Path -LiteralPath $work) { throw 'Cleanup did not remove the fixture.' }
    } finally {
        if (Test-Path -LiteralPath $work) { Remove-Item -LiteralPath $work -Recurse -Force }
    }
}
Write-Host 'Installer cleanup: completed uninstall is not repeated; failure does not abort cleanup.'
