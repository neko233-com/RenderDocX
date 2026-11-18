#requires -Version 7.0
<#
  Native visual-driving verification for the RenderDocX GPUI window.

  RenderDocX's GPUI window exposes an accessibility tree (AccessKit), so
  Windows UI Automation can enumerate its controls, scroll them into view and
  invoke them, and Win32 BitBlt can capture real pixels. This provides desktop
  visual-driving verification without an external agent-desktop service.

  Element bounding rectangles are reported in client-area coordinates (verified
  empirically: a control moved from y=1360 to y=580 after scrolling). The script
  therefore converts them to screen coordinates with ClientToScreen before
  clicking, and scrolls controls into view first.

  Examples:
    pwsh -File scripts/desktop-drive.ps1 -Exe target/release/renderdocx.exe
    pwsh -File scripts/desktop-drive.ps1 -InvokeName "主题: 深色"
    pwsh -File scripts/desktop-drive.ps1 -InvokeName "日志" -UseMouse
#>
[CmdletBinding()]
param(
  [string]$Exe = '',
  [string]$OutDir = '',
  [int]$SettleMs = 5000,
  [string]$InvokeName = '',
  [string]$InvokeId = '',
  [switch]$UseMouse,
  [switch]$DumpTree
)

$ErrorActionPreference = 'Stop'
$repo = Split-Path -Parent $PSScriptRoot
if (-not $Exe) { $Exe = Join-Path $repo 'target/release/renderdocx.exe' }
if (-not $OutDir) { $OutDir = Join-Path $repo 'target/visual' }
New-Item -ItemType Directory -Force -Path $OutDir | Out-Null
if (-not (Test-Path -LiteralPath $Exe)) { throw "Executable not found: $Exe" }

Add-Type -AssemblyName UIAutomationClient
Add-Type -AssemblyName UIAutomationTypes
Add-Type -AssemblyName System.Drawing
Add-Type -AssemblyName System.Windows.Forms

Add-Type @'
using System;
using System.Runtime.InteropServices;
public class DDWin {
  [StructLayout(LayoutKind.Sequential)] public struct RECT { public int Left, Top, Right, Bottom; }
  [StructLayout(LayoutKind.Sequential)] public struct POINT { public int X, Y; }
  [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr hWnd, out RECT rect);
  [DllImport("user32.dll")] public static extern bool GetClientRect(IntPtr hWnd, out RECT rect);
  [DllImport("user32.dll")] public static extern bool ClientToScreen(IntPtr hWnd, ref POINT p);
  [DllImport("user32.dll")] public static extern bool SetForegroundWindow(IntPtr hWnd);
  [DllImport("user32.dll")] public static extern bool ShowWindow(IntPtr hWnd, int cmd);
  [DllImport("user32.dll")] public static extern IntPtr GetDesktopWindow();
  [DllImport("user32.dll")] public static extern IntPtr GetWindowDC(IntPtr hWnd);
  [DllImport("user32.dll")] public static extern int ReleaseDC(IntPtr hWnd, IntPtr dc);
  [DllImport("user32.dll")] public static extern void mouse_event(uint flags, int dx, int dy, int data, IntPtr extra);
  [DllImport("user32.dll")] public static extern bool SetWindowPos(IntPtr hWnd, IntPtr after, int x, int y, int cx, int cy, uint flags);
  [DllImport("gdi32.dll")] public static extern bool BitBlt(IntPtr dst, int x, int y, int w, int h, IntPtr src, int sx, int sy, uint op);
}
'@

function Save-Window([IntPtr]$h, [string]$path) {
  $r = New-Object DDWin+RECT
  [DDWin]::GetWindowRect($h, [ref]$r) | Out-Null
  $w = $r.Right - $r.Left
  $hgt = $r.Bottom - $r.Top
  if ($w -le 0 -or $hgt -le 0) { throw "Invalid window bounds ${w}x${hgt}" }
  $bmp = New-Object System.Drawing.Bitmap($w, $hgt)
  $g = [System.Drawing.Graphics]::FromImage($bmp)
  $screen = [DDWin]::GetDesktopWindow()
  $dc = [DDWin]::GetWindowDC($screen)
  [DDWin]::BitBlt($g.GetHdc(), 0, 0, $w, $hgt, $dc, $r.Left, $r.Top, 0x00CC0020) | Out-Null
  $g.ReleaseHdc() | Out-Null
  [DDWin]::ReleaseDC($screen, $dc) | Out-Null
  $bmp.Save($path, [System.Drawing.Imaging.ImageFormat]::Png)
  $g.Dispose()
  $bmp.Dispose()
  Write-Host "Captured ${w}x${hgt} -> $path"
}

function Get-Descendants($root) {
  $walker = [System.Windows.Automation.TreeWalker]::ControlViewWalker
  $stack = New-Object System.Collections.Stack
  $stack.Push($root)
  $items = New-Object System.Collections.ArrayList
  while ($stack.Count -gt 0) {
    $node = $stack.Pop()
    $items.Add($node) | Out-Null
    $child = $walker.GetFirstChild($node)
    while ($null -ne $child) {
      $stack.Push($child)
      $child = $walker.GetNextSibling($child)
    }
  }
  return $items
}

function Get-ClientOrigin([IntPtr]$h) {
  $pt = New-Object DDWin+POINT
  $pt.X = 0
  $pt.Y = 0
  [DDWin]::ClientToScreen($h, [ref]$pt) | Out-Null
  return $pt
}

function Scroll-IntoView($window, $target, [IntPtr]$h) {
  $client = New-Object DDWin+RECT
  [DDWin]::GetClientRect($h, [ref]$client) | Out-Null
  $origin = Get-ClientOrigin $h
  $height = $client.Bottom - $client.Top
  $margin = 4
  for ($i = 0; $i -lt 80; $i++) {
    $b = $target.Current.BoundingRectangle
    if ($b.Height -gt 0 -and $b.Y -ge $margin -and ($b.Y + $b.Height) -le ($height - $margin)) {
      return $true
    }
    # Decide the wheel direction from where the control currently sits.
    $wheel = if ($b.Y -ge ($height - $margin)) { -120 } else { 120 }
    [DDWin]::SetForegroundWindow($h) | Out-Null
    [System.Windows.Forms.Cursor]::Position = New-Object System.Drawing.Point(
      ($origin.X + [int]($client.Right / 2)), ($origin.Y + [int]($height / 2)))
    [DDWin]::mouse_event(0x0800, 0, 0, $wheel, [IntPtr]::Zero)
    Start-Sleep -Milliseconds 100
  }
  return $false
}

Write-Host "Launching $Exe ..."
$proc = Start-Process -FilePath $Exe -PassThru
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
    throw "No UI Automation window for PID $($proc.Id). Run this on an interactive desktop."
  }
  $hwnd = [IntPtr]$window.Current.NativeWindowHandle
  [DDWin]::ShowWindow($hwnd, 9) | Out-Null
  # Pin the window at (0,0) so client coordinates are screen coordinates and
  # clicks cannot fall off-screen (the default centred placement can overflow
  # the bottom of a 1080p screen).
  [DDWin]::SetWindowPos($hwnd, [IntPtr]::Zero, 0, 0, 1056, 799, 0x0040) | Out-Null
  [DDWin]::SetForegroundWindow($hwnd) | Out-Null
  Start-Sleep -Milliseconds $SettleMs

  $elements = Get-Descendants $window
  Write-Host ("UIA elements: {0}" -f $elements.Count)

  $rows = foreach ($el in $elements) {
    $c = $el.Current
    [pscustomobject]@{
      Name = $c.Name; AutomationId = $c.AutomationId
      ControlType = $c.ControlType.ProgrammaticName
      X = [int]$c.BoundingRectangle.X; Y = [int]$c.BoundingRectangle.Y
      W = [int]$c.BoundingRectangle.Width; H = [int]$c.BoundingRectangle.Height
    }
  }
  $rows | ConvertTo-Json -Depth 4 | Set-Content -LiteralPath (Join-Path $OutDir 'uia-tree.json') -Encoding UTF8
  Write-Host "UIA tree -> uia-tree.json"

  Save-Window $hwnd (Join-Path $OutDir 'drive-01-initial.png')

  $target = $null
  if ($InvokeId) {
    $target = $elements | Where-Object { $_.Current.AutomationId -eq $InvokeId } | Select-Object -First 1
  } elseif ($InvokeName) {
    $target = $elements | Where-Object { $_.Current.Name -eq $InvokeName } | Select-Object -First 1
    if ($null -eq $target) {
      $target = $elements | Where-Object { $_.Current.Name -like "*$InvokeName*" } | Select-Object -First 1
    }
  }

  if ($null -ne $target) {
    $visible = Scroll-IntoView $window $target $hwnd
    $origin = Get-ClientOrigin $hwnd
    $b = $target.Current.BoundingRectangle
    $sx = $origin.X + [int]($b.X + $b.Width / 2)
    $sy = $origin.Y + [int]($b.Y + $b.Height / 2)
    Write-Host ("Target '{0}' visible={1} client=({2},{3}) screen=({4},{5})" -f `
      $target.Current.Name, $visible, [int]$b.X, [int]$b.Y, $sx, $sy)

    $pattern = $null
    if (-not $UseMouse -and $target.TryGetCurrentPattern(
        [System.Windows.Automation.InvokePattern]::Pattern, [ref]$pattern)) {
      $pattern.Invoke()
      Write-Host ("Invoked: {0}" -f $target.Current.Name)
    } else {
      [DDWin]::SetForegroundWindow($hwnd) | Out-Null
      [System.Windows.Forms.Cursor]::Position = New-Object System.Drawing.Point($sx, $sy)
      Start-Sleep -Milliseconds 120
      [DDWin]::mouse_event(0x0002, 0, 0, 0, [IntPtr]::Zero)
      Start-Sleep -Milliseconds 60
      [DDWin]::mouse_event(0x0004, 0, 0, 0, [IntPtr]::Zero)
      Write-Host ("Clicked at {0},{1}: {2}" -f $sx, $sy, $target.Current.Name)
    }
    Start-Sleep -Milliseconds 900
    Save-Window $hwnd (Join-Path $OutDir 'drive-02-after-invoke.png')

    $after = Get-Descendants $window
    $rowsAfter = foreach ($el in $after) {
      $c = $el.Current
      [pscustomobject]@{ Name = $c.Name; AutomationId = $c.AutomationId; ControlType = $c.ControlType.ProgrammaticName }
    }
    $rowsAfter | ConvertTo-Json -Depth 4 | Set-Content -LiteralPath (Join-Path $OutDir 'uia-tree-after.json') -Encoding UTF8
    $beforeKeys = @{}
    foreach ($r in $rows) { $beforeKeys[($r.Name + '|' + $r.AutomationId)] = $true }
    $added = @($rowsAfter | Where-Object { -not $beforeKeys.ContainsKey($_.Name + '|' + $_.AutomationId) })
    Write-Host ("UIA elements after: {0}; new elements: {1}" -f $after.Count, $added.Count)
    foreach ($a in $added | Select-Object -First 8) {
      Write-Output ("  new: " + $a.ControlType.Split('.')[-1] + " " + $a.Name)
    }
  } elseif ($InvokeId -or $InvokeName) {
    Write-Host "Target not found; see uia-tree.json for available names/ids."
  }

  Write-Host "Desktop-drive verification complete in $OutDir"
} finally {
  if (-not $proc.HasExited) { Stop-Process -Id $proc.Id -Force -ErrorAction SilentlyContinue }
}
