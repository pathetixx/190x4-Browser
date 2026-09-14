# Live check for stage 3: page find, audio state, downloads.
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
public class Win3 {
  [DllImport("user32.dll")] public static extern bool EnumWindows(EnumWindowsProc cb, IntPtr p);
  [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr h, out RECT r);
  [DllImport("user32.dll")] public static extern int GetClassName(IntPtr h, StringBuilder s, int max);
  [DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr h, out uint pid);
  [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr h);
  [DllImport("user32.dll")] public static extern bool SetForegroundWindow(IntPtr h);
  public delegate bool EnumWindowsProc(IntPtr h, IntPtr p);
  [StructLayout(LayoutKind.Sequential)] public struct RECT { public int Left, Top, Right, Bottom; }
}
"@

function Find-AppWindow([int]$procId) {
  $script:found = [IntPtr]::Zero
  $cb = [Win3+EnumWindowsProc]{
    param($h, $p)
    $other = 0
    [void][Win3]::GetWindowThreadProcessId($h, [ref]$other)
    if ($other -ne $procId) { return $true }
    if (-not [Win3]::IsWindowVisible($h)) { return $true }
    $sb = New-Object System.Text.StringBuilder 256
    [void][Win3]::GetClassName($h, $sb, 256)
    if ($sb.ToString() -ne "Tauri Window") { return $true }
    $script:found = $h
    return $false
  }
  [void][Win3]::EnumWindows($cb, [IntPtr]::Zero)
  return $script:found
}

function Save-Shot([IntPtr]$hwnd, [string]$path) {
  $r = New-Object Win3+RECT
  [void][Win3]::GetWindowRect($hwnd, [ref]$r)
  $w = $r.Right - $r.Left; $h = $r.Bottom - $r.Top
  if ($w -le 0 -or $h -le 0) { return }
  $bmp = New-Object System.Drawing.Bitmap $w, $h
  $g = [System.Drawing.Graphics]::FromImage($bmp)
  $g.CopyFromScreen($r.Left, $r.Top, 0, 0, (New-Object System.Drawing.Size $w, $h))
  $bmp.Save($path, [System.Drawing.Imaging.ImageFormat]::Png)
  $g.Dispose(); $bmp.Dispose()
}

Start-Process $exe
Start-Sleep -Seconds 16

$proc = Get-Process -Name "190x4-browser" -ErrorAction SilentlyContinue | Select-Object -First 1
if (-not $proc) { throw "browser is not running" }
$hwnd = Find-AppWindow $proc.Id
if ($hwnd -eq [IntPtr]::Zero) { throw "no window" }
[void][Win3]::SetForegroundWindow($hwnd)
Start-Sleep -Seconds 2

# Session restores habr.com from the previous probe; find a latin word on it.
[System.Windows.Forms.SendKeys]::SendWait("^f")
Start-Sleep -Seconds 1
[System.Windows.Forms.SendKeys]::SendWait("Anthropic")
Start-Sleep -Seconds 3
Save-Shot $hwnd "$out\find-1.png"

[System.Windows.Forms.SendKeys]::SendWait("{ENTER}")
Start-Sleep -Seconds 2
Save-Shot $hwnd "$out\find-2.png"

[System.Windows.Forms.SendKeys]::SendWait("{ESC}")
Start-Sleep -Seconds 2
Save-Shot $hwnd "$out\find-3.png"

Stop-Process -Id $proc.Id -Force
"done"
