#requires -Version 7.0
<#
  Run the real-GPU / real-capture tests against the locally built RenderDoc
  core and replay bridge.

  Prerequisites:
    pwsh -File scripts/build-replay-bridge.ps1 -CoreConfiguration Release -WithFixtures

  Optional external inputs for the capture-based tests:
    RENDERDOCX_TEST_CAPTURE                 path to a D3D11 .rdc
    RENDERDOCX_TEST_VULKAN_MESH_CAPTURE      path to a Vulkan mesh-shader .rdc
    RENDERDOCX_TEST_MESH_PRINTF_CAPTURE      path to the mesh-printf .rdc
#>
[CmdletBinding()]
param(
  [string]$Bridge = '',
  # Comma-separated test names (pwsh -File does not bind arrays).
  [string]$Test = '',
  [switch]$ListOnly
)

$ErrorActionPreference = 'Stop'
$repo = Split-Path -Parent $PSScriptRoot
if (-not $Bridge) {
  $Bridge = Join-Path $repo 'target/renderdocx-runtime/renderdocx-replay.exe'
}
if (-not (Test-Path -LiteralPath $Bridge)) {
  throw "Replay bridge not found: $Bridge. Run scripts/build-replay-bridge.ps1 first."
}
$env:RENDERDOCX_TEST_BRIDGE = (Resolve-Path $Bridge).Path

$liveFixture = Join-Path $repo 'target/replay-bridge/Release/renderdocx-d3d11-live-fixture.exe'
if (Test-Path -LiteralPath $liveFixture) {
  $env:RENDERDOCX_TEST_LIVE_FIXTURE = $liveFixture
}
$d3d11Fixture = Join-Path $repo 'target/replay-bridge/Release/renderdocx-d3d11-fixture.exe'
if (Test-Path -LiteralPath $d3d11Fixture) {
  $env:RENDERDOCX_TEST_FIXTURE = $d3d11Fixture
}
$glFixture = Join-Path $repo 'target/replay-bridge/Release/renderdocx-opengl-fixture.exe'
if (Test-Path -LiteralPath $glFixture) {
  $env:RENDERDOCX_TEST_OPENGL_FIXTURE = $glFixture
}
$remoteFixture = Join-Path $repo 'target/replay-bridge/Release/renderdocx-remote-server-fixture.exe'
if (Test-Path -LiteralPath $remoteFixture) {
  $env:RENDERDOCX_TEST_REMOTE_SERVER = $remoteFixture
}
$shaderProcessor = Join-Path $repo 'target/replay-bridge/Release/renderdocx-shader-processor-fixture.exe'
if (Test-Path -LiteralPath $shaderProcessor) {
  $env:RENDERDOCX_TEST_SHADER_PROCESSOR = $shaderProcessor
}
$descriptorFixture = Join-Path $repo 'target/replay-bridge/Release/renderdocx-descriptor-batch-fixture.exe'
if (Test-Path -LiteralPath $descriptorFixture) {
  $env:RENDERDOCX_TEST_DESCRIPTOR_FIXTURE = $descriptorFixture
}
$dependencyFixture = Join-Path $repo 'target/replay-bridge/Release/renderdocx-dependency-fixture.exe'
if (Test-Path -LiteralPath $dependencyFixture) {
  $env:RENDERDOCX_TEST_DEPENDENCY_FIXTURE = $dependencyFixture
}

Write-Host "Bridge: $($env:RENDERDOCX_TEST_BRIDGE)"
if ($env:RENDERDOCX_TEST_LIVE_FIXTURE) { Write-Host "Live fixture: $($env:RENDERDOCX_TEST_LIVE_FIXTURE)" }
if ($env:RENDERDOCX_TEST_CAPTURE) { Write-Host "Capture: $($env:RENDERDOCX_TEST_CAPTURE)" } else { Write-Host "Capture: (unset; capture-based tests will fail fast)" }

Push-Location $repo
try {
  if ($ListOnly) {
    cargo test --features ui-tests --test agent_cli -- --ignored --list
    return
  }
  $names = @($Test -split ',' | ForEach-Object { $_.Trim() } | Where-Object { $_ })
  if ($names.Count -eq 0) {
    cargo test --features ui-tests --test agent_cli -- --ignored
  } else {
    foreach ($name in $names) {
      Write-Host "=== $name ==="
      cargo test --features ui-tests --test agent_cli $name -- --ignored --exact
    }
  }
} finally {
  Pop-Location
}
