param(
    [Parameter(Mandatory = $true)][string]$UnityCli,
    [Parameter(Mandatory = $true)][string]$Editor,
    [Parameter(Mandatory = $true)][string]$Project,
    [Parameter(Mandatory = $true)][string]$Agent,
    [Parameter(Mandatory = $true)][string]$Bridge
)

$ErrorActionPreference = 'Stop'
if (-not $IsWindows -or -not [Environment]::Is64BitProcess) { throw 'Use 64-bit PowerShell 7 on Windows.' }
$root = Split-Path -Parent $PSScriptRoot
$cliPath = (Resolve-Path -LiteralPath $UnityCli).Path
$editorPath = (Resolve-Path -LiteralPath $Editor).Path
$projectPath = (Resolve-Path -LiteralPath $Project).Path
$agentPath = (Resolve-Path -LiteralPath $Agent).Path
$bridgePath = (Resolve-Path -LiteralPath $Bridge).Path
if (-not (Test-Path -LiteralPath (Join-Path $projectPath 'ProjectSettings/ProjectVersion.txt'))) {
    throw 'Project must be an existing disposable Unity project with the RenderDocX package installed.'
}
$fixture = Join-Path $projectPath 'Assets/RenderDocXPlayerFixture'
if (Test-Path -LiteralPath $fixture) { throw 'Fixture directory exists; use a fresh disposable project.' }
New-Item -ItemType Directory -Path (Join-Path $fixture 'Editor') -Force | Out-Null
Copy-Item -LiteralPath (Join-Path $root 'tests/unity/RenderDocXPlayerFixture.cs') -Destination $fixture
Copy-Item -LiteralPath (Join-Path $root 'tests/unity/RenderDocXPlayerBuild.cs') -Destination (Join-Path $fixture 'Editor')
$run = Join-Path $root ('target/unity-fixtures/player-' + [Guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $run -Force | Out-Null
$player = Join-Path $run 'build/RenderDocXFixture.exe'
New-Item -ItemType Directory -Path (Split-Path -Parent $player) -Force | Out-Null
$buildLog = Join-Path $run 'build.log'
& $cliPath build $projectPath --target StandaloneWindows64 --execute-method RenderDocXPlayerBuild.Build `
    --output $player --editor-path $editorPath --architecture x86_64 --log-file $buildLog --no-tail --non-interactive --format json
if ($LASTEXITCODE -ne 0 -or -not (Test-Path -LiteralPath $player) -or
    -not (Select-String -LiteralPath $buildLog -SimpleMatch -Quiet -Pattern 'RENDERDOCX_PLAYER_BUILT')) {
    throw "Unity fixture build failed; see $buildLog"
}

$previousReady = $env:RENDERDOCX_PLAYER_READY
$previousCapture = $env:RENDERDOCX_PLAYER_CAPTURE
$previousStop = $env:RENDERDOCX_PLAYER_STOP
$previousTemp = $env:TEMP
$previousTmp = $env:TMP
$env:RENDERDOCX_PLAYER_READY = Join-Path $run 'ready'
$env:RENDERDOCX_PLAYER_CAPTURE = Join-Path $run 'capture'
$env:RENDERDOCX_PLAYER_STOP = Join-Path $run 'stop'
$env:TEMP = $run
$env:TMP = $run
$playerLog = Join-Path $run 'player.log'
$playerProcess = $null
$agentProcess = $null
$forcedExit = $false
$playerExit = $null
function Send-Agent($request) {
    $agentProcess.StandardInput.WriteLine(($request | ConvertTo-Json -Depth 12 -Compress))
    $agentProcess.StandardInput.Flush()
    $line = $agentProcess.StandardOutput.ReadLineAsync()
    if (-not $line.Wait(35000) -or [string]::IsNullOrEmpty($line.Result)) { throw 'Agent response timed out or closed.' }
    return $line.Result | ConvertFrom-Json
}
try {
    # This start mode was verified with Unity 2022.3.51f1c1; other versions may not load RenderDoc here.
    $playerArgs = @('-force-d3d11', '-load-renderdoc', '-screen-width', '320', '-screen-height', '240', '-logFile', "`"$playerLog`"")
    $playerProcess = Start-Process -FilePath $player -ArgumentList $playerArgs -WindowStyle Hidden -PassThru
    $deadline = [DateTime]::UtcNow.AddSeconds(60)
    while (-not (Test-Path -LiteralPath $env:RENDERDOCX_PLAYER_READY) -and -not $playerProcess.HasExited -and [DateTime]::UtcNow -lt $deadline) { Start-Sleep -Milliseconds 250 }
    if (-not (Test-Path -LiteralPath $env:RENDERDOCX_PLAYER_READY)) { throw "Unity player did not become ready; see $playerLog" }
    $ready = Get-Content -LiteralPath $env:RENDERDOCX_PLAYER_READY
    if ($ready.Count -ne 3 -or [int]$ready[0] -ne $playerProcess.Id -or $ready[1] -ne 'Direct3D11' -or $ready[2] -ne 'True') {
        throw "Player did not report its PID, Direct3D11 and attached profiler: $ready"
    }
    $info = [Diagnostics.ProcessStartInfo]::new($agentPath)
    $info.UseShellExecute = $false; $info.CreateNoWindow = $true
    $info.RedirectStandardInput = $true; $info.RedirectStandardOutput = $true
    $info.ArgumentList.Add('--bridge'); $info.ArgumentList.Add($bridgePath); $info.ArgumentList.Add('serve')
    $agentProcess = [Diagnostics.Process]::Start($info)
    $connected = Send-Agent @{ command = 'target_connect_pid'; pid = $playerProcess.Id }
    if (-not $connected.ok -or $connected.data.target.pid -ne $playerProcess.Id) { throw "Player target connection failed: $($connected | ConvertTo-Json -Compress -Depth 8)" }
    Set-Content -LiteralPath $env:RENDERDOCX_PLAYER_CAPTURE -Value 'capture'
    $deadline = [DateTime]::UtcNow.AddSeconds(45)
    $captured = $null
    while ($null -eq $captured -and -not $playerProcess.HasExited -and [DateTime]::UtcNow -lt $deadline) {
        $events = Send-Agent @{ command = 'target_events' }
        if (-not $events.ok -or -not $events.data.connected) { throw 'Player target disconnected.' }
        $captured = $events.data.events | Where-Object { $_.type -eq 'capture' -and $_.local } | Select-Object -First 1
        if ($null -eq $captured) { Start-Sleep -Milliseconds 200 }
    }
    $resultPath = $env:RENDERDOCX_PLAYER_CAPTURE + '.result'
    $deadline = [DateTime]::UtcNow.AddSeconds(10)
    while (-not (Test-Path -LiteralPath $resultPath) -and -not $playerProcess.HasExited -and [DateTime]::UtcNow -lt $deadline) { Start-Sleep -Milliseconds 100 }
    if ($null -eq $captured -or -not (Test-Path -LiteralPath $captured.path) -or
        -not (Test-Path -LiteralPath $resultPath) -or (Get-Content -LiteralPath $resultPath -Raw).Trim() -ne 'capture-ended') {
        throw "Unity did not finish a capture; see $playerLog"
    }
    $capturePath = [IO.Path]::GetFullPath($captured.path)
    $runPrefix = [IO.Path]::GetFullPath($run).TrimEnd([IO.Path]::DirectorySeparatorChar) + [IO.Path]::DirectorySeparatorChar
    if (-not $capturePath.StartsWith($runPrefix, [StringComparison]::OrdinalIgnoreCase)) { throw 'Capture path is outside the isolated test directory.' }
    $opened = Send-Agent @{ command = 'open'; path = $capturePath }
    if (-not $opened.ok -or $opened.data.capture.api -ne 0) { throw "Unity D3D11 capture did not replay: $($opened | ConvertTo-Json -Compress -Depth 6)" }
    $draw = $opened.data.capture.actions | Where-Object { $_.name -like '*DrawIndexed*' -and $_.indices -eq 36 } | Select-Object -First 1
    $texture = $opened.data.capture.textures | Where-Object { $_.width -eq 64 -and $_.height -eq 64 } | Select-Object -First 1
    if ($null -eq $draw -or $null -eq $texture) { throw 'Capture is missing the fixture draw or render target.' }
    $selected = Send-Agent @{ command = 'select'; event_id = $draw.event_id }
    if (-not $selected.ok -or $selected.data.event_id -ne $draw.event_id -or -not $selected.data.pipeline.available) { throw 'Unity draw event replay failed.' }
    $closed = Send-Agent @{ command = 'target_close' }
    if (-not $closed.ok) { throw 'Target close failed.' }
    $closed = Send-Agent @{ command = 'quit' }
    if (-not $closed.ok -or -not $agentProcess.WaitForExit(10000) -or $agentProcess.ExitCode -ne 0) { throw 'Agent did not quit cleanly.' }
    Write-Host "Unity Player capture and replay passed: event $($draw.event_id); capture $capturePath"
}
finally {
    if ($playerProcess) {
        Set-Content -LiteralPath $env:RENDERDOCX_PLAYER_STOP -Value 'stop'
        if (-not $playerProcess.WaitForExit(15000)) { $forcedExit = $true; $playerProcess.Kill(); $playerProcess.WaitForExit() }
        $playerExit = $playerProcess.ExitCode
        $playerProcess.Dispose()
    }
    if ($agentProcess -and -not $agentProcess.HasExited) { if (-not $agentProcess.WaitForExit(10000)) { $agentProcess.Kill(); $agentProcess.WaitForExit() } }
    if ($agentProcess) { $agentProcess.Dispose() }
    $env:RENDERDOCX_PLAYER_READY = $previousReady
    $env:RENDERDOCX_PLAYER_CAPTURE = $previousCapture
    $env:RENDERDOCX_PLAYER_STOP = $previousStop
    $env:TEMP = $previousTemp
    $env:TMP = $previousTmp
}
if ($forcedExit -or $playerExit -ne 0) { throw "Player did not exit cleanly (exit $playerExit); see $playerLog" }
