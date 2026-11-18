#requires -Version 7.0
<#
  Run the real Vulkan tests. Requires:
    - scripts/build-replay-bridge.ps1 -CoreConfiguration Release
    - scripts/build-vulkan-vertex-fixture.ps1 -Fixture Mesh/Vertex/Geometry
    - target/tools/glslang/bin/glslangValidator.exe (or glslc) in PATH
    - the captures produced by scripts/make-vulkan-capture.ps1
#>
[CmdletBinding()]
param(
  [string]$Bridge = ''
)

$ErrorActionPreference = 'Stop'
$repo = Split-Path -Parent $PSScriptRoot
if (-not $Bridge) { $Bridge = Join-Path $repo 'target/renderdocx-runtime/renderdocx-replay.exe' }

function Latest-Capture([string]$subdir) {
  $dir = Join-Path $repo "target/renderdocx-captures/$subdir"
  if (-not (Test-Path $dir)) { return $null }
  Get-ChildItem $dir -Filter *.rdc -Recurse -File |
    Sort-Object LastWriteTime -Descending | Select-Object -First 1 -ExpandProperty FullName
}

$env:PATH = (Join-Path $repo 'target/tools/glslang/bin') + ';' + $env:PATH
$env:RENDERDOCX_TEST_BRIDGE = $Bridge
$env:RENDERDOCX_TEST_VULKAN_MESH_DEMO = Join-Path $repo 'target/vulkan-mesh-fixture/demos_x64.exe'
$mesh = Latest-Capture 'vulkan'
$vertex = Latest-Capture 'vulkan-vertex'
$geometry = Latest-Capture 'vulkan-geometry'
$meshPrintf = Latest-Capture 'vulkan-mesh-printf'
if ($mesh) { $env:RENDERDOCX_TEST_VULKAN_MESH_CAPTURE = $mesh }
if ($vertex) {
  $env:RENDERDOCX_TEST_VERTEX_PRINTF_CAPTURE = $vertex
  $env:RENDERDOCX_TEST_VULKAN_PRINTF_CAPTURE = $vertex
}
if ($geometry) { $env:RENDERDOCX_TEST_GEOMETRY_PRINTF_CAPTURE = $geometry }
if ($meshPrintf) { $env:RENDERDOCX_TEST_MESH_PRINTF_CAPTURE = $meshPrintf }

Write-Host "mesh=$mesh"
Write-Host "vertex=$vertex"
Write-Host "geometry=$geometry"
Write-Host "meshPrintf=$meshPrintf"

$tests = @(
  'real_vulkan_target_launch_uses_packaged_layer',
  'real_vulkan_mesh_shader_debug_trace',
  'real_vulkan_vertex_shader_messages_resolve_and_debug',
  'real_vulkan_geometry_shader_messages_show_approximate_output',
  'real_vulkan_shader_messages_and_debug_handoff',
  'real_vulkan_task_and_mesh_shader_messages',
  'real_vulkan_task_message_sanitizes_older_core',
  'real_vulkan_mesh_printf_core_survives_and_initializes_locations'
)

Push-Location $repo
try {
  $failed = @()
  foreach ($test in $tests) {
    Write-Host "=== $test ==="
    cargo test --features ui-tests --test agent_cli $test -- --ignored --exact
    if ($LASTEXITCODE -ne 0) { $failed += $test }
  }
  if ($failed.Count -gt 0) {
    Write-Output ("FAILED: " + ($failed -join ', '))
    exit 1
  }
  Write-Output "ALL VULKAN TESTS PASSED"
} finally {
  Pop-Location
}
