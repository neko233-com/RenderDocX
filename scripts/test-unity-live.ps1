param(
    [Parameter(Mandatory = $true)][string]$Editor,
    [Parameter(Mandatory = $true)][string]$Project,
    [Parameter(Mandatory = $true)][string]$Agent,
    [Parameter(Mandatory = $true)][string]$Bridge
)

$ErrorActionPreference = 'Stop'
if (-not $IsWindows -or -not [Environment]::Is64BitProcess) { throw 'Use 64-bit PowerShell 7 on Windows.' }
$root = Split-Path -Parent $PSScriptRoot
$editorPath = (Resolve-Path -LiteralPath $Editor).Path
$projectPath = (Resolve-Path -LiteralPath $Project).Path
$agentPath = (Resolve-Path -LiteralPath $Agent).Path
$bridgePath = (Resolve-Path -LiteralPath $Bridge).Path
if (-not (Test-Path -LiteralPath (Join-Path $projectPath 'ProjectSettings/ProjectVersion.txt'))) {
    throw 'Project must be an existing disposable Unity project.'
}
$destination = Join-Path $projectPath 'Assets/Editor/RenderDocXBootstrap'
New-Item -ItemType Directory -Path $destination -Force | Out-Null
Copy-Item -LiteralPath (Join-Path $root 'tests/unity/RenderDocXLive.cs') -Destination $destination -Force
$run = Join-Path $root ('target/unity-fixtures/live-' + [Guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $run -Force | Out-Null
$readyPath = Join-Path $run 'ready'
$stopPath = Join-Path $run 'stop'
$log = Join-Path $run 'editor.log'
$previousReady = $env:RENDERDOCX_UNITY_READY
$previousStop = $env:RENDERDOCX_UNITY_STOP
$env:RENDERDOCX_UNITY_READY = $readyPath
$env:RENDERDOCX_UNITY_STOP = $stopPath

function Start-Agent {
    $info = [Diagnostics.ProcessStartInfo]::new($agentPath)
    $info.UseShellExecute = $false
    $info.CreateNoWindow = $true
    $info.RedirectStandardInput = $true
    $info.RedirectStandardOutput = $true
    $info.ArgumentList.Add('--bridge')
    $info.ArgumentList.Add($bridgePath)
    $info.ArgumentList.Add('serve')
    return [Diagnostics.Process]::Start($info)
}
function Send-Agent($process, $request) {
    $process.StandardInput.WriteLine(($request | ConvertTo-Json -Compress))
    $process.StandardInput.Flush()
    $response = $process.StandardOutput.ReadLineAsync()
    if (-not $response.Wait(35000) -or [string]::IsNullOrEmpty($response.Result)) { throw 'Agent response timed out or closed.' }
    return $response.Result | ConvertFrom-Json
}

$editorProcess = $null
$first = $null
$second = $null
$forcedExit = $false
$editorExit = $null
try {
    $editorArgs = @('-batchmode', '-force-d3d11', '-load-renderdoc', '-projectPath', "`"$projectPath`"", '-executeMethod', 'RenderDocXLive.Run', '-logFile', "`"$log`"")
    $editorProcess = Start-Process -FilePath $editorPath -ArgumentList $editorArgs -WindowStyle Hidden -PassThru
    $deadline = [DateTime]::UtcNow.AddSeconds(90)
    while (-not (Test-Path -LiteralPath $readyPath) -and -not $editorProcess.HasExited -and [DateTime]::UtcNow -lt $deadline) { Start-Sleep -Milliseconds 250 }
    if (-not (Test-Path -LiteralPath $readyPath)) { throw "Unity did not become ready; see $log" }
    $ready = Get-Content -LiteralPath $readyPath
    if ($ready.Count -ne 2 -or [int]$ready[0] -ne $editorProcess.Id -or $ready[1] -ne 'Direct3D11') { throw "Unexpected Unity process or graphics API: $ready" }
    $first = Start-Agent
    $connected = Send-Agent $first @{ command = 'target_connect_pid'; pid = $editorProcess.Id }
    if (-not $connected.ok -or $connected.data.target.pid -ne $editorProcess.Id) { throw "Unity target connection failed: $($connected | ConvertTo-Json -Compress -Depth 8)" }
    $ident = $connected.data.target.ident
    $second = Start-Agent
    $busy = Send-Agent $second @{ command = 'target_connect_pid'; pid = $editorProcess.Id }
    if ($busy.ok -or $busy.error.code -ne 'target_connect_failed') { throw 'A second agent unexpectedly acquired the busy Unity target.' }
    $closed = Send-Agent $first @{ command = 'target_close' }
    if (-not $closed.ok -or $editorProcess.HasExited) { throw 'Closing the target failed or stopped Unity.' }
    $deadline = [DateTime]::UtcNow.AddSeconds(10)
    do {
        $reconnected = Send-Agent $second @{ command = 'target_connect_pid'; pid = $editorProcess.Id }
        if (-not $reconnected.ok) { Start-Sleep -Milliseconds 100 }
    } while (-not $reconnected.ok -and [DateTime]::UtcNow -lt $deadline)
    if (-not $reconnected.ok -or $reconnected.data.target.pid -ne $editorProcess.Id -or $reconnected.data.target.ident -ne $ident) { throw 'Unity target reconnection failed.' }
    foreach ($process in @($first, $second)) {
        $reply = Send-Agent $process @{ command = 'quit' }
        if (-not $reply.ok -or -not $process.WaitForExit(10000) -or $process.ExitCode -ne 0) { throw 'Agent did not quit cleanly.' }
    }
    $testedPid = $editorProcess.Id
}
finally {
    foreach ($process in @($first, $second)) {
        if ($process -and -not $process.HasExited) { $process.Kill(); $process.WaitForExit() }
        if ($process) { $process.Dispose() }
    }
    if ($editorProcess) {
        Set-Content -LiteralPath $stopPath -Value 'stop'
        if (-not $editorProcess.WaitForExit(45000)) { $forcedExit = $true; $editorProcess.Kill(); $editorProcess.WaitForExit() }
        $editorExit = $editorProcess.ExitCode
        $editorProcess.Dispose()
    }
    $env:RENDERDOCX_UNITY_READY = $previousReady
    $env:RENDERDOCX_UNITY_STOP = $previousStop
    if ($forcedExit) { Write-Warning "Unity needed forced shutdown after the test; inspect $log" }
}
if ($forcedExit -or $editorExit -ne 0) { throw "Unity did not exit cleanly (exit $editorExit); inspect $log" }
Write-Host "Unity live connection passed: PID $testedPid, Direct3D11, target $ident."
Write-Host 'Batch mode does not establish a presented-frame capture or replay.'
