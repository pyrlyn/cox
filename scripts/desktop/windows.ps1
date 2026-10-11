# Copyright (c) 2026 Ivan Tugay
# SPDX-License-Identifier: GPL-3.0-or-later
# Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

# Windows counterpart of scripts/desktop/app.sh (T58.3). The machine-wide dotnet host can
# still be 9 while SDK 10 lives in the user install, so this script puts that host first.
#
#   just desktop-windows-open   Visual Studio, the way Xcode opens Cox.xcodeproj
#   just desktop-windows         Debug build of Cox.sln
#   just desktop-windows-test    Cox.Model layout test
#   just desktop-windows-run     build, then the unpackaged Cox.App window

param(
    [Parameter(Position = 0)]
    [ValidateSet('open', 'build', 'test', 'run')]
    [string]$Command = 'build'
)

$ErrorActionPreference = 'Stop'
$root = (Resolve-Path (Join-Path $PSScriptRoot '..\..')).Path
$win = Join-Path $root 'desktop\windows'
$exe = Join-Path $win 'App\bin\x64\Debug\net10.0-windows10.0.26100.0\Cox.App.exe'

function Use-DotNet10 {
    $local = Join-Path $env:LOCALAPPDATA 'Microsoft\dotnet'
    $hostExe = Join-Path $local 'dotnet.exe'
    if (Test-Path $hostExe) {
        $env:DOTNET_ROOT = $local
        if (-not $env:PATH.StartsWith($local)) {
            $env:PATH = "$local;$env:PATH"
        }
    }
}

function Invoke-DotNet {
    param([Parameter(ValueFromRemainingArguments = $true)][string[]]$Args)
    Use-DotNet10
    Push-Location $win
    try {
        & dotnet @Args
        if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
    }
    finally {
        Pop-Location
    }
}

function Stop-BuiltApp {
    Get-Process -Name 'Cox.App' -ErrorAction SilentlyContinue | ForEach-Object {
        $path = $null
        try { $path = $_.Path } catch { $path = $null }
        if ($path -eq $exe) {
            Stop-Process -Id $_.Id -Force
        }
    }
}

switch ($Command) {
    'open' {
        $vswhere = Join-Path ${env:ProgramFiles(x86)} 'Microsoft Visual Studio\Installer\vswhere.exe'
        if (-not (Test-Path $vswhere)) { throw "Visual Studio installer not found: $vswhere" }
        $devenv = & $vswhere -latest -prerelease -find 'Common7\IDE\devenv.exe' | Select-Object -First 1
        if (-not $devenv) { throw 'devenv.exe was not found' }
        Start-Process -FilePath $devenv -ArgumentList (Join-Path $win 'Cox.sln')
        Write-Output $devenv
    }
    'build' {
        Invoke-DotNet build Cox.sln -c Debug
    }
    'test' {
        Invoke-DotNet test --project Cox.Tests -c Debug
    }
    'run' {
        Stop-BuiltApp
        Invoke-DotNet build Cox.sln -c Debug
        if (-not (Test-Path $exe)) { throw "no $exe" }
        $start = New-Object System.Diagnostics.ProcessStartInfo
        $start.FileName = $exe
        $start.WorkingDirectory = Split-Path $exe
        $start.UseShellExecute = $false
        if ($env:DOTNET_ROOT) { $start.EnvironmentVariables['DOTNET_ROOT'] = $env:DOTNET_ROOT }
        $start.EnvironmentVariables['PATH'] = $env:PATH
        $proc = [System.Diagnostics.Process]::Start($start)
        Write-Output "pid=$($proc.Id) $exe"
    }
}
