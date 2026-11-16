param([switch]$IncludeFast)

$ErrorActionPreference = 'Stop'
if (-not $IsWindows -or -not [Environment]::Is64BitProcess) {
    throw 'Use 64-bit PowerShell 7 on Windows.'
}
$root = Split-Path -Parent $PSScriptRoot
$previousBridge = $env:RENDERDOCX_TEST_BRIDGE
$previousFixture = $env:RENDERDOCX_TEST_FIXTURE
$previousMismatch = $env:RENDERDOCX_TEST_MISMATCH_BRIDGE
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

    cargo test --no-default-features
    if ($LASTEXITCODE -ne 0) { throw 'Headless tests failed.' }
    cargo test --no-default-features --test agent_cli -- --ignored
    if ($LASTEXITCODE -ne 0) { throw 'Real-core and D3D11 integration tests failed.' }
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
    Pop-Location
}
