param([int]$Frames = 70, [string]$Out = "E:\test\probe\frames")
# Frame capture of the probe window around a new tab opening (ASCII only).
Add-Type -AssemblyName System.Drawing
Add-Type @"
using System;
using System.Runtime.InteropServices;
using System.Text;
public class Cap {
  [DllImport("user32.dll")] public static extern bool EnumWindows(EnumWindowsProc cb, IntPtr p);
  [DllImport("user32.dll")] public static extern int GetClassName(IntPtr h, StringBuilder s, int max);
  [DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr h, out uint pid);
  [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr h);
  [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr h, out RECT r);
  [DllImport("user32.dll")] public static extern bool SetProcessDPIAware();
  [DllImport("user32.dll")] public static extern bool SetForegroundWindow(IntPtr h);
  public delegate bool EnumWindowsProc(IntPtr h, IntPtr p);
  [StructLayout(LayoutKind.Sequential)] public struct RECT { public int Left, Top, Right, Bottom; }
}
"@
[Cap]::SetProcessDPIAware() | Out-Null
$proc = Get-Process 190x4-browser-probe -ErrorAction SilentlyContinue | Select-Object -First 1
$script:hwnd = [IntPtr]::Zero
$cb = [Cap+EnumWindowsProc]{ param($h, $p)
  $o = 0; [void][Cap]::GetWindowThreadProcessId($h, [ref]$o)
  if ($o -ne $proc.Id -or -not [Cap]::IsWindowVisible($h)) { return $true }
  $sb = New-Object System.Text.StringBuilder 256; [void][Cap]::GetClassName($h, $sb, 256)
  if ($sb.ToString() -ne "Tauri Window") { return $true }
  $script:hwnd = $h; return $false }
[void][Cap]::EnumWindows($cb, [IntPtr]::Zero)
[void][Cap]::SetForegroundWindow($script:hwnd)
$r = New-Object Cap+RECT
[void][Cap]::GetWindowRect($script:hwnd, [ref]$r)
$w = $r.Right - $r.Left; $h = $r.Bottom - $r.Top
if (Test-Path $Out) { Remove-Item $Out -Recurse -Force }
New-Item -ItemType Directory -Path $Out | Out-Null
$list = New-Object System.Collections.Generic.List[System.Drawing.Bitmap]
$times = New-Object System.Collections.Generic.List[long]
Start-Sleep -Milliseconds 300
Set-Content -Path "E:\test\probe\ready.txt" -Value "ready"
$sw = [Diagnostics.Stopwatch]::StartNew()
for ($i = 0; $i -lt $Frames; $i++) {
  $bmp = New-Object System.Drawing.Bitmap $w, $h
  $g = [System.Drawing.Graphics]::FromImage($bmp)
  $g.CopyFromScreen($r.Left, $r.Top, 0, 0, $bmp.Size)
  $g.Dispose()
  $list.Add($bmp); $times.Add($sw.ElapsedMilliseconds)
}
for ($i = 0; $i -lt $list.Count; $i++) { $list[$i].Save(("{0}\f{1:D3}.png" -f $Out, $i), [System.Drawing.Imaging.ImageFormat]::Png); $list[$i].Dispose() }
Remove-Item "E:\test\probe\ready.txt" -ErrorAction SilentlyContinue
"rect $($r.Left),$($r.Top) ${w}x${h}"
"times " + ($times -join ",")
