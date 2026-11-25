<#
.SYNOPSIS
  Builds Rustle in release mode and packages it into .\dist.

.DESCRIPTION
  Always produces   dist\Rustle-<version>-portable.zip  (+ a .sha256 file).
  Also produces     dist\Rustle-<version>-setup.exe     when Inno Setup 6 (ISCC.exe) is installed.

  Optional code signing: set RUSTLE_SIGN_PFX to a .pfx file and RUSTLE_SIGN_PASSWORD to its
  password. Without them the binaries are left unsigned (Windows SmartScreen may warn).

.EXAMPLE
  .\scripts\package.ps1
  .\scripts\package.ps1 -SkipBuild
#>
param([switch]$SkipBuild)

$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot
Set-Location $root

$version = (Select-String -Path 'Cargo.toml' -Pattern '^version = "(.+)"').Matches[0].Groups[1].Value
Write-Host "Rustle $version"

if (-not $SkipBuild) {
    cargo build --release -p rustle-app
    if ($LASTEXITCODE -ne 0) { throw 'cargo build failed' }
}

$exe = Join-Path $root 'target\release\rustle.exe'
if (-not (Test-Path $exe)) { throw "Missing $exe - build first." }

function Find-SignTool {
    $c = Get-Command signtool.exe -ErrorAction SilentlyContinue
    if ($c) { return $c.Source }
    Get-ChildItem "${env:ProgramFiles(x86)}\Windows Kits\10\bin\*\x64\signtool.exe" -ErrorAction SilentlyContinue |
        Sort-Object FullName -Descending | Select-Object -First 1 -ExpandProperty FullName
}

function Invoke-Sign($file) {
    if (-not $env:RUSTLE_SIGN_PFX) { return }
    $tool = Find-SignTool
    if (-not $tool) { Write-Warning 'signtool.exe not found - skipping signing'; return }
    & $tool sign /f $env:RUSTLE_SIGN_PFX /p $env:RUSTLE_SIGN_PASSWORD /fd SHA256 `
        /tr http://timestamp.digicert.com /td SHA256 $file
    if ($LASTEXITCODE -ne 0) { throw "signing failed for $file" }
}

Invoke-Sign $exe

$dist = Join-Path $root 'dist'
New-Item -ItemType Directory -Force $dist | Out-Null

# --- portable zip ---------------------------------------------------------------------------
$stage = Join-Path $dist "Rustle-$version"
if (Test-Path $stage) { Remove-Item -Recurse -Force $stage }
New-Item -ItemType Directory $stage | Out-Null
Copy-Item $exe $stage
Copy-Item (Join-Path $root 'LICENSE') $stage
Copy-Item (Join-Path $root 'README.md') $stage
$zip = Join-Path $dist "Rustle-$version-portable.zip"
if (Test-Path $zip) { Remove-Item -Force $zip }
Compress-Archive -Path (Join-Path $stage '*') -DestinationPath $zip
Remove-Item -Recurse -Force $stage
Write-Host "Created $zip"

# --- installer (optional) -----------------------------------------------------------------
$iscc = (Get-Command ISCC.exe -ErrorAction SilentlyContinue).Source
if (-not $iscc) {
    $iscc = @("${env:ProgramFiles(x86)}\Inno Setup 6\ISCC.exe", "$env:ProgramFiles\Inno Setup 6\ISCC.exe",
        "$env:LOCALAPPDATA\Programs\Inno Setup 6\ISCC.exe") | Where-Object { Test-Path $_ } | Select-Object -First 1
}
if ($iscc) {
    & $iscc "/DAppVersion=$version" (Join-Path $root 'installer\rustle.iss')
    if ($LASTEXITCODE -ne 0) { throw 'Inno Setup failed' }
    $setup = Join-Path $dist "Rustle-$version-setup.exe"
    Invoke-Sign $setup
    Write-Host "Created $setup"
} else {
    Write-Warning 'Inno Setup not found - skipped the installer. Install it from https://jrsoftware.org/isinfo.php'
}

# --- checksums -----------------------------------------------------------------------------
Get-ChildItem $dist -File | Where-Object { $_.Extension -in '.zip', '.exe' } | ForEach-Object {
    $h = (Get-FileHash $_.FullName -Algorithm SHA256).Hash.ToLower()
    "$h  $($_.Name)" | Set-Content -Encoding ascii "$($_.FullName).sha256"
}
Write-Host 'Done.'
