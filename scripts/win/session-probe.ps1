# Integration check for stage 3: history, session restore, bookmarks.
#
#  1. start the browser, type an address into the omnibox (Ctrl+L works only
#     if accelerator forwarding from the tab is alive);
#  2. screenshot the loaded page;
#  3. kill the app, start it again and screenshot what came back.
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
public class Win2 {
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
  $cb = [Win2+EnumWindowsProc]{
    param($h, $p)
    $other = 0
    [void][Win2]::GetWindowThreadProcessId($h, [ref]$other)
    if ($other -ne $procId) { return $true }
    if (-not [Win2]::IsWindowVisible($h)) { return $true }
    $sb = New-Object System.Text.StringBuilder 256
    [void][Win2]::GetClassName($h, $sb, 256)
    if ($sb.ToString() -ne "Tauri Window") { return $true }
    $script:found = $h
    return $false
  }
  [void][Win2]::EnumWindows($cb, [IntPtr]::Zero)
  return $script:found
}

function Save-Shot([IntPtr]$hwnd, [string]$path) {
  $r = New-Object Win2+RECT
  [void][Win2]::GetWindowRect($hwnd, [ref]$r)
  $w = $r.Right - $r.Left; $h = $r.Bottom - $r.Top
  if ($w -le 0 -or $h -le 0) { return }
  $bmp = New-Object System.Drawing.Bitmap $w, $h
  $g = [System.Drawing.Graphics]::FromImage($bmp)
  $g.CopyFromScreen($r.Left, $r.Top, 0, 0, (New-Object System.Drawing.Size $w, $h))
  $bmp.Save($path, [System.Drawing.Imaging.ImageFormat]::Png)
  $g.Dispose(); $bmp.Dispose()
}

function Start-Browser {
  Start-Process $exe
  Start-Sleep -Seconds 14
  $proc = Get-Process -Name "190x4-browser" -ErrorAction SilentlyContinue | Select-Object -First 1
  if (-not $proc) { throw "browser is not running" }
  $hwnd = Find-AppWindow $proc.Id
  if ($hwnd -eq [IntPtr]::Zero) { throw "no window" }
  [void][Win2]::SetForegroundWindow($hwnd)
  Start-Sleep -Seconds 1
  return @($proc, $hwnd)
}

# --- first run: type an address ---
$first = Start-Browser
$proc = $first[0]; $hwnd = $first[1]

[System.Windows.Forms.SendKeys]::SendWait("^l")
Start-Sleep -Seconds 1
[System.Windows.Forms.SendKeys]::SendWait("habr.com/ru/feed/")
Start-Sleep -Milliseconds 700
[System.Windows.Forms.SendKeys]::SendWait("{ENTER}")
Start-Sleep -Seconds 18

Save-Shot $hwnd "$out\session-1.png"
Stop-Process -Id $proc.Id -Force
Start-Sleep -Seconds 5

# --- second run: what came back ---
$second = Start-Browser
$proc2 = $second[0]; $hwnd2 = $second[1]
Start-Sleep -Seconds 6
Save-Shot $hwnd2 "$out\session-2.png"

# history panel: rail button #4 (shield, downloads, translate, bookmarks, history)
$r = New-Object Win2+RECT
[void][Win2]::GetWindowRect($hwnd2, [ref]$r)
[System.Windows.Forms.SendKeys]::SendWait("^k")
Start-Sleep -Seconds 2
Save-Shot $hwnd2 "$out\session-3.png"

Stop-Process -Id $proc2.Id -Force
"done"
