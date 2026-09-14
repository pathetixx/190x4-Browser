# First live run of the browser: answers three questions that cannot be
# answered without actually starting it.
#
#  1. Do chrome and tabs really share one browser process?
#  2. Does overlay mode pull the native surface out from under the palette?
#  3. Does the layout survive a non-100% DPI?
#
# ASCII only (PowerShell 5.1 reads a BOM-less .ps1 as ANSI).

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
public class Win {
  [DllImport("user32.dll")] public static extern bool EnumWindows(EnumWindowsProc cb, IntPtr p);
  [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr h, out RECT r);
  [DllImport("user32.dll")] public static extern int GetClassName(IntPtr h, StringBuilder s, int max);
  [DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr h, out uint pid);
  [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr h);
  [DllImport("user32.dll")] public static extern bool SetForegroundWindow(IntPtr h);
  [DllImport("user32.dll")] public static extern bool SetCursorPos(int x, int y);
  [DllImport("user32.dll")] public static extern void mouse_event(uint f, uint x, uint y, uint d, IntPtr e);
  [DllImport("shcore.dll")] public static extern int GetDpiForMonitor(IntPtr m, int t, out uint x, out uint y);
  [DllImport("user32.dll")] public static extern IntPtr MonitorFromWindow(IntPtr h, int flags);
  public delegate bool EnumWindowsProc(IntPtr h, IntPtr p);
  [StructLayout(LayoutKind.Sequential)] public struct RECT { public int Left, Top, Right, Bottom; }
}
"@

function Find-AppWindow([int]$procId) {
  $found = [IntPtr]::Zero
  $cb = [Win+EnumWindowsProc]{
    param($h, $p)
    $pid2 = 0
    [void][Win]::GetWindowThreadProcessId($h, [ref]$pid2)
    if ($pid2 -ne $procId) { return $true }
    if (-not [Win]::IsWindowVisible($h)) { return $true }
    $sb = New-Object System.Text.StringBuilder 256
    [void][Win]::GetClassName($h, $sb, 256)
    # Tauri also creates tray_icon_app / Tao Thread Event Target windows.
    if ($sb.ToString() -ne "Tauri Window") { return $true }
    $script:found = $h
    return $false
  }
  [void][Win]::EnumWindows($cb, [IntPtr]::Zero)
  return $script:found
}

function Save-WindowShot([IntPtr]$hwnd, [string]$path) {
  $r = New-Object Win+RECT
  [void][Win]::GetWindowRect($hwnd, [ref]$r)
  $w = $r.Right - $r.Left
  $h = $r.Bottom - $r.Top
  if ($w -le 0 -or $h -le 0) { return $false }
  $bmp = New-Object System.Drawing.Bitmap $w, $h
  $g = [System.Drawing.Graphics]::FromImage($bmp)
  $g.CopyFromScreen($r.Left, $r.Top, 0, 0, (New-Object System.Drawing.Size $w, $h))
  $bmp.Save($path, [System.Drawing.Imaging.ImageFormat]::Png)
  $g.Dispose(); $bmp.Dispose()
  return $true
}

Start-Process $exe
Start-Sleep -Seconds 12

$proc = Get-Process -Name "190x4-browser" -ErrorAction SilentlyContinue | Select-Object -First 1
if (-not $proc) { throw "190x4-browser is not running" }

$hwnd = Find-AppWindow $proc.Id
if ($hwnd -eq [IntPtr]::Zero) { throw "no Tauri Window found" }

# DPI of the monitor the window sits on: layout bugs show up above 100%.
$mon = [Win]::MonitorFromWindow($hwnd, 2)
$dx = 0; $dy = 0
[void][Win]::GetDpiForMonitor($mon, 0, [ref]$dx, [ref]$dy)

# Give the first tab time to load something real.
Start-Sleep -Seconds 8
[void][Win]::SetForegroundWindow($hwnd)
Start-Sleep -Seconds 1
[void](Save-WindowShot $hwnd "$out\app-main.png")

# Overlay check: the command palette must cover the page, not hide under it.
[System.Windows.Forms.SendKeys]::SendWait("^k")
Start-Sleep -Seconds 2
[void](Save-WindowShot $hwnd "$out\app-palette.png")
[System.Windows.Forms.SendKeys]::SendWait("{ESC}")
Start-Sleep -Seconds 1

# Panel check: it must push the page aside, not overlap it.
# Clicking the rail instead of typing a command: palette commands are in
# Russian and SendKeys cannot type them reliably.
$scale = $dx / 96.0
$r2 = New-Object Win+RECT
[void][Win]::GetWindowRect($hwnd, [ref]$r2)
# Rail sits below titlebar (38) + toolbar (46); first button center ~ (26, 110) in CSS px.
$clickX = [int]($r2.Left + 26 * $scale)
$clickY = [int]($r2.Top + 110 * $scale)
[void][Win]::SetCursorPos($clickX, $clickY)
Start-Sleep -Milliseconds 400
[Win]::mouse_event(0x0002, 0, 0, 0, [IntPtr]::Zero)
[Win]::mouse_event(0x0004, 0, 0, 0, [IntPtr]::Zero)
Start-Sleep -Seconds 2
[void](Save-WindowShot $hwnd "$out\app-panel.png")

$wv = Get-CimInstance Win32_Process -Filter "Name='msedgewebview2.exe'" |
  Where-Object { $_.CommandLine -like "*190x4*" }
$browserProcs  = @($wv | Where-Object { $_.CommandLine -notlike "*--type=*" })
$rendererProcs = @($wv | Where-Object { $_.CommandLine -like "*--type=renderer*" })

$rect = New-Object Win+RECT
[void][Win]::GetWindowRect($hwnd, [ref]$rect)

[pscustomobject]@{
  pid                = $proc.Id
  window_class       = "Tauri Window"
  window_rect        = "$($rect.Left),$($rect.Top) $($rect.Right - $rect.Left)x$($rect.Bottom - $rect.Top)"
  monitor_dpi        = $dx
  scale_percent      = [math]::Round($dx / 96 * 100)
  webview2_total     = @($wv).Count
  webview2_browser   = $browserProcs.Count
  webview2_renderers = $rendererProcs.Count
  private_mb         = [math]::Round(($wv | ForEach-Object { (Get-Process -Id $_.ProcessId -ErrorAction SilentlyContinue).PrivateMemorySize64 } | Measure-Object -Sum).Sum / 1MB, 1)
} | ConvertTo-Json | Set-Content -Path "$out\app-probe.json" -Encoding ASCII

Stop-Process -Id $proc.Id -Force -ErrorAction SilentlyContinue
