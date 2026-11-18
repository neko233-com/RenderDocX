param(
    [ValidateSet('Development', 'Release')]
    [string]$CoreConfiguration = 'Development',
    [switch]$WithFixtures
)

$ErrorActionPreference = 'Stop'
if (-not $IsWindows -or -not [Environment]::Is64BitProcess) {
    throw 'Use 64-bit PowerShell 7 on Windows to build the Windows replay bridge.'
}
$root = Split-Path -Parent $PSScriptRoot
Push-Location $root
try {
    $coreChanges = @(git status --porcelain=v1 --untracked-files=all -- renderdoc)
    if ($LASTEXITCODE -ne 0 -or $coreChanges.Count -ne 0) {
        throw 'Commit or otherwise resolve RenderDoc core changes before building a version-matched bridge.'
    }
    $commit = (git rev-parse --verify HEAD).Trim()
    if ($LASTEXITCODE -ne 0 -or $commit -notmatch '^[0-9a-f]{40}$') { throw 'Could not determine the source revision.' }
    $vswhere = Join-Path ${env:ProgramFiles(x86)} 'Microsoft Visual Studio/Installer/vswhere.exe'
    $vs = & $vswhere -latest -products '*' -requires Microsoft.Component.MSBuild -property installationPath
    if (-not $vs) { throw 'Visual Studio with MSBuild and the C++ toolchain is required.' }
    $msbuild = Join-Path $vs 'MSBuild/Current/Bin/amd64/MSBuild.exe'
    $solutionDir = $root.TrimEnd('\') + '\'
    # renderdoc.vcxproj links breakpad_common/crash_generation_client/
    # exception_handler and needs the generated version header, but those are
    # not ProjectReferences, so build them explicitly first.
    $dependencies = @(
        'renderdoc/3rdparty/breakpad/client/windows/common.vcxproj',
        'renderdoc/3rdparty/breakpad/client/windows/crash_generation/crash_generation_client.vcxproj',
        'renderdoc/3rdparty/breakpad/client/windows/handler/exception_handler.vcxproj',
        'renderdoc/renderdoc_version.vcxproj'
    )
    foreach ($dependency in $dependencies) {
        & $msbuild $dependency /t:Build /m:1 "/p:Configuration=$CoreConfiguration" /p:Platform=x64 "/p:SolutionDir=$solutionDir" /p:CL_MPCount=2 /v:minimal
        if ($LASTEXITCODE -ne 0) { throw "RenderDoc core dependency build failed: $dependency" }
    }
    & $msbuild 'renderdoc/renderdoc.vcxproj' /t:Build /m:1 "/p:Configuration=$CoreConfiguration" /p:Platform=x64 "/p:SolutionDir=$solutionDir" /p:CL_MPCount=2 /v:minimal
    if ($LASTEXITCODE -ne 0) { throw 'RenderDoc core build failed.' }
    if ((git rev-parse --verify HEAD).Trim() -ne $commit -or @(git status --porcelain=v1 --untracked-files=all -- renderdoc).Count -ne 0) {
        throw 'The core source changed during the build; build again from a stable revision.'
    }
    $library = Join-Path $root "x64/$CoreConfiguration/renderdoc.lib"
    $dll = Join-Path $root "x64/$CoreConfiguration/renderdoc.dll"
    $fixtures = if ($WithFixtures) { 'ON' } else { 'OFF' }
    cmake -S bridge/replay -B target/replay-bridge -A x64 "-DRENDERDOC_LIBRARY=$library" "-DRENDERDOC_CORE_COMMIT=$commit" "-DRENDERDOCX_BUILD_FIXTURES=$fixtures"
    if ($LASTEXITCODE -ne 0) { throw 'Replay bridge configuration failed.' }
    # Both the upstream core and this bridge use the release MSVC CRT, including
    # when the core itself is built with the Development configuration.
    cmake --build target/replay-bridge --config Release
    if ($LASTEXITCODE -ne 0) { throw 'Replay bridge build failed.' }
    $destination = Join-Path $root 'target/renderdocx-runtime'
    New-Item -ItemType Directory -Force -Path $destination | Out-Null
    Copy-Item -LiteralPath 'target/replay-bridge/Release/renderdocx-replay.exe', 'target/replay-bridge/Release/renderdocx-target.exe', $dll -Destination $destination -Force
    # An explicit, process-local layer lets the matching core capture Vulkan without
    # modifying the machine's registered RenderDoc installation.
    $versionHeader = Get-Content 'renderdoc/api/replay/version.h' -Raw
    if ($versionHeader -notmatch '(?m)^#define RENDERDOC_VERSION_MINOR\s+(\d+)\s*$') { throw 'Cannot determine the RenderDoc minor version.' }
    $minorVersion = $Matches[1]
    $layer = Get-Content 'renderdoc/driver/vulkan/renderdoc.json' -Raw | ConvertFrom-Json
    $layer.layer.name = 'VK_LAYER_RENDERDOCX_Capture'
    $layer.layer.library_path = '.\renderdoc.dll'
    $layer.layer.implementation_version = $minorVersion
    $layer.layer.PSObject.Properties.Remove('enable_environment')
    $layer.layer.PSObject.Properties.Remove('disable_environment')
    $layer.layer.PSObject.Properties.Remove('pre_instance_functions')
    $layer | ConvertTo-Json -Depth 20 | Set-Content -LiteralPath (Join-Path $destination 'renderdocx-vulkan.json') -Encoding utf8
    foreach ($dependency in @('d3dcompiler_47.dll', 'dbghelp.dll', 'symsrv.dll', 'symsrv.yes')) {
        $path = Join-Path $root "x64/$CoreConfiguration/$dependency"
        if (Test-Path -LiteralPath $path -PathType Leaf) { Copy-Item -LiteralPath $path -Destination $destination -Force }
    }
    Write-Host "Replay bridge: $(Join-Path $destination 'renderdocx-replay.exe')"
    Write-Host "Matching core commit: $commit"
    if ($WithFixtures) { Copy-Item -LiteralPath 'target/replay-bridge/Release/renderdocx-remote-server-fixture.exe', 'target/replay-bridge/Release/renderdocx-dependency-fixture.exe' -Destination $destination -Force }
    if ($WithFixtures) { Write-Host "D3D11 fixture: $(Join-Path $root 'target/replay-bridge/Release/renderdocx-d3d11-fixture.exe')" }
}
finally { Pop-Location }
