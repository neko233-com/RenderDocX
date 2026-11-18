#requires -Version 7.0
<#
  Build a deliberately mismatched replay bridge: it is compiled against the real
  RenderDoc core library but records a fake core commit hash, so at runtime the
  bridge must reject the core ("does not match"). Used by
  tests/agent_cli.rs::incompatible_core_is_rejected_before_capture_open.

  Output: target/replay-bridge-mismatch/Release/renderdocx-replay.exe
#>
[CmdletBinding()]
param(
  [string]$CoreConfiguration = 'Release',
  [string]$FakeCommit = '0000000000000000000000000000000000000000'
)

$ErrorActionPreference = 'Stop'
$repo = Split-Path -Parent $PSScriptRoot
$library = Join-Path $repo "x64/$CoreConfiguration/renderdoc.lib"
$dll = Join-Path $repo "x64/$CoreConfiguration/renderdoc.dll"
if (-not (Test-Path -LiteralPath $library)) {
  throw "RenderDoc core library missing: $library. Run scripts/build-replay-bridge.ps1 first."
}
$build = Join-Path $repo 'target/replay-bridge-mismatch'
Push-Location $repo
try {
  cmake -S bridge/replay -B $build -A x64 `
    "-DRENDERDOC_LIBRARY=$library" `
    "-DRENDERDOC_CORE_COMMIT=$FakeCommit" `
    "-DRENDERDOCX_BUILD_FIXTURES=OFF"
  if ($LASTEXITCODE -ne 0) { throw 'Mismatch bridge configure failed.' }
  cmake --build $build --config Release --target renderdocx-replay
  if ($LASTEXITCODE -ne 0) { throw 'Mismatch bridge build failed.' }
  $outDir = Join-Path $build 'Release'
  if (Test-Path -LiteralPath $dll) { Copy-Item -LiteralPath $dll -Destination $outDir -Force }
  foreach ($dep in @('d3dcompiler_47.dll', 'dbghelp.dll', 'symsrv.dll', 'symsrv.yes')) {
    $path = Join-Path $repo "x64/$CoreConfiguration/$dep"
    if (Test-Path -LiteralPath $path) { Copy-Item -LiteralPath $path -Destination $outDir -Force }
  }
  Write-Output ("Mismatch bridge: " + (Join-Path $outDir 'renderdocx-replay.exe'))
} finally {
  Pop-Location
}
