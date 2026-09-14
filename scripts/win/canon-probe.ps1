# Live check of the canonical browser features (tabs "+", bookmarks bar and
# Ctrl+D, downloads bubble and page, settings with passwords, menus, app icon).
# Buttons are pressed through UI Automation by their accessible names, not by
# guessed coordinates. ASCII only: Cyrillic names are built from code points.

$ErrorActionPreference = "Continue"

$root = "E:\test\190x4-browser"
$exe  = "$root\target\release\190x4-browser.exe"
$out  = "$root\spike-out\canon"
New-Item -ItemType Directory -Force -Path $out | Out-Null
$log = "$out\canon.log"
Set-Content -Path $log -Value "start $(Get-Date -Format s)" -Encoding UTF8

function Log([string]$text) { Add-Content -Path $log -Value $text -Encoding UTF8 }
function U([int[]]$codes) { -join ($codes | ForEach-Object { [char]$_ }) }

Add-Type -AssemblyName System.Drawing, System.Windows.Forms, UIAutomationClient, UIAutomationTypes
Add-Type @"
using System;
using System.Runtime.InteropServices;
using System.Text;
public class WinC {
  [DllImport("user32.dll")] public static extern bool EnumWindows(EnumWindowsProc cb, IntPtr p);
  [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr h, out RECT r);
  [DllImport("user32.dll")] public static extern int GetClassName(IntPtr h, StringBuilder s, int max);
  [DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr h, out uint pid);
  [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr h);
  [DllImport("user32.dll")] public static extern bool SetForegroundWindow(IntPtr h);
  [DllImport("user32.dll")] public static extern IntPtr GetForegroundWindow();
  [DllImport("user32.dll")] public static extern bool SetCursorPos(int x, int y);
  [DllImport("user32.dll")] public static extern void mouse_event(uint f, uint x, uint y, uint d, IntPtr e);
  [DllImport("user32.dll")] public static extern void keybd_event(byte vk, byte scan, uint flags, IntPtr extra);
  [DllImport("user32.dll")] public static extern uint MapVirtualKey(uint code, uint mapType);
  public delegate bool EnumWindowsProc(IntPtr h, IntPtr p);
  [StructLayout(LayoutKind.Sequential)] public struct RECT { public int Left, Top, Right, Bottom; }
}
"@

$AE = [System.Windows.Automation.AutomationElement]
$TS = [System.Windows.Automation.TreeScope]
$CT = [System.Windows.Automation.ControlType]

$NEW_TAB = (U @(0x041D,0x043E,0x0432,0x0430,0x044F,0x0020,0x0432,0x043A,0x043B,0x0430,0x0434,0x043A,0x0430))
$MENU = (U @(0x041D,0x0430,0x0441,0x0442,0x0440,0x043E,0x0439,0x043A,0x0438,0x0020,0x0438,0x0020,0x043F,0x0440,0x043E,0x0447,0x0435,0x0435))
$EXTENSIONS = (U @(0x0420,0x0430,0x0441,0x0448,0x0438,0x0440,0x0435,0x043D,0x0438,0x044F))
$SETTINGS = (U @(0x041D,0x0430,0x0441,0x0442,0x0440,0x043E,0x0439,0x043A,0x0438))
$PASSWORDS_NAV = (U @(0x041F,0x0430,0x0440,0x043E,0x043B,0x0438,0x0020,0x0438,0x0020,0x0430,0x0432,0x0442,0x043E,0x0437,0x0430,0x043F,0x043E,0x043B,0x043D,0x0435,0x043D,0x0438,0x0435))
$SAVE = (U @(0x0421,0x043E,0x0445,0x0440,0x0430,0x043D,0x0438,0x0442,0x044C))
$KEY = (U @(0x0421,0x043E,0x0445,0x0440,0x0430,0x043D,0x0451,0x043D,0x043D,0x044B,0x0435,0x0020,0x043F,0x0430,0x0440,0x043E,0x043B,0x0438,0x0020,0x0434,0x043B,0x044F,0x0020,0x044D,0x0442,0x043E,0x0433,0x043E,0x0020,0x0441,0x0430,0x0439,0x0442,0x0430))
$DONE = (U @(0x0413,0x043E,0x0442,0x043E,0x0432,0x043E))
$DELETE = (U @(0x0423,0x0434,0x0430,0x043B,0x0438,0x0442,0x044C))
$EXPORT = (U @(0x042D,0x043A,0x0441,0x043F,0x043E,0x0440,0x0442,0x0438,0x0440,0x043E,0x0432,0x0430,0x0442,0x044C))
$CHOOSE_FILE = (U @(0x0412,0x044B,0x0431,0x0440,0x0430,0x0442,0x044C,0x0020,0x0444,0x0430,0x0439,0x043B))

function Find-AppWindow([int]$procId) {
  $script:found = [IntPtr]::Zero
  $cb = [WinC+EnumWindowsProc]{
    param($h, $p)
    $other = 0
    [void][WinC]::GetWindowThreadProcessId($h, [ref]$other)
    if ($other -ne $procId) { return $true }
    if (-not [WinC]::IsWindowVisible($h)) { return $true }
    $sb = New-Object System.Text.StringBuilder 256
    [void][WinC]::GetClassName($h, $sb, 256)
    if ($sb.ToString() -ne "Tauri Window") { return $true }
    $script:found = $h
    return $false
  }
  [void][WinC]::EnumWindows($cb, [IntPtr]::Zero)
  return $script:found
}

function Shot([string]$name) {
  $r = New-Object WinC+RECT
  [void][WinC]::GetWindowRect($script:hwnd, [ref]$r)
  $w = $r.Right - $r.Left; $h = $r.Bottom - $r.Top
  if ($w -le 0 -or $h -le 0) { return }
  $bmp = New-Object System.Drawing.Bitmap $w, $h
  $g = [System.Drawing.Graphics]::FromImage($bmp)
  $g.CopyFromScreen($r.Left, $r.Top, 0, 0, (New-Object System.Drawing.Size $w, $h))
  $bmp.Save("$out\$name.png", [System.Drawing.Imaging.ImageFormat]::Png)
  $g.Dispose(); $bmp.Dispose()
  Log "shot $name"
}

function Keys([string]$keys) {
  [System.Windows.Forms.SendKeys]::SendWait($keys)
  Start-Sleep -Milliseconds 300
}

function Chord([byte[]]$vks) {
  # Real keys, as a keyboard sends them: virtual key plus scan code. SendKeys
  # types letters as VK_PACKET on a non-Latin layout, and Chromium takes
  # KeyboardEvent.code from the scan code, so it must not be zero either.
  foreach ($vk in $vks) { [WinC]::keybd_event($vk, [byte][WinC]::MapVirtualKey($vk, 0), 0, [IntPtr]::Zero); Start-Sleep -Milliseconds 40 }
  [array]::Reverse($vks)
  foreach ($vk in $vks) { [WinC]::keybd_event($vk, [byte][WinC]::MapVirtualKey($vk, 0), 2, [IntPtr]::Zero); Start-Sleep -Milliseconds 40 }
  Start-Sleep -Milliseconds 300
}

function Find-Element([string]$name, $type, [int]$timeoutSec = 8) {
  $deadline = (Get-Date).AddSeconds($timeoutSec)
  $byName = New-Object System.Windows.Automation.PropertyCondition($AE::NameProperty, $name)
  $cond = $byName
  if ($type) {
    $byType = New-Object System.Windows.Automation.PropertyCondition($AE::ControlTypeProperty, $type)
    $cond = New-Object System.Windows.Automation.AndCondition($byName, $byType)
  }
  $byPid = New-Object System.Windows.Automation.PropertyCondition($AE::ProcessIdProperty, $script:proc.Id)
  while ((Get-Date) -lt $deadline) {
    foreach ($window in $AE::RootElement.FindAll($TS::Children, $byPid)) {
      $el = $window.FindFirst($TS::Descendants, $cond)
      if ($el) { return $el }
    }
    Start-Sleep -Milliseconds 400
  }
  return $null
}

function Press([string]$name, $type, [string]$label) {
  $el = Find-Element $name $type
  if (-not $el) { Log "MISSING $label"; return $false }
  $rect = $el.Current.BoundingRectangle
  $x = [int]($rect.X + $rect.Width / 2); $y = [int]($rect.Y + $rect.Height / 2)
  [void][WinC]::SetCursorPos($x, $y)
  Start-Sleep -Milliseconds 250
  [WinC]::mouse_event(0x0002, 0, 0, 0, [IntPtr]::Zero)
  [WinC]::mouse_event(0x0004, 0, 0, 0, [IntPtr]::Zero)
  Log "pressed $label at $x,$y"
  return $true
}

function Press-Last([string]$name, $type, [string]$label) {
  # The last match in the tree: a confirmation dialog is appended after the
  # row button with the same name.
  $byName = New-Object System.Windows.Automation.PropertyCondition($AE::NameProperty, $name)
  $byType = New-Object System.Windows.Automation.PropertyCondition($AE::ControlTypeProperty, $type)
  $cond = New-Object System.Windows.Automation.AndCondition($byName, $byType)
  $byPid = New-Object System.Windows.Automation.PropertyCondition($AE::ProcessIdProperty, $script:proc.Id)
  $last = $null
  foreach ($window in $AE::RootElement.FindAll($TS::Children, $byPid)) {
    foreach ($el in $window.FindAll($TS::Descendants, $cond)) { $last = $el }
  }
  if (-not $last) { Log "MISSING $label"; return $false }
  $rect = $last.Current.BoundingRectangle
  $x = [int]($rect.X + $rect.Width / 2); $y = [int]($rect.Y + $rect.Height / 2)
  [void][WinC]::SetCursorPos($x, $y)
  Start-Sleep -Milliseconds 250
  [WinC]::mouse_event(0x0002, 0, 0, 0, [IntPtr]::Zero)
  [WinC]::mouse_event(0x0004, 0, 0, 0, [IntPtr]::Zero)
  Log "pressed $label at $x,$y"
  return $true
}

function Find-Dialog([int]$timeoutSec = 10) {
  # A system file dialog of the browser process: window class #32770. UI
  # Automation would have to walk the whole WebView2 tree of the owner window
  # to reach it, and does not make it in time.
  $deadline = (Get-Date).AddSeconds($timeoutSec)
  while ((Get-Date) -lt $deadline) {
    $script:dialog = [IntPtr]::Zero
    $cb = [WinC+EnumWindowsProc]{
      param($h, $p)
      $other = 0
      [void][WinC]::GetWindowThreadProcessId($h, [ref]$other)
      if ($other -ne $script:proc.Id -or -not [WinC]::IsWindowVisible($h)) { return $true }
      $sb = New-Object System.Text.StringBuilder 64
      [void][WinC]::GetClassName($h, $sb, 64)
      if ($sb.ToString() -ne "#32770") { return $true }
      $script:dialog = $h
      return $false
    }
    [void][WinC]::EnumWindows($cb, [IntPtr]::Zero)
    if ($script:dialog -ne [IntPtr]::Zero) { return $script:dialog }
    Start-Sleep -Milliseconds 400
  }
  return [IntPtr]::Zero
}

function File-Dialog([string]$path, [string]$label) {
  # Full path into the file name field, then Enter.
  $dialog = Find-Dialog
  if ($dialog -eq [IntPtr]::Zero) { Log "MISSING $label dialog"; return $false }
  [void][WinC]::SetForegroundWindow($dialog)
  Start-Sleep -Milliseconds 700
  # The path goes into the field whole: typed key by key, it loses letters to
  # the field's autocomplete. Search only inside the dialog - that is fast.
  # AutomationId 1001 is the file name in Save, 1148 in Open.
  $set = $false
  $root = $AE::FromHandle($dialog)
  $byType = New-Object System.Windows.Automation.PropertyCondition($AE::ControlTypeProperty, $CT::Edit)
  foreach ($edit in $root.FindAll($TS::Descendants, $byType)) {
    if (@("1001", "1148") -notcontains $edit.Current.AutomationId) { continue }
    try {
      $edit.GetCurrentPattern([System.Windows.Automation.ValuePattern]::Pattern).SetValue($path)
      $edit.SetFocus()
      $set = $true
      break
    } catch { }
  }
  if (-not $set) {
    # The user's clipboard is borrowed for a moment and given back.
    $previous = $null
    try { if ([System.Windows.Forms.Clipboard]::ContainsText()) { $previous = [System.Windows.Forms.Clipboard]::GetText() } } catch { }
    [System.Windows.Forms.Clipboard]::SetText($path)
    Chord @(0x11, 0x41)
    Chord @(0x11, 0x56)
    Start-Sleep -Milliseconds 300
    try {
      if ($null -ne $previous) { [System.Windows.Forms.Clipboard]::SetText($previous) } else { [System.Windows.Forms.Clipboard]::Clear() }
    } catch { }
    Log "$label path pasted from clipboard"
  }
  Start-Sleep -Milliseconds 400
  Chord @(0x0D)
  Log "$label dialog done"
  return $true
}

function Count-Tabs {
  $byPid = New-Object System.Windows.Automation.PropertyCondition($AE::ProcessIdProperty, $script:proc.Id)
  $byType = New-Object System.Windows.Automation.PropertyCondition($AE::ControlTypeProperty, $CT::TabItem)
  $total = 0
  foreach ($window in $AE::RootElement.FindAll($TS::Children, $byPid)) {
    $total += $window.FindAll($TS::Descendants, $byType).Count
  }
  return $total
}

function Click-Page {
  # Empty area of the page: a user clicks into the site before pressing Ctrl+L.
  $r = New-Object WinC+RECT
  [void][WinC]::GetWindowRect($script:hwnd, [ref]$r)
  $x = [int]($r.Left + ($r.Right - $r.Left) * 0.62)
  $y = [int]($r.Top + ($r.Bottom - $r.Top) * 0.74)
  [void][WinC]::SetCursorPos($x, $y)
  Start-Sleep -Milliseconds 250
  [WinC]::mouse_event(0x0002, 0, 0, 0, [IntPtr]::Zero)
  [WinC]::mouse_event(0x0004, 0, 0, 0, [IntPtr]::Zero)
  Log "clicked page at $x,$y"
}

function Go([string]$url, [switch]$FromPage) {
  # SetForegroundWindow on an already active window drops keyboard focus to
  # the bare top-level window, and keys reach neither the page nor the chrome.
  if ([WinC]::GetForegroundWindow() -ne $script:hwnd) {
    [void][WinC]::SetForegroundWindow($script:hwnd)
    Start-Sleep -Milliseconds 400
    Log "foreground restored"
  }
  if ($FromPage) {
    Click-Page
    Start-Sleep -Milliseconds 500
  }
  Chord @(0x11, 0x4C)
  Start-Sleep -Milliseconds 900
  Keys $url
  Keys "{ENTER}"
}

Get-Process -Name "190x4-browser" -ErrorAction SilentlyContinue | Stop-Process -Force
Start-Sleep -Seconds 2
Start-Process $exe
Start-Sleep -Seconds 15

$script:proc = Get-Process -Name "190x4-browser" -ErrorAction SilentlyContinue | Select-Object -First 1
if (-not $script:proc) { Log "FAIL not running"; exit 1 }
$script:hwnd = Find-AppWindow $script:proc.Id
[void][WinC]::SetForegroundWindow($script:hwnd)
Start-Sleep -Seconds 2
Log "process $($script:proc.ProcessName) pid $($script:proc.Id)"
Shot "01-start"

# 1. "+" right after the tabs opens a new tab.
$before = Count-Tabs
[void](Press $NEW_TAB $CT::Button "new-tab button")
Start-Sleep -Seconds 4
$after = Count-Tabs
Log "tabs before=$before after=$after"
Shot "02-plus"

# 1b. No saved password for the test site: the offer to save must appear.
[void](Press $SETTINGS $CT::Button "rail settings")
Start-Sleep -Seconds 3
[void](Press $PASSWORDS_NAV $CT::Button "passwords section")
Start-Sleep -Seconds 3
if (Press $DELETE $CT::Button "saved password delete") {
  Start-Sleep -Seconds 2
  [void](Press-Last $DELETE $CT::Button "confirm delete")
  Start-Sleep -Seconds 2
}
Shot "02b-passwords-cleared"

# 2. A login page, bookmarked with Ctrl+D.
Go "https://the-internet.herokuapp.com/login"
Start-Sleep -Seconds 9
Shot "03-login-page"
Chord @(0x11, 0x44)
Start-Sleep -Seconds 3
Shot "04-bookmark-bubble"
Keys "{ENTER}"
Start-Sleep -Seconds 2
Shot "05-bookmarks-bar"

# 3. Log in: the browser must offer to save the password.
$user = Find-Element "Username" $CT::Edit
$pass = Find-Element "Password" $CT::Edit
if ($user -and $pass) {
  # Fields may already be filled: select all and overwrite.
  $user.SetFocus(); Start-Sleep -Milliseconds 300
  Chord @(0x11, 0x41); Keys "{DEL}"
  Keys "tomsmith"
  $pass.SetFocus(); Start-Sleep -Milliseconds 300
  Chord @(0x11, 0x41); Keys "{DEL}"
  Keys "SuperSecretPassword{!}"
  Keys "{ENTER}"
  Start-Sleep -Seconds 8
  Shot "06-password-offer"
  $key = Find-Element $KEY $CT::Button 3
  Log "key icon found=$([bool]$key) visible=$(if ($key) { -not $key.Current.IsOffscreen } else { $false })"
  [void](Press $SAVE $CT::Button "save password")
  Start-Sleep -Seconds 3
} else {
  Log "MISSING login fields"
}

# 4. Back to the login form: it must be filled automatically.
Go "https://the-internet.herokuapp.com/login" -FromPage
Start-Sleep -Seconds 9
$user = Find-Element "Username" $CT::Edit
if ($user) {
  try {
    $value = $user.GetCurrentPattern([System.Windows.Automation.ValuePattern]::Pattern).Current.Value
    Log "autofill username='$value'"
  } catch { Log "autofill value unreadable" }
}
Shot "07-autofill"

# 5. A real download: bubble on start, then the downloads page (Ctrl+J).
Go "https://www.7-zip.org/a/7z2408-x64.exe" -FromPage
Start-Sleep -Seconds 10
Shot "08-download-bubble"
Keys "{ESC}"
Start-Sleep -Seconds 1
[void][WinC]::SetForegroundWindow($script:hwnd)
Start-Sleep -Milliseconds 500
Chord @(0x11, 0x4A)
Start-Sleep -Seconds 3
Shot "09-downloads-page"

# 6. Settings: general and passwords.
[void](Press $SETTINGS $CT::Button "rail settings")
Start-Sleep -Seconds 3
Shot "10-settings"
[void](Press $PASSWORDS_NAV $CT::Button "passwords section")
Start-Sleep -Seconds 3
Shot "11-settings-passwords"

# 6b. Passwords: export to CSV, import the same file back. The file may hold
# real passwords of this profile: only counts go to the log, and the file is
# deleted right after the import.
$pwFile = "$out\passwords-export.csv"
if (Press $EXPORT $CT::Button "passwords export") {
  Start-Sleep -Seconds 2
  [void](Press-Last $EXPORT $CT::Button "confirm passwords export")
  Start-Sleep -Seconds 3
  if (File-Dialog $pwFile "passwords export") {
    Start-Sleep -Seconds 3
    if (Test-Path $pwFile) {
      $lines = @(Get-Content $pwFile)
      Log "passwords csv lines=$($lines.Count) header=$($lines[0] -like 'name,url,username,password*') site=$([bool]($lines -match 'the-internet\.herokuapp\.com'))"
    } else { Log "passwords csv missing" }
  }
}
Shot "11b-passwords-exported"
if ((Test-Path $pwFile) -and (Press $CHOOSE_FILE $CT::Button "passwords import")) {
  Start-Sleep -Seconds 3
  [void](File-Dialog $pwFile "passwords import")
  Start-Sleep -Seconds 2
  Shot "11c-passwords-imported"
}
Remove-Item $pwFile -ErrorAction SilentlyContinue

# 6c. Bookmarks: export to HTML, import the same file back.
Go "190x4://settings/bookmarks"
Start-Sleep -Seconds 3
$bmFile = "$out\bookmarks-export.html"
if (Press $EXPORT $CT::Button "bookmarks export") {
  Start-Sleep -Seconds 3
  if (File-Dialog $bmFile "bookmarks export") {
    Start-Sleep -Seconds 3
    if (Test-Path $bmFile) {
      $html = Get-Content $bmFile -Raw -Encoding UTF8
      Log "bookmarks html bytes=$($html.Length) netscape=$($html.Contains('NETSCAPE-Bookmark-file-1')) links=$(([regex]::Matches($html, '<A ')).Count)"
    } else { Log "bookmarks html missing" }
  }
}
Shot "11d-bookmarks-exported"
if ((Test-Path $bmFile) -and (Press $CHOOSE_FILE $CT::Button "bookmarks import")) {
  Start-Sleep -Seconds 3
  [void](File-Dialog $bmFile "bookmarks import")
  Start-Sleep -Seconds 2
  Shot "11e-bookmarks-imported"
}
Remove-Item $bmFile -ErrorAction SilentlyContinue

# 7. Menus: "Settings and more" and extensions.
[void](Press $MENU $CT::Button "main menu")
Start-Sleep -Seconds 3
Shot "12-main-menu"
Keys "{ESC}"
Start-Sleep -Seconds 1
[void](Press $EXTENSIONS $CT::Button "extensions menu")
Start-Sleep -Seconds 3
Shot "13-extensions"
Keys "{ESC}"
Start-Sleep -Seconds 1

# 8. Taskbar icon.
$screen = [System.Windows.Forms.Screen]::PrimaryScreen.Bounds
$bmp = New-Object System.Drawing.Bitmap $screen.Width, 56
$g = [System.Drawing.Graphics]::FromImage($bmp)
$g.CopyFromScreen(0, $screen.Height - 56, 0, 0, (New-Object System.Drawing.Size $screen.Width, 56))
$bmp.Save("$out\14-taskbar.png", [System.Drawing.Imaging.ImageFormat]::Png)
$g.Dispose(); $bmp.Dispose()

$file = Join-Path ([Environment]::GetFolderPath("UserProfile")) "Downloads\7z2408-x64.exe"
Log "downloaded exists=$(Test-Path $file)"
Log "windows=$((Get-Process -Id $script:proc.Id).MainWindowTitle)"

Stop-Process -Id $script:proc.Id -Force
Log "done"
