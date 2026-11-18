#requires -Version 7.0
<#
  Generate a real Vulkan mesh-shader .rdc capture with the locally built
  RenderDoc core, replay bridge and VK_Mesh_Shader demo, then print its path.

  Requires `glslangValidator` in PATH (target/tools/glslang/bin).
#>
[CmdletBinding()]
param(
  [string]$Bridge = '',
  [string]$Demo = '',
  [string]$OutDir = '',
  [string]$DemoArguments = 'VK_Mesh_Shader --width 128 --height 128',
  [int]$TimeoutSeconds = 90
)

$ErrorActionPreference = 'Stop'
$repo = Split-Path -Parent $PSScriptRoot
if (-not $Bridge) { $Bridge = Join-Path $repo 'target/renderdocx-runtime/renderdocx-replay.exe' }
if (-not $Demo) { $Demo = Join-Path $repo 'target/vulkan-mesh-fixture/demos_x64.exe' }
if (-not $OutDir) { $OutDir = Join-Path $repo 'target/renderdocx-captures/vulkan' }
$cli = Join-Path $repo 'target/debug/renderdocx-cli.exe'
$glslang = Join-Path $repo 'target/tools/glslang/bin'
foreach ($path in @($Bridge, $Demo, $cli)) {
  if (-not (Test-Path -LiteralPath $path)) { throw "Missing: $path" }
}
if (Test-Path -LiteralPath $glslang) { $env:PATH = $glslang + ';' + $env:PATH }
New-Item -ItemType Directory -Force -Path $OutDir | Out-Null
$work = Join-Path $OutDir 'work'
New-Item -ItemType Directory -Force -Path $work | Out-Null
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
$script:nextId = 1

function Send-Json([string]$json) {
  $script:nextId++
  $id = $script:nextId
  $withId = $json -replace '^\{', "{`"id`":$id,"
  $proc.StandardInput.WriteLine($withId)
  $proc.StandardInput.Flush()
  $deadline = (Get-Date).AddSeconds(60)
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
    path = $Demo
    capture_directory = $OutDir
    configuration = @{
      arguments = $DemoArguments
      working_directory = $work
      options = @{
        allow_vsync = $false
        soft_memory_limit = 256
        ref_all_resources = $true
        # Keep shader debugPrintf output in the capture.
        debug_output_mute = $false
      }
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
    if (-not $ready) { Start-Sleep -Milliseconds 200 }
  }
  if (-not $ready) { throw 'Vulkan demo never reported a supported presenting API' }

  $trigger = Send-Json '{"command":"target_trigger","frames":1}' | ConvertFrom-Json
  Write-Output ("trigger submitted=" + $trigger.data.submitted)

  $capture = $null
  while ((Get-Date) -lt $deadline -and $null -eq $capture) {
    $state = Send-Json '{"command":"target_events"}' | ConvertFrom-Json
    $capture = @($state.data.events | Where-Object { $_.type -eq 'capture' }) | Select-Object -First 1
    if ($null -eq $capture) { Start-Sleep -Milliseconds 200 }
  }
  if ($null -eq $capture) { throw 'Vulkan demo did not report a completed capture' }

  $copy = @{ command = 'target_copy'; capture_id = [uint64]$capture.capture_id; capture_directory = $OutDir } |
    ConvertTo-Json -Compress
  $copied = Send-Json $copy | ConvertFrom-Json
  if (-not $copied.ok) { throw ("target_copy failed: " + ($copied | ConvertTo-Json -Compress)) }

  $path = $null
  while ((Get-Date) -lt $deadline -and $null -eq $path) {
    $state = Send-Json '{"command":"target_events"}' | ConvertFrom-Json
    $done = @($state.data.events | Where-Object { $_.type -eq 'copied' -and $_.capture_id -eq $capture.capture_id }) |
      Select-Object -First 1
    if ($done) { $path = $copied.data.path; break }
    Start-Sleep -Milliseconds 200
  }
  if (-not $path) { throw 'Vulkan capture copy did not complete' }

  Send-Json '{"command":"quit"}' | Out-Null
  Start-Sleep -Milliseconds 300
  Write-Output ("CAPTURE=" + $path)
} finally {
  if (-not $proc.HasExited) { $proc.Kill() }
}
