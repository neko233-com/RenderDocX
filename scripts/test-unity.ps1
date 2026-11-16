param(
    [Parameter(Mandatory = $true)][string]$Editor,
    [Parameter(Mandatory = $true)][string]$Project
)

$ErrorActionPreference = 'Stop'
if (-not $IsWindows -or -not [Environment]::Is64BitProcess) {
    throw 'Use 64-bit PowerShell 7 on Windows.'
}
# Project must be a disposable test project, not a user's working project.
$root = Split-Path -Parent $PSScriptRoot
$editorPath = (Resolve-Path -LiteralPath $Editor).Path
$projectPath = (Resolve-Path -LiteralPath $Project).Path
if (-not (Test-Path -LiteralPath (Join-Path $projectPath 'ProjectSettings/ProjectVersion.txt'))) {
    throw 'Project must be an existing disposable Unity project.'
}
$destination = Join-Path $projectPath 'Assets/Editor/RenderDocXBootstrap'
if (Test-Path -LiteralPath $destination) {
    throw 'Bootstrap directory already exists; use a fresh disposable project.'
}
New-Item -ItemType Directory -Path $destination -Force | Out-Null
Copy-Item -LiteralPath (Join-Path $root 'tests/unity/RenderDocXPackageInstall.cs'), (Join-Path $root 'tests/unity/RenderDocXSmoke.cs') -Destination $destination
$previousPackage = $env:RENDERDOCX_TEST_PACKAGE
$previousCapture = $env:RENDERDOCX_OPEN_CAPTURE
$previousPid = $env:RENDERDOCX_CONNECT_PID
try {
    $env:RENDERDOCX_TEST_PACKAGE = (Resolve-Path -LiteralPath (Join-Path $root 'integrations/unity')).Path
    foreach ($step in @(
        @{ Method = 'RenderDocXPackageInstall.Install'; Log = 'renderdocx-install.log'; Marker = 'RENDERDOCX_PACKAGE_INSTALLED com.neko233.renderdocx'; Quit = $false },
        @{ Method = 'RenderDocXSmoke.Check'; Log = 'renderdocx-smoke.log'; Marker = 'RENDERDOCX_UNITY_SMOKE_PASSED'; Quit = $true }
    )) {
        # Verify that inherited handoff values are cleared when they do not apply.
        $env:RENDERDOCX_OPEN_CAPTURE = 'stale capture'
        $env:RENDERDOCX_CONNECT_PID = '42'
        $log = Join-Path $projectPath $step.Log
        $editorArgs = @('-batchmode', '-nographics', '-projectPath', "`"$projectPath`"", '-executeMethod', $step.Method, '-logFile', "`"$log`"")
        if ($step.Quit) { $editorArgs += '-quit' }
        $process = Start-Process -FilePath $editorPath -ArgumentList $editorArgs -WindowStyle Hidden -PassThru -Wait
        if ($process.ExitCode -ne 0 -or -not (Select-String -LiteralPath $log -SimpleMatch -Quiet -Pattern $step.Marker)) {
            throw "Unity step $($step.Method) failed (exit $($process.ExitCode)); see $log"
        }
        Write-Host $step.Marker
    }
}
finally {
    $env:RENDERDOCX_TEST_PACKAGE = $previousPackage
    $env:RENDERDOCX_OPEN_CAPTURE = $previousCapture
    $env:RENDERDOCX_CONNECT_PID = $previousPid
}
