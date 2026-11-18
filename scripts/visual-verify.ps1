#requires -Version 7.0
<#
  Visual verification for the native RenderDocX GPUI window.
  Launches the app, locates its top-level window via Win32, captures the real
  on-screen pixels (BitBlt from the screen DC, which also captures wgpu content)
  into PNG files for review.

  NOTE: agent-desktop (native desktop driver) is not available in this session.
  This script provides equivalent native-window capture using the Windows API
  directly, and pairs with the GPUI element-tree assertions in `ui-tests`.
#>
[CmdletBinding()]
param(
  [string]$Exe = '',
  [string]$OutDir = '',
  [int]$SettleMs = 4000
)

$ErrorActionPreference = 'Stop'
Add-Type -AssemblyName System.Drawing

$repo = Split-Path -Parent $PSScriptRoot
if (-not $Exe) { $Exe = Join-Path $repo 'target/debug/renderdocx.exe' }
if (-not $OutDir) { $OutDir = Join-Path $repo 'target/visual' }
New-Item -ItemType Directory -Force -Path $OutDir | Out-Null
if (-not (Test-Path -LiteralPath $Exe)) { throw "Executable not found: $Exe" }

Add-Type @'
using System;
using System.Text;
using System.Runtime.InteropServices;
public class VWin {
  [StructLayout(LayoutKind.Sequential)] public struct RECT { public int Left, Top, Right, Bottom; }
  public delegate bool EnumWindowsProc(IntPtr hWnd, IntPtr lParam);
  [DllImport("user32.dll")] public static extern bool EnumWindows(EnumWindowsProc cb, IntPtr lParam);
  [DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr hWnd, out uint pid);
  [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr hWnd);
  [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr hWnd, out RECT rect);
  [DllImport("user32.dll")] public static extern bool SetForegroundWindow(IntPtr hWnd);
  [DllImport("user32.dll")] public static extern bool ShowWindow(IntPtr hWnd, int cmd);
  [DllImport("user32.dll")] public static extern IntPtr GetDesktopWindow();
  [DllImport("user32.dll")] public static extern IntPtr GetWindowDC(IntPtr hWnd);
  [DllImport("user32.dll")] public static extern int ReleaseDC(IntPtr hWnd, IntPtr dc);
  [DllImport("gdi32.dll")] public static extern bool BitBlt(IntPtr dst, int x, int y, int w, int h, IntPtr src, int sx, int sy, uint op);
}
'@

function Find-MainWindow([System.Diagnostics.Process]$procObj) {
  $procObj.Refresh()
  $h = $procObj.MainWindowHandle
  if ($null -eq $h) { return [IntPtr]::Zero }
  return $h
}

function Save-Window([IntPtr]$h, [string]$path) {
  $r = New-Object VWin+RECT
  [VWin]::GetWindowRect($h, [ref]$r) | Out-Null
  $w = $r.Right - $r.Left; $hgt = $r.Bottom - $r.Top
  if ($w -le 0 -or $hgt -le 0) { throw "Invalid window bounds ${w}x${hgt}" }
  $bmp = New-Object System.Drawing.Bitmap($w, $hgt)
  $g = [System.Drawing.Graphics]::FromImage($bmp)
  $screen = [VWin]::GetDesktopWindow()
  $dc = [VWin]::GetWindowDC($screen)
  # SRCCOPY = 0x00CC0020
  [VWin]::BitBlt($g.GetHdc(), 0, 0, $w, $hgt, $dc, $r.Left, $r.Top, 0x00CC0020) | Out-Null
  $g.ReleaseHdc() | Out-Null
  [VWin]::ReleaseDC($screen, $dc) | Out-Null
  $bmp.Save($path, [System.Drawing.Imaging.ImageFormat]::Png)
  $g.Dispose(); $bmp.Dispose()
  Write-Host "Captured ${w}x${hgt} -> $path"
}

Write-Host "Launching $Exe ..."
$proc = Start-Process -FilePath $Exe -PassThru
try {
  $hwnd = [IntPtr]::Zero
  for ($i = 0; $i -lt 40; $i++) {
    Start-Sleep -Milliseconds 250
    $hwnd = Find-MainWindow $proc
    if ($null -ne $hwnd -and $hwnd -ne [IntPtr]::Zero) { break }
  }
  if ($null -eq $hwnd -or $hwnd -eq [IntPtr]::Zero) { throw 'Could not locate the RenderDocX window' }
  [VWin]::ShowWindow($hwnd, 9) | Out-Null # SW_RESTORE
  [VWin]::SetForegroundWindow($hwnd) | Out-Null
  Start-Sleep -Milliseconds $SettleMs

  Save-Window $hwnd (Join-Path $OutDir '01-main.png')

  # Interact: open the Help/About dialog via the keyboard where possible, then
  # capture again. SendKeys is best-effort for GPUI custom widgets.
  Add-Type -AssemblyName System.Windows.Forms
  [System.Windows.Forms.SendKeys]::SendWait('{TAB}')
  Start-Sleep -Milliseconds 600
  Save-Window $hwnd (Join-Path $OutDir '02-focus.png')

  Write-Host "Visual capture complete in $OutDir"
  Write-Host ("Process id={0}, window handle=0x{1:X}" -f $proc.Id, $hwnd.ToInt64())
} finally {
  if (-not $proc.HasExited) { Stop-Process -Id $proc.Id -Force -ErrorAction SilentlyContinue }
}
