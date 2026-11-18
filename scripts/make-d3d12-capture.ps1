param(
    [string]$Fixture = '',
    [string]$OutDir = '',
    [string]$Cli = '',
    [string]$Bridge = ''
)

$ErrorActionPreference = 'Stop'
if (-not $IsWindows) {
    throw 'This capture helper requires Windows.'
}
$repo = Split-Path -Parent $PSScriptRoot
if (-not $Fixture) { $Fixture = Join-Path $repo 'target/d3d12-fixture/demos_x64.exe' }
if (-not $OutDir) { $OutDir = Join-Path $repo 'target/d3d12-fixture/captures' }
if (-not $Bridge) { $Bridge = Join-Path $repo 'target/renderdocx-runtime/renderdocx-replay.exe' }
if (-not (Test-Path -LiteralPath $Fixture -PathType Leaf)) {
    & (Join-Path $PSScriptRoot 'build-d3d12-fixture.ps1') | Out-Null
}
if (-not (Test-Path -LiteralPath $Fixture -PathType Leaf)) {
    throw "D3D12 fixture was not built: $Fixture"
}
if (-not $Cli) {
    foreach ($candidate in @(
        (Join-Path $repo 'target/release/renderdocx-cli.exe'),
        (Join-Path $repo 'target/debug/renderdocx-cli.exe')
    )) {
        if (Test-Path -LiteralPath $candidate -PathType Leaf) { $Cli = $candidate; break }
    }
}
if (-not $Cli -or -not (Test-Path -LiteralPath $Cli -PathType Leaf)) {
    throw 'renderdocx-cli.exe was not found; build it before generating the D3D12 capture.'
}
if (-not (Test-Path -LiteralPath $Bridge -PathType Leaf)) {
    throw "Replay bridge was not found: $Bridge"
}
$OutDir = [IO.Path]::GetFullPath($OutDir)
New-Item -ItemType Directory -Force -Path $OutDir | Out-Null

$psi = New-Object System.Diagnostics.ProcessStartInfo
$psi.FileName = (Resolve-Path $Cli).Path
$psi.Arguments = '--bridge "' + (Resolve-Path $Bridge).Path + '" serve'
$psi.WorkingDirectory = $repo
$psi.RedirectStandardInput = $true
$psi.RedirectStandardOutput = $true
$psi.RedirectStandardError = $true
$psi.UseShellExecute = $false
$psi.CreateNoWindow = $true
$process = [System.Diagnostics.Process]::Start($psi)
try {
    Start-Sleep -Milliseconds 300
    $launch = @{
        id = 1
        command = 'target_launch'
        path = (Resolve-Path $Fixture).Path
        capture_directory = $OutDir
        arguments = 'D3D12_Leak_Check --frame-count 100000'
    } | ConvertTo-Json -Compress
    $process.StandardInput.WriteLine($launch)
    $process.StandardInput.Flush()
    Start-Sleep -Seconds 2
    $process.StandardInput.WriteLine('{"id":2,"command":"target_trigger","frames":1}')
    $process.StandardInput.Flush()
    Start-Sleep -Seconds 3
    $process.StandardInput.WriteLine('{"id":3,"command":"target_close"}')
    $process.StandardInput.Flush()
    Start-Sleep -Milliseconds 500
    $process.StandardInput.Close()
    if (-not $process.WaitForExit(15000)) { $process.Kill() }
    $stdout = $process.StandardOutput.ReadToEnd()
    $stderr = $process.StandardError.ReadToEnd()
} finally {
    if (-not $process.HasExited) { $process.Kill() }
}
if ($stdout -notmatch '"ok":true' -or $stdout -match '"ok":false') {
    throw "D3D12 target launch failed: $stdout $stderr"
}
$capture = Get-ChildItem $OutDir -Recurse -Filter *.rdc -File |
    Sort-Object LastWriteTime -Descending | Select-Object -First 1
if (-not $capture) { throw "D3D12 capture was not produced in $OutDir" }
Write-Output $capture.FullName
