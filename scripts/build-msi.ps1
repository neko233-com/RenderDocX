#requires -Version 7.0
<#
  Build the RenderDocX MSI installer with WiX v4/v5.
  Uses RenderDocX's own package definition (installer/RenderDocX.wxs),
  not the upstream RenderDoc installer under util/installer.
#>
[CmdletBinding()]
param(
  [string]$Configuration = 'release',
  [switch]$SkipBuild
)

$ErrorActionPreference = 'Stop'
$repo = Split-Path -Parent $PSScriptRoot

# Locate wix.exe (dotnet tool on PATH, or the well-known global-tools path).
$wix = (Get-Command wix -ErrorAction SilentlyContinue).Source
if (-not $wix) {
  $candidate = Join-Path $HOME '.dotnet/tools/wix.exe'
  if (Test-Path -LiteralPath $candidate) { $wix = $candidate }
}
if (-not $wix) { throw 'WiX v4/v5 tool (wix.exe) not found. Install with: dotnet tool install -g wix' }

$versionLine = Select-String -Path (Join-Path $repo 'Cargo.toml') -Pattern '^version\s*=\s*"([^"]+)"' | Select-Object -First 1
if (-not $versionLine) { throw 'Could not read package version from Cargo.toml' }
$semver = $versionLine.Matches[0].Groups[1].Value
$msiVersion = if ($semver -match '^(\d+\.\d+\.\d+)') { $Matches[1] + '.0' } else { '0.1.0.0' }

$targetDir = Join-Path $repo "target/$Configuration"
if (-not $SkipBuild) {
  Write-Host "Building RenderDocX ($Configuration)..."
  Push-Location $repo
  try { cargo build --bins --configuration $Configuration } finally { Pop-Location }
}

foreach ($exe in @('renderdocx.exe', 'renderdocx-cli.exe')) {
  $path = Join-Path $targetDir $exe
  if (-not (Test-Path -LiteralPath $path)) { throw "Build artifact missing: $path" }
}

$outDir = Join-Path $repo 'target/msi'
New-Item -ItemType Directory -Force -Path $outDir | Out-Null
$msi = Join-Path $outDir "RenderDocX-$semver-x64.msi"

Write-Host "Building MSI version $msiVersion with $wix ..."
& $wix build `
  -arch x64 `
  -d "Version=$msiVersion" `
  -d "BinDir=$targetDir" `
  -d "RepoDir=$repo" `
  -out $msi `
  (Join-Path $repo 'installer/RenderDocX.wxs')
if ($LASTEXITCODE -ne 0) { throw "WiX build failed with exit code $LASTEXITCODE" }

$hash = (Get-FileHash -LiteralPath $msi -Algorithm SHA256).Hash
Write-Host "MSI created: $msi"
Write-Host "Size: $([math]::Round((Get-Item $msi).Length / 1MB, 2)) MB"
Write-Host "SHA256: $hash"
