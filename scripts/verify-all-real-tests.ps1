#requires -Version 7.0
<#
  Run every `#[ignore]` real test (20 agent_cli + 3 lib) against locally built
  RenderDoc core, bridge, fixtures, Vulkan toolchain, captures and mismatch
  bridge.

  Prerequisites: see docs/renderdocx-gpu-verification.md.
#>
[CmdletBinding()]
param(
  [string]$Bridge = ''
)

$ErrorActionPreference = 'Stop'
$repo = Split-Path -Parent $PSScriptRoot
if (-not $Bridge) { $Bridge = Join-Path $repo 'target/renderdocx-runtime/renderdocx-replay.exe' }
$glslang = Join-Path $repo 'target/tools/glslang/bin'
if (Test-Path $glslang) { $env:PATH = $glslang + ';' + $env:PATH }

function LatestCapture([string]$subdir) {
  $dir = Join-Path $repo "target/renderdocx-captures/$subdir"
  if (-not (Test-Path $dir)) { return $null }
  Get-ChildItem $dir -Filter *.rdc -Recurse -File |
    Sort-Object LastWriteTime -Descending | Select-Object -First 1 -ExpandProperty FullName
}

$env:RENDERDOCX_TEST_BRIDGE = $Bridge
foreach ($pair in @(
    @('RENDERDOCX_TEST_LIVE_FIXTURE', 'target/replay-bridge/Release/renderdocx-d3d11-live-fixture.exe'),
    @('RENDERDOCX_TEST_FIXTURE', 'target/replay-bridge/Release/renderdocx-d3d11-fixture.exe'),
    @('RENDERDOCX_TEST_OPENGL_FIXTURE', 'target/replay-bridge/Release/renderdocx-opengl-fixture.exe'),
    @('RENDERDOCX_TEST_REMOTE_SERVER', 'target/replay-bridge/Release/renderdocx-remote-server-fixture.exe'),
    @('RENDERDOCX_TEST_SHADER_PROCESSOR', 'target/replay-bridge/Release/renderdocx-shader-processor-fixture.exe'),
    @('RENDERDOCX_TEST_DEPENDENCY_FIXTURE', 'target/replay-bridge/Release/renderdocx-dependency-fixture.exe'),
    @('RENDERDOCX_TEST_VULKAN_MESH_DEMO', 'target/vulkan-mesh-fixture/demos_x64.exe'),
    @('RENDERDOCX_TEST_MISMATCH_BRIDGE', 'target/replay-bridge-mismatch/Release/renderdocx-replay.exe')
  )) {
  $path = Join-Path $repo $pair[1]
  if (Test-Path -LiteralPath $path) { Set-Item -Path ("env:" + $pair[0]) -Value $path }
}

$mesh = LatestCapture 'vulkan'
$vertex = LatestCapture 'vulkan-vertex'
$geometry = LatestCapture 'vulkan-geometry'
$meshPrintf = LatestCapture 'vulkan-mesh-printf'
$shaderPrintf = LatestCapture 'vulkan-shader-printf'
if ($mesh) { $env:RENDERDOCX_TEST_VULKAN_MESH_CAPTURE = $mesh }
if ($vertex) {
  $env:RENDERDOCX_TEST_VERTEX_PRINTF_CAPTURE = $vertex
  $env:RENDERDOCX_TEST_VULKAN_PRINTF_CAPTURE = $vertex
  $env:RENDERDOCX_TEST_VULKAN_VERTEX_CAPTURE = $vertex
}
if ($geometry) { $env:RENDERDOCX_TEST_GEOMETRY_PRINTF_CAPTURE = $geometry }
if ($meshPrintf) { $env:RENDERDOCX_TEST_MESH_PRINTF_CAPTURE = $meshPrintf }
if ($shaderPrintf) { $env:RENDERDOCX_TEST_VULKAN_PRINTF_CAPTURE = $shaderPrintf }

Write-Host "bridge=$($env:RENDERDOCX_TEST_BRIDGE)"
Write-Host "vulkan fonts: mesh=$mesh vertex=$vertex geometry=$geometry meshPrintf=$meshPrintf shaderPrintf=$shaderPrintf"

Push-Location $repo
try {
  cargo test --features ui-tests --test agent_cli -- --ignored --test-threads=1
  # Lib-level ignored tests for embedded-Python dynamic queries (mesh/pipeline
  # capture and vertex-input capture). The other ignored lib tests are
  # subprocess fixtures and must not be run directly.
  cargo test --features ui-tests --lib python_ -- --ignored --test-threads=1
} finally {
  Pop-Location
}
