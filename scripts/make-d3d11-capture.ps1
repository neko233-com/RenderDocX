#requires -Version 7.0
<#
  Generate a real D3D11 .rdc capture using the locally built RenderDoc core,
  replay bridge and D3D11 live fixture, then print the capture path.

  Mirrors tests/agent_cli.rs: wait for a supported presenting API, trigger one
  frame, poll target_events for the capture, then target_copy it to OutDir.

  Usage:
    pwsh -File scripts/make-d3d11-capture.ps1
#>
[CmdletBinding()]
param(
  [string]$Bridge = '',
  [string]$Fixture = '',
  [string]$OutDir = '',
  [int]$TimeoutSeconds = 60
)

$ErrorActionPreference = 'Stop'
$repo = Split-Path -Parent $PSScriptRoot
if (-not $Bridge) { $Bridge = Join-Path $repo 'target/renderdocx-runtime/renderdocx-replay.exe' }
if (-not $Fixture) { $Fixture = Join-Path $repo 'target/replay-bridge/Release/renderdocx-d3d11-live-fixture.exe' }
if (-not $OutDir) { $OutDir = Join-Path $repo 'target/renderdocx-captures' }
$cli = Join-Path $repo 'target/debug/renderdocx-cli.exe'
foreach ($path in @($Bridge, $Fixture, $cli)) {
  if (-not (Test-Path -LiteralPath $path)) { throw "Missing: $path" }
}
New-Item -ItemType Directory -Force -Path $OutDir | Out-Null
$work = Join-Path $OutDir 'work'
New-Item -ItemType Directory -Force -Path $work | Out-Null
$stop = Join-Path $work 'stop.flag'
Remove-Item -LiteralPath $stop -ErrorAction SilentlyContinue
Get-ChildItem -LiteralPath $OutDir -Filter *.rdc -File -ErrorAction SilentlyContinue | Remove-Item -Force

$psi = New-Object System.Diagnostics.ProcessStartInfo
$psi.FileName = $cli
$psi.Arguments = "--bridge `"$Bridge`" serve"
$psi.RedirectStandardInput = $true
$psi.RedirectStandardOutput = $true
$psi.RedirectStandardError = $true
$psi.UseShellExecute = $false
$psi.CreateNoWindow = $true
$proc = [System.Diagnostics.Process]::Start($psi)
$stdout = $proc.StandardOutput
$nextId = 1

function Send-Json([string]$json) {
  $script:nextId++
  $id = $script:nextId
  $withId = $json -replace '^\{', "{`"id`":$id,"
  $proc.StandardInput.WriteLine($withId)
  $proc.StandardInput.Flush()
  $deadline = (Get-Date).AddSeconds(30)
  while ((Get-Date) -lt $deadline) {
    $line = $stdout.ReadLine()
    if ($null -eq $line) { break }
    if ($line -match "`"id`"\s*:\s*$id\b") { return $line }
  }
  throw "No reply for request $id"
}

try {
  $launch = @{
    command = 'target_launch'
    path = $Fixture
    capture_directory = $OutDir
    configuration = @{
      arguments = $stop
      working_directory = $work
      options = @{ allow_vsync = $false; soft_memory_limit = 64; ref_all_resources = $true }
    }
  } | ConvertTo-Json -Depth 6 -Compress
  $reply = Send-Json $launch
  Write-Output $reply

  $deadline = (Get-Date).AddSeconds($TimeoutSeconds)
  $ready = $false
  while ((Get-Date) -lt $deadline -and -not $ready) {
    $state = Send-Json '{"command":"target_events"}' | ConvertFrom-Json
    $ready = @($state.data.events | Where-Object {
        $_.type -eq 'api' -and $_.supported -eq $true -and $_.presenting -eq $true
      }).Count -gt 0
    if (-not $ready) { Start-Sleep -Milliseconds 100 }
  }
  if (-not $ready) { throw 'Target never reported a supported presenting API' }

  $trigger = Send-Json '{"command":"target_trigger","frames":1}' | ConvertFrom-Json
  Write-Output ("trigger submitted=" + $trigger.data.submitted)

  $capture = $null
  while ((Get-Date) -lt $deadline -and $null -eq $capture) {
    $state = Send-Json '{"command":"target_events"}' | ConvertFrom-Json
    $capture = @($state.data.events | Where-Object { $_.type -eq 'capture' }) | Select-Object -First 1
    if ($null -eq $capture) { Start-Sleep -Milliseconds 100 }
  }
  if ($null -eq $capture) { throw 'Target did not report a completed capture' }

  $copy = @{ command = 'target_copy'; capture_id = [uint64]$capture.capture_id; capture_directory = $OutDir } |
    ConvertTo-Json -Compress
  $copied = Send-Json $copy | ConvertFrom-Json
  if (-not $copied.ok) { throw ("target_copy failed: " + ($copied | ConvertTo-Json -Compress)) }

  # Wait for the copy to finish.
  $path = $null
  while ((Get-Date) -lt $deadline -and $null -eq $path) {
    $state = Send-Json '{"command":"target_events"}' | ConvertFrom-Json
    $done = @($state.data.events | Where-Object { $_.type -eq 'copied' -and $_.capture_id -eq $capture.capture_id }) |
      Select-Object -First 1
    if ($done) { $path = $copied.data.path; break }
    Start-Sleep -Milliseconds 100
  }
  if (-not $path) { throw 'capture copy did not complete' }

  Send-Json '{"command":"quit"}' | Out-Null
  Start-Sleep -Milliseconds 300
  Write-Output ("CAPTURE=" + $path)
} finally {
  if (-not $proc.HasExited) { $proc.Kill() }
  Remove-Item -LiteralPath $stop -ErrorAction SilentlyContinue
}
