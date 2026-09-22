param([int]$Vk = 27, [int]$Mod = 0, [int]$X = -1, [int]$Y = -1)
# Real keyboard and mouse input for the probe window (ASCII only).
Add-Type @"
using System;
using System.Runtime.InteropServices;
using System.Text;
public class K {
  [DllImport("user32.dll")] public static extern bool EnumWindows(EnumWindowsProc cb, IntPtr p);
  [DllImport("user32.dll")] public static extern int GetClassName(IntPtr h, StringBuilder s, int max);
  [DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr h, out uint pid);
  [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr h);
  [DllImport("user32.dll")] public static extern bool SetForegroundWindow(IntPtr h);
  [DllImport("user32.dll")] public static extern bool SetProcessDPIAware();
  [DllImport("user32.dll")] public static extern bool SetCursorPos(int x, int y);
  [DllImport("user32.dll")] public static extern void mouse_event(uint f, uint x, uint y, uint d, IntPtr e);
  [DllImport("user32.dll")] public static extern void keybd_event(byte vk, byte scan, uint flags, IntPtr extra);
  [DllImport("user32.dll")] public static extern uint MapVirtualKey(uint code, uint mapType);
  public delegate bool EnumWindowsProc(IntPtr h, IntPtr p);
}
"@
[K]::SetProcessDPIAware() | Out-Null
$proc = Get-Process 190x4-browser-probe -ErrorAction SilentlyContinue | Select-Object -First 1
$script:hwnd = [IntPtr]::Zero
$cb = [K+EnumWindowsProc]{ param($h, $p)
  $o = 0; [void][K]::GetWindowThreadProcessId($h, [ref]$o)
  if ($o -ne $proc.Id -or -not [K]::IsWindowVisible($h)) { return $true }
  $sb = New-Object System.Text.StringBuilder 256; [void][K]::GetClassName($h, $sb, 256)
  if ($sb.ToString() -ne "Tauri Window") { return $true }
  $script:hwnd = $h; return $false }
[void][K]::EnumWindows($cb, [IntPtr]::Zero)
"hwnd $($script:hwnd)"
[void][K]::SetForegroundWindow($script:hwnd)
Start-Sleep -Milliseconds 300
if ($X -ge 0) {
  [void][K]::SetCursorPos($X, $Y); Start-Sleep -Milliseconds 120
  if ($Mod -gt 0) { [K]::keybd_event([byte]$Mod, [byte][K]::MapVirtualKey($Mod, 0), 0, [IntPtr]::Zero); Start-Sleep -Milliseconds 60 }
  [K]::mouse_event(0x0002, 0, 0, 0, [IntPtr]::Zero); Start-Sleep -Milliseconds 60
  [K]::mouse_event(0x0004, 0, 0, 0, [IntPtr]::Zero); Start-Sleep -Milliseconds 120
  if ($Mod -gt 0) { [K]::keybd_event([byte]$Mod, [byte][K]::MapVirtualKey($Mod, 0), 2, [IntPtr]::Zero) }
  "clicked $X,$Y mod $Mod"
}
if ($Vk -gt 0) {
  if ($Mod -gt 0 -and $X -lt 0) { [K]::keybd_event([byte]$Mod, [byte][K]::MapVirtualKey($Mod, 0), 0, [IntPtr]::Zero); Start-Sleep -Milliseconds 60 }
  [K]::keybd_event([byte]$Vk, [byte][K]::MapVirtualKey($Vk, 0), 0, [IntPtr]::Zero); Start-Sleep -Milliseconds 60
  [K]::keybd_event([byte]$Vk, [byte][K]::MapVirtualKey($Vk, 0), 2, [IntPtr]::Zero)
  if ($Mod -gt 0 -and $X -lt 0) { [K]::keybd_event([byte]$Mod, [byte][K]::MapVirtualKey($Mod, 0), 2, [IntPtr]::Zero) }
  "key $Vk mod $Mod"
}
