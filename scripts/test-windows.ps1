param([switch]$IncludeFast, [switch]$IncludeOpenGL)

$ErrorActionPreference = 'Stop'
if (-not $IsWindows -or -not [Environment]::Is64BitProcess) {
    throw 'Use 64-bit PowerShell 7 on Windows.'
}
$root = Split-Path -Parent $PSScriptRoot
$previousBridge = $env:RENDERDOCX_TEST_BRIDGE
$previousFixture = $env:RENDERDOCX_TEST_FIXTURE
$previousMismatch = $env:RENDERDOCX_TEST_MISMATCH_BRIDGE
$previousLiveFixture = $env:RENDERDOCX_TEST_LIVE_FIXTURE
$previousOpenGLFixture = $env:RENDERDOCX_TEST_OPENGL_FIXTURE
$previousShaderProcessor = $env:RENDERDOCX_TEST_SHADER_PROCESSOR
Push-Location $root
try {
    & (Join-Path $PSScriptRoot 'build-replay-bridge.ps1') -WithFixtures
    $library = Join-Path $root 'x64/Development/renderdoc.lib'
    cmake -S bridge/replay -B target/replay-mismatch -A x64 "-DRENDERDOC_LIBRARY=$library" -DRENDERDOC_CORE_COMMIT=0000000000000000000000000000000000000000
    if ($LASTEXITCODE -ne 0) { throw 'Mismatch bridge configuration failed.' }
    cmake --build target/replay-mismatch --config Release
    if ($LASTEXITCODE -ne 0) { throw 'Mismatch bridge build failed.' }
    Copy-Item -LiteralPath 'target/renderdocx-runtime/renderdoc.dll' -Destination 'target/replay-mismatch/Release/' -Force
    $env:RENDERDOCX_TEST_BRIDGE = Join-Path $root 'target/renderdocx-runtime/renderdocx-replay.exe'
    $env:RENDERDOCX_TEST_FIXTURE = Join-Path $root 'target/replay-bridge/Release/renderdocx-d3d11-fixture.exe'
    $env:RENDERDOCX_TEST_MISMATCH_BRIDGE = Join-Path $root 'target/replay-mismatch/Release/renderdocx-replay.exe'
    $env:RENDERDOCX_TEST_LIVE_FIXTURE = Join-Path $root 'target/replay-bridge/Release/renderdocx-d3d11-live-fixture.exe'
    $env:RENDERDOCX_TEST_SHADER_PROCESSOR = Join-Path $root 'target/replay-bridge/Release/renderdocx-shader-processor-fixture.exe'
    if ($IncludeOpenGL) {
        $env:RENDERDOCX_TEST_OPENGL_FIXTURE = Join-Path $root 'target/replay-bridge/Release/renderdocx-opengl-fixture.exe'
    } else { $env:RENDERDOCX_TEST_OPENGL_FIXTURE = $null }

    cargo test --no-default-features
    if ($LASTEXITCODE -ne 0) { throw 'Headless tests failed.' }
    cargo test --no-default-features --test shader_processor -- --ignored
    if ($LASTEXITCODE -ne 0) { throw 'External shader processor tests failed.' }
    cargo test --no-default-features --test agent_cli -- --ignored --skip real_opengl_vertex_inputs_include_generic_and_normalized_values
    if ($LASTEXITCODE -ne 0) { throw 'Real-core and D3D11 integration tests failed.' }
    if ($IncludeOpenGL) {
        cargo test --no-default-features --test agent_cli real_opengl_vertex_inputs_include_generic_and_normalized_values -- --ignored
        if ($LASTEXITCODE -ne 0) { throw 'OpenGL capture and vertex input integration failed.' }
    }
    Write-Host 'GPUI interaction tests include real D3D11 capture and replay.'
    cargo test --features ui-tests --bin renderdocx
    if ($LASTEXITCODE -ne 0) { throw 'GPUI interaction tests failed.' }
    if ($IncludeFast) {
        cargo test --features ui-tests,gpui-fast --bin renderdocx
        if ($LASTEXITCODE -ne 0) { throw 'GPUI Fast interaction tests failed.' }
    }
}
finally {
    $env:RENDERDOCX_TEST_BRIDGE = $previousBridge
    $env:RENDERDOCX_TEST_FIXTURE = $previousFixture
    $env:RENDERDOCX_TEST_MISMATCH_BRIDGE = $previousMismatch
    $env:RENDERDOCX_TEST_LIVE_FIXTURE = $previousLiveFixture
    $env:RENDERDOCX_TEST_OPENGL_FIXTURE = $previousOpenGLFixture
    $env:RENDERDOCX_TEST_SHADER_PROCESSOR = $previousShaderProcessor
    Pop-Location
}
