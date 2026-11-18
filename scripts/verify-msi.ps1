#requires -Version 7.0
<#
  End-to-end MSI verification: silent install, locate the installed payload,
  launch it and capture a real window screenshot, then uninstall and confirm
  removal.

  Per-user install does not need administrator rights. If the MSI is configured
  per-machine and the session is not elevated, msiexec returns 1603/1625 and
  this script reports it instead of pretending success.

  Usage:
    pwsh -File scripts/verify-msi.ps1 -Msi target/msi/RenderDocX-0.1.0-x64.msi
#>
[CmdletBinding()]
param(
  [string]$Msi = '',
  [string]$OutDir = '',
  [int]$SettleMs = 5000
)

$ErrorActionPreference = 'Stop'
$repo = Split-Path -Parent $PSScriptRoot
if (-not $Msi) { $Msi = Get-ChildItem (Join-Path $repo 'target/msi') -Filter 'RenderDocX-*.msi' |
    Sort-Object LastWriteTime -Descending | Select-Object -First 1 -ExpandProperty FullName }
if (-not $Msi) { throw 'No MSI found under target/msi' }
if (-not $OutDir) { $OutDir = Join-Path $repo 'target/visual' }
New-Item -ItemType Directory -Force -Path $OutDir | Out-Null
$Msi = (Resolve-Path $Msi).Path

Add-Type -AssemblyName System.Drawing
Add-Type @'
using System;
using System.Runtime.InteropServices;
public class MVWin {
  [StructLayout(LayoutKind.Sequential)] public struct RECT { public int Left, Top, Right, Bottom; }
  [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr hWnd, out RECT rect);
  [DllImport("user32.dll")] public static extern IntPtr GetDesktopWindow();
  [DllImport("user32.dll")] public static extern IntPtr GetWindowDC(IntPtr hWnd);
  [DllImport("user32.dll")] public static extern int ReleaseDC(IntPtr hWnd, IntPtr dc);
  [DllImport("gdi32.dll")] public static extern bool BitBlt(IntPtr dst, int x, int y, int w, int h, IntPtr src, int sx, int sy, uint op);
}
'@
Add-Type -AssemblyName UIAutomationClient
Add-Type -AssemblyName UIAutomationTypes

function Install-UserScope([string]$msi, [string]$log) {
  $args = @('/i', "`"$msi`"", '/qn', '/norestart', 'ALLUSERS=2', 'MSIINSTALLPERUSER=1', "/l*v", "`"$log`"")
  $p = Start-Process msiexec -ArgumentList $args -Wait -PassThru
  return $p.ExitCode
}

function Uninstall-Msi([string]$msi, [string]$log) {
  $args = @('/x', "`"$msi`"", '/qn', '/norestart', "/l*v", "`"$log`"")
  $p = Start-Process msiexec -ArgumentList $args -Wait -PassThru
  return $p.ExitCode
}

function Find-InstalledExe {
  $candidates = @(
    (Join-Path $env:LOCALAPPDATA 'Programs\RenderDocX\renderdocx.exe'),
    (Join-Path $env:ProgramFiles 'RenderDocX\renderdocx.exe')
  )
  foreach ($c in $candidates) { if (Test-Path -LiteralPath $c) { return $c } }
  return $null
}

function Save-Window([IntPtr]$h, [string]$path) {
  $r = New-Object MVWin+RECT
  [MVWin]::GetWindowRect($h, [ref]$r) | Out-Null
  $w = $r.Right - $r.Left
  $hgt = $r.Bottom - $r.Top
  if ($w -le 0 -or $hgt -le 0) { throw "Invalid window bounds ${w}x${hgt}" }
  $bmp = New-Object System.Drawing.Bitmap($w, $hgt)
  $g = [System.Drawing.Graphics]::FromImage($bmp)
  $screen = [MVWin]::GetDesktopWindow()
  $dc = [MVWin]::GetWindowDC($screen)
  [MVWin]::BitBlt($g.GetHdc(), 0, 0, $w, $hgt, $dc, $r.Left, $r.Top, 0x00CC0020) | Out-Null
  $g.ReleaseHdc() | Out-Null
  [MVWin]::ReleaseDC($screen, $dc) | Out-Null
  $bmp.Save($path, [System.Drawing.Imaging.ImageFormat]::Png)
  $g.Dispose()
  $bmp.Dispose()
  Write-Host "Captured installed app ${w}x${hgt} -> $path"
}

$installLog = Join-Path $OutDir 'msi-install.log'
$uninstallLog = Join-Path $OutDir 'msi-uninstall.log'
Write-Host "Installing $Msi (per-user, silent) ..."
$code = Install-UserScope $Msi $installLog
Write-Host "msiexec /i exit code: $code"
if ($code -ne 0) {
  Write-Output "MSI install failed with $code. See $installLog (1603/1625 usually means elevation is required)."
  exit 1
}

$exe = Find-InstalledExe
if (-not $exe) {
  Write-Output "Install succeeded but renderdocx.exe was not found in the usual locations."
} else {
  Write-Output "Installed executable: $exe"
  $installedDebugpy = Join-Path (Split-Path -Parent $exe) 'python-site\debugpy\__init__.py'
  Write-Output ("Bundled debugpy installed: {0}" -f (Test-Path -LiteralPath $installedDebugpy))
  $proc = Start-Process -FilePath $exe -PassThru
  try {
    $condition = New-Object System.Windows.Automation.PropertyCondition(
      [System.Windows.Automation.AutomationElement]::ProcessIdProperty, $proc.Id)
    $window = $null
    for ($i = 0; $i -lt 40; $i++) {
      Start-Sleep -Milliseconds 300
      $window = [System.Windows.Automation.AutomationElement]::RootElement.FindFirst(
        [System.Windows.Automation.TreeScope]::Children, $condition)
      if ($null -ne $window) { break }
    }
    if ($null -eq $window) {
      Write-Output 'Installed app started but exposed no UIA window.'
    } else {
      Start-Sleep -Milliseconds $SettleMs
      Save-Window ([IntPtr]$window.Current.NativeWindowHandle) (Join-Path $OutDir 'msi-installed-app.png')
      Write-Output ("Installed app window: {0}" -f $window.Current.Name)
    }
  } finally {
    if (-not $proc.HasExited) { Stop-Process -Id $proc.Id -Force -ErrorAction SilentlyContinue }
  }
}

Write-Host "Uninstalling ..."
$code = Uninstall-Msi $Msi $uninstallLog
Write-Host "msiexec /x exit code: $code"
$stillThere = Find-InstalledExe
$debugpyStillThere = $false
if ($exe) {
  $debugpyStillThere = Test-Path -LiteralPath (Join-Path (Split-Path -Parent $exe) 'python-site\debugpy\__init__.py')
}
Write-Output ("Uninstall removed payload: {0}" -f (-not $stillThere -and -not $debugpyStillThere))
if ($code -ne 0) { exit 1 }
