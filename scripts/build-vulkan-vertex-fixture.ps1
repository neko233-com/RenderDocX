param(
    [ValidateSet("Vertex", "Geometry", "Mesh")]
    [string]$Fixture = "Vertex",
    [string]$OutputDirectory = "",
    [string]$MSBuildPath = ""
)

$ErrorActionPreference = "Stop"
if (-not $IsWindows -or [IntPtr]::Size -ne 8) {
    throw "Run this fixture build with 64-bit PowerShell 7 on Windows."
}
$root = Split-Path -Parent $PSScriptRoot
$demoDirectory = Join-Path $root "util/test/demos"
$upstreamProject = Join-Path $demoDirectory "demos.vcxproj"
if (-not $OutputDirectory) { $OutputDirectory = Join-Path $root ("target/vulkan-{0}-fixture" -f $Fixture.ToLowerInvariant()) }
$OutputDirectory = [IO.Path]::GetFullPath($OutputDirectory)
$objectDirectory = Join-Path $OutputDirectory "obj"
New-Item -ItemType Directory -Force -Path $OutputDirectory, $objectDirectory | Out-Null
if ($Fixture -eq "Mesh") {
    $upstreamFixture = Join-Path $demoDirectory "vk/vk_mesh_shader.cpp"
    if (-not (Test-Path -LiteralPath $upstreamFixture -PathType Leaf)) { throw "The upstream Vulkan mesh demo was not found." }
    $sourceText = [IO.File]::ReadAllText($upstreamFixture).Replace("`r`n", "`n")
    $edits = @(
        @('RD_TEST(VK_Mesh_Shader, VulkanGraphicsTest)', 'RD_TEST(VK_RenderDocX_Mesh_Printf, VulkanGraphicsTest)'),
        @('devExts.push_back(VK_EXT_MESH_SHADER_EXTENSION_NAME);', "devExts.push_back(VK_EXT_MESH_SHADER_EXTENSION_NAME);`n    devExts.push_back(VK_KHR_SHADER_NON_SEMANTIC_INFO_EXTENSION_NAME);"),
        @("void main()`n{`n  for (int i = 0; i < 4; ++i)`n    payLoad.tri[i] = i;", "void main()`n{`n  debugPrintfEXT(`"rdx-task:%u`", gl_WorkGroupID.x);`n  for (int i = 0; i < 4; ++i)`n    payLoad.tri[i] = i;"),
        @("void main()`n{`n  uint triangleCount = 1;", "void main()`n{`n  debugPrintfEXT(`"rdx-child:%u`", gl_WorkGroupID.x);`n  uint triangleCount = 1;"),
        @("void main()`n{`n  uint triangleCount = 2;", "void main()`n{`n  debugPrintfEXT(`"rdx-mesh:%u`", gl_WorkGroupID.x);`n  uint triangleCount = 2;")
    )
    foreach ($edit in $edits) {
        $first = $sourceText.IndexOf($edit[0], [StringComparison]::Ordinal)
        if ($first -lt 0 -or $first -ne $sourceText.LastIndexOf($edit[0], [StringComparison]::Ordinal)) {
            throw "The upstream mesh demo changed; a fixture insertion point is missing or ambiguous."
        }
        $sourceText = $sourceText.Replace($edit[0], $edit[1])
    }
    $extension = '#extension GL_EXT_mesh_shader : require'
    if (-not $sourceText.Contains($extension)) { throw "The upstream mesh shader extension declaration was not found." }
    $sourceText = $sourceText.Replace($extension, "$extension`n#extension GL_EXT_debug_printf : require")
    $fixturePath = Join-Path $OutputDirectory "mesh_printf.cpp"
    [IO.File]::WriteAllText($fixturePath, $sourceText, [Text.UTF8Encoding]::new($false))
} else {
    $fixturePath = Join-Path $root ("tests/fixtures/vulkan_{0}_printf.cpp" -f $Fixture.ToLowerInvariant())
}
if (-not (Test-Path -LiteralPath $fixturePath -PathType Leaf) -or
    -not (Test-Path -LiteralPath $upstreamProject -PathType Leaf)) {
    throw "The fixture and RenderDoc upstream demos project must be present."
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
    "3rdparty\fmt\format.cc", "3rdparty\md5\md5.c", "3rdparty\volk\volk.c",
    "main.cpp", "test_common.cpp", "vk\vk_helpers.cpp", "vk\vk_test.cpp",
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
    throw "The upstream demos project has changed; required source files were not found."
}
$entry = $project.CreateElement("ClCompile", $project.DocumentElement.NamespaceURI)
$entry.SetAttribute("Include", $fixturePath)
[void]$group.AppendChild($entry)
$include = $project.SelectSingleNode("//*[local-name()='ItemDefinitionGroup']/*[local-name()='ClCompile']/*[local-name()='AdditionalIncludeDirectories']")
if (-not $include) { throw "The upstream C++ include configuration was not found." }
$include.InnerText = "$(Join-Path $demoDirectory 'vk/official');$demoDirectory;$(Join-Path $demoDirectory 'vk');%(AdditionalIncludeDirectories)"
$multiprocess = $project.SelectSingleNode("//*[local-name()='ItemDefinitionGroup']/*[local-name()='ClCompile']/*[local-name()='MultiProcessorCompilation']")
if ($multiprocess) { $multiprocess.InnerText = "false" }
$generatedProject = Join-Path $OutputDirectory "vertex-fixture.vcxproj"
$project.Save($generatedProject)
$solutionDirectory = (Join-Path $root "util/test") + [IO.Path]::DirectorySeparatorChar
$outArgument = $OutputDirectory + [IO.Path]::DirectorySeparatorChar
$objArgument = $objectDirectory + [IO.Path]::DirectorySeparatorChar
& $MSBuildPath $generatedProject /m:1 /nologo /v:minimal /p:Configuration=Development /p:Platform=x64 "/p:SolutionDir=$solutionDirectory" "/p:OutDir=$outArgument" "/p:IntDir=$objArgument" /p:CL_MPCount=1
if ($LASTEXITCODE -ne 0) { throw "Vulkan fixture build failed with exit code $LASTEXITCODE." }
Write-Output (Join-Path $OutputDirectory "demos_x64.exe")
