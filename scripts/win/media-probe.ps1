# Stage 4 check: download media through the 190x4 relay.
#  1. open a YouTube video,
#  2. right click on the player -> our item must be in the native menu,
#  3. click it, wait for the format list, then start the smallest download.
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
public class Win5 {
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
  $cb = [Win5+EnumWindowsProc]{
    param($h, $p)
    $other = 0
    [void][Win5]::GetWindowThreadProcessId($h, [ref]$other)
    if ($other -ne $procId) { return $true }
    if (-not [Win5]::IsWindowVisible($h)) { return $true }
    $sb = New-Object System.Text.StringBuilder 256
    [void][Win5]::GetClassName($h, $sb, 256)
    if ($sb.ToString() -ne "Tauri Window") { return $true }
    $script:found = $h
    return $false
  }
  [void][Win5]::EnumWindows($cb, [IntPtr]::Zero)
  return $script:found
}

function Save-Shot([IntPtr]$hwnd, [string]$path) {
  $r = New-Object Win5+RECT
  [void][Win5]::GetWindowRect($hwnd, [ref]$r)
  $w = $r.Right - $r.Left; $h = $r.Bottom - $r.Top
  if ($w -le 0 -or $h -le 0) { return }
  $bmp = New-Object System.Drawing.Bitmap $w, $h
  $g = [System.Drawing.Graphics]::FromImage($bmp)
  $g.CopyFromScreen($r.Left, $r.Top, 0, 0, (New-Object System.Drawing.Size $w, $h))
  $bmp.Save($path, [System.Drawing.Imaging.ImageFormat]::Png)
  $g.Dispose(); $bmp.Dispose()
}

function Click([int]$x, [int]$y, [int]$times, [int]$downFlag, [int]$upFlag) {
  [void][Win5]::SetCursorPos($x, $y)
  Start-Sleep -Milliseconds 300
  for ($i = 0; $i -lt $times; $i++) {
    [Win5]::mouse_event($downFlag, 0, 0, 0, [IntPtr]::Zero)
    [Win5]::mouse_event($upFlag, 0, 0, 0, [IntPtr]::Zero)
    Start-Sleep -Milliseconds 90
  }
}

Start-Process $exe
Start-Sleep -Seconds 18

$proc = Get-Process -Name "190x4-browser" -ErrorAction SilentlyContinue | Select-Object -First 1
if (-not $proc) { throw "browser is not running" }
$hwnd = Find-AppWindow $proc.Id
if ($hwnd -eq [IntPtr]::Zero) { throw "no window" }
[void][Win5]::SetForegroundWindow($hwnd)
Start-Sleep -Seconds 3

[System.Windows.Forms.SendKeys]::SendWait("^l")
Start-Sleep -Seconds 1
# Short link: no "?" and "=" to confuse SendKeys, and the restored session
# cannot leave leftovers in the field.
[System.Windows.Forms.SendKeys]::SendWait("^a")
Start-Sleep -Milliseconds 200
[System.Windows.Forms.SendKeys]::SendWait("youtu.be/aqz-KE-bpKQ")
Start-Sleep -Milliseconds 700
[System.Windows.Forms.SendKeys]::SendWait("{ENTER}")
Start-Sleep -Seconds 25

$r = New-Object Win5+RECT
[void][Win5]::GetWindowRect($hwnd, [ref]$r)
# Player area: right click there gives a video target menu.
$x = $r.Left + 640
$y = $r.Top + 400

Save-Shot $hwnd "$out\md-1-page.png"

# Ctrl+Shift+D asks the server what can be downloaded from this page.
# A cookie banner on top makes the right-click menu useless, the shortcut does not care.
[System.Windows.Forms.SendKeys]::SendWait("^+d")
# yt-dlp разбирает ссылку десятки секунд — раньше список не отрисуется.
Start-Sleep -Seconds 50
Save-Shot $hwnd "$out\md-2-formats.png"

# Smallest option sits last in the list: audio only.
$panelX = $r.Left + 200
$panelY = $r.Top + 330
# Two separate clicks: the first one only brings the window to the front if it
# lost focus, and then the row never gets its click.
[void][Win5]::SetForegroundWindow($hwnd)
Start-Sleep -Milliseconds 800
# Четвёртая строка списка — 720p (~153 МБ): видно и прогресс, и финал.
Click $panelX ($r.Top + 372) 1 $LEFT_DOWN $LEFT_UP
Start-Sleep -Seconds 20
Save-Shot $hwnd "$out\md-3-download.png"
Start-Sleep -Seconds 150
Save-Shot $hwnd "$out\md-4-done.png"

Stop-Process -Id $proc.Id -Force
"done"
