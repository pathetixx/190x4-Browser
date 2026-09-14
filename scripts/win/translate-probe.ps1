# Stage 4 check: translate selected text through the 190x4 relay.
#  1. select a paragraph on the restored page (triple click),
#  2. right click -> our item must be first in the native menu,
#  3. click it and screenshot the translation panel.
# ASCII only.

$ErrorActionPreference = "Stop"

$root = "E:\test\190x4-browser"
$exe  = "$root\target\release\190x4-browser.exe"
$out  = "$root\spike-out"
New-Item -ItemType Directory -Force -Path $out | Out-Null

Add-Type -AssemblyName System.Drawing, System.Windows.Forms
Add-Type @"
using System;
using System.Runtime.InteropServices;
using System.Text;
public class Win4 {
  [DllImport("user32.dll")] public static extern bool EnumWindows(EnumWindowsProc cb, IntPtr p);
  [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr h, out RECT r);
  [DllImport("user32.dll")] public static extern int GetClassName(IntPtr h, StringBuilder s, int max);
  [DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr h, out uint pid);
  [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr h);
  [DllImport("user32.dll")] public static extern bool SetForegroundWindow(IntPtr h);
  [DllImport("user32.dll")] public static extern bool SetCursorPos(int x, int y);
  [DllImport("user32.dll")] public static extern void mouse_event(uint f, uint x, uint y, uint d, IntPtr e);
  public delegate bool EnumWindowsProc(IntPtr h, IntPtr p);
  [StructLayout(LayoutKind.Sequential)] public struct RECT { public int Left, Top, Right, Bottom; }
}
"@

$LEFT_DOWN = 0x0002; $LEFT_UP = 0x0004; $RIGHT_DOWN = 0x0008; $RIGHT_UP = 0x0010

function Find-AppWindow([int]$procId) {
  $script:found = [IntPtr]::Zero
  $cb = [Win4+EnumWindowsProc]{
    param($h, $p)
    $other = 0
    [void][Win4]::GetWindowThreadProcessId($h, [ref]$other)
    if ($other -ne $procId) { return $true }
    if (-not [Win4]::IsWindowVisible($h)) { return $true }
    $sb = New-Object System.Text.StringBuilder 256
    [void][Win4]::GetClassName($h, $sb, 256)
    if ($sb.ToString() -ne "Tauri Window") { return $true }
    $script:found = $h
    return $false
  }
  [void][Win4]::EnumWindows($cb, [IntPtr]::Zero)
  return $script:found
}

function Save-Shot([IntPtr]$hwnd, [string]$path) {
  $r = New-Object Win4+RECT
  [void][Win4]::GetWindowRect($hwnd, [ref]$r)
  $w = $r.Right - $r.Left; $h = $r.Bottom - $r.Top
  if ($w -le 0 -or $h -le 0) { return }
  $bmp = New-Object System.Drawing.Bitmap $w, $h
  $g = [System.Drawing.Graphics]::FromImage($bmp)
  $g.CopyFromScreen($r.Left, $r.Top, 0, 0, (New-Object System.Drawing.Size $w, $h))
  $bmp.Save($path, [System.Drawing.Imaging.ImageFormat]::Png)
  $g.Dispose(); $bmp.Dispose()
}

function Click([int]$x, [int]$y, [int]$times, [int]$downFlag, [int]$upFlag) {
  [void][Win4]::SetCursorPos($x, $y)
  Start-Sleep -Milliseconds 300
  for ($i = 0; $i -lt $times; $i++) {
    [Win4]::mouse_event($downFlag, 0, 0, 0, [IntPtr]::Zero)
    [Win4]::mouse_event($upFlag, 0, 0, 0, [IntPtr]::Zero)
    Start-Sleep -Milliseconds 90
  }
}

Start-Process $exe
Start-Sleep -Seconds 18

$proc = Get-Process -Name "190x4-browser" -ErrorAction SilentlyContinue | Select-Object -First 1
if (-not $proc) { throw "browser is not running" }
$hwnd = Find-AppWindow $proc.Id
if ($hwnd -eq [IntPtr]::Zero) { throw "no window" }
[void][Win4]::SetForegroundWindow($hwnd)
Start-Sleep -Seconds 3

$r = New-Object Win4+RECT
[void][Win4]::GetWindowRect($hwnd, [ref]$r)
# Paragraph text under the article title; the image sits higher and gives
# an image menu instead of a selection menu.
$x = $r.Left + 400
$y = $r.Top + 658

# Triple click selects the paragraph under the cursor.
Click $x $y 3 $LEFT_DOWN $LEFT_UP
Start-Sleep -Seconds 1
Save-Shot $hwnd "$out\tr-1-selection.png"

# Right click opens the native menu with our item on top.
Click $x $y 1 $RIGHT_DOWN $RIGHT_UP
Start-Sleep -Seconds 2
Save-Shot $hwnd "$out\tr-2-menu.png"

# First menu entry sits right under the cursor.
Click ($x + 60) ($y + 22) 1 $LEFT_DOWN $LEFT_UP
Start-Sleep -Seconds 12
Save-Shot $hwnd "$out\tr-3-result.png"

Stop-Process -Id $proc.Id -Force
"done"
