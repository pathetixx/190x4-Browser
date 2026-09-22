param([int]$Percent = 0)
# Read or set the display scale of the primary monitor (DisplayConfig API).
# Without -Percent: print current scale. ASCII only.
Add-Type @"
using System;
using System.Runtime.InteropServices;
public static class DpiScale {
  [StructLayout(LayoutKind.Sequential)] public struct LUID { public uint Low; public int High; }
  [StructLayout(LayoutKind.Sequential)] public struct HEADER { public int type; public int size; public LUID adapter; public uint id; }
  [StructLayout(LayoutKind.Sequential)] public struct GETDPI { public HEADER h; public int min; public int cur; public int max; }
  [StructLayout(LayoutKind.Sequential)] public struct SETDPI { public HEADER h; public int rel; }
  [StructLayout(LayoutKind.Sequential, Size = 72)] public struct PATH { public LUID adapter; public uint id; public uint modeIdx; public uint flags; }
  [StructLayout(LayoutKind.Sequential, Size = 64)] public struct MODE { public int type; }
  [DllImport("user32.dll")] public static extern int GetDisplayConfigBufferSizes(uint flags, out uint paths, out uint modes);
  [DllImport("user32.dll")] public static extern int QueryDisplayConfig(uint flags, ref uint np, [Out] PATH[] paths, ref uint nm, [Out] MODE[] modes, IntPtr topo);
  [DllImport("user32.dll")] public static extern int DisplayConfigGetDeviceInfo(ref GETDPI info);
  [DllImport("user32.dll")] public static extern int DisplayConfigSetDeviceInfo(ref SETDPI info);
}
"@
$scales = @(100,125,150,175,200,225,250,300,350,400,450,500)
$np = 0; $nm = 0
[void][DpiScale]::GetDisplayConfigBufferSizes(2, [ref]$np, [ref]$nm)
$paths = New-Object DpiScale+PATH[] $np
$modes = New-Object DpiScale+MODE[] $nm
[void][DpiScale]::QueryDisplayConfig(2, [ref]$np, $paths, [ref]$nm, $modes, [IntPtr]::Zero)
$p = $paths[0]
# Nested structs are copied on read in PowerShell: build the header first.
$get = New-Object DpiScale+GETDPI
$h = New-Object DpiScale+HEADER
$h.type = -3; $h.size = [Runtime.InteropServices.Marshal]::SizeOf($get); $h.adapter = $p.adapter; $h.id = $p.id
$get.h = $h
$rg = [DpiScale]::DisplayConfigGetDeviceInfo([ref]$get)
"get result $rg min $($get.min) cur $($get.cur) max $($get.max)"
$recommended = -$get.min
$current = $scales[$recommended + $get.cur]
"current $current recommended $($scales[$recommended])"
if ($Percent -gt 0) {
  $set = New-Object DpiScale+SETDPI
  $hs = New-Object DpiScale+HEADER
  $hs.type = -4; $hs.size = [Runtime.InteropServices.Marshal]::SizeOf($set); $hs.adapter = $p.adapter; $hs.id = $p.id
  $set.h = $hs
  $set.rel = [array]::IndexOf($scales, $Percent) - $recommended
  $r = [DpiScale]::DisplayConfigSetDeviceInfo([ref]$set)
  "set $Percent result $r"
}
