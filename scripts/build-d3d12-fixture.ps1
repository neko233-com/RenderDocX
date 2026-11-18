param(
    [string]$OutputDirectory = "",
    [string]$MSBuildPath = ""
)

$ErrorActionPreference = 'Stop'
if (-not $IsWindows -or [IntPtr]::Size -ne 8) {
    throw "Run this fixture build with 64-bit PowerShell 7 on Windows."
}
$root = Split-Path -Parent $PSScriptRoot
$demoDirectory = Join-Path $root "util/test/demos"
$upstreamProject = Join-Path $demoDirectory "demos.vcxproj"
if (-not $OutputDirectory) { $OutputDirectory = Join-Path $root "target/d3d12-fixture" }
$OutputDirectory = [IO.Path]::GetFullPath($OutputDirectory)
$objectDirectory = Join-Path $OutputDirectory "obj"
New-Item -ItemType Directory -Force -Path $OutputDirectory, $objectDirectory | Out-Null

if (-not (Test-Path -LiteralPath $upstreamProject -PathType Leaf)) {
    throw "The upstream RenderDoc demos project was not found."
}
if (-not $MSBuildPath) {
    $vswhere = Join-Path ${env:ProgramFiles(x86)} "Microsoft Visual Studio/Installer/vswhere.exe"
    if (-not (Test-Path -LiteralPath $vswhere -PathType Leaf)) { throw "Visual Studio vswhere.exe was not found; pass -MSBuildPath." }
    $MSBuildPath = & $vswhere -latest -products * -requires Microsoft.Component.MSBuild -find "MSBuild\**\Bin\amd64\MSBuild.exe" | Select-Object -First 1
}
if (-not $MSBuildPath -or -not (Test-Path -LiteralPath $MSBuildPath -PathType Leaf)) {
    throw "64-bit MSBuild was not found; pass -MSBuildPath."
}

[xml]$project = Get-Content -LiteralPath $upstreamProject -Raw
$keep = @(
    "3rdparty\fmt\format.cc", "3rdparty\lz4\lz4.c", "3rdparty\md5\md5.c",
    "main.cpp", "test_common.cpp", "dx\d3d_helpers.cpp",
    "d3d12\d3d12_helpers.cpp", "d3d12\d3d12_test.cpp", "d3d12\d3d12_leak_check.cpp",
    "win32\win32_platform.cpp", "win32\win32_window.cpp"
)
$compiled = @($project.SelectNodes("//*[local-name()='ItemGroup']/*[local-name()='ClCompile'][@Include]"))
$seen = @{}
$group = $null
foreach ($item in $compiled) {
    $relative = $item.GetAttribute("Include")
    if ($relative -in $keep) {
        $seen[$relative] = $true
        $item.SetAttribute("Include", (Join-Path $demoDirectory $relative))
        $group = $item.ParentNode
    } else {
        [void]$item.ParentNode.RemoveChild($item)
    }
}
if (-not $group -or @($keep | Where-Object { -not $seen.ContainsKey($_) }).Count -ne 0) {
    throw "The upstream demos project has changed; required D3D12 source files were not found."
}
$include = $project.SelectSingleNode("//*[local-name()='ItemDefinitionGroup']/*[local-name()='ClCompile']/*[local-name()='AdditionalIncludeDirectories']")
if (-not $include) { throw "The upstream C++ include configuration was not found." }
$include.InnerText = "$(Join-Path $demoDirectory 'd3d12');$(Join-Path $demoDirectory 'dx');$demoDirectory;%(AdditionalIncludeDirectories)"
$multiprocess = $project.SelectSingleNode("//*[local-name()='ItemDefinitionGroup']/*[local-name()='ClCompile']/*[local-name()='MultiProcessorCompilation']")
if ($multiprocess) { $multiprocess.InnerText = "false" }
$generatedProject = Join-Path $OutputDirectory "d3d12-fixture.vcxproj"
$project.Save($generatedProject)

$solutionDirectory = (Join-Path $root "util/test") + [IO.Path]::DirectorySeparatorChar
$outArgument = $OutputDirectory + [IO.Path]::DirectorySeparatorChar
$objArgument = $objectDirectory + [IO.Path]::DirectorySeparatorChar
& $MSBuildPath $generatedProject /m:1 /nologo /v:minimal /p:Configuration=Development /p:Platform=x64 "/p:SolutionDir=$solutionDirectory" "/p:OutDir=$outArgument" "/p:IntDir=$objArgument" /p:CL_MPCount=1
if ($LASTEXITCODE -ne 0) { throw "D3D12 fixture build failed with exit code $LASTEXITCODE." }
Write-Output (Join-Path $OutputDirectory "demos_x64.exe")
